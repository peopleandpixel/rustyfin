//! Choosing between watching and polling a library path, by the filesystem it is on, and the
//! manager that monitors the library paths of the libraries with real-time monitoring on.

#[cfg(target_os = "linux")]
mod kernel;
#[cfg(target_os = "linux")]
mod mount_watch;
mod mounts;
mod poll;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rustyfin_db::repo_library;
use rustyfin_model::library::Library;
use sqlx::SqlitePool;
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::library_monitor::{ChangeQueue, unit_for};

pub use mounts::{
    MountChange, automount_points, fs_type_from_mountinfo, fs_type_of, is_local_fs_type,
    mount_changes, restarts_monitor, restarts_polled_monitor,
};
pub use poll::FolderPoller;

/// The poll interval until `set_poll_interval` changes it.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// The first wait before a failed reconcile, or a failed reading of the mount table, is tried
/// again. Each failure in a row doubles the wait, up to `RETRY_MAX`.
pub(super) const RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait before a failed reconcile, or a failed reading of the mount table, is tried
/// again. See `RETRY_FIRST`.
pub(super) const RETRY_MAX: Duration = Duration::from_secs(60);

/// The reason of a path that is missing when its monitor starts. That monitor polls the path, and
/// the next reload after the path has appeared replaces it.
const PATH_NOT_FOUND: &str = "path not found";

/// The reason of a path whose library path the kernel reports gone: deleted, moved, unmounted or
/// its watch ignored. That monitor polls the path, and the next reload after the path is back
/// replaces it, as with `PATH_NOT_FOUND`.
const LIBRARY_PATH_GONE: &str = "library path gone";

/// The reason of a path on a network or unknown filesystem, whose changes the kernel does not
/// report, so it is polled. A mount that appears restarts that monitor only once a local
/// filesystem holds the path.
const NOT_LOCAL: &str = "network or unknown filesystem";

/// The reason of a path polled because the kernel's watch limit is reached. The next reload tries
/// the kernel again once a limit has changed (a test limit, an inotify limit or the open file
/// limit); while none has changed, the monitor is kept with its baseline.
const WATCH_LIMIT_REACHED: &str = "watch limit reached";

/// The reason of a path polled because the kernel's inotify instance limit is reached. Like
/// `WATCH_LIMIT_REACHED`, the next reload tries the kernel again once a limit has changed, and
/// keeps the monitor with its baseline while none has.
const INSTANCE_LIMIT_REACHED: &str = "inotify instance limit reached";

/// The reason of a path polled because the process's open file limit is reached. Like
/// `WATCH_LIMIT_REACHED`, the next reload tries the kernel again once a limit has changed, and
/// keeps the monitor with its baseline while none has.
const FILE_LIMIT_REACHED: &str = "open file limit reached";

/// How a library path is monitored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Kernel change notifications (inotify).
    Watch,
    /// Folder timestamps, read at the poll interval.
    Poll,
}

/// A monitored library path, as `LibraryWatcher::monitored` lists it. Until its monitor has
/// checked the path, `method`, `fs_type` and `reason` are placeholders and `ready` is false.
#[derive(Clone, Debug)]
pub struct MonitoredPath {
    pub library_id: Uuid,
    pub library_name: String,
    pub path: PathBuf,
    pub method: Method,
    /// The filesystem type, or `unknown` when the path cannot be found.
    pub fs_type: String,
    /// Why the path is not watched.
    pub reason: Option<String>,
    /// Folders the monitor sees under the path.
    pub folders: usize,
    /// Kernel watches in place; 0 when polled.
    pub watches: usize,
    /// True once the watches or the baseline exist.
    pub ready: bool,
}

/// Monitors the library paths of the libraries with real-time monitoring on, and reports their
/// changes to a `ChangeQueue`. Clones share one manager.
#[derive(Clone)]
pub struct LibraryWatcher {
    inner: Arc<Inner>,
}

/// A monitor is keyed by its library id and its path.
type MonitorKey = (Uuid, PathBuf);

/// The hook that `set_poll_hook` and `set_walk_hook` install. A poll calls it on the polling
/// thread, and a walk on the thread that walks the folders.
type PollHook = Arc<dyn Fn(&Path) + Send + Sync>;

struct Inner {
    pool: SqlitePool,
    changes: ChangeQueue,
    data_dir: PathBuf,
    /// Notified by `libraries_changed`; the manager then reloads the libraries.
    reload: Notify,
    /// The poll interval. Every poll monitor subscribes to it.
    interval: watch::Sender<Duration>,
    state: Mutex<State>,
    /// The test limit of kernel watches for the monitors started from now on. See
    /// `set_watch_limit`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    watch_limit: Mutex<Option<usize>>,
    /// Whether the warning of the kernel's watch limit is logged. It is logged once per watcher.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    watch_limit_warned: AtomicBool,
    /// Whether the warning of the kernel's instance limit is logged. It is logged once per watcher.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    instance_limit_warned: AtomicBool,
    /// Whether the warning of the open file limit is logged. It is logged once per watcher.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    file_limit_warned: AtomicBool,
    /// The hook of `set_poll_hook`, read before each poll. The lock is not held while it runs.
    poll_hook: Mutex<Option<PollHook>>,
    /// The hook of `set_walk_hook`, read when a kernel watch starts. The lock is not held while it
    /// runs.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    walk_hook: Mutex<Option<PollHook>>,
    /// The readings of the mount table so far, injected failures too. See `mount_readings`.
    mount_reads: AtomicU64,
    /// The next readings of the mount table that fail. See `fail_mount_reads`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fail_mount_reads: AtomicUsize,
    /// The next reconciles that fail. See `fail_reconciles`.
    fail_reconciles: AtomicUsize,
}

#[derive(Default)]
struct State {
    /// Bumped by `start` and `stop`. A manager reconciles only while its generation is current.
    generation: u64,
    manager: Option<JoinHandle<()>>,
    /// Follows the mount table from `start` until `stop`. Linux only; see `mount_watch::follow`.
    follower: Option<JoinHandle<()>>,
    force_poll: bool,
    next_id: u64,
    monitors: BTreeMap<MonitorKey, Monitor>,
}

struct Monitor {
    /// Tells this monitor apart from an earlier one for the same key that may still be running.
    id: u64,
    info: MonitoredPath,
    /// The canonical path that `choose` found, as the mount table names it. None when the path
    /// cannot be canonicalized; the key's path stands in then.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    canonical: Option<PathBuf>,
    /// The canonical paths of the symlinked folders that the kernel watching checked, as
    /// `Tree::link_targets` keeps them. A change at one of them restarts a watched monitor; see
    /// `mount_watch::mark_restarts`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    link_targets: Vec<PathBuf>,
    /// The limits when its kernel setup gave up at a limit; None otherwise.
    limits: Option<Limits>,
    /// Set when a mount change concerns the path. Reconcile then replaces this monitor.
    restart: bool,
    /// All paths of the library: a report is coalesced by them.
    library_paths: Vec<String>,
    /// Notified by `fake_overflow`, which stands in for an overflow of the kernel's queue.
    overflow: Arc<Notify>,
    task: JoinHandle<()>,
}

/// The limits a kernel setup can run into. A monitor that fell back at a limit is replaced once
/// one of them has changed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Limits {
    /// The test limit of kernel watches; see `set_watch_limit`.
    test_watches: Option<usize>,
    /// The kernel's `fs.inotify.max_user_watches`.
    max_user_watches: Option<u64>,
    /// The kernel's `fs.inotify.max_user_instances`.
    max_user_instances: Option<u64>,
    /// The soft limit on open files of the process.
    open_files: Option<u64>,
}

impl Limits {
    /// The limits as they are now. A value that cannot be read or parsed is None.
    fn read(inner: &Inner) -> Self {
        let test_watches = *inner
            .watch_limit
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Self {
            test_watches,
            max_user_watches: std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
                .ok()
                .and_then(|text| text.trim().parse().ok()),
            max_user_instances: std::fs::read_to_string("/proc/sys/fs/inotify/max_user_instances")
                .ok()
                .and_then(|text| text.trim().parse().ok()),
            open_files: std::fs::read_to_string("/proc/self/limits")
                .ok()
                .and_then(|text| open_file_limit(&text)),
        }
    }
}

/// The soft limit on open files in the text of `/proc/self/limits`, where `unlimited` is
/// `u64::MAX`. None when there is no `Max open files` line or its value does not parse.
fn open_file_limit(limits: &str) -> Option<u64> {
    let rest = limits
        .lines()
        .find_map(|line| line.strip_prefix("Max open files"))?;
    match rest.split_whitespace().next()? {
        "unlimited" => Some(u64::MAX),
        soft => soft.parse().ok(),
    }
}

/// How a monitor starts: its method, the filesystem type (`unknown` when the path cannot be
/// found) and why the path is polled.
struct Choice {
    method: Method,
    fs_type: String,
    /// Why the path is polled; None when it is watched.
    reason: Option<&'static str>,
    /// The canonical path, as `Monitor::canonical` keeps it.
    canonical: Option<PathBuf>,
}

impl LibraryWatcher {
    /// A watcher that is not running yet. Reports go to `changes`. Every walk leaves out
    /// `data_dir` and everything under it.
    pub fn new(pool: SqlitePool, changes: ChangeQueue, data_dir: PathBuf) -> Self {
        let (interval, _) = watch::channel(DEFAULT_POLL_INTERVAL);
        Self {
            inner: Arc::new(Inner {
                pool,
                changes,
                data_dir,
                reload: Notify::new(),
                interval,
                state: Mutex::new(State::default()),
                watch_limit: Mutex::new(None),
                watch_limit_warned: AtomicBool::new(false),
                instance_limit_warned: AtomicBool::new(false),
                file_limit_warned: AtomicBool::new(false),
                poll_hook: Mutex::new(None),
                walk_hook: Mutex::new(None),
                mount_reads: AtomicU64::new(0),
                fail_mount_reads: AtomicUsize::new(0),
                fail_reconciles: AtomicUsize::new(0),
            }),
        }
    }

    /// Starts the manager, which starts the monitors in the background. Returns at once, and
    /// does nothing while a manager runs. Needs a tokio runtime.
    pub fn start(&self) {
        let mut state = self.inner.lock();
        if state.manager.is_none() {
            state.generation += 1;
            let generation = state.generation;
            #[cfg(target_os = "linux")]
            {
                state.follower = mount_watch::follow(&self.inner);
            }
            state.manager = Some(tokio::spawn(run_manager(
                Arc::clone(&self.inner),
                generation,
            )));
        }
    }

    /// Aborts the manager, the mount table follower and every monitor, and forgets the monitors.
    pub fn stop(&self) {
        let mut state = self.inner.lock();
        // A reconcile still running on another thread must not start monitors after this.
        state.generation += 1;
        if let Some(manager) = state.manager.take() {
            manager.abort();
        }
        if let Some(follower) = state.follower.take() {
            follower.abort();
        }
        for (_, monitor) in std::mem::take(&mut state.monitors) {
            monitor.task.abort();
            tracing::info!(
                "library monitor: stopped '{}' of '{}'",
                monitor.info.path.display(),
                monitor.info.library_name
            );
        }
    }

    /// Asks the manager to reload the libraries. Never blocks.
    pub fn libraries_changed(&self) {
        self.inner.reload.notify_one();
    }

    /// Sets the poll interval. Monitors that are sleeping apply it at once.
    pub fn set_poll_interval(&self, interval: Duration) {
        self.inner.interval.send_replace(interval);
    }

    /// The poll interval, 60 s by default.
    pub fn poll_interval(&self) -> Duration {
        *self.inner.interval.borrow()
    }

    /// Makes the monitors started from now on poll, whatever their filesystem.
    pub fn set_force_poll(&self, on: bool) {
        self.inner.lock().force_poll = on;
    }

    /// Makes the kernel monitors started from now on fail their watches once they hold `limit` of
    /// them, as if the kernel's watch limit were reached. For the tests.
    #[doc(hidden)]
    pub fn set_watch_limit(&self, limit: Option<usize>) {
        *self
            .inner
            .watch_limit
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = limit;
    }

    /// Makes each watching monitor of `library_id` act as if the kernel's queue had overflowed: it
    /// rescans its library and walks its folders again. For the tests.
    #[doc(hidden)]
    pub fn fake_overflow(&self, library_id: Uuid) {
        let state = self.inner.lock();
        for ((id, _), monitor) in &state.monitors {
            if *id == library_id && monitor.info.method == Method::Watch {
                monitor.overflow.notify_one();
            }
        }
    }

    /// Calls `hook` with the library path on the polling thread before each poll of a poll
    /// monitor, the baseline too; `None` removes it. A hook that blocks stands for a share that
    /// doesn't answer. For the tests.
    #[doc(hidden)]
    pub fn set_poll_hook(&self, hook: Option<PollHook>) {
        *self
            .inner
            .poll_hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = hook;
    }

    /// The follower's readings of the mount table so far. An injected failure counts as a reading.
    /// For the tests.
    #[doc(hidden)]
    pub fn mount_readings(&self) -> u64 {
        self.inner.mount_reads.load(Ordering::Relaxed)
    }

    /// Makes the next `n` readings of the mount table fail, as a failed read does; `n` replaces the
    /// count. The reading that `start` makes first is never failed. For the tests.
    #[doc(hidden)]
    pub fn fail_mount_reads(&self, n: usize) {
        self.inner.fail_mount_reads.store(n, Ordering::Relaxed);
    }

    /// Makes the next `n` reconciles fail as a database error does, so each keeps the monitors as
    /// they are; `n` replaces the count. For the tests.
    #[doc(hidden)]
    pub fn fail_reconciles(&self, n: usize) {
        self.inner.fail_reconciles.store(n, Ordering::Relaxed);
    }

    /// Calls `hook` with each folder before its kernel watch is added: in the setup walk, in the
    /// walks of new folders and after an overflow. It applies to the kernel watches started from
    /// now on; `None` removes it. The lock is not held while the hook runs. For the tests.
    #[doc(hidden)]
    pub fn set_walk_hook(&self, hook: Option<PollHook>) {
        *self
            .inner
            .walk_hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = hook;
    }

    /// The monitored library paths, sorted by library name, then path.
    pub fn monitored(&self) -> Vec<MonitoredPath> {
        let mut paths: Vec<MonitoredPath> = self
            .inner
            .lock()
            .monitors
            .values()
            .map(|monitor| monitor.info.clone())
            .collect();
        paths.sort_by(|a, b| (&a.library_name, &a.path).cmp(&(&b.library_name, &b.path)));
        paths
    }
}

impl Inner {
    /// The state. A poisoned lock is used anyway, since no step leaves it half-changed.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on monitor `id` of `key`, unless that monitor is no longer listed.
    fn with_monitor<T>(
        &self,
        key: &MonitorKey,
        id: u64,
        f: impl FnOnce(&mut Monitor) -> T,
    ) -> Option<T> {
        let mut state = self.lock();
        let monitor = state
            .monitors
            .get_mut(key)
            .filter(|monitor| monitor.id == id)?;
        Some(f(monitor))
    }
}

/// Takes one of the injected failures counted in `count`: counts one down and returns true, while
/// `count` is above zero.
pub(super) fn take_injected(count: &AtomicUsize) -> bool {
    count
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_ok()
}

/// Reconciles the monitors with the libraries, then waits for `libraries_changed`, for ever. After
/// a reconcile that fails, the next one is tried after a wait of `RETRY_FIRST`, doubled after each
/// failure in a row up to `RETRY_MAX`; a `libraries_changed` that comes first ends the wait early.
/// A reconcile that works resets the wait to `RETRY_FIRST`.
async fn run_manager(inner: Arc<Inner>, generation: u64) {
    let mut wait = RETRY_FIRST;
    loop {
        if reconcile(&inner, generation).await {
            wait = RETRY_FIRST;
            inner.reload.notified().await;
        } else {
            let _ = tokio::time::timeout(wait, inner.reload.notified()).await;
            wait = (wait * 2).min(RETRY_MAX);
        }
    }
}

/// Stops the monitor of `key`, if it has one: aborts its task and logs the stop.
fn stop_monitor(monitors: &mut BTreeMap<MonitorKey, Monitor>, key: &MonitorKey) {
    if let Some(monitor) = monitors.remove(key) {
        monitor.task.abort();
        tracing::info!(
            "library monitor: stopped '{}' of '{}'",
            key.1.display(),
            monitor.info.library_name
        );
    }
}

/// Brings the monitors in line with the libraries: stops the ones not wanted any more, starts
/// the missing ones and updates the library names and paths of the others. A monitor whose task
/// ended early, or whose path was missing at start or gone from its kernel watch, and has appeared
/// since, is replaced by a new one for its key. A monitor polling because a kernel limit is
/// reached is replaced too, ready or not, once a limit has changed since it fell back. While none
/// has changed, it is kept with its baseline. A database error keeps the monitors as they are and
/// returns false, so the manager tries again later; otherwise this returns true.
async fn reconcile(inner: &Arc<Inner>, generation: u64) -> bool {
    let loaded = if take_injected(&inner.fail_reconciles) {
        Err(sqlx::Error::Io(std::io::Error::other(
            "injected for a test",
        )))
    } else {
        monitored_libraries(&inner.pool).await
    };
    let libraries = match loaded {
        Ok(libraries) => libraries,
        Err(err) => {
            tracing::warn!(
                "library monitor: could not load the libraries ({err}); keeping the current monitors"
            );
            return false;
        }
    };
    let limits = Limits::read(inner);
    let mut guard = inner.lock();
    let state = &mut *guard;
    if state.generation != generation {
        return true; // stopped while the libraries were loaded
    }
    let wanted: HashSet<MonitorKey> = libraries
        .iter()
        .flat_map(|library| {
            library
                .paths
                .iter()
                .map(|path| (library.id, PathBuf::from(path)))
        })
        .collect();
    let gone: Vec<MonitorKey> = state
        .monitors
        .keys()
        .filter(|key| !wanted.contains(*key))
        .cloned()
        .collect();
    for key in gone {
        stop_monitor(&mut state.monitors, &key);
    }
    for library in &libraries {
        for path in &library.paths {
            let key = (library.id, PathBuf::from(path));
            if let Some(monitor) = state.monitors.get_mut(&key) {
                let replaced = monitor.task.is_finished()
                    || monitor.restart
                    || monitor
                        .limits
                        .as_ref()
                        .is_some_and(|at_fallback| *at_fallback != limits)
                    || (matches!(
                        monitor.info.reason.as_deref(),
                        Some(PATH_NOT_FOUND | LIBRARY_PATH_GONE)
                    ) && monitor.info.ready);
                if !replaced {
                    monitor.info.library_name.clone_from(&library.name);
                    monitor.library_paths.clone_from(&library.paths);
                    continue;
                }
            }
            stop_monitor(&mut state.monitors, &key);
            let id = state.next_id;
            state.next_id += 1;
            let overflow = Arc::new(Notify::new());
            // Spawned and inserted under one lock, so the monitor finds its entry when it runs.
            let task = tokio::spawn(run_monitor(
                Arc::clone(inner),
                key.clone(),
                library.name.clone(),
                id,
                state.force_poll,
                inner.interval.subscribe(),
                Arc::clone(&overflow),
            ));
            let info = MonitoredPath {
                library_id: library.id,
                library_name: library.name.clone(),
                path: key.1.clone(),
                method: Method::Poll,
                fs_type: "unknown".to_string(),
                reason: None,
                folders: 0,
                watches: 0,
                ready: false,
            };
            state.monitors.insert(
                key,
                Monitor {
                    id,
                    info,
                    canonical: None,
                    link_targets: Vec::new(),
                    limits: None,
                    restart: false,
                    library_paths: library.paths.clone(),
                    overflow,
                    task,
                },
            );
        }
    }
    true
}

/// The libraries with real-time monitoring on.
async fn monitored_libraries(pool: &SqlitePool) -> Result<Vec<Library>, sqlx::Error> {
    let mut monitored = Vec::new();
    for library in repo_library::get_all_libraries(pool).await? {
        if repo_library::get_library_options(pool, library.id)
            .await?
            .enable_realtime_monitor
        {
            monitored.push(library);
        }
    }
    Ok(monitored)
}

/// Sets up the monitor of one library path, then watches it with the kernel, or polls it, until
/// the monitor is aborted. A failed setup or poll ends the task instead, and the next reload
/// replaces the monitor. A watch that the kernel gives up on becomes a poll.
async fn run_monitor(
    inner: Arc<Inner>,
    key: MonitorKey,
    library_name: String,
    id: u64,
    force_poll: bool,
    mut interval: watch::Receiver<Duration>,
    overflow: Arc<Notify>,
) {
    let path = key.1.clone();
    // Canonicalizing can hang on a dead share, so the setup runs on a blocking thread.
    let setup_path = path.clone();
    let data_dir = inner.data_dir.clone();
    let setup = off_runtime(move || {
        let choice = choose(&setup_path, force_poll);
        (choice, FolderPoller::new(setup_path, Some(data_dir)))
    })
    .await;
    let (choice, mut poller) = match setup {
        Ok(setup) => setup,
        Err(err) => {
            tracing::warn!(
                "library monitor: could not set up '{}': {err}; restarting it at the next library change",
                path.display()
            );
            return;
        }
    };
    let Choice {
        method,
        fs_type,
        reason,
        canonical,
    } = choice;
    let listed = inner.with_monitor(&key, id, |monitor| {
        monitor.info.method = method;
        monitor.info.fs_type.clone_from(&fs_type);
        monitor.info.reason = reason.map(str::to_string);
        monitor.canonical = canonical;
    });
    if listed.is_none() {
        return;
    }
    let reason = match reason {
        Some(reason) => reason.to_string(),
        None => {
            let Some(reason) =
                watch_kernel(&inner, &key, &library_name, &fs_type, id, &overflow).await
            else {
                return;
            };
            // The kernel gave up on the path: it is polled from now on, for this reason. The
            // limits are read before the state lock is taken, and kept for a limit reason only.
            let limits = match reason.as_str() {
                WATCH_LIMIT_REACHED | INSTANCE_LIMIT_REACHED | FILE_LIMIT_REACHED => {
                    Some(Limits::read(&inner))
                }
                _ => None,
            };
            let listed = inner.with_monitor(&key, id, |monitor| {
                monitor.info.method = Method::Poll;
                monitor.info.reason = Some(reason.clone());
                monitor.info.folders = 0;
                monitor.info.watches = 0;
                monitor.info.ready = false;
                monitor.limits = limits;
            });
            if listed.is_none() {
                return;
            }
            reason
        }
    };

    let mut logged_baseline = false;
    loop {
        // The lock ends with this statement, so the hook runs without it held.
        let poll_hook = inner
            .poll_hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let hook_path = path.clone();
        let (returned, reported, took) = match off_runtime(move || {
            if let Some(hook) = poll_hook {
                hook(&hook_path);
            }
            let started = std::time::Instant::now();
            let reported = poller.poll();
            (poller, reported, started.elapsed())
        })
        .await
        {
            Ok(done) => done,
            Err(err) => {
                inner.with_monitor(&key, id, |monitor| monitor.info.ready = false);
                tracing::warn!(
                    "library monitor: polling '{}' failed: {err}; restarting it at the next library change",
                    path.display()
                );
                return;
            }
        };
        poller = returned;
        // The interval counts from the end of the poll.
        let finished = tokio::time::Instant::now();
        let folders = poller.folders();
        let ready = poller.has_baseline();
        let listed = inner.with_monitor(&key, id, |monitor| {
            monitor.info.folders = folders;
            monitor.info.ready = ready;
        });
        if listed.is_none() {
            return;
        }
        let ms = took.as_millis();
        tracing::debug!(
            "library monitor: polled '{}': {folders} folders in {ms} ms, {} reported",
            path.display(),
            reported.len()
        );
        if ready && !logged_baseline {
            logged_baseline = true;
            tracing::info!(
                "library monitor: polling '{}' of '{}' ({fs_type}, {reason}): {folders} folders, every {:?}, baseline in {ms} ms",
                path.display(),
                library_name,
                *interval.borrow()
            );
        }
        if !reported.is_empty() {
            let Some(library_paths) =
                inner.with_monitor(&key, id, |monitor| monitor.library_paths.clone())
            else {
                return;
            };
            let units = coalesce(&library_paths, &reported);
            if !units.is_empty() {
                inner.changes.report_all(&units).await;
            }
        }
        sleep_between_polls(finished, &mut interval).await;
    }
}

/// Watches the path with the kernel's notifications until the kernel gives up on it. Returns the
/// reason to poll with, or None when the monitor is gone.
#[cfg(target_os = "linux")]
async fn watch_kernel(
    inner: &Inner,
    key: &MonitorKey,
    library_name: &str,
    fs_type: &str,
    id: u64,
    overflow: &Notify,
) -> Option<String> {
    kernel::watch(inner, key, library_name, fs_type, id, overflow).await
}

/// Only Linux has kernel watching. `choose` gives every other path a reason to poll, so this is
/// never reached there.
#[cfg(not(target_os = "linux"))]
async fn watch_kernel(
    _inner: &Inner,
    _key: &MonitorKey,
    _library_name: &str,
    _fs_type: &str,
    _id: u64,
    _overflow: &Notify,
) -> Option<String> {
    unreachable!("choose polls every path on this OS")
}

/// Sleeps until `interval` has passed since `last`, at once if it already has. A change of the
/// interval wakes the sleep, which then counts from `last` again.
async fn sleep_between_polls(last: tokio::time::Instant, interval: &mut watch::Receiver<Duration>) {
    loop {
        let every = *interval.borrow_and_update();
        tokio::select! {
            () = tokio::time::sleep_until(last + every) => return,
            changed = interval.changed() => {
                if changed.is_err() {
                    // The sender lives in the same `Inner` as this monitor, so this does not
                    // happen. If it did, the current interval still applies.
                    tokio::time::sleep_until(last + every).await;
                    return;
                }
            }
        }
    }
}

/// Runs `work` on a new thread and waits for its result. Unlike `spawn_blocking`, the runtime
/// doesn't wait for the thread when it shuts down, so a read of a share that doesn't answer
/// doesn't hold up the server's exit; the thread ends when the read does. Errs when the thread
/// can't be started or `work` panics.
pub(super) async fn off_runtime<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<T> {
    let (done, result) = oneshot::channel();
    std::thread::Builder::new()
        .name("library-monitor".into())
        .spawn(move || {
            // Nobody waits for a result once its caller is gone, so the result is dropped here.
            let _ = done.send(work());
        })?;
    result
        .await
        .map_err(|_| std::io::Error::other("the library monitor thread panicked"))
}

/// How a path is monitored, and why, and its filesystem type. It reads the disk, so it runs on a
/// blocking thread. A path on a local filesystem is watched on Linux; any other path is polled,
/// and the reason says why.
fn choose(path: &Path, force_poll: bool) -> Choice {
    let canonical = std::fs::canonicalize(path).ok();
    let fs_type = canonical.as_deref().and_then(fs_type_of);
    let reason = if force_poll {
        Some("forced")
    } else if !cfg!(target_os = "linux") {
        Some("no kernel watching on this OS")
    } else if canonical.is_none() {
        Some(PATH_NOT_FOUND)
    } else if !fs_type.as_deref().is_some_and(is_local_fs_type) {
        Some(NOT_LOCAL)
    } else {
        None
    };
    Choice {
        method: if reason.is_some() {
            Method::Poll
        } else {
            Method::Watch
        },
        fs_type: fs_type.unwrap_or_else(|| "unknown".to_string()),
        reason,
        canonical,
    }
}

/// The paths to report for a poll: the first path of each unit, in input order. A unit is what
/// `unit_for` says a change rescans. A path with no unit is dropped.
pub fn coalesce(library_paths: &[String], paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut units = HashSet::new();
    paths
        .iter()
        .filter(|path| unit_for(library_paths, path).is_some_and(|unit| units.insert(unit)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::open_file_limit;

    #[test]
    fn the_open_file_limit_is_the_soft_limit_on_its_line() {
        let limits = "Limit                     Soft Limit           Hard Limit           Units     \n\
                      Max cpu time              unlimited            unlimited            seconds   \n\
                      Max open files            1024                 524288               files     \n\
                      Max locked memory         8388608              8388608              bytes     \n";
        assert_eq!(open_file_limit(limits), Some(1024));
        let unlimited = limits.replace("1024     ", "unlimited");
        assert_eq!(open_file_limit(&unlimited), Some(u64::MAX));
        assert_eq!(
            open_file_limit("Max cpu time  unlimited  unlimited  seconds\n"),
            None
        );
        assert_eq!(
            open_file_limit("Max open files  many  524288  files\n"),
            None
        );
        assert_eq!(open_file_limit(""), None);
    }
}
