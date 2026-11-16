//! "database is locked": a write that waited `busy_timeout` for another connection's write lock
//! and gave up. A long writer (a filter index rebuild, VACUUM, a big sort name pass) can hold the
//! lock past that, so background writers, which nobody waits on, try again a few times. So they
//! do when no pool connection came in time (`PoolTimedOut`).

use std::future::Future;
use std::time::Duration;

use tracing::warn;

/// The waits before the second, third and fourth tries. A try that meets a held lock usually
/// fails after `busy_timeout` (10 s), so a writer typically gives up after about a minute. At
/// worst each try also waits out the pool's acquire timeout first (30 s), and a writer gives up
/// after 4 × 40 s plus these 17 s: about three minutes.
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

/// Whether trying `e`'s write again later may work: "database is locked", or no connection in
/// time.
fn is_transient(e: &sqlx::Error) -> bool {
    is_busy(e) || matches!(e, sqlx::Error::PoolTimedOut)
}

/// Runs `write` again while it fails with "database is locked" or no connection came in time, up
/// to three more times. `write` must be one statement or one whole transaction, so a failed try
/// wrote nothing. `what` names it in the log.
pub async fn retry_busy<T, F, Fut>(what: &str, write: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    retry_busy_until(what, std::future::pending, write).await
}

/// `retry_busy` that stops waiting to try again once `stop()` completes (a cancelled job, say),
/// and returns the last try's error.
pub async fn retry_busy_until<T, F, Fut, S, Stop>(
    what: &str,
    mut stop: S,
    mut write: F,
) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
    S: FnMut() -> Stop,
    Stop: Future<Output = ()>,
{
    for wait in RETRY_WAITS {
        match write().await {
            Err(e) if is_transient(&e) => {
                warn!(what, retry_in_secs = wait.as_secs(), error = %e,
                    "the database is busy; trying again");
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = stop() => return Err(e),
                }
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
    fn r3dbpool_only_a_busy_database_is_retried() {
        assert!(!is_busy(&sqlx::Error::PoolTimedOut));
        assert!(is_transient(&sqlx::Error::PoolTimedOut));
        assert!(!is_transient(&sqlx::Error::RowNotFound));
    }

    // A stop ends the wait before the next try at once, with the last try's error.
    #[tokio::test]
    async fn r3dbpool_retry_busy_stops_waiting_when_told() {
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let mut tries = 0;
        let started = std::time::Instant::now();
        let stopping = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            stop.send(true).unwrap();
        });
        let result: Result<(), _> = retry_busy_until(
            "test write",
            || {
                let mut stopped = stopped.clone();
                async move {
                    let _ = stopped.wait_for(|s| *s).await;
                }
            },
            || {
                tries += 1;
                async { Err(sqlx::Error::PoolTimedOut) }
            },
        )
        .await;
        stopping.await.unwrap();
        assert!(matches!(result, Err(sqlx::Error::PoolTimedOut)));
        assert_eq!(tries, 1);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
