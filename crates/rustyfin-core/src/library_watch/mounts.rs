//! Filesystem types, and the changes of the mount table, read from mountinfo text. A path's
//! filesystem type chooses between watching and polling; a mount change says which paths to set
//! up again.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Types whose changes reach the kernel, so a folder on them can be watched.
const LOCAL_FS_TYPES: &[&str] = &[
    "ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs", "jfs", "reiserfs", "bcachefs", "nilfs2",
    "overlay", "tmpfs", "ramfs", "vfat", "msdos", "exfat", "ntfs", "ntfs3", "fuseblk", "hfs",
    "hfsplus", "apfs", "udf", "iso9660", "squashfs", "erofs",
];

/// Filesystem type of the mount that holds `path`, read from mountinfo text.
/// The mount point with the most components wins; a later line wins a tie.
pub fn fs_type_from_mountinfo(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        // Optional fields come after field 5 and end with a lone "-"; the type follows it.
        let Some(dash) = fields
            .iter()
            .skip(6)
            .position(|field| *field == "-")
            .map(|index| index + 6)
        else {
            continue;
        };
        let (Some(mount), Some(fs_type)) = (fields.get(4), fields.get(dash + 1)) else {
            continue;
        };
        let mount = PathBuf::from(unescape(mount));
        if !path.starts_with(&mount) {
            continue;
        }
        let depth = mount.components().count();
        if best
            .as_ref()
            .is_none_or(|(best_depth, _)| depth >= *best_depth)
        {
            best = Some((depth, fs_type.to_string()));
        }
    }
    best.map(|(_, fs_type)| fs_type)
}

/// Whether a filesystem of this type reports its changes, so the folder can be watched.
pub fn is_local_fs_type(fs_type: &str) -> bool {
    LOCAL_FS_TYPES.contains(&fs_type)
}

/// A mount that appeared in the mount table, or went away from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MountChange {
    /// The mount is in the new table only.
    Mounted,
    /// The mount is in the old table only.
    Unmounted,
}

/// A mount in the mount table, keyed by its mount id, its `major:minor` and its unescaped mount
/// point. See `mount_keys`.
pub(super) type MountKey = (String, String, PathBuf);

/// The mounts that appeared and went away between two readings of the mount table, with their
/// mount points: sorted by mount point, then `Mounted` before `Unmounted`, without duplicates. A
/// mount is keyed by its id, `major:minor` and mount point, so a change of its options alone is
/// no change.
pub fn mount_changes(before: &str, after: &str) -> Vec<(PathBuf, MountChange)> {
    changes_between(&mount_keys(before), &mount_keys(after))
}

/// The mounts that appeared and went away between two sets of mount keys, with their mount
/// points: sorted by mount point, then `Mounted` before `Unmounted`, without duplicates.
pub(super) fn changes_between(
    before: &HashSet<MountKey>,
    after: &HashSet<MountKey>,
) -> Vec<(PathBuf, MountChange)> {
    let mut changes: Vec<(PathBuf, MountChange)> = after
        .difference(before)
        .map(|(_, _, point)| (point.clone(), MountChange::Mounted))
        .chain(
            before
                .difference(after)
                .map(|(_, _, point)| (point.clone(), MountChange::Unmounted)),
        )
        .collect();
    changes.sort();
    changes.dedup();
    changes
}

/// The keys of the mounts in mountinfo text: mount id, `major:minor` and unescaped mount point. A
/// line with fewer than five fields is skipped.
pub(super) fn mount_keys(mountinfo: &str) -> HashSet<MountKey> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let [id, _, dev, _, point, ..] = fields.as_slice() else {
                return None;
            };
            Some((
                id.to_string(),
                dev.to_string(),
                PathBuf::from(unescape(point)),
            ))
        })
        .collect()
}

/// Whether a change of the mount at `point` restarts the monitor of the library path `path`. A
/// mount that appears at, above or under the path does: inotify can't see into a filesystem
/// mounted over a watched folder, so the path is set up again and its method is picked again. A
/// mount that goes away strictly under the path does too. One that goes away at or above the path
/// does not: the monitor handles a path that goes away by itself.
pub fn restarts_monitor(path: &Path, point: &Path, change: MountChange) -> bool {
    match change {
        MountChange::Mounted => path.starts_with(point) || point.starts_with(path),
        MountChange::Unmounted => point.starts_with(path) && point != path,
    }
}

/// The mount points of the autofs mounts in mountinfo text. An automount is mounted and expired as
/// its path is accessed, so a change at it says nothing about the share behind it. A line whose
/// filesystem type cannot be read is skipped.
pub fn automount_points(mountinfo: &str) -> HashSet<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            // The type follows the lone "-" after field 6, as in `fs_type_from_mountinfo`.
            let dash = fields
                .iter()
                .skip(6)
                .position(|field| *field == "-")
                .map(|index| index + 6)?;
            let (mount, fs_type) = (fields.get(4)?, fields.get(dash + 1)?);
            (*fs_type == "autofs").then(|| PathBuf::from(unescape(mount)))
        })
        .collect()
}

/// Whether a change at `point` restarts the monitor of `path` that keeps its last state, see
/// `restarts_monitor`. A change at an automount point, or at a folder directly in one, restarts
/// nothing. A mount that appears restarts only when `watchable`; a mount that goes away restarts
/// as `restarts_monitor` says.
pub fn restarts_polled_monitor(
    path: &Path,
    point: &Path,
    change: MountChange,
    watchable: bool,
    automounts: &HashSet<PathBuf>,
) -> bool {
    let under_automount = point
        .parent()
        .is_some_and(|parent| automounts.contains(parent));
    if automounts.contains(point) || under_automount {
        return false;
    }
    match change {
        MountChange::Mounted => watchable && restarts_monitor(path, point, change),
        MountChange::Unmounted => restarts_monitor(path, point, change),
    }
}

/// Filesystem type of the mount that holds `path`, from `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
pub fn fs_type_of(path: &Path) -> Option<String> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok()?;
    fs_type_from_mountinfo(&mountinfo, path)
}

/// Filesystem type of the mount that holds `path`, from `statfs`.
#[cfg(target_os = "macos")]
pub fn fs_type_of(path: &Path) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `buf` is a valid out-pointer.
    let rc = unsafe { libc::statfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statfs returned 0, so it has initialised `buf`.
    let buf = unsafe { buf.assume_init() };
    // SAFETY: statfs wrote a NUL-terminated name into `f_fstypename`.
    let name = unsafe { CStr::from_ptr(buf.f_fstypename.as_ptr()) };
    name.to_str().ok().map(str::to_owned)
}

/// Filesystem types are not read on this OS.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn fs_type_of(_path: &Path) -> Option<String> {
    None
}

/// Undoes the octal escapes the kernel writes for space, tab, newline and backslash.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match octal_escape(&bytes[i..]) {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte that a `\ooo` escape at the start of `bytes` stands for.
fn octal_escape(bytes: &[u8]) -> Option<u8> {
    let [b'\\', a, b, c, ..] = bytes else {
        return None;
    };
    let digits = [*a, *b, *c];
    if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0u16, |acc, d| acc * 8 + u16::from(d - b'0'));
    u8::try_from(value).ok()
}
