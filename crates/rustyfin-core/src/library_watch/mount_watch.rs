//! Follows the mount table, so a library path whose mounts change is set up again. inotify doesn't
//! see into a filesystem mounted over a watched folder, and a path's method is picked only once,
//! when its monitor starts. A polled path that keeps its last state is set up again only when it
//! can be watched now, or when a mount under it went away. The table is read at most twice in a
//! row, then about once a second: a burst of changes is read about once a second, while a lone
//! change, or two changes in a row, are read at once. A failed wait or reading is tried again after
//! waits of 1 s, doubled up to 60 s, and the reading that works is compared with the last good one.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::mounts::{
    MountChange, automount_points, changes_between, fs_type_from_mountinfo, is_local_fs_type,
    mount_keys, restarts_monitor, restarts_polled_monitor,
};
use super::{
    Inner, LIBRARY_PATH_GONE, Method, MonitoredPath, NOT_LOCAL, PATH_NOT_FOUND, RETRY_FIRST,
    RETRY_MAX, take_injected,
};

/// The mount table of this process. The kernel wakes a poll of it once per change.
const MOUNT_TABLE: &str = "/proc/self/mountinfo";

/// Once `READ_BURST` readings of the mount table came back to back, one more is allowed each
/// `READ_INTERVAL`, so a burst of mount changes is read about once a second.
const READ_INTERVAL: Duration = Duration::from_secs(1);

/// How many readings of the mount table may come back to back. See `READ_INTERVAL`.
const READ_BURST: u32 = 2;

/// Follows the mount table in a task, which marks the monitors that a mount change restarts. The
/// table is read before this returns, so a monitor set up after it sees a table at least as new.
/// None, after a warning, when the table cannot be opened, registered or read.
pub(super) fn follow(inner: &Arc<Inner>) -> Option<JoinHandle<()>> {
    let fd = match open_table() {
        Ok(fd) => fd,
        Err(err) => {
            warn_cannot_follow(&err);
            return None;
        }
    };
    inner.mount_reads.fetch_add(1, Ordering::Relaxed);
    let table = match read_table(fd.get_ref()) {
        Ok(table) => table,
        Err(err) => {
            warn_cannot_follow(&err);
            return None;
        }
    };
    Some(tokio::spawn(follow_table(Arc::clone(inner), fd, table)))
}

/// Opens the mount table and registers it with the reactor for the priority events that the kernel
/// sends at each change of the table. The file is closed when the returned `AsyncFd` is dropped.
fn open_table() -> io::Result<AsyncFd<File>> {
    File::open(MOUNT_TABLE).and_then(|file| {
        // SAFETY: `file` moves into the `AsyncFd`, which owns it for its whole life. Nothing else
        // closes or replaces its file descriptor.
        unsafe { AsyncFd::register_with_interest(file, Interest::PRIORITY) }
            .map_err(io::Error::from)
    })
}

/// Waits for the kernel to wake the poll of the table, then reads the table and marks the monitors
/// that its changes restart. Up to `READ_BURST` readings come back to back; after that the follower
/// sleeps until one more reading is allowed, so a burst of changes is read about once a second,
/// while a lone change, or two changes in a row, are read at once. The sleep comes after the wait
/// and before the read. The readiness stays set during it, so a change made meanwhile is part of
/// the reading after the sleep. A reading with the same text as the last one changes nothing. A
/// failed wait or reading is tried again by `retry_table`, which replaces `fd`; the reading that
/// works is compared with the last good one, like any other.
async fn follow_table(inner: Arc<Inner>, mut fd: AsyncFd<File>, mut table: String) {
    let mut keys = mount_keys(&table);
    let mut due = Instant::now();
    loop {
        let read = wait_and_read(&inner, &fd, &mut due).await;
        let new = match read {
            Ok(new) => new,
            Err(err) => {
                let (registered, new) = retry_table(&inner, &err).await;
                fd = registered;
                new
            }
        };
        if new != table {
            let new_keys = mount_keys(&new);
            let changes = changes_between(&keys, &new_keys);
            table = new;
            keys = new_keys;
            if !changes.is_empty() {
                mark_restarts(&inner, &table, &changes);
            }
        }
    }
}

/// Waits for the kernel to wake the poll of the table, then reads the table once `due` allows it,
/// as `follow_table` describes. The reading moves `due` one `READ_INTERVAL` on. An injected
/// failure fails it.
async fn wait_and_read(inner: &Inner, fd: &AsyncFd<File>, due: &mut Instant) -> io::Result<String> {
    let mut ready = fd.ready(Interest::PRIORITY).await?;
    // Each reading moves `due` one `READ_INTERVAL` on. A reading may come up to `slack` before
    // `due`, which lets `READ_BURST` readings come back to back.
    let ahead = due.saturating_duration_since(Instant::now());
    let slack = READ_INTERVAL * (READ_BURST - 1);
    if ahead > slack {
        tokio::time::sleep(ahead - slack).await;
    }
    *due = (*due).max(Instant::now()) + READ_INTERVAL;
    ready.clear_ready();
    inner.mount_reads.fetch_add(1, Ordering::Relaxed);
    if take_injected(&inner.fail_mount_reads) {
        return Err(io::Error::other("injected for a test"));
    }
    read_table(ready.get_inner())
}

/// Tries again to open, register and read the mount table, after `follow_table` failed a wait or a
/// reading with `err`. Warns once. Then each try comes after a wait of `RETRY_FIRST`, doubled after
/// each failed try up to `RETRY_MAX`, and the failed tries are not warned about again. Each try
/// counts as one reading. The first try that works is logged, and its registered table and text
/// are returned.
async fn retry_table(inner: &Inner, err: &io::Error) -> (AsyncFd<File>, String) {
    tracing::warn!(
        "library monitor: cannot read the mount table ({err}); trying again in {} s",
        RETRY_FIRST.as_secs()
    );
    let mut wait = RETRY_FIRST;
    loop {
        tokio::time::sleep(wait).await;
        inner.mount_reads.fetch_add(1, Ordering::Relaxed);
        let reading = if take_injected(&inner.fail_mount_reads) {
            Err(io::Error::other("injected for a test"))
        } else {
            open_table().and_then(|fd| {
                let table = read_table(fd.get_ref())?;
                Ok((fd, table))
            })
        };
        match reading {
            Ok(found) => {
                tracing::info!("library monitor: following the mount table again");
                return found;
            }
            Err(_) => {
                wait = (wait * 2).min(RETRY_MAX);
            }
        }
    }
}

/// Marks each monitor that a change restarts, by the first such change in `changes`, then wakes
/// the manager once when any monitor was marked. Each marked monitor is logged once. A polled path
/// that keeps its last state is marked only when it can be watched in `table`, the mount table
/// after the changes, or when a mount under it went away. A watched path is also marked for a
/// change at the target of one of its symlinked folders.
fn mark_restarts(inner: &Inner, table: &str, changes: &[(PathBuf, MountChange)]) {
    let automounts = automount_points(table);
    let any = {
        let mut state = inner.lock();
        let force_poll = state.force_poll;
        let mut any = false;
        for monitor in state.monitors.values_mut() {
            let path = monitor
                .canonical
                .as_deref()
                .unwrap_or(monitor.info.path.as_path());
            let found = if keeps_baseline(&monitor.info) {
                // A polled path can be watched only if polling is not forced and a local
                // filesystem now holds it.
                let watchable = !force_poll
                    && monitor.info.reason.as_deref() == Some(NOT_LOCAL)
                    && fs_type_from_mountinfo(table, path).is_some_and(|fs| is_local_fs_type(&fs));
                changes.iter().find(|(point, change)| {
                    restarts_polled_monitor(path, point, *change, watchable, &automounts)
                })
            } else {
                // A watched path is also set up again for a change at a symlinked folder's target.
                let watched = monitor.info.method == Method::Watch;
                changes.iter().find(|(point, change)| {
                    restarts_monitor(path, point, *change)
                        || (watched
                            && monitor
                                .link_targets
                                .iter()
                                .any(|target| restarts_monitor(target, point, *change)))
                })
            };
            let Some((point, change)) = found else {
                continue;
            };
            monitor.restart = true;
            any = true;
            let verb = match change {
                MountChange::Mounted => "mounted",
                MountChange::Unmounted => "unmounted",
            };
            tracing::info!(
                "library monitor: '{}' was {verb}; setting up '{}' of '{}' again",
                point.display(),
                monitor.info.path.display(),
                monitor.info.library_name
            );
        }
        any
    };
    if any {
        inner.reload.notify_one();
    }
}

/// Whether a monitor keeps its baseline, its last state, through a mount change: a polled path
/// whose reason is not `PATH_NOT_FOUND` or `LIBRARY_PATH_GONE`. Every other monitor keeps the rule
/// of `restarts_monitor`.
fn keeps_baseline(info: &MonitoredPath) -> bool {
    info.method == Method::Poll
        && info
            .reason
            .as_deref()
            .is_some_and(|reason| reason != PATH_NOT_FOUND && reason != LIBRARY_PATH_GONE)
}

/// Reads the whole mount table through `file`, from its start, so each read is a new reading.
fn read_table(mut file: &File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut table = String::new();
    file.read_to_string(&mut table)?;
    Ok(table)
}

/// Logs that the mount table cannot be followed at `start`, when `follow` gives no follower. A
/// failed wait or reading later on is tried again by `retry_table` instead.
fn warn_cannot_follow(err: &io::Error) {
    tracing::warn!(
        "library monitor: cannot follow the mount table ({err}); a share mounted under a library path from now on is seen after a restart"
    );
}
