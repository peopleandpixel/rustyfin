//! "database is locked": a write that waited `busy_timeout` for another connection's write lock
//! and gave up. A long writer (a filter index rebuild, VACUUM, a big sort name pass) can hold the
//! lock past that, so background writers, which nobody waits on, try again a few times.

use std::future::Future;
use std::time::Duration;

use tracing::warn;

/// The waits before the second, third and fourth tries. With `busy_timeout` in each try, a
/// writer gives up after about a minute.
const RETRY_WAITS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
];

/// Whether `e` is SQLite's SQLITE_BUSY, in any of its extended forms.
pub fn is_busy(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => db
            .code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| code & 0xff == 5),
        _ => false,
    }
}

/// Runs `write` again while it fails with "database is locked", up to three more times. `write`
/// must be one statement or one whole transaction, so a failed try wrote nothing. `what` names
/// it in the log.
pub async fn retry_busy<T, F, Fut>(what: &str, mut write: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    for wait in RETRY_WAITS {
        match write().await {
            Err(e) if is_busy(&e) => {
                warn!(
                    what,
                    retry_in_secs = wait.as_secs(),
                    "database is locked; trying again"
                );
                tokio::time::sleep(wait).await;
            }
            done => return done,
        }
    }
    write().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    // A writer holds the lock past busy_timeout, so the first try fails, and a later one succeeds.
    #[tokio::test]
    async fn r3dbpool_retry_busy_outlasts_a_long_writer() {
        let dir = crate::tests::temp_dir();
        let path = dir.join("busy.db");
        let path = path.to_str().unwrap();
        let options = crate::connect_options(path).busy_timeout(Duration::from_millis(200));
        let pool = crate::pool_options(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (v INTEGER)")
            .execute(&pool)
            .await
            .unwrap();
        let mut holder = sqlx::SqliteConnection::connect_with(&options)
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut holder)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            sqlx::query("COMMIT").execute(&mut holder).await.unwrap();
        });

        let mut tries = 0;
        let written = retry_busy("test write", || {
            tries += 1;
            sqlx::query("INSERT INTO t (v) VALUES (1)").execute(&pool)
        })
        .await;
        release.await.unwrap();

        assert!(written.is_ok(), "{written:?}");
        assert_eq!(tries, 2);
    }

    #[test]
    fn r3dbpool_only_sqlite_busy_is_retried() {
        assert!(!is_busy(&sqlx::Error::PoolTimedOut));
        assert!(!is_busy(&sqlx::Error::RowNotFound));
    }
}
