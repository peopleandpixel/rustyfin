//! Limits for background work that reads media files: their CPU priority, and how many whole-file
//! reads run at once from a network share.
//!
//! Background ffmpeg and ffprobe runs at a low CPU priority, so playback transcodes come first.
//! A job that reads a whole file (intro detection, and trickplay tiles where they don't seek to
//! key frames) is bound by the network when the file is on a
//! network share: one such read can fill the link, and more at once only add seeks and memory.
//! So at most `RUSTYFIN_NETWORK_READ_JOBS` of them (default 1) run at once from network shares;
//! reads from local disks are bounded by the image job slots alone.
//!
//! A filesystem counts as local when the library watcher knows it as local (ext4, xfs, btrfs,
//! zfs and the like), or when it is a FUSE pool of local disks (`fuse.mergerfs`, and `fuse.shfs`,
//! Unraid's user shares). Everything else counts as a network share: NFS, SMB, and FUSE mounts of
//! remote storage such as rclone (`fuse.rclone`) and sshfs (`fuse.sshfs`), and a filesystem whose
//! type can't be read.

use std::path::Path;
use std::sync::OnceLock;

use tokio::process::Command;
use tokio::sync::{Semaphore, SemaphorePermit};
use tracing::warn;

/// Runs `cmd` at a low CPU priority (nice 10) on Unix, so playback transcodes come first.
pub(crate) fn lower_priority(cmd: &mut Command) {
    #[cfg(unix)]
    // SAFETY: the hook only makes the nice(2) system call, which is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::nice(10);
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// How many whole-file reads from network shares may run at once: `RUSTYFIN_NETWORK_READ_JOBS`,
/// from 1 to 16, else 1.
pub fn network_read_jobs() -> usize {
    static JOBS: OnceLock<usize> = OnceLock::new();
    *JOBS.get_or_init(|| match std::env::var("RUSTYFIN_NETWORK_READ_JOBS") {
        Err(_) => 1,
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(n) => n.clamp(1, 16),
            Err(_) => {
                warn!(value = %v, "RUSTYFIN_NETWORK_READ_JOBS is not a whole number");
                1
            }
        },
    })
}

/// FUSE filesystems that pool local disks, so their reads are local ones.
const POOLED_LOCAL_FUSE: &[&str] = &["fuse.mergerfs", "fuse.shfs"];

/// Whether a filesystem of type `fs_type` is read like a local disk.
pub fn is_local_read_fs_type(fs_type: &str) -> bool {
    crate::library_watch::is_local_fs_type(fs_type) || POOLED_LOCAL_FUSE.contains(&fs_type)
}

/// Whether `path` is on a filesystem that isn't read like a local disk (`is_local_read_fs_type`):
/// a network share, or one whose type can't be read.
pub fn on_network_share(path: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    !crate::library_watch::fs_type_of(&path).is_some_and(|fs| is_local_read_fs_type(&fs))
}

/// A permit for one whole-file read of `path`: one of the `network_read_jobs` when the file is on a
/// network share, else none is needed.
pub(crate) async fn whole_file_read(path: &Path) -> Option<SemaphorePermit<'static>> {
    static READS: OnceLock<Semaphore> = OnceLock::new();
    if !on_network_share(path) {
        return None;
    }
    let reads = READS.get_or_init(|| Semaphore::new(network_read_jobs()));
    Some(
        reads
            .acquire()
            .await
            .expect("the semaphore is never closed"),
    )
}

/// Decoder threads for each of the image jobs: the CPUs shared out among them, 1 to 4.
pub(crate) fn decode_threads() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    (cpus / crate::trickplay::image_jobs()).clamp(1, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3perf_temp_files_are_local_and_the_limits_are_in_range() {
        assert!(!on_network_share(&std::env::temp_dir()));
        assert!((1..=4).contains(&decode_threads()));
        assert!((1..=16).contains(&network_read_jobs()));
    }

    #[test]
    fn r3perf_pooled_local_disks_are_local_and_remote_fuse_mounts_are_not() {
        for local in ["ext4", "zfs", "fuse.mergerfs", "fuse.shfs"] {
            assert!(is_local_read_fs_type(local), "{local}");
        }
        for network in [
            "nfs",
            "nfs4",
            "cifs",
            "smb3",
            "fuse.rclone",
            "fuse.sshfs",
            "fuse",
        ] {
            assert!(!is_local_read_fs_type(network), "{network}");
        }
    }
}
