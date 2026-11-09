//! The library monitor's change queue (B18). It maps each path that changed to the unit of a library
//! that rescans it. A folder unit takes the name that its folder has on disk when it is rescanned,
//! not when it is reported. The unit waits until it has been quiet for the settle delay, then it is
//! rescanned. A pending full scan keeps the instant at which it became pending, so the reports that
//! it takes in do not postpone it. One worker runs the due rescans one at a time, oldest report
//! first.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use rustyfin_model::library::Library;
use sqlx::SqlitePool;
use tokio::sync::Notify;
use tokio::time::Instant;
use uuid::Uuid;

use crate::naming;
use crate::scan_progress::ScanTicket;
use crate::scanner::{self, AUDIO_EXTENSIONS, DirId, ScanChanges, ScanScope};

/// How many pending units one library keeps. One more of them becomes a full scan of the library.
pub const FULL_SCAN_LIMIT: usize = 50;

/// How many paths one request to `POST /Library/Media/Updated` or the Webhook may report.
pub const MAX_REPORTED_PATHS: usize = 10_000;

/// Rescans one unit of a library. The API attaches a runner that also clears its caches and sends
/// `LibraryChanged`.
pub trait RescanRunner: Send + Sync {
    /// Rescans `scope` of `library`. Returns what the scan changed, or why it failed.
    fn rescan(
        &self,
        library: Library,
        scope: ScanScope,
    ) -> Pin<Box<dyn Future<Output = Result<ScanChanges, String>> + Send>>;
}

/// The queue of changed units. Clones share one queue.
#[derive(Clone)]
pub struct ChangeQueue {
    inner: Arc<Inner>,
}

struct Inner {
    pool: SqlitePool,
    runner: RwLock<Arc<dyn RescanRunner>>,
    delay: RwLock<Duration>,
    /// Each pending unit, with the instant of its last report. A pending full scan keeps the
    /// instant at which it became pending.
    pending: Mutex<HashMap<(Uuid, ScanScope), Instant>>,
    notify: Notify,
    /// Set once the worker has been spawned.
    worker: AtomicBool,
}

impl ChangeQueue {
    /// A queue on `pool`, with the default runner (`scan_with_metadata` and no metadata providers)
    /// and a settle delay of 60 s.
    pub fn new(pool: SqlitePool) -> Self {
        let runner: Arc<dyn RescanRunner> = Arc::new(DefaultRunner {
            pool: pool.clone(),
            http_client: reqwest::Client::new(),
        });
        Self {
            inner: Arc::new(Inner {
                pool,
                runner: RwLock::new(runner),
                delay: RwLock::new(Duration::from_secs(60)),
                pending: Mutex::new(HashMap::new()),
                notify: Notify::new(),
                worker: AtomicBool::new(false),
            }),
        }
    }

    /// Sets the runner that rescans the units.
    pub fn set_runner(&self, runner: Arc<dyn RescanRunner>) {
        *self
            .inner
            .runner
            .write()
            .unwrap_or_else(PoisonError::into_inner) = runner;
    }

    /// Sets how long a unit must stay unreported before it is rescanned. Pending units wait for the
    /// new delay too.
    pub fn set_delay(&self, delay: Duration) {
        *self
            .inner
            .delay
            .write()
            .unwrap_or_else(PoisonError::into_inner) = delay;
        self.inner.notify.notify_one();
    }

    /// How long a unit must stay unreported before it is rescanned.
    pub fn delay(&self) -> Duration {
        self.inner.delay()
    }

    /// Queues the unit of every library that holds `path`. Returns true when at least one library
    /// queued it. A path that no library holds, or that lies in a folder the scanner skips, is
    /// dropped. It reads nothing on disk, so a hung mount cannot hold up a report. Units are
    /// queued under the reported names, and the worker names them on disk just before the rescan.
    pub async fn report(&self, path: &Path) -> bool {
        self.report_all(&[path.to_path_buf()])
            .await
            .first()
            .copied()
            .unwrap_or(false)
    }

    /// Queues the units of `paths`. Returns one answer per path, in order: true when at least one
    /// library queued it. The libraries are loaded once for all paths. It reads nothing on disk, so
    /// a hung mount cannot hold up a report. Units are queued under the reported names, and the
    /// worker names them on disk just before the rescan.
    pub async fn report_all(&self, paths: &[PathBuf]) -> Vec<bool> {
        if paths.is_empty() {
            return Vec::new();
        }
        let libraries = match rustyfin_db::repo_library::get_all_libraries(&self.inner.pool).await {
            Ok(libraries) => libraries,
            Err(e) => {
                tracing::warn!(
                    "library monitor: could not load the libraries for {} paths: {e}",
                    paths.len()
                );
                return vec![false; paths.len()];
            }
        };
        let mut answers = Vec::with_capacity(paths.len());
        let mut units = Vec::new();
        for path in paths {
            let found: Vec<(Uuid, ScanScope)> = libraries
                .iter()
                .filter_map(|library| {
                    unit_for(&library.paths, path).map(|scope| (library.id, scope))
                })
                .collect();
            if found.is_empty() {
                tracing::debug!(
                    "library monitor: {} is in no library or in an ignored folder",
                    path.display()
                );
            }
            answers.push(!found.is_empty());
            units.extend(found);
        }
        if units.is_empty() {
            return answers;
        }

        let now = Instant::now();
        {
            let mut pending = self.inner.lock_pending();
            for (library, scope) in units {
                queue_unit(&mut pending, library, scope, now);
            }
        }
        if !self.inner.worker.swap(true, Ordering::SeqCst) {
            tokio::spawn(work(self.inner.clone()));
        }
        self.inner.notify.notify_one();
        answers
    }

    /// The pending units, sorted. For diagnostics and tests.
    pub fn pending(&self) -> Vec<(Uuid, ScanScope)> {
        let mut units: Vec<(Uuid, ScanScope)> = self.inner.lock_pending().keys().cloned().collect();
        units.sort();
        units
    }
}

/// Queues `scope` of `library` at `now`. A pending full scan of the library takes the unit in and
/// keeps the instant at which it became pending. More than `FULL_SCAN_LIMIT` units of one library
/// become one full scan, pending from `now`.
fn queue_unit(
    pending: &mut HashMap<(Uuid, ScanScope), Instant>,
    library: Uuid,
    scope: ScanScope,
    now: Instant,
) {
    if pending.contains_key(&(library, ScanScope::Library)) {
        return;
    }
    if scope == ScanScope::Library {
        full_scan(pending, library, now);
    } else {
        pending.insert((library, scope), now);
        if pending.keys().filter(|(id, _)| *id == library).count() > FULL_SCAN_LIMIT {
            full_scan(pending, library, now);
        }
    }
}

/// Replaces the pending units of `library` with one full scan of it, which is pending from `now`.
fn full_scan(pending: &mut HashMap<(Uuid, ScanScope), Instant>, library: Uuid, now: Instant) {
    pending.retain(|(id, _), _| *id != library);
    pending.insert((library, ScanScope::Library), now);
}

/// The name that `folder` has on disk. `folder` is `root.join(name)` for its library path `root`,
/// which is listed here, once. A name that the listing holds stays. Otherwise, when `folder`
/// exists, it takes the name of a listed folder with the same identity: a name that ignores case
/// first, then the others, each group in sorted order.
async fn folder_on_disk(folder: &Path) -> PathBuf {
    let (Some(root), Some(name)) = (folder.parent(), folder.file_name()) else {
        return folder.to_path_buf();
    };
    let Some(names) = list_names(root).await else {
        return folder.to_path_buf();
    };
    if names
        .binary_search_by(|known| known.as_os_str().cmp(name))
        .is_ok()
    {
        return folder.to_path_buf();
    }
    let Ok(id) = scanner::dir_id(folder).await else {
        return folder.to_path_buf();
    };
    let by_id = index_by_id(root, &names).await;
    let Some(same) = by_id.get(&id) else {
        return folder.to_path_buf();
    };
    let wanted = name.to_string_lossy().to_lowercase();
    let chosen = same
        .iter()
        .find(|candidate| candidate.to_string_lossy().to_lowercase() == wanted)
        .or(same.first());
    chosen.map_or_else(|| folder.to_path_buf(), |chosen| root.join(chosen))
}

/// The names in the folder `root`, sorted. None when it cannot be listed.
async fn list_names(root: &Path) -> Option<Vec<OsString>> {
    let mut entries = tokio::fs::read_dir(root).await.ok()?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await.ok()? {
        names.push(entry.file_name());
    }
    names.sort();
    Some(names)
}

/// The names in `root` by the identity of the folder that each names, each list in the order of
/// `names`. A name whose identity cannot be read is left out.
async fn index_by_id(root: &Path, names: &[OsString]) -> HashMap<DirId, Vec<OsString>> {
    let mut by_id: HashMap<DirId, Vec<OsString>> = HashMap::new();
    for name in names {
        if let Ok(id) = scanner::dir_id(&root.join(name)).await {
            by_id.entry(id).or_default().push(name.clone());
        }
    }
    by_id
}

/// The scope to rescan, as it is on disk now. A folder unit that names a file becomes the root
/// files of its library path. Any other folder unit takes the name that its folder has on disk,
/// and every other scope comes back unchanged. Two limits are known. A folder directly in a
/// library path whose name ends in a video or audio extension rescans the root files. A folder
/// reported in two cases, on a case-insensitive mount, is rescanned twice, each time by its name
/// on disk.
async fn on_disk(scope: ScanScope) -> ScanScope {
    match scope {
        ScanScope::Folder(folder) => {
            let is_file = tokio::fs::metadata(&folder)
                .await
                .is_ok_and(|meta| meta.is_file());
            match folder.parent() {
                Some(parent) if is_file => ScanScope::RootFiles(parent.to_path_buf()),
                _ => ScanScope::Folder(folder_on_disk(&folder).await),
            }
        }
        other => other,
    }
}

/// The runner that a new queue starts with: `scan_with_metadata`, with no metadata providers.
struct DefaultRunner {
    pool: SqlitePool,
    http_client: reqwest::Client,
}

impl RescanRunner for DefaultRunner {
    fn rescan(
        &self,
        library: Library,
        scope: ScanScope,
    ) -> Pin<Box<dyn Future<Output = Result<ScanChanges, String>> + Send>> {
        let pool = self.pool.clone();
        let http_client = self.http_client.clone();
        Box::pin(async move {
            scanner::scan_with_metadata(
                &pool,
                &library,
                &scope,
                &http_client,
                None,
                None,
                None,
                &ScanTicket::untracked(),
            )
            .await
            .map_err(|e| e.to_string())
        })
    }
}

/// What the worker does next.
enum Next {
    /// A unit that is due. It has been taken out of the pending map.
    Run(Uuid, ScanScope),
    /// Nothing is due. The first pending unit becomes due at this instant.
    Sleep(Instant),
    /// Nothing is pending.
    Idle,
}

impl Inner {
    fn delay(&self) -> Duration {
        *self.delay.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn current_runner(&self) -> Arc<dyn RescanRunner> {
        self.runner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn lock_pending(&self) -> MutexGuard<'_, HashMap<(Uuid, ScanScope), Instant>> {
        // A panic under the lock leaves the map usable, so a poisoned lock is still used.
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the oldest unit that is due out of the pending map, or says when the next one is due.
    fn next(&self) -> Next {
        let delay = self.delay();
        let now = Instant::now();
        let mut pending = self.lock_pending();
        let oldest_due = pending
            .iter()
            .filter(|(_, last)| **last + delay <= now)
            .min_by_key(|(_, last)| **last)
            .map(|(unit, _)| unit.clone());
        if let Some(unit) = oldest_due {
            pending.remove(&unit);
            return Next::Run(unit.0, unit.1);
        }
        match pending.values().min() {
            Some(last) => Next::Sleep(*last + delay),
            None => Next::Idle,
        }
    }

    /// Rescans `scope` of the library `library_id` and logs how it ended.
    async fn run(&self, library_id: Uuid, scope: ScanScope) {
        let library = match rustyfin_db::repo_library::get_library_by_id(&self.pool, library_id)
            .await
        {
            Ok(Some(library)) => library,
            Ok(None) => {
                tracing::debug!(
                    "library monitor: library {library_id} was deleted, so {scope:?} is skipped"
                );
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "library monitor: could not load library {library_id}, so {scope:?} is skipped: {e}"
                );
                return;
            }
        };
        let scope = on_disk(scope).await;
        let name = library.name.clone();
        tracing::info!("library monitor: rescanning {scope:?} of '{name}'");
        let runner = self.current_runner();
        let rescan_scope = scope.clone();
        let started = std::time::Instant::now();
        // A task of its own, so that a runner which panics before it returns its future is caught.
        let rescan = tokio::spawn(async move { runner.rescan(library, rescan_scope).await });
        match rescan.await {
            Ok(Ok(changes)) => tracing::info!(
                "library monitor: rescanned {scope:?} of '{name}' in {} ms: {} added, {} removed, {} re-read",
                started.elapsed().as_millis(),
                changes.added,
                changes.removed,
                changes.reread,
            ),
            Ok(Err(e)) => {
                tracing::warn!("library monitor: rescan of {scope:?} of '{name}' failed: {e}")
            }
            Err(e) => {
                tracing::warn!("library monitor: rescan of {scope:?} of '{name}' panicked: {e}")
            }
        }
    }
}

/// The worker. It runs the due units one at a time, and sleeps until the next unit is due or a
/// report or a new delay wakes it.
async fn work(inner: Arc<Inner>) {
    loop {
        match inner.next() {
            Next::Run(library_id, scope) => inner.run(library_id, scope).await,
            Next::Sleep(due) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(due) => {}
                    _ = inner.notify.notified() => {}
                }
            }
            Next::Idle => inner.notify.notified().await,
        }
    }
}

/// The unit that a change at `path` rescans, for a library whose paths are `library_paths`. It
/// reads nothing on disk.
///
/// The longest library path that holds `path` decides. `path` equal to it is the whole library.
/// Below it, the unit is the top-level folder that `path` lies in, with its subfolders. A single
/// name below the library path is the root files when it ends in a video or audio extension, else
/// a folder. The worker turns a folder unit that names a file into the root files. Returns `None`
/// when no library path holds `path`, or when a name on the way is one that the scanner skips.
pub fn unit_for(library_paths: &[String], path: &Path) -> Option<ScanScope> {
    let root = library_paths
        .iter()
        .map(Path::new)
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())?;
    let names: Vec<&OsStr> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(Component::as_os_str)
        .collect();
    if names
        .iter()
        .any(|name| naming::is_ignored_dir(&name.to_string_lossy()))
    {
        return None;
    }
    match names.as_slice() {
        [] => Some(ScanScope::Library),
        [first, _, ..] => Some(ScanScope::Folder(root.join(first))),
        [only] => {
            if is_media_name(only) {
                Some(ScanScope::RootFiles(root.to_path_buf()))
            } else {
                Some(ScanScope::Folder(root.join(only)))
            }
        }
    }
}

/// Whether `name` ends in a video or an audio extension, in any case.
fn is_media_name(name: &OsStr) -> bool {
    // A `.bin` counts only when it is named as an episode (`scanner::is_episode_file`).
    scanner::is_episode_file(Path::new(name))
        || Path::new(name)
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_lowercase)
            .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext.as_str()))
}
