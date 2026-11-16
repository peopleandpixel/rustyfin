//! Metadata providers' answers kept on disk, so that a restart asks a provider nothing it has
//! answered before. One file per request, named by the SHA-256 of the request's key (its path and
//! query, never the API key), holding the key and the answer. "Not found" is an answer too;
//! failures are never stored. Entries have no expiry: an explicit "replace all metadata" refresh
//! forgets an item's entries (`remove_if`), and nothing else does.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// A stored answer: the JSON, or `None` for the provider's "not found".
pub type Answer = Option<serde_json::Value>;

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    key: String,
    answer: Answer,
}

/// A directory of answers, or nowhere (`AnswerStore::memory_only`).
#[derive(Debug, Clone, Default)]
pub struct AnswerStore {
    dir: Option<PathBuf>,
}

impl AnswerStore {
    /// Answers under `dir`, which is made if missing.
    pub fn new(dir: impl Into<PathBuf>) -> AnswerStore {
        let dir = dir.into();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("metadata answer cache {} unusable: {e}", dir.display());
        }
        AnswerStore { dir: Some(dir) }
    }

    /// Keeps nothing on disk.
    pub fn memory_only() -> AnswerStore {
        AnswerStore { dir: None }
    }

    fn file(dir: &Path, key: &str) -> PathBuf {
        let hash = Sha256::digest(key.as_bytes());
        let name: String = hash.iter().map(|b| format!("{b:02x}")).collect();
        dir.join(format!("{name}.json"))
    }

    /// The stored answer for `key`: `Some(answer)` when there is one.
    pub fn get(&self, key: &str) -> Option<Answer> {
        let dir = self.dir.as_ref()?;
        let bytes = std::fs::read(Self::file(dir, key)).ok()?;
        let entry: Entry = serde_json::from_slice(&bytes).ok()?;
        // A hash collision is as good as a miss
        (entry.key == key).then_some(entry.answer)
    }

    /// Stores `answer` for `key`, whole or not at all (written aside, then renamed).
    pub fn put(&self, key: &str, answer: &Answer) {
        let Some(dir) = &self.dir else {
            return;
        };
        let entry = Entry {
            key: key.to_string(),
            answer: answer.clone(),
        };
        let Ok(bytes) = serde_json::to_vec(&entry) else {
            return;
        };
        let path = Self::file(dir, key);
        let temp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&temp, bytes)
            .and_then(|()| std::fs::rename(&temp, &path))
            .is_err()
        {
            let _ = std::fs::remove_file(&temp);
            tracing::debug!("metadata answer cache: could not store an answer");
        }
    }

    /// Forgets every answer whose key `matches`.
    pub fn remove_if(&self, matches: impl Fn(&str) -> bool) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|e| e.path()) {
            let key = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
                .map(|entry| entry.key);
            if key.as_deref().is_some_and(&matches) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3meta_answers_and_not_found_survive_a_new_store() {
        let dir = std::env::temp_dir().join(format!("rf-answers-{}", uuid::Uuid::new_v4()));
        let store = AnswerStore::new(&dir);
        store.put("/movie/1?append=a", &Some(serde_json::json!({"id": 1})));
        store.put("/movie/2", &None);
        let again = AnswerStore::new(&dir);
        assert_eq!(
            again.get("/movie/1?append=a"),
            Some(Some(serde_json::json!({"id": 1})))
        );
        assert_eq!(again.get("/movie/2"), Some(None));
        assert_eq!(again.get("/movie/3"), None);
        again.remove_if(|key| key.starts_with("/movie/1"));
        assert_eq!(AnswerStore::new(&dir).get("/movie/1?append=a"), None);
        assert_eq!(AnswerStore::new(&dir).get("/movie/2"), Some(None));
        assert_eq!(AnswerStore::memory_only().get("/movie/2"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
