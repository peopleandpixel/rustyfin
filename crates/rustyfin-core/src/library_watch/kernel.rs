//! Watching a library path with the kernel's change notifications (inotify, Linux). Every folder
//! under the path has a watch. Events are batched by unit and reported to the change queue. A path
//! that cannot be watched is handed to the poller instead.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};
use tokio::io::unix::AsyncFd;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::{
    FILE_LIMIT_REACHED, INSTANCE_LIMIT_REACHED, Inner, LIBRARY_PATH_GONE, MonitorKey, PollHook,
    WATCH_LIMIT_REACHED, fs_type_from_mountinfo, is_local_fs_type, off_runtime,
};
use crate::library_monitor::unit_for;
use crate::naming::is_ignored_dir;
use crate::scanner::ScanScope;

/// The events a folder's watch reports. Plain writes, reads and attribute changes raise nothing.
const WATCH_MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF)
    .union(WatchMask::ONLYDIR);

/// How long a batch collects events before it is reported.
const BATCH_DELAY: Duration = Duration::from_millis(500);

/// Bytes read from the kernel at a time. An event takes a few hundred bytes at most.
const READ_BUFFER: usize = 16 * 1024;

/// `EMFILE` on Linux: the limit on inotify instances, or on open files, is reached. libc is not a
/// dependency of this crate on Linux, so the errno is written out.
const EMFILE: i32 = 24;

/// A folder's identity: device and inode. Two paths with one identity are one folder.
type Id = (u64, u64);

/// A watched folder.
struct Folder {
    wd: WatchDescriptor,
    id: Id,
}

/// The folders under a library path, each with its watch. The monitor moves it into the blocking
/// threads that walk folders, and takes it back.
struct Tree {
    root: PathBuf,
    watches: Watches,
    /// The watched folders by path.
    folders: BTreeMap<PathBuf, Folder>,
    /// The folder of each watch.
    paths: HashMap<WatchDescriptor, PathBuf>,
    /// The identities of the watched folders and of the data dir. A folder with one of them is
    /// not watched again.
    ids: HashSet<Id>,
    /// The test limit of watches, if one is set: an add fails once the tree holds this many.
    limit: Option<usize>,
    /// The hook of `set_walk_hook`, called with each folder before its watch is added.
    walk_hook: Option<PollHook>,
    /// The fallback that ends the kernel watching, once a walk or an event meets it. The walks stop
    /// then, and the events after it only report their paths.
    fallback: Option<Fallback>,
    /// Set once the monitor's task is gone, and a walk under way then stops at the next folder.
    stopped: Arc<AtomicBool>,
    /// The mount table, read by the first check of the walk under way. None between walks.
    mounts: Option<String>,
    /// The canonical paths of the symlinked folders checked so far, local or not. Forgetting a
    /// folder keeps its target, so a share mounted there again is still matched; see
    /// `mount_watch::mark_restarts`.
    link_targets: BTreeSet<PathBuf>,
}

/// What a read of the kernel's queue gives: an event copied out of the read buffer, or the
/// overflow of the queue. A `fake_overflow` gives one too.
enum Seen {
    Overflow,
    Event {
        wd: WatchDescriptor,
        mask: EventMask,
        name: Option<OsString>,
    },
}

/// Why the kernel watching of a path ends. The monitor then polls the path, for this reason.
enum Fallback {
    /// The kernel's watch limit is reached (`ENOSPC`, `StorageFull`), or the test limit is.
    WatchLimit,
    /// The kernel's limit on inotify instances is reached (`EMFILE` from `Inotify::init`).
    InstanceLimit,
    /// The process's own limit on open files is reached: `Inotify::init` gives `EMFILE`, and so
    /// does a plain open of `/dev/null`.
    FileLimit,
    /// The library path's own watch reports it deleted, moved, unmounted or ignored.
    LibraryPathGone,
    /// A symlinked folder, or a mount inside the library, is not on a local filesystem. The reason.
    NotLocal(String),
    /// Any other failure of the kernel's watching, with its message.
    Failed(String),
}

/// Sets its flag when it is dropped. `watch` holds one for as long as it runs, so an abort, which
/// drops the future of `watch`, stops a walk under way at its next folder.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Watches the library path of the monitor `key` until the monitor has to poll it instead, and
/// returns the reason, which the poller logs. Returns None when the monitor is no longer listed,
/// or when its task failed, which is logged. The next reload then replaces the monitor.
pub(super) async fn watch(
    inner: &Inner,
    key: &MonitorKey,
    library_name: &str,
    fs_type: &str,
    id: u64,
    overflow: &Notify,
) -> Option<String> {
    let root = key.1.clone();
    let setup_root = root.clone();
    let data_dir = inner.data_dir.clone();
    let limit = *inner
        .watch_limit
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let walk_hook = inner
        .walk_hook
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    // Set when this future is dropped, as an abort does: the setup walk then stops at its next
    // folder, and off_runtime drops what it made.
    let stopped = Arc::new(AtomicBool::new(false));
    let _stop_walks = StopOnDrop(Arc::clone(&stopped));
    let setup = off_runtime(move || setup(&setup_root, &data_dir, limit, walk_hook, stopped)).await;
    let (inotify, mut tree, took) = match setup {
        Ok(Ok(setup)) => setup,
        Ok(Err(fallback)) => return Some(fallback_reason(inner, &root, fallback)),
        Err(err) => {
            tracing::warn!(
                "library monitor: could not set up '{}': {err}; restarting it at the next library change",
                root.display()
            );
            return None;
        }
    };
    let folders = tree.folders.len();
    // The symlink targets are published with the counts, and again after each batch that adds some.
    let mut published = tree.link_targets.len();
    inner.with_monitor(key, id, |monitor| {
        monitor.info.folders = folders;
        monitor.info.watches = folders;
        monitor.info.ready = true;
        monitor.link_targets = tree.link_targets.iter().cloned().collect();
    })?;
    tracing::info!(
        "library monitor: watching '{}' of '{library_name}' ({fs_type}): {folders} folders, {folders} watches, ready in {} ms",
        root.display(),
        took.as_millis()
    );

    // SAFETY: `inotify` moves into the `AsyncFd`, which owns it for its whole life. Nothing else
    // closes or replaces its file descriptor.
    let mut fd = match unsafe { AsyncFd::register(inotify) } {
        Ok(fd) => fd,
        Err(err) => {
            let (_, cause) = err.into_parts();
            return Some(format!("kernel watching failed: {cause}"));
        }
    };
    let library_paths = [root.to_string_lossy().into_owned()];
    let mut batch: BTreeMap<ScanScope, PathBuf> = BTreeMap::new();
    // When the batch is reported. Set only while the batch is not empty.
    let mut due: Option<Instant> = None;
    let mut buf = vec![0u8; READ_BUFFER];
    loop {
        let flush = async move {
            match due {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        let seen = tokio::select! {
            read = read_ready(&mut fd, &mut buf) => match read {
                Ok(seen) => seen,
                Err(err) => {
                    report(inner, &mut batch).await;
                    return Some(format!("kernel watching failed: {err}"));
                }
            },
            () = overflow.notified() => match tree.folders.get(&root) {
                Some(folder) => vec![seen_of(folder.wd.clone(), EventMask::Q_OVERFLOW, None)],
                None => Vec::new(),
            },
            () = flush => {
                report(inner, &mut batch).await;
                due = None;
                Vec::new()
            }
        };
        if seen.is_empty() {
            continue;
        }
        if seen.iter().any(|event| matches!(event, Seen::Overflow)) {
            tracing::warn!(
                "library monitor: the event queue of '{}' overflowed; rescanning '{library_name}'",
                root.display()
            );
        }
        let (returned, reported) = match apply_on_thread(tree, seen).await {
            Ok(done) => done,
            Err(err) => {
                inner.with_monitor(key, id, |monitor| monitor.info.ready = false);
                tracing::warn!(
                    "library monitor: watching '{}' failed: {err}; restarting it at the next library change",
                    root.display()
                );
                return None;
            }
        };
        tree = returned;
        for path in reported {
            let Some(unit) = unit_for(&library_paths, &path) else {
                continue;
            };
            if batch.is_empty() {
                due = Some(Instant::now() + BATCH_DELAY);
            }
            batch.entry(unit).or_insert(path);
        }
        if let Some(fallback) = tree.fallback.take() {
            // The batch goes out first, so the paths of this chunk are rescanned.
            report(inner, &mut batch).await;
            return Some(fallback_reason(inner, &root, fallback));
        }
        let folders = tree.folders.len();
        // The set only grows, so its length tells whether it grew since it was last published.
        let grown = tree.link_targets.len() != published;
        published = tree.link_targets.len();
        inner.with_monitor(key, id, |monitor| {
            monitor.info.folders = folders;
            monitor.info.watches = folders;
            if grown {
                monitor.link_targets = tree.link_targets.iter().cloned().collect();
            }
        })?;
    }
}

/// Starts an inotify instance and watches the library path, with the folders under it. Runs on a
/// blocking thread. Returns the instance, the tree and how long the setup took, or the fallback
/// that ends the kernel watching: the instance or a watch could not be had.
fn setup(
    root: &Path,
    data_dir: &Path,
    limit: Option<usize>,
    walk_hook: Option<PollHook>,
    stopped: Arc<AtomicBool>,
) -> Result<(Inotify, Tree, Duration), Fallback> {
    let started = std::time::Instant::now();
    let inotify = Inotify::init().map_err(|err| {
        if err.raw_os_error() != Some(EMFILE) {
            return Fallback::Failed(err.to_string());
        }
        // Both the kernel's limit on instances and the process's limit on open files give
        // EMFILE. A plain open fails only at the file limit, so it tells the two apart.
        match fs::File::open("/dev/null") {
            Err(open_err) if open_err.raw_os_error() == Some(EMFILE) => Fallback::FileLimit,
            Ok(probe) => {
                drop(probe);
                Fallback::InstanceLimit
            }
            Err(_) => Fallback::InstanceLimit,
        }
    })?;
    let meta = fs::metadata(root).map_err(|err| Fallback::Failed(err.to_string()))?;
    let mut tree = Tree {
        root: root.to_path_buf(),
        watches: inotify.watches(),
        folders: BTreeMap::new(),
        paths: HashMap::new(),
        // The data dir is left out of every walk, from the start.
        ids: fs::metadata(data_dir)
            .ok()
            .map(|meta| (meta.dev(), meta.ino()))
            .into_iter()
            .collect(),
        limit,
        walk_hook,
        fallback: None,
        stopped,
        mounts: None,
        link_targets: BTreeSet::new(),
    };
    tree.watch_folder(root, (meta.dev(), meta.ino()))
        .map_err(add_failure)?;
    // The walk of the library path may have met a fallback. The instance goes with the tree.
    match tree.fallback.take() {
        Some(fallback) => Err(fallback),
        None => Ok((inotify, tree, started.elapsed())),
    }
}

/// The fallback for a failed add of a watch: the watch limit when the kernel has no more watches
/// to give, and the failure otherwise.
fn add_failure(err: io::Error) -> Fallback {
    if err.kind() == io::ErrorKind::StorageFull {
        Fallback::WatchLimit
    } else {
        Fallback::Failed(err.to_string())
    }
}

/// The reason a monitor of `root` polls instead, once its kernel watching ends with `fallback`.
/// Logs the warning of the watch limit, the instance limit or the file limit, once each in the
/// watcher.
fn fallback_reason(inner: &Inner, root: &Path, fallback: Fallback) -> String {
    match fallback {
        Fallback::WatchLimit => {
            if !inner.watch_limit_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "library monitor: the kernel's inotify watch limit is reached; polling '{}' instead. Raise fs.inotify.max_user_watches to watch it; the next SIGHUP or library change tries again.",
                    root.display()
                );
            }
            WATCH_LIMIT_REACHED.to_string()
        }
        Fallback::InstanceLimit => {
            if !inner.instance_limit_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "library monitor: the kernel's inotify instance limit is reached; polling '{}' instead. Raise fs.inotify.max_user_instances to watch it; the next SIGHUP or library change tries again.",
                    root.display()
                );
            }
            INSTANCE_LIMIT_REACHED.to_string()
        }
        Fallback::FileLimit => {
            if !inner.file_limit_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "library monitor: the open file limit of the process is reached; polling '{}' instead. Raise it and restart Rustyfin (ulimit -n, systemd LimitNOFILE=, docker run --ulimit nofile=), or raise it live with prlimit --pid; a SIGHUP or library change then tries again.",
                    root.display()
                );
            }
            FILE_LIMIT_REACHED.to_string()
        }
        Fallback::LibraryPathGone => LIBRARY_PATH_GONE.to_string(),
        Fallback::NotLocal(reason) => reason,
        Fallback::Failed(err) => format!("kernel watching failed: {err}"),
    }
}

/// Waits until the kernel has events, then reads all of them. Copies them out of the buffer.
async fn read_ready(fd: &mut AsyncFd<Inotify>, buf: &mut [u8]) -> io::Result<Vec<Seen>> {
    let mut ready = fd.readable_mut().await?;
    let mut seen = Vec::new();
    loop {
        match ready.get_inner_mut().read_events(buf) {
            Ok(events) => {
                for event in events {
                    seen.push(seen_of(
                        event.wd,
                        event.mask,
                        event.name.map(OsStr::to_os_string),
                    ));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                ready.clear_ready();
                return Ok(seen);
            }
            Err(err) => return Err(err),
        }
    }
}

/// What an event read from the kernel's queue is: the overflow of the queue, or an event of a
/// watched folder. A fake overflow goes through here too.
fn seen_of(wd: WatchDescriptor, mask: EventMask, name: Option<OsString>) -> Seen {
    if mask.contains(EventMask::Q_OVERFLOW) {
        Seen::Overflow
    } else {
        Seen::Event { wd, mask, name }
    }
}

/// Applies the events to the tree on a blocking thread, since a walk of a new folder reads the
/// disk. Returns the tree and the paths to report.
async fn apply_on_thread(mut tree: Tree, seen: Vec<Seen>) -> io::Result<(Tree, Vec<PathBuf>)> {
    off_runtime(move || {
        let mut reported = Vec::new();
        for event in seen {
            tree.apply(event, &mut reported);
        }
        (tree, reported)
    })
    .await
}

/// Reports the paths of the batch, and empties it.
async fn report(inner: &Inner, batch: &mut BTreeMap<ScanScope, PathBuf>) {
    let paths: Vec<PathBuf> = std::mem::take(batch).into_values().collect();
    inner.changes.report_all(&paths).await;
}

impl Tree {
    /// Applies one event, or the overflow of the queue, and collects the paths to report.
    fn apply(&mut self, seen: Seen, reported: &mut Vec<PathBuf>) {
        match seen {
            Seen::Overflow => {
                // Events were lost, so the whole library is rescanned. Folders that appeared
                // meanwhile get their watches.
                reported.push(self.root.clone());
                self.overflow_walk();
            }
            Seen::Event { wd, mask, name } => self.event(&wd, mask, name, reported),
        }
    }

    /// Applies one event of a folder's watch to the tree, and collects the path it reports.
    fn event(
        &mut self,
        wd: &WatchDescriptor,
        mask: EventMask,
        name: Option<OsString>,
        reported: &mut Vec<PathBuf>,
    ) {
        // An event of a watch that is no longer in the tree has nothing to say.
        let Some(dir) = self.paths.get(wd).cloned() else {
            return;
        };
        let Some(name) = name else {
            // Without a name, the event is about the folder itself. The library path's own watch
            // going away ends the kernel watching. The kernel dropping a subfolder's watch forgets
            // the folder.
            if dir == self.root {
                let gone = EventMask::DELETE_SELF
                    | EventMask::MOVE_SELF
                    | EventMask::UNMOUNT
                    | EventMask::IGNORED;
                if mask.intersects(gone) {
                    self.fallback.get_or_insert(Fallback::LibraryPathGone);
                }
            } else if mask.contains(EventMask::IGNORED) {
                self.forget(&dir, false);
            }
            return;
        };
        if name.to_str().is_some_and(is_ignored_dir) {
            return;
        }
        let path = dir.join(&name);
        if self.fallback.is_some() {
            // The walks have stopped, so the tree is left as it is. The path still reports.
            reported.push(path);
            return;
        }
        // A symlink to a folder is created or moved in without ISDIR, so its target is checked.
        let creates_folder = if mask.contains(EventMask::ISDIR) {
            mask.intersects(EventMask::CREATE | EventMask::MOVED_TO)
        } else {
            mask.intersects(EventMask::CREATE | EventMask::MOVED_TO) && folder_id(&path).is_some()
        };
        if creates_folder {
            // Reported only when it is watched now: not when it is gone, the data dir, or watched
            // under another name already. A fallback that the folder causes reports it as well, so
            // a folder whose walk failed is still rescanned.
            self.mounts = None;
            let parent_dev = self.dev_of(&dir);
            let watched = self.watch_new(&path, parent_dev);
            if watched || self.fallback.is_some() {
                reported.push(path);
            }
            return;
        }
        if mask.intersects(EventMask::DELETE | EventMask::MOVED_FROM) {
            // The name is gone, so a folder watched under it, and what is under it, are
            // forgotten. A removed symlink to a folder raises no IGNORED of its own.
            self.forget_subtree(&path);
        }
        reported.push(path);
    }

    /// Watches the folder at `dir`, of identity `id`, then the folders under it. The watch comes
    /// before the listing, so a subfolder made meanwhile is listed or raises an event. Errors when
    /// the watch on `dir` itself fails.
    fn watch_folder(&mut self, dir: &Path, id: Id) -> io::Result<()> {
        if let Some(hook) = &self.walk_hook {
            hook(dir);
        }
        let wd = self.add_watch(dir)?;
        self.record(dir, wd, id);
        self.walk_children(dir, id.0);
        Ok(())
    }

    /// Adds the watch of `dir`. With the test limit set, the add fails with `StorageFull` once the
    /// tree holds that many folders, before the kernel is asked.
    fn add_watch(&mut self, dir: &Path) -> io::Result<WatchDescriptor> {
        if self.limit.is_some_and(|limit| self.folders.len() >= limit) {
            return Err(io::ErrorKind::StorageFull.into());
        }
        self.watches.add(dir, WATCH_MASK)
    }

    /// The device of the watched folder at `dir`, which is the device its subfolders are checked
    /// against. Zero when the tree does not hold `dir`, which an event's folder never is.
    fn dev_of(&self, dir: &Path) -> u64 {
        self.folders.get(dir).map_or(0, |folder| folder.id.0)
    }

    /// Records the watch `wd` of the folder at `dir`. An entry that `dir` had before (the folder
    /// was replaced, and the events of its removal were lost) is forgotten.
    fn record(&mut self, dir: &Path, wd: WatchDescriptor, id: Id) {
        if let Some(stale) = self
            .folders
            .insert(dir.to_path_buf(), Folder { wd: wd.clone(), id })
        {
            self.paths.remove(&stale.wd);
            self.ids.remove(&stale.id);
            if stale.wd != wd {
                let _ = self.watches.remove(stale.wd);
            }
        }
        self.paths.insert(wd, dir.to_path_buf());
        self.ids.insert(id);
    }

    /// Watches the new folders under `dir`, of device `dev`, in name order. The walk stops at a
    /// fallback, or once the monitor's task is gone.
    fn walk_children(&mut self, dir: &Path, dev: u64) {
        for name in subfolders(dir) {
            if self.fallback.is_some() || self.stopped.load(Ordering::Relaxed) {
                return;
            }
            self.watch_new(&dir.join(name), dev);
        }
    }

    /// Watches `dir`, a folder that appeared in a folder of device `parent_dev`, and the folders
    /// under it. Returns whether `dir` is watched now. It is not when it is gone, when it is the
    /// data dir or watched under another name, or when its watch fails. A watch limit or a folder
    /// that is not local sets the fallback instead, and then nothing more is watched. Nothing is
    /// watched either once the monitor's task is gone.
    fn watch_new(&mut self, dir: &Path, parent_dev: u64) -> bool {
        if self.fallback.is_some() || self.stopped.load(Ordering::Relaxed) {
            return false;
        }
        let Some(id) = folder_id(dir) else {
            tracing::debug!(
                "library monitor: '{}' is gone or not a folder; not watched",
                dir.display()
            );
            return false;
        };
        if self.ids.contains(&id) {
            return false;
        }
        self.check_folder(dir, id, parent_dev);
        if self.fallback.is_some() {
            return false;
        }
        match self.watch_folder(dir, id) {
            Ok(()) => true,
            Err(err) if err.kind() == io::ErrorKind::StorageFull => {
                self.fallback.get_or_insert(Fallback::WatchLimit);
                false
            }
            Err(err) => {
                tracing::debug!("library monitor: not watching '{}': {err}", dir.display());
                false
            }
        }
    }

    /// Checks the folder at `dir`, of identity `id`, whose parent is on device `parent_dev`. A
    /// symlinked folder, or a mount inside the library, must be on a local filesystem, judged by
    /// the fs type of its canonical path. Sets the fallback when it is not. The canonical path of
    /// a symlinked folder is kept in `link_targets`, local or not.
    fn check_folder(&mut self, dir: &Path, id: Id, parent_dev: u64) {
        let is_link = fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink());
        if !is_link && id.0 == parent_dev {
            return;
        }
        let canonical = fs::canonicalize(dir).ok();
        let fs_type = self.fs_type_at(canonical.as_deref());
        if let Some(target) = canonical.filter(|_| is_link) {
            self.link_targets.insert(target);
        }
        if is_local_fs_type(&fs_type) {
            return;
        }
        let reason = if is_link {
            format!("symlinked folder on {fs_type}")
        } else {
            format!("mount of {fs_type} inside")
        };
        self.fallback.get_or_insert(Fallback::NotLocal(reason));
    }

    /// The fs type of `canonical`, the canonical path of a folder, as the mount table has it. The
    /// mount table is read at the first check of a walk. "unknown" when there is no canonical path
    /// or no answer.
    fn fs_type_at(&mut self, canonical: Option<&Path>) -> String {
        let Some(canonical) = canonical else {
            return "unknown".to_string();
        };
        let mounts = self
            .mounts
            .get_or_insert_with(|| fs::read_to_string("/proc/self/mountinfo").unwrap_or_default());
        fs_type_from_mountinfo(mounts.as_str(), canonical).unwrap_or_else(|| "unknown".to_string())
    }

    /// Forgets the folder at `dir`, and removes its watch when `unwatch` is set. The kernel has
    /// dropped the watch already on IGNORED, and removing it again fails, which is fine.
    fn forget(&mut self, dir: &Path, unwatch: bool) {
        let Some(folder) = self.folders.remove(dir) else {
            return;
        };
        self.paths.remove(&folder.wd);
        self.ids.remove(&folder.id);
        if unwatch {
            let _ = self.watches.remove(folder.wd);
        }
    }

    /// Forgets the folder at `dir` and the folders under it, with their watches.
    fn forget_subtree(&mut self, dir: &Path) {
        let under: Vec<PathBuf> = self
            .folders
            .range(dir.to_path_buf()..)
            .take_while(|(path, _)| path.starts_with(dir))
            .map(|(path, _)| path.clone())
            .collect();
        for path in under {
            self.forget(&path, true);
        }
    }

    /// The walk after an overflow. Folders that are gone, or have another identity, are forgotten
    /// first, since their events may have been lost. The walk then goes down through the watched
    /// folders under the library path and watches the new ones, adding no second watch.
    fn overflow_walk(&mut self) {
        if self.fallback.is_some() {
            return;
        }
        self.mounts = None;
        let known: Vec<(PathBuf, Id)> = self
            .folders
            .iter()
            .map(|(path, folder)| (path.clone(), folder.id))
            .collect();
        for (path, id) in known {
            // Dropped already with an ancestor, or the library path, which is kept.
            if path == self.root || !self.folders.contains_key(&path) {
                continue;
            }
            if folder_id(&path) != Some(id) {
                self.forget_subtree(&path);
            }
        }
        let root = self.root.clone();
        let root_dev = self.dev_of(&root);
        self.overflow_dir(&root, root_dev);
    }

    /// Goes down through the watched folders under `dir`, of device `dev`, and watches the new
    /// ones. The walk stops at a fallback, or once the monitor's task is gone.
    fn overflow_dir(&mut self, dir: &Path, dev: u64) {
        for name in subfolders(dir) {
            if self.fallback.is_some() || self.stopped.load(Ordering::Relaxed) {
                return;
            }
            let child = dir.join(name);
            let Some(id) = folder_id(&child) else {
                continue;
            };
            if self
                .folders
                .get(&child)
                .is_some_and(|folder| folder.id == id)
            {
                self.overflow_dir(&child, id.0);
            } else {
                // New, or watched under another name, or the data dir: `watch_new` tells.
                self.watch_new(&child, dev);
            }
        }
    }
}

/// The identity of the folder at `path`, following a symlink. None when there is no folder there.
fn folder_id(path: &Path) -> Option<Id> {
    fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_dir())
        .map(|meta| (meta.dev(), meta.ino()))
}

/// The names of the folders in `dir` that the scanner does not skip, in name order. A symlink is
/// followed to see whether it is a folder.
fn subfolders(dir: &Path) -> BTreeSet<OsString> {
    let Ok(entries) = fs::read_dir(dir) else {
        tracing::debug!("library monitor: '{}' cannot be listed", dir.display());
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            let is_dir = match entry.file_type() {
                Ok(kind) if kind.is_symlink() => {
                    fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir())
                }
                Ok(kind) => kind.is_dir(),
                Err(_) => false,
            };
            is_dir && !entry.file_name().to_str().is_some_and(is_ignored_dir)
        })
        .map(|entry| entry.file_name())
        .collect()
}

#[cfg(test)]
#[path = "kernel_tests.rs"]
mod tests;
