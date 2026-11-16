//! Cheap diagnostics for the connection pools. sqlx warns about a slow acquire or statement only
//! once it ends, so a pool whose connections never come back logs nothing. This warns while every
//! connection of a pool stays in use and none comes back, and logs the pools' state when an
//! acquire times out.

use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;
use tracing::{info, warn};

/// How often the watchdog looks at a pool.
const TICK: Duration = Duration::from_secs(5);
/// How long every connection must stay in use, with none given back, before the watchdog warns,
/// and how often it warns again while that lasts.
const STALL: Duration = Duration::from_secs(20);
/// At most one timeout report in this many seconds.
const TIMEOUT_REPORT_SECS: u64 = 10;

static POOLS: Mutex<Vec<(&'static str, SqlitePool)>> = Mutex::new(Vec::new());
static LAST_TIMEOUT_REPORT: AtomicU64 = AtomicU64::new(0);

/// How many connections a pool's callers have given back. A busy pool gives some back between
/// two looks; a stuck one doesn't.
#[derive(Clone, Default)]
pub(crate) struct Releases(Arc<AtomicU64>);

impl Releases {
    /// `options` that count their pool's releases here.
    pub(crate) fn count(&self, options: SqlitePoolOptions) -> SqlitePoolOptions {
        let releases = self.0.clone();
        options.after_release(move |_, _| {
            releases.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok(true) })
        })
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Registers `pool` under `name` for `stats`, replacing an earlier pool of that name, and starts
/// its watchdog on the current runtime. `releases` must count `pool`'s releases.
pub(crate) fn watch(name: &'static str, pool: &SqlitePool, releases: Releases) {
    {
        let mut pools = POOLS.lock().unwrap_or_else(|p| p.into_inner());
        pools.retain(|(n, _)| *n != name);
        pools.push((name, pool.clone()));
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(watchdog(name, pool.clone(), releases));
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

/// What one look at a pool found.
#[derive(Clone, Debug, PartialEq)]
enum Verdict {
    Fine,
    /// Every connection in use, none given back, for this long: warn.
    Stuck(Duration),
    /// A pool warned about gave a connection back after this long.
    Freed(Duration),
}

/// A pool's state between the watchdog's looks.
struct Stall {
    last: Instant,
    releases: u64,
    /// The start of the run of looks that found the pool full with nothing given back.
    since: Option<Instant>,
    warned: Option<Instant>,
}

impl Stall {
    fn new(now: Instant, releases: u64) -> Self {
        Self {
            last: now,
            releases,
            since: None,
            warned: None,
        }
    }

    /// Takes a look at `now`: whether every connection is in use, and the releases so far.
    fn look(&mut self, now: Instant, saturated: bool, releases: u64) -> Verdict {
        let gave_back = releases != self.releases;
        self.releases = releases;
        let last = std::mem::replace(&mut self.last, now);
        if !saturated || gave_back {
            return match (self.since.take(), self.warned.take()) {
                (Some(since), Some(_)) => Verdict::Freed(now - since),
                _ => Verdict::Fine,
            };
        }
        let since = *self.since.get_or_insert(last);
        let due = self
            .warned
            .map_or(now - since >= STALL, |w| now - w >= STALL);
        if !due {
            return Verdict::Fine;
        }
        self.warned = Some(now);
        Verdict::Stuck(now - since)
    }
}

async fn watchdog(name: &'static str, pool: SqlitePool, releases: Releases) {
    let mut stall = Stall::new(Instant::now(), releases.get());
    while !pool.is_closed() {
        tokio::time::sleep(TICK).await;
        match stall.look(Instant::now(), saturated(&pool), releases.get()) {
            Verdict::Fine => {}
            Verdict::Stuck(for_) => warn!(pool = name, for_secs = for_.as_secs(), stats = %stats(),
                "every database connection is in use and none has come back; callers are waiting"),
            Verdict::Freed(after) => info!(pool = name, after_secs = after.as_secs(),
                stats = %stats(), "database connections are coming back again"),
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
        let releases = Releases::default();
        let pool = releases
            .count(crate::pool_options(2))
            .connect_with(crate::connect_options(
                dir.join("stats.db").to_str().unwrap(),
            ))
            .await
            .unwrap();
        let first = pool.acquire().await.unwrap();
        let second = pool.acquire().await.unwrap();
        let before = releases.get();
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
        assert_eq!(releases.get(), before + 2);
    }

    /// Looks every `TICK` from `start`, as the watchdog does.
    fn looks(stall: &mut Stall, start: Instant, ticks: &[(bool, u64)]) -> Vec<Verdict> {
        ticks
            .iter()
            .zip(1u32..)
            .map(|(&(saturated, releases), n)| stall.look(start + TICK * n, saturated, releases))
            .collect()
    }

    // A full pool that keeps giving connections back, a long scan say, is busy, not stuck.
    #[test]
    fn r3dbpool_a_busy_pool_is_not_stuck() {
        let start = Instant::now();
        let mut stall = Stall::new(start, 0);
        let ticks: Vec<(bool, u64)> = (1..=20).map(|n| (true, n)).collect();
        assert!(
            looks(&mut stall, start, &ticks)
                .iter()
                .all(|v| *v == Verdict::Fine)
        );
    }

    // Full with nothing given back: a warning once that has lasted `STALL`, then every `STALL`,
    // and a note when a connection comes back.
    #[test]
    fn r3dbpool_a_stuck_pool_warns_until_it_frees() {
        let start = Instant::now();
        let mut stall = Stall::new(start, 7);
        let mut ticks = vec![(true, 7); 8];
        ticks.push((true, 8));
        let verdicts = looks(&mut stall, start, &ticks);
        let secs = |s| Duration::from_secs(s);
        let mut wanted = vec![Verdict::Fine; 8];
        wanted[3] = Verdict::Stuck(secs(20));
        wanted[7] = Verdict::Stuck(secs(40));
        wanted.push(Verdict::Freed(secs(45)));
        assert_eq!(verdicts, wanted);
    }

    // A pool with a free connection never warns, however long nothing comes back.
    #[test]
    fn r3dbpool_a_pool_with_room_is_not_stuck() {
        let start = Instant::now();
        let mut stall = Stall::new(start, 0);
        let verdicts = looks(&mut stall, start, &[(false, 0); 10]);
        assert!(verdicts.iter().all(|v| *v == Verdict::Fine));
    }
}
