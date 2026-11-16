//! Metadata providers' answers kept on disk, so that a restart asks a provider nothing it has
//! answered before. Answers are filed by owner: what they are about (a movie's or series' id, an
//! IMDb id, a searched title), one folder each, so that forgetting an item touches only its
//! files. One file per request, named by the SHA-256 of the request's key (its path and query,
//! never the API key), holding the key, the answer and when it was stored. "Not found" is an
//! answer too; failures are never stored. Entries have no expiry of their own: the caller decides
//! whether an old one still counts (`Stored::stored_at`). File I/O runs on tokio's blocking pool.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// A stored answer: the JSON, or `None` for the provider's "not found".
pub type Answer = Option<serde_json::Value>;

/// An answer and when it was stored (Unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub answer: Answer,
    pub stored_at: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    key: String,
    answer: Answer,
    #[serde(default)]
    stored_at: u64,
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A directory of answers, or nowhere (`AnswerStore::memory_only`).
#[derive(Debug, Clone, Default)]
pub struct AnswerStore {
    dir: Option<PathBuf>,
}

impl AnswerStore {
    /// Answers under `dir`, which is made when the first answer is stored.
    pub fn new(dir: impl Into<PathBuf>) -> AnswerStore {
        AnswerStore {
            dir: Some(dir.into()),
        }
    }

    /// Keeps nothing on disk.
    pub fn memory_only() -> AnswerStore {
        AnswerStore { dir: None }
    }

    /// The folder of `owner`'s answers: its name made safe for a path, or a hash of it.
    fn folder(dir: &Path, owner: &str) -> PathBuf {
        let safe = !owner.is_empty()
            && owner.len() <= 64
            && owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if safe {
            dir.join(owner)
        } else {
            dir.join(format!("h-{}", hex(owner)))
        }
    }

    fn file(dir: &Path, owner: &str, key: &str) -> PathBuf {
        Self::folder(dir, owner).join(format!("{}.json", hex(key)))
    }

    /// The stored answer for `key`, filed under `owner`.
    pub async fn get(&self, owner: &str, key: &str) -> Option<Stored> {
        let path = Self::file(self.dir.as_ref()?, owner, key);
        let key = key.to_string();
        blocking(move || {
            let bytes = std::fs::read(path).ok()?;
            let entry: Entry = serde_json::from_slice(&bytes).ok()?;
            // A hash collision is as good as a miss
            (entry.key == key).then_some(Stored {
                answer: entry.answer,
                stored_at: entry.stored_at,
            })
        })
        .await
    }

    /// Stores `answer` for `key` under `owner`, stamped now, whole or not at all (written aside
    /// under a name of its own, then renamed).
    pub async fn put(&self, owner: &str, key: &str, answer: &Answer) {
        let Some(dir) = &self.dir else {
            return;
        };
        let path = Self::file(dir, owner, key);
        let entry = Entry {
            key: key.to_string(),
            answer: answer.clone(),
            stored_at: now_secs(),
        };
        blocking(move || {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let bytes = serde_json::to_vec(&entry).ok()?;
            let temp = path.with_extension(format!(
                "tmp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&temp, bytes))
                .and_then(|()| std::fs::rename(&temp, &path));
            if written.is_err() {
                let _ = std::fs::remove_file(&temp);
                tracing::debug!("metadata answer cache: could not store an answer");
            }
            Some(())
        })
        .await;
    }

    /// Forgets every answer filed under `owner`.
    pub async fn forget(&self, owner: &str) {
        let Some(dir) = &self.dir else {
            return;
        };
        let folder = Self::folder(dir, owner);
        blocking(move || std::fs::remove_dir_all(folder).ok()).await;
    }
}

fn hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Runs `work` on tokio's blocking pool.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn r3meta_answers_and_not_found_survive_a_new_store() {
        let dir = std::env::temp_dir().join(format!("rf-answers-{}", uuid::Uuid::new_v4()));
        let store = AnswerStore::new(&dir);
        let found = Some(serde_json::json!({"id": 1}));
        store.put("movie-1", "/movie/1?append=a", &found).await;
        store.put("movie-2", "/movie/2", &None).await;
        store
            .put("search/one two", "/search/movie?query=one+two", &None)
            .await;
        let again = AnswerStore::new(&dir);
        let stored = again.get("movie-1", "/movie/1?append=a").await.unwrap();
        assert_eq!(stored.answer, found);
        assert!(now_secs() - stored.stored_at < 60);
        assert_eq!(again.get("movie-2", "/movie/2").await.unwrap().answer, None);
        assert_eq!(again.get("movie-3", "/movie/3").await, None);
        // Filed by owner: the same key under another owner is another entry
        assert_eq!(again.get("movie-9", "/movie/1?append=a").await, None);
        let search = again.get("search/one two", "/search/movie?query=one+two");
        assert!(search.await.is_some());

        again.forget("movie-1").await;
        assert_eq!(
            AnswerStore::new(&dir)
                .get("movie-1", "/movie/1?append=a")
                .await,
            None
        );
        assert!(
            AnswerStore::new(&dir)
                .get("movie-2", "/movie/2")
                .await
                .is_some()
        );
        again.forget("search/one two").await;
        let search = again.get("search/one two", "/search/movie?query=one+two");
        assert!(search.await.is_none());
        assert_eq!(
            AnswerStore::memory_only().get("movie-2", "/movie/2").await,
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
