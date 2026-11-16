//! Cheap diagnostics for the connection pools. sqlx warns about a slow acquire or statement only
//! once it ends, so a pool whose connections never come back logs nothing. This warns while every
//! connection of a pool stays in use, and logs the pools' state when an acquire times out.

use std::panic::Location;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use tracing::{info, warn};

/// How often the watchdog looks at a pool.
const TICK: Duration = Duration::from_secs(5);
/// How long every connection must stay in use before the watchdog warns, and how often it warns
/// again while they do.
const STALL: Duration = Duration::from_secs(20);
/// At most one timeout report in this many seconds.
const TIMEOUT_REPORT_SECS: u64 = 10;

static POOLS: Mutex<Vec<(&'static str, SqlitePool)>> = Mutex::new(Vec::new());
static LAST_TIMEOUT_REPORT: AtomicU64 = AtomicU64::new(0);

/// Registers `pool` under `name` for `stats`, replacing an earlier pool of that name, and starts
/// its watchdog on the current runtime.
pub(crate) fn watch(name: &'static str, pool: &SqlitePool) {
    {
        let mut pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
        pools.retain(|(n, _)| *n != name);
        pools.push((name, pool.clone()));
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(watchdog(name, pool.clone()));
    }
}

/// `in use/size (max N)` of a pool.
fn describe(pool: &SqlitePool) -> String {
    let size = pool.size();
    let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX);
    format!(
        "{}/{} in use (max {})",
        size.saturating_sub(idle),
        size,
        pool.options().get_max_connections()
    )
}

/// Every registered pool's use, as `main 10/10 in use (max 10), read-only 1/2 in use (max 10)`.
pub fn stats() -> String {
    let pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
    pools
        .iter()
        .map(|(name, pool)| format!("{name} {}", describe(pool)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn saturated(pool: &SqlitePool) -> bool {
    pool.num_idle() == 0 && pool.size() >= pool.options().get_max_connections()
}

async fn watchdog(name: &'static str, pool: SqlitePool) {
    let mut since: Option<Instant> = None;
    let mut warned: Option<Instant> = None;
    while !pool.is_closed() {
        tokio::time::sleep(TICK).await;
        if !saturated(&pool) {
            if let (Some(start), Some(_)) = (since, warned) {
                info!(pool = name, after_secs = start.elapsed().as_secs(), stats = %stats(),
                    "database connections are free again");
            }
            (since, warned) = (None, None);
            continue;
        }
        let start = *since.get_or_insert_with(Instant::now);
        let due = warned.map_or(start.elapsed() >= STALL, |w| w.elapsed() >= STALL);
        if due {
            warned = Some(Instant::now());
            warn!(pool = name, for_secs = start.elapsed().as_secs(), stats = %stats(),
                "every database connection has been in use the whole time; callers are waiting");
        }
    }
}

/// Logs the pools' state and the caller when `e` is an acquire timeout, at most once in
/// `TIMEOUT_REPORT_SECS`.
pub fn note_error(e: &sqlx::Error, caller: &Location<'_>) {
    if !matches!(e, sqlx::Error::PoolTimedOut) {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let last = LAST_TIMEOUT_REPORT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < TIMEOUT_REPORT_SECS
        || LAST_TIMEOUT_REPORT
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    warn!(at = %caller, stats = %stats(), "timed out waiting for a database connection");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn r3dbpool_stats_count_the_connections_in_use() {
        let dir = crate::tests::temp_dir();
        let pool = crate::pool_options(2)
            .connect_with(crate::connect_options(
                dir.join("stats.db").to_str().unwrap(),
            ))
            .await
            .unwrap();
        let first = pool.acquire().await.unwrap();
        let second = pool.acquire().await.unwrap();
        assert_eq!(describe(&pool), "2/2 in use (max 2)");
        assert!(saturated(&pool));
        assert!(pool.try_acquire().is_none());
        drop((first, second));
        // A dropped connection goes back to the pool in a task of its own.
        let freed = async {
            while saturated(&pool) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(2), freed)
                .await
                .is_ok()
        );
    }
}
