//! Write transactions, one at a time per database file. SQLite lets one connection write at a
//! time anyway; a writer that waits for the lock inside SQLite holds a pool connection meanwhile,
//! and a few of them behind a long write took every connection, so reads waited too. A writer
//! waits for its turn here first, without a connection.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::error::{DatabaseError, ErrorKind};
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// A write transaction, begun with `BEGIN IMMEDIATE`, holding its database's writer turn. It is
/// used as the transaction's connection (`&mut *tx`), and rolled back when dropped uncommitted.
pub struct WriteTx {
    tx: Transaction<'static, Sqlite>,
    turn: OwnedSemaphorePermit,
}

impl WriteTx {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        let Self { tx, turn } = self;
        let done = tx.commit().await;
        drop(turn);
        done
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        let Self { tx, turn } = self;
        let done = tx.rollback().await;
        drop(turn);
        done
    }
}

impl Deref for WriteTx {
    type Target = SqliteConnection;

    fn deref(&self) -> &SqliteConnection {
        &self.tx
    }
}

impl DerefMut for WriteTx {
    fn deref_mut(&mut self) -> &mut SqliteConnection {
        &mut self.tx
    }
}

/// The writer turn of `pool`'s database file.
fn turns(pool: &SqlitePool) -> Arc<Semaphore> {
    static TURNS: Mutex<Vec<(PathBuf, Arc<Semaphore>)>> = Mutex::new(Vec::new());
    let file = pool.connect_options().get_filename().to_path_buf();
    let mut turns = TURNS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((_, turn)) = turns.iter().find(|(f, _)| *f == file) {
        return turn.clone();
    }
    let turn = Arc::new(Semaphore::new(1));
    turns.push((file, turn.clone()));
    turn
}

/// SQLite's "database is locked" (SQLITE_BUSY), for a writer whose turn did not come within
/// `crate::BUSY_TIMEOUT`: what SQLite says when its own wait for the lock runs out, so callers
/// and `busy::retry_busy` take both alike.
#[derive(Debug)]
struct TurnTimedOut;

const TURN_TIMED_OUT: &str = "database is locked (another writer held it past the busy timeout)";

impl std::fmt::Display for TurnTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(TURN_TIMED_OUT)
    }
}

impl StdError for TurnTimedOut {}

impl DatabaseError for TurnTimedOut {
    fn message(&self) -> &str {
        TURN_TIMED_OUT
    }

    fn code(&self) -> Option<Cow<'_, str>> {
        Some(Cow::Borrowed("5"))
    }

    fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
        self
    }

    fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
        self
    }

    fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
        self
    }

    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

/// Begins a write transaction on `pool` once no other writer of its database holds the turn.
/// A writer that waits `crate::BUSY_TIMEOUT` for it fails with "database is locked", as one
/// that waits that long for SQLite's lock does: one that never ends holds no one up for good.
pub async fn begin_write(pool: &SqlitePool) -> Result<WriteTx, sqlx::Error> {
    begin_write_within(pool, crate::BUSY_TIMEOUT).await
}

async fn begin_write_within(pool: &SqlitePool, wait: Duration) -> Result<WriteTx, sqlx::Error> {
    let Ok(Ok(turn)) = tokio::time::timeout(wait, turns(pool).acquire_owned()).await else {
        return Err(sqlx::Error::Database(Box::new(TurnTimedOut)));
    };
    let tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    Ok(WriteTx { tx, turn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    async fn pool(path: &std::path::Path, size: u32) -> SqlitePool {
        crate::pool_options(size)
            .connect_with(crate::connect_options(path.to_str().unwrap()))
            .await
            .unwrap()
    }

    // A writer waiting for its turn holds no connection: with one writer in a transaction and
    // three more waiting, a pool of two still answers a read at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r3dbpool_waiting_writers_leave_connections_for_reads() {
        let dir = crate::tests::temp_dir();
        let pool = pool(&dir.join("w.db"), 2).await;
        sqlx::query("CREATE TABLE t (v INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        let mut first = begin_write(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (v) VALUES (0)")
            .execute(&mut *first)
            .await
            .unwrap();
        let waiting: Vec<_> = (1..=3)
            .map(|v| {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let mut tx = begin_write(&pool).await?;
                    sqlx::query("INSERT INTO t (v) VALUES (?1)")
                        .bind(v)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(300)).await;

        let started = Instant::now();
        let read = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM t").fetch_one(&pool);
        let read = tokio::time::timeout(Duration::from_secs(2), read).await;
        assert!(matches!(read, Ok(Ok(0))), "{read:?}");
        assert!(started.elapsed() < Duration::from_secs(1));

        first.commit().await.unwrap();
        for writer in waiting {
            writer.await.unwrap().unwrap();
        }
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM t")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 4);
    }

    // A turn that doesn't come in time is "database is locked", which background writers retry.
    #[tokio::test]
    async fn r3dbpool_a_writer_turn_that_never_comes_is_database_locked() {
        let dir = crate::tests::temp_dir();
        let pool = pool(&dir.join("w.db"), 2).await;
        let held = begin_write(&pool).await.unwrap();
        let late = begin_write_within(&pool, Duration::from_millis(200)).await;
        let Err(e) = late else {
            panic!("began while the turn was held")
        };
        assert!(crate::busy::is_busy(&e), "{e}");
        drop(held);
        assert!(
            begin_write_within(&pool, Duration::from_secs(2))
                .await
                .is_ok()
        );
    }

    // Separate database files have separate turns.
    #[tokio::test]
    async fn r3dbpool_each_database_has_its_own_writer_turn() {
        let dir = crate::tests::temp_dir();
        let (a, b) = (
            pool(&dir.join("a.db"), 2).await,
            pool(&dir.join("b.db"), 2).await,
        );
        let _held = begin_write(&a).await.unwrap();
        let other = tokio::time::timeout(Duration::from_secs(1), begin_write(&b)).await;
        assert!(matches!(other, Ok(Ok(_))));
    }
}
