// B18 part 2: watching library folders for changes. These tests cover choosing the method (the
// filesystem type of a library path), polling folder timestamps (`FolderPoller`), the manager
// that monitors the library paths whose libraries have real-time monitoring on
// (`LibraryWatcher`), and watching local paths with the kernel's change notifications.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustyfin_core::library_monitor::ChangeQueue;
use rustyfin_core::library_watch::{
    FolderPoller, LibraryWatcher, Method, MonitoredPath, MountChange, automount_points, coalesce,
    fs_type_from_mountinfo, fs_type_of, is_local_fs_type, mount_changes, restarts_monitor,
    restarts_polled_monitor,
};
use rustyfin_core::scanner::ScanScope;
use rustyfin_model::library::{Library, LibraryContentType, LibraryOptions};
use sqlx::SqlitePool;
use uuid::Uuid;

// ── Helpers ──

/// A temporary folder holding the library path `root`, removed when dropped.
struct Tree(PathBuf);

impl Tree {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_watch_{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("root")).unwrap();
        Self(dir)
    }

    /// The library path.
    fn root(&self) -> PathBuf {
        self.0.join("root")
    }

    /// `rel` inside the library path.
    fn at(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    /// `rel` next to the library path, outside the library.
    fn outside(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Creates the file `path` with `len` bytes, and the folders it is in.
fn write(path: &Path, len: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![b'x'; len]).unwrap();
}

/// Appends `len` bytes to the file `path`, as a copy in progress does.
fn append(path: &Path, len: usize) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(&vec![b'x'; len]).unwrap();
}

/// Waits until a change gets a later modified time than the change before, also where the
/// kernel's clock for timestamps is coarse.
fn tick() {
    std::thread::sleep(Duration::from_millis(60));
}

fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}

/// Everything any test in this binary logs from now on, at every level.
fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = buf.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || LogWriter(sink.clone()))
            .try_init()
            .expect("no other global subscriber in this test binary");
        buf
    })
    .clone()
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The captured log lines that contain every one of `needles`.
fn log_lines(needles: &[&str]) -> Vec<String> {
    let logs = captured_logs();
    let text = String::from_utf8_lossy(&logs.lock().unwrap()).to_string();
    text.lines()
        .filter(|line| needles.iter().all(|needle| line.contains(needle)))
        .map(str::to_string)
        .collect()
}

/// Waits up to 20 s for a log line that contains every one of `needles`.
async fn wait_for_log(needles: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(line) = log_lines(needles).into_iter().next() {
            return line;
        }
        assert!(
            Instant::now() < deadline,
            "no log line with {needles:?} within 20 s"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// ── Filesystem types ──

/// A /proc/self/mountinfo with zero, one and two optional fields, escaped mount points, two mounts
/// on one mount point and a line that doesn't parse.
const MOUNTINFO: &str = r"22 1 0:21 / / rw,relatime shared:1 - btrfs /dev/sda2 rw,ssd
25 22 0:22 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
30 22 0:25 / /tmp rw,nosuid,nodev shared:5 master:2 - tmpfs tmpfs rw,size=8g
40 22 0:30 /media /mnt/media ro,relatime master:3 - zfs tank/media ro,xattr
41 40 0:31 / /mnt/media/Film rw - tmpfs tmpfs rw
50 22 0:40 / /mnt/nas/Movies rw,relatime shared:9 - nfs4 nas:/export/movies rw,vers=4.2
51 22 8:17 / /mnt/nas/local\040disk rw,relatime - ext4 /dev/sdb1 rw
60 22 8:33 / /srv rw - xfs /dev/sdc1 rw
61 60 8:49 / /srv rw - ext4 /dev/sdd1 rw
70 22 0:50 / /mnt/tab\011name rw - vfat /dev/sde1 rw
71 22 0:51 / /mnt/back\134slash rw - cifs //nas/share rw
72 22 0:52 / /mnt/rclone rw,nosuid,nodev - fuse.rclone remote: rw
this line does not parse
73 22 0:53 / /mnt/sshfs rw - fuse.sshfs user@host:/ rw
";

fn fs_of(path: &str) -> Option<String> {
    fs_type_from_mountinfo(MOUNTINFO, Path::new(path))
}

#[test]
fn mountinfo_gives_the_type_of_the_longest_mount_point_that_holds_the_path() {
    assert_eq!(
        fs_of("/mnt/media/Film/Heat (1995)").as_deref(),
        Some("tmpfs")
    );
    assert_eq!(fs_of("/mnt/media/Film").as_deref(), Some("tmpfs"));
    assert_eq!(
        fs_of("/mnt/media/Filmography").as_deref(),
        Some("zfs"),
        "mount points hold paths by whole components"
    );
    assert_eq!(fs_of("/mnt/media").as_deref(), Some("zfs"));
    assert_eq!(
        fs_of("/mnt/nas/Movies/Heat (1995)/Heat (1995).mkv").as_deref(),
        Some("nfs4")
    );
    assert_eq!(fs_of("/mnt/nas/Moviesque").as_deref(), Some("btrfs"));
    assert_eq!(fs_of("/home/me/Music").as_deref(), Some("btrfs"));
    assert_eq!(fs_of("/proc/self").as_deref(), Some("proc"));
    assert_eq!(
        fs_of("/mnt/sshfs/films").as_deref(),
        Some("fuse.sshfs"),
        "a line that doesn't parse is skipped"
    );
}

#[test]
fn mountinfo_mount_points_are_unescaped_and_the_later_of_two_mounts_wins() {
    assert_eq!(fs_of("/mnt/nas/local disk/Movies").as_deref(), Some("ext4"));
    assert_eq!(
        fs_of(r"/mnt/nas/local\040disk/Movies").as_deref(),
        Some("btrfs"),
        "the escape is not part of the name"
    );
    assert_eq!(fs_of("/mnt/tab\tname/Movies").as_deref(), Some("vfat"));
    assert_eq!(fs_of(r"/mnt/back\slash/Movies").as_deref(), Some("cifs"));
    assert_eq!(fs_of("/mnt/rclone/Movies").as_deref(), Some("fuse.rclone"));
    assert_eq!(
        fs_of("/srv/media").as_deref(),
        Some("ext4"),
        "mounted over the xfs one"
    );
    assert_eq!(fs_type_from_mountinfo("", Path::new("/mnt/media")), None);
    assert_eq!(
        fs_type_from_mountinfo("not mountinfo\n", Path::new("/")),
        None
    );
}

#[test]
fn local_filesystems_are_watched_and_all_others_polled() {
    for local in [
        "ext4", "ext3", "xfs", "btrfs", "zfs", "f2fs", "bcachefs", "overlay", "tmpfs", "vfat",
        "exfat", "ntfs", "ntfs3", "fuseblk", "apfs", "hfsplus", "squashfs", "iso9660",
    ] {
        assert!(is_local_fs_type(local), "{local} is local");
    }
    for polled in [
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "smbfs",
        "9p",
        "ceph",
        "glusterfs",
        "afs",
        "davfs",
        "fuse",
        "fuse.sshfs",
        "fuse.rclone",
        "fuse.mergerfs",
        "proc",
        "",
        "somethingnew",
    ] {
        assert!(!is_local_fs_type(polled), "{polled} is polled");
    }
}

#[test]
fn fs_type_of_reads_the_mounts_of_this_machine() {
    let temp = std::env::temp_dir();
    if cfg!(target_os = "linux") {
        assert_eq!(fs_type_of(Path::new("/proc/self")).as_deref(), Some("proc"));
    }
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        let fs = fs_type_of(&temp).expect("the temp dir is on a mount");
        assert!(is_local_fs_type(&fs), "the temp dir is on {fs}");
    } else {
        assert_eq!(fs_type_of(&temp), None);
    }
}

// ── Mount changes ──

fn changes(list: &[(&str, MountChange)]) -> Vec<(PathBuf, MountChange)> {
    list.iter()
        .map(|(point, change)| (PathBuf::from(point), *change))
        .collect()
}

#[test]
fn mount_changes_lists_the_mounts_that_came_and_went() {
    use MountChange::{Mounted, Unmounted};
    // The tmpfs on Film went away, /srv was remounted read-only, rclone was mounted again, and a
    // share with a space in its name and two stacked disks came.
    let after = MOUNTINFO
        .replace("41 40 0:31 / /mnt/media/Film rw - tmpfs tmpfs rw\n", "")
        .replace(
            "60 22 8:33 / /srv rw - xfs /dev/sdc1 rw",
            "60 22 8:33 / /srv ro,relatime - xfs /dev/sdc1 ro",
        )
        .replace("72 22 0:52 / /mnt/rclone", "74 22 0:54 / /mnt/rclone")
        + r"80 50 0:60 / /mnt/nas/Movies/New\040Films rw - nfs4 nas:/export/new rw
81 22 8:65 / /mnt/usb rw - vfat /dev/sdf1 rw
82 81 8:81 / /mnt/usb rw - ext4 /dev/sdg1 rw
";
    assert!(
        after.contains("/srv ro,relatime"),
        "the remount is in the table"
    );
    assert_eq!(
        mount_changes(MOUNTINFO, &after),
        changes(&[
            ("/mnt/media/Film", Unmounted),
            ("/mnt/nas/Movies/New Films", Mounted),
            ("/mnt/rclone", Mounted),
            ("/mnt/rclone", Unmounted),
            ("/mnt/usb", Mounted),
        ]),
        "sorted by mount point, Mounted first, stacked mounts once, a change of options is none"
    );
    assert_eq!(
        mount_changes(&after, MOUNTINFO),
        changes(&[
            ("/mnt/media/Film", Mounted),
            ("/mnt/nas/Movies/New Films", Unmounted),
            ("/mnt/rclone", Mounted),
            ("/mnt/rclone", Unmounted),
            ("/mnt/usb", Unmounted),
        ])
    );
    assert_eq!(mount_changes(MOUNTINFO, MOUNTINFO), Vec::new());
    assert_eq!(
        mount_changes(
            "1 2 3:4\n",
            "90 1 0:70 / /a rw - tmpfs tmpfs rw\n1 2 3:4 /\n"
        ),
        changes(&[("/a", Mounted)]),
        "a line with fewer than five fields is skipped"
    );
}

#[test]
fn a_mount_at_above_or_under_a_path_restarts_its_monitor_and_an_unmount_only_under_it() {
    let path = Path::new("/mnt/nas/Movies");
    for (point, mounted, unmounted, what) in [
        ("/mnt/nas/Movies", true, false, "at the path"),
        ("/mnt/nas", true, false, "above the path"),
        ("/", true, false, "at the root"),
        ("/mnt/nas/Movies/Heat (1995)", true, true, "under the path"),
        (
            "/mnt/nas/Moviesque",
            false,
            false,
            "beside the path, by whole components",
        ),
        ("/mnt/other", false, false, "elsewhere"),
    ] {
        let point = Path::new(point);
        assert_eq!(
            restarts_monitor(path, point, MountChange::Mounted),
            mounted,
            "a mount {what}"
        );
        assert_eq!(
            restarts_monitor(path, point, MountChange::Unmounted),
            unmounted,
            "an unmount {what}"
        );
    }
}

#[test]
fn automount_points_are_the_mount_points_of_autofs_mounts() {
    // An indirect map on /net with a share mounted in it, a direct map with a disk on it, and a
    // tmpfs whose source is named autofs.
    let table = MOUNTINFO.to_string()
        + r"90 22 0:80 / /net rw,relatime shared:20 - autofs -hosts rw,fd=7,pgrp=1,timeout=300,indirect
91 90 0:81 / /net/nas rw,relatime - nfs4 nas:/export rw
92 22 0:82 / /mnt/usb\040disk rw,relatime - autofs systemd-1 rw,fd=51,timeout=60,direct
93 92 8:97 / /mnt/usb\040disk rw,relatime - ext4 /dev/sdh1 rw
94 22 0:83 / /mnt/scratch rw - tmpfs autofs rw
";
    assert_eq!(
        sorted(automount_points(&table).into_iter().collect()),
        vec![PathBuf::from("/mnt/usb disk"), PathBuf::from("/net")],
        "autofs mounts alone, by their type, unescaped"
    );
    assert!(automount_points(MOUNTINFO).is_empty());
}

#[test]
fn a_polled_path_that_keeps_its_last_state_is_restarted_only_to_be_watched_or_for_an_unmount_under_it()
 {
    use MountChange::{Mounted, Unmounted};
    let path = Path::new("/mnt/nas/Movies");
    let no_automounts = HashSet::new();
    for (point, watchable, mounted, unmounted, what) in [
        ("/mnt/nas/Movies", false, false, false, "at the path"),
        (
            "/mnt/nas/Movies",
            true,
            true,
            false,
            "at the path, watchable",
        ),
        ("/mnt/nas", false, false, false, "above the path"),
        ("/mnt/nas", true, true, false, "above the path, watchable"),
        (
            "/mnt/nas/Movies/Heat (1995)",
            false,
            false,
            true,
            "under the path",
        ),
        (
            "/mnt/nas/Movies/Heat (1995)",
            true,
            true,
            true,
            "under the path, watchable",
        ),
        ("/mnt/other", true, false, false, "elsewhere"),
    ] {
        let point = Path::new(point);
        assert_eq!(
            restarts_polled_monitor(path, point, Mounted, watchable, &no_automounts),
            mounted,
            "a mount {what}"
        );
        assert_eq!(
            restarts_polled_monitor(path, point, Unmounted, watchable, &no_automounts),
            unmounted,
            "an unmount {what}"
        );
    }

    // An automount comes and goes by itself: a direct map's point, or a folder directly in an
    // indirect map's point, restarts nothing; a mount deeper in the share keeps the rule.
    let automounts: HashSet<PathBuf> = ["/mnt/nas/Movies", "/net"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    for (path, point, restarts, what) in [
        (
            "/mnt/nas/Movies",
            "/mnt/nas/Movies",
            false,
            "a direct map at the path",
        ),
        (
            "/net/nas/Movies",
            "/net/nas",
            false,
            "an indirect map above the path",
        ),
        ("/net/nas/Movies", "/net", false, "the indirect map itself"),
        (
            "/net/nas/Movies",
            "/net/nas/Movies/Heat (1995)",
            true,
            "a share deeper in it",
        ),
    ] {
        let (path, point) = (Path::new(path), Path::new(point));
        assert_eq!(
            restarts_polled_monitor(path, point, Mounted, true, &automounts),
            restarts,
            "a mount: {what}"
        );
        assert_eq!(
            restarts_polled_monitor(path, point, Unmounted, true, &automounts),
            restarts,
            "an unmount: {what}"
        );
    }
}

// ── Polling ──

#[test]
fn the_first_poll_only_records_the_baseline() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Show/Season 1/S01E01.mkv"), 10);
    write(&tree.at("Loose (2001).mkv"), 10);
    std::fs::create_dir_all(tree.at(".hidden/inner")).unwrap();
    std::fs::create_dir_all(tree.at("@eaDir/Heat (1995)")).unwrap();
    std::fs::create_dir_all(tree.at("Heat (1995)/Heat (1995).trickplay")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(!poller.has_baseline());

    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
    assert!(poller.has_baseline());
    assert!(poller.available());
    // The library path, Heat (1995), Show and Season 1; ignored folders are left out.
    assert_eq!(poller.folders(), 4);
    assert_eq!(poller.settling(), 0);

    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new(), "nothing changed");
}

#[test]
fn a_poll_reports_new_changed_and_removed_folders() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
    write(&tree.at("Show/Season 1/S01E01.mkv"), 10);
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());

    tick();
    write(&tree.at("Heat (1995)/Heat (1995).srt"), 10);
    std::fs::remove_dir_all(tree.at("Alien (1979)")).unwrap();
    write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
    write(&tree.at("Up (2009)/extras/Making of.mkv"), 10);
    write(&tree.at("Show/Season 1/S01E02.mkv"), 10);
    assert_eq!(
        poller.poll(),
        sorted(vec![
            tree.at("Alien (1979)"),
            tree.at("Heat (1995)"),
            tree.at("Show/Season 1"),
            tree.at("Up (2009)"),
        ]),
        "a new folder is reported without the folders inside it"
    );
    // The library path, Heat (1995), Show, Season 1, Up (2009) and its extras.
    assert_eq!(poller.folders(), 6);
    assert!(poller.settling() > 0, "changed folders settle");

    tick();
    write(&tree.at("Up (2009)/extras/Trailer.mkv"), 10);
    assert_eq!(
        poller.poll(),
        vec![tree.at("Up (2009)/extras")],
        "the folders inside a new folder are polled"
    );

    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
    assert_eq!(poller.settling(), 0, "every folder held still");
}

#[test]
fn a_growing_file_is_reported_again_once_it_holds_still() {
    let tree = Tree::new();
    std::fs::create_dir_all(tree.at("Heat (1995)")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());

    tick();
    let film = tree.at("Heat (1995)/Heat (1995).mkv");
    write(&film, 1000);
    assert_eq!(poller.poll(), vec![tree.at("Heat (1995)")], "the new file");
    for _ in 0..2 {
        tick();
        append(&film, 1000);
        assert_eq!(poller.poll(), Vec::<PathBuf>::new(), "still growing");
        assert_eq!(poller.settling(), 1);
    }

    tick();
    assert_eq!(
        poller.poll(),
        vec![tree.at("Heat (1995)")],
        "held still since the last poll"
    );
    assert_eq!(poller.settling(), 0);
    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
}

#[test]
fn a_folder_made_while_its_parent_settles_is_reported_though_the_parent_time_is_put_back() {
    // Fix round 1, item 4: a copy tool that keeps times can put a folder's time back after making
    // a subfolder in it. While the folder settles, its subfolders are compared too.
    let tree = Tree::new();
    let show = tree.at("Show (2001)");
    std::fs::create_dir_all(&show).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());

    tick();
    write(&show.join("Show (2001) S01E01.mkv"), 1000);
    assert_eq!(poller.poll(), vec![show.clone()], "the new file");
    assert_eq!(poller.settling(), 1, "the folder settles");

    let listed = std::fs::metadata(&show).unwrap().modified().unwrap();
    tick();
    let season = show.join("Season 02");
    std::fs::create_dir(&season).unwrap();
    std::fs::File::open(&show)
        .unwrap()
        .set_modified(listed)
        .unwrap();
    assert_eq!(
        std::fs::metadata(&show).unwrap().modified().unwrap(),
        listed,
        "the folder's time is put back"
    );
    write(&season.join("Show (2001) S02E01.mkv"), 1000);

    let mut reported = poller.poll();
    tick();
    reported.extend(poller.poll());
    assert!(
        reported.contains(&season),
        "the new folder is reported: {reported:?}"
    );
}

#[test]
fn a_copy_that_ended_before_the_next_poll_is_reported_once() {
    let tree = Tree::new();
    std::fs::create_dir_all(tree.at("Heat (1995)")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());

    tick();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 1000);
    assert_eq!(poller.poll(), vec![tree.at("Heat (1995)")]);
    tick();
    assert_eq!(
        poller.poll(),
        Vec::<PathBuf>::new(),
        "unchanged since it was reported"
    );
    assert_eq!(poller.settling(), 0);
}

#[test]
fn files_in_the_library_path_are_reported_by_their_own_paths() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995).mkv"), 10);
    write(&tree.at("Alien (1979).mkv"), 10);
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());

    tick();
    std::fs::remove_file(tree.at("Alien (1979).mkv")).unwrap();
    let up = tree.at("Up (2009).mkv");
    write(&up, 1000);
    assert_eq!(
        poller.poll(),
        sorted(vec![tree.at("Alien (1979).mkv"), up.clone()]),
        "never the library path itself"
    );
    tick();
    append(&up, 1000);
    assert_eq!(poller.poll(), Vec::<PathBuf>::new(), "still growing");
    tick();
    assert_eq!(
        poller.poll(),
        vec![up],
        "held still: only the file that changed"
    );
    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
}

#[test]
fn a_renamed_or_moved_folder_is_reported_under_both_names_and_polled_under_the_new_one() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Show/Season 1/S01E01.mkv"), 10);
    write(&tree.at("Show/Season 2/S02E01.mkv"), 10);
    write(&tree.at("Other/S01E01.nfo"), 10);
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());
    assert_eq!(poller.folders(), 6);

    tick();
    std::fs::rename(tree.at("Heat (1995)"), tree.at("Heat (1995) [1080p]")).unwrap();
    std::fs::rename(tree.at("Show/Season 1"), tree.at("Show/Season 01")).unwrap();
    // "Other" sorts before "Show": the move is seen from both sides in one poll.
    std::fs::rename(tree.at("Show/Season 2"), tree.at("Other/Season 2")).unwrap();
    assert_eq!(
        poller.poll(),
        sorted(vec![
            tree.at("Heat (1995)"),
            tree.at("Heat (1995) [1080p]"),
            tree.at("Other"),
            tree.at("Other/Season 2"),
            tree.at("Show"),
            tree.at("Show/Season 01"),
            tree.at("Show/Season 1"),
            tree.at("Show/Season 2"),
        ])
    );
    assert_eq!(poller.folders(), 6);

    tick();
    write(&tree.at("Show/Season 01/S01E02.mkv"), 10);
    write(&tree.at("Other/Season 2/S02E02.mkv"), 10);
    write(&tree.at("Heat (1995) [1080p]/Heat (1995).srt"), 10);
    assert_eq!(
        poller.poll(),
        sorted(vec![
            tree.at("Heat (1995) [1080p]"),
            tree.at("Other/Season 2"),
            tree.at("Show/Season 01"),
        ]),
        "polled under the new names"
    );
}

#[test]
fn a_missing_library_path_keeps_its_baseline_until_it_is_back() {
    captured_logs();
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());
    let root = tree.root().display().to_string();
    let missing = format!("library monitor: '{root}' is missing or unreadable");
    let back = format!("library monitor: '{root}' is back");

    let away = tree.outside("away");
    std::fs::rename(tree.root(), &away).unwrap();
    for _ in 0..3 {
        tick();
        assert_eq!(poller.poll(), Vec::<PathBuf>::new());
        assert!(!poller.available());
        assert!(poller.has_baseline());
    }
    let warnings = log_lines(&[&missing]);
    assert_eq!(warnings.len(), 1, "one warning per outage: {warnings:?}");
    assert!(warnings[0].contains("WARN"), "{warnings:?}");

    // Changed while it was away.
    std::fs::remove_dir_all(away.join("Alien (1979)")).unwrap();
    write(&away.join("Up (2009)/Up (2009).mkv"), 10);
    std::fs::rename(&away, tree.root()).unwrap();
    tick();
    assert_eq!(
        poller.poll(),
        sorted(vec![tree.at("Alien (1979)"), tree.at("Up (2009)")])
    );
    assert!(poller.available());
    let lines = log_lines(&[&back]);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("INFO"), "{lines:?}");

    std::fs::rename(tree.root(), &away).unwrap();
    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
    assert_eq!(log_lines(&[&missing]).len(), 2, "a new outage warns again");
}

#[test]
fn an_emptied_library_path_counts_as_unavailable() {
    captured_logs();
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());
    let root = tree.root().display().to_string();

    // An unmounted share leaves its mount point empty.
    let away = tree.outside("away");
    std::fs::create_dir(&away).unwrap();
    for name in ["Heat (1995)", "Alien (1979)"] {
        std::fs::rename(tree.at(name), away.join(name)).unwrap();
    }
    tick();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());
    assert!(!poller.available());
    let warnings = log_lines(&[&format!("'{root}' is missing or unreadable")]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");

    // Back with other folders: the kept baseline tells what changed.
    write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
    tick();
    assert_eq!(
        poller.poll(),
        sorted(vec![
            tree.at("Alien (1979)"),
            tree.at("Heat (1995)"),
            tree.at("Up (2009)"),
        ])
    );
    assert!(poller.available());
}

#[test]
fn a_library_path_missing_at_the_first_poll_gets_its_baseline_when_it_appears() {
    captured_logs();
    let tree = Tree::new();
    let root = tree.outside("later");
    let mut poller = FolderPoller::new(root.clone(), None);
    for _ in 0..2 {
        assert_eq!(poller.poll(), Vec::<PathBuf>::new());
        assert!(!poller.has_baseline());
        assert!(!poller.available());
    }
    let shown = root.display().to_string();
    let warnings = log_lines(&[&format!("'{shown}' is missing or unreadable")]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");

    write(&root.join("Heat (1995)/Heat (1995).mkv"), 10);
    assert_eq!(
        poller.poll(),
        Vec::<PathBuf>::new(),
        "the first poll that works records the baseline"
    );
    assert!(poller.has_baseline());
    assert!(poller.available());
    assert_eq!(poller.folders(), 2);

    tick();
    write(&root.join("Up (2009)/Up (2009).mkv"), 10);
    assert_eq!(poller.poll(), vec![root.join("Up (2009)")]);
}

#[test]
fn ignored_folders_and_the_data_dir_are_not_polled() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    std::fs::create_dir_all(tree.at(".hidden")).unwrap();
    std::fs::create_dir_all(tree.at("@eaDir/Heat (1995)")).unwrap();
    std::fs::create_dir_all(tree.at("data/images")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), Some(tree.at("data")));
    assert!(poller.poll().is_empty());
    assert_eq!(poller.folders(), 2, "the library path and Heat (1995)");

    tick();
    write(&tree.at(".hidden/Heat (1995).mkv"), 10);
    write(&tree.at("@eaDir/Heat (1995)/SYNO_EA.jpg"), 10);
    write(&tree.at("data/images/poster.jpg"), 10);
    std::fs::create_dir_all(tree.at(".new hidden")).unwrap();
    assert_eq!(poller.poll(), Vec::<PathBuf>::new());

    // A library inside the data dir is polled; only the data dir itself is left out.
    let mut inside = FolderPoller::new(tree.root(), Some(tree.0.clone()));
    assert!(inside.poll().is_empty());
    assert_eq!(
        inside.folders(),
        4,
        "the library path, Heat (1995), data and data/images"
    );
}

#[cfg(unix)]
#[test]
fn the_data_dir_is_left_out_whatever_form_its_path_has() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    std::fs::create_dir_all(tree.at("data/images")).unwrap();
    let alias = tree.outside("alias");
    std::os::unix::fs::symlink(tree.root(), &alias).unwrap();
    let forms = [
        (tree.root(), tree.at("Heat (1995)/../data")),
        (tree.root(), alias.join("data")),
        (alias.clone(), tree.at("data")),
    ];
    for (n, (root, data_dir)) in forms.into_iter().enumerate() {
        let mut poller = FolderPoller::new(root.clone(), Some(data_dir.clone()));
        assert!(poller.poll().is_empty());
        assert_eq!(
            poller.folders(),
            2,
            "{data_dir:?}: the library path and Heat (1995)"
        );

        tick();
        write(&tree.at(&format!("data/images/poster {n}.jpg")), 10);
        write(&tree.at(&format!("data/cache {n}/subtitle.srt")), 10);
        assert_eq!(poller.poll(), Vec::<PathBuf>::new(), "{data_dir:?}");

        tick();
        write(&tree.at("Heat (1995)/Heat (1995).srt"), 10);
        assert_eq!(
            poller.poll(),
            vec![root.join("Heat (1995)")],
            "{data_dir:?}"
        );
        std::fs::remove_file(tree.at("Heat (1995)/Heat (1995).srt")).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_folder_is_polled_and_reported_by_its_path_in_the_library() {
    let tree = Tree::new();
    let heat = tree.outside("elsewhere/Heat (1995)");
    write(&heat.join("Heat (1995).mkv"), 10);
    std::os::unix::fs::symlink(&heat, tree.at("Heat (1995)")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());
    assert_eq!(poller.folders(), 2);

    tick();
    write(&heat.join("Heat (1995).srt"), 10);
    assert_eq!(poller.poll(), vec![tree.at("Heat (1995)")]);

    tick();
    let alien = tree.outside("elsewhere/Alien (1979)");
    write(&alien.join("Alien (1979).mkv"), 10);
    std::os::unix::fs::symlink(&alien, tree.at("Alien (1979)")).unwrap();
    assert_eq!(poller.poll(), vec![tree.at("Alien (1979)")]);
    assert_eq!(poller.folders(), 3);
}

#[cfg(unix)]
#[test]
fn a_library_path_that_is_a_symlink_reports_paths_under_the_link() {
    let tree = Tree::new();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    let link = tree.outside("films");
    std::os::unix::fs::symlink(tree.root(), &link).unwrap();
    let mut poller = FolderPoller::new(link.clone(), None);
    assert!(poller.poll().is_empty());

    tick();
    write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
    write(&tree.at("Heat (1995)/Heat (1995).srt"), 10);
    assert_eq!(
        poller.poll(),
        sorted(vec![link.join("Heat (1995)"), link.join("Up (2009)")])
    );
}

#[cfg(unix)]
#[test]
fn symlink_loops_and_two_links_to_one_folder_are_walked_once() {
    let tree = Tree::new();
    write(&tree.at("Show/Season 1/S01E01.mkv"), 10);
    std::os::unix::fs::symlink(tree.root(), tree.at("Show/Season 1/loop")).unwrap();
    let heat = tree.outside("elsewhere/Heat (1995)");
    write(&heat.join("Heat (1995).mkv"), 10);
    std::os::unix::fs::symlink(&heat, tree.at("Heat A")).unwrap();
    std::os::unix::fs::symlink(&heat, tree.at("Heat B")).unwrap();
    let mut poller = FolderPoller::new(tree.root(), None);
    assert!(poller.poll().is_empty());
    // The library path, Show, Season 1 and one of the two links.
    assert_eq!(poller.folders(), 4);

    tick();
    write(&heat.join("Heat (1995).srt"), 10);
    let reported = poller.poll();
    assert!(
        reported == vec![tree.at("Heat A")] || reported == vec![tree.at("Heat B")],
        "{reported:?}"
    );

    // Skipped folders are known by name, so they are never new.
    tick();
    write(&tree.at("Show/Season 1/S01E02.mkv"), 10);
    assert_eq!(poller.poll(), vec![tree.at("Show/Season 1")]);
    tick();
    std::fs::create_dir(tree.at("Up (2009)")).unwrap();
    assert_eq!(poller.poll(), vec![tree.at("Up (2009)")]);
}

// ── Manager ──

/// A database in `tree`, and a queue on it whose settle delay of an hour keeps every report
/// pending.
async fn manager_fixture(tree: &Tree) -> (SqlitePool, ChangeQueue) {
    let db = tree.outside("test.db");
    let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
    let changes = ChangeQueue::new(pool.clone());
    changes.set_delay(Duration::from_secs(3600));
    (pool, changes)
}

fn path_string(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

/// Creates the movie library `name` with `paths`, and their folders.
async fn add_library(pool: &SqlitePool, name: &str, paths: &[PathBuf]) -> Library {
    for path in paths {
        std::fs::create_dir_all(path).unwrap();
    }
    let lib = Library {
        id: Uuid::new_v4(),
        name: name.to_string(),
        content_type: LibraryContentType::Movies,
        paths: paths.iter().map(|path| path_string(path)).collect(),
    };
    rustyfin_db::repo_library::create_library(pool, &lib)
        .await
        .unwrap();
    lib
}

/// Turns real-time monitoring of `lib` on or off, and tells `watcher`.
async fn set_monitoring(pool: &SqlitePool, watcher: &LibraryWatcher, lib: &Library, on: bool) {
    let options = LibraryOptions {
        enable_realtime_monitor: on,
        ..LibraryOptions::default()
    };
    rustyfin_db::repo_library::set_library_options(pool, lib.id, &options)
        .await
        .unwrap();
    watcher.libraries_changed();
}

/// A started watcher that polls every library path every 100 ms, whatever its filesystem.
fn polling_watcher(pool: &SqlitePool, changes: &ChangeQueue, data_dir: PathBuf) -> LibraryWatcher {
    let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), data_dir);
    watcher.set_force_poll(true);
    watcher.set_poll_interval(Duration::from_millis(100));
    watcher.start();
    watcher
}

/// Waits up to 20 s until `check` holds for the monitored paths, and returns them.
async fn wait_monitored(
    watcher: &LibraryWatcher,
    what: &str,
    check: impl Fn(&[MonitoredPath]) -> bool,
) -> Vec<MonitoredPath> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let monitored = watcher.monitored();
        if check(&monitored) {
            return monitored;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: not within 20 s: {monitored:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The paths whose monitors are ready, in the order `monitored` lists them.
fn ready_paths(monitored: &[MonitoredPath]) -> Vec<PathBuf> {
    monitored
        .iter()
        .filter(|path| path.ready)
        .map(|path| path.path.clone())
        .collect()
}

/// The monitored path `path`, if it is listed.
fn monitor_of<'a>(monitored: &'a [MonitoredPath], path: &Path) -> Option<&'a MonitoredPath> {
    monitored.iter().find(|m| m.path.as_path() == path)
}

/// Waits up to 20 s until every one of `units` is pending.
async fn wait_pending(changes: &ChangeQueue, units: &[(Uuid, ScanScope)]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let pending = changes.pending();
        if units.iter().all(|unit| pending.contains(unit)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{units:?} not all pending within 20 s: {pending:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Gives the monitors time for several polls, so that a report that should not come would have.
async fn several_polls() {
    tokio::time::sleep(Duration::from_millis(600)).await;
}

#[test]
fn coalesce_keeps_the_first_path_of_each_unit_in_order() {
    let library_paths = vec!["/media/movies".to_string()];
    let paths = [
        "/media/movies/Heat (1995)/Heat (1995).srt",
        "/media/movies/Up (2009)",
        "/media/movies/Heat (1995)",
        "/elsewhere/Heat (1995)",
        "/media/movies/.recycle/Heat (1995)",
        "/media/movies/Alien (1979).mkv",
        "/media/movies/Brazil (1985).mkv",
        "/media/movies/Up (2009)/Up (2009).mkv",
        "/media/movies",
    ]
    .map(PathBuf::from);
    assert_eq!(
        coalesce(&library_paths, &paths),
        [
            "/media/movies/Heat (1995)/Heat (1995).srt",
            "/media/movies/Up (2009)",
            "/media/movies/Alien (1979).mkv",
            "/media/movies",
        ]
        .map(PathBuf::from)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_polled_library_path_is_logged_and_its_changes_are_queued_by_unit() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.at("Brazil (1985)/Brazil (1985).mkv"), 10);
    // The data dir lies inside the library path here.
    write(&tree.at("rustyfin-data/images/poster.jpg"), 10);
    let lib = add_library(&pool, "Watch B Films", &[tree.root()]).await;
    let watcher = polling_watcher(&pool, &changes, tree.at("rustyfin-data"));

    let monitored = wait_monitored(&watcher, "the library path is ready", |m| {
        m.len() == 1 && m[0].ready
    })
    .await;
    let fs = fs_type_of(&tree.root().canonicalize().unwrap()).unwrap_or_else(|| "unknown".into());
    let path = &monitored[0];
    assert_eq!(
        (path.library_id, path.library_name.as_str(), &path.path),
        (lib.id, "Watch B Films", &tree.root())
    );
    assert_eq!(
        (path.method, path.fs_type.as_str(), path.reason.as_deref()),
        (Method::Poll, fs.as_str(), Some("forced"))
    );
    assert_eq!(
        (path.folders, path.watches),
        (3, 0),
        "the library path, Heat (1995) and Brazil (1985)"
    );
    let polling = format!(
        "library monitor: polling '{}' of 'Watch B Films' ({fs}, forced): 3 folders, every 100ms, \
         baseline in ",
        tree.root().display()
    );
    wait_for_log(&[&polling, " ms"]).await;
    let polled = format!(
        "library monitor: polled '{}': 3 folders in ",
        tree.root().display()
    );
    wait_for_log(&[&polled, " ms, 0 reported"]).await;

    // Starting again starts nothing more.
    watcher.start();
    several_polls().await;
    assert_eq!(log_lines(&[&polling]).len(), 1, "one monitor for the path");
    assert_eq!(watcher.monitored().len(), 1);

    tick();
    write(&tree.at("Heat (1995)/Heat (1995).srt"), 10);
    write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
    write(&tree.at("Alien (1979).mkv"), 10);
    std::fs::remove_dir_all(tree.at("Brazil (1985)")).unwrap();
    write(&tree.at("rustyfin-data/images/backdrop.jpg"), 10);
    write(&tree.at("rustyfin-data/transcodes/1/segment0.ts"), 10);
    let expected = sorted(vec![
        (lib.id, ScanScope::Folder(tree.at("Heat (1995)"))),
        (lib.id, ScanScope::Folder(tree.at("Up (2009)"))),
        (lib.id, ScanScope::Folder(tree.at("Brazil (1985)"))),
        (lib.id, ScanScope::RootFiles(tree.root())),
    ]);
    wait_pending(&changes, &expected).await;
    // Nothing else: not the data dir, and never the whole library.
    several_polls().await;
    assert_eq!(changes.pending(), expected);

    watcher.stop();
    wait_monitored(&watcher, "nothing is monitored after stop", |m| {
        m.is_empty()
    })
    .await;
    tick();
    write(&tree.at("Dune (2021)/Dune (2021).mkv"), 10);
    several_polls().await;
    assert_eq!(
        changes.pending(),
        expected,
        "nothing is reported after stop"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_library_path_is_monitored_and_quiet_until_it_appears() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    let lib = add_library(&pool, "Watch C Films", &[tree.root()]).await;
    std::fs::remove_dir(tree.root()).unwrap();
    let watcher = polling_watcher(&pool, &changes, tree.outside("data"));

    let missing = format!(
        "library monitor: '{}' is missing or unreadable",
        tree.root().display()
    );
    wait_for_log(&[&missing]).await;
    let monitored = watcher.monitored();
    assert_eq!(monitored.len(), 1, "{monitored:#?}");
    let path = &monitored[0];
    assert_eq!(
        (path.method, path.fs_type.as_str(), path.reason.as_deref()),
        (Method::Poll, "unknown", Some("forced"))
    );
    assert!(!path.ready, "no baseline yet");
    several_polls().await;
    assert_eq!(log_lines(&[&missing]).len(), 1, "warned once");
    assert!(changes.pending().is_empty());

    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    wait_monitored(&watcher, "the path is ready once it appears", |m| {
        m.len() == 1 && m[0].ready && m[0].folders == 2
    })
    .await;
    let polling = format!(
        "library monitor: polling '{}' of 'Watch C Films' (",
        tree.root().display()
    );
    wait_for_log(&[&polling, ": 2 folders, every 100ms, baseline in "]).await;
    assert!(
        changes.pending().is_empty(),
        "the first poll that works only records the baseline"
    );

    tick();
    write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
    wait_pending(
        &changes,
        &[(lib.id, ScanScope::Folder(tree.at("Up (2009)")))],
    )
    .await;
    wait_monitored(&watcher, "the new folder is counted", |m| {
        m.len() == 1 && m[0].folders == 3
    })
    .await;
    watcher.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_path_missing_at_start_is_checked_again_at_the_next_reload_once_it_appears() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    let late = tree.root();
    let gone = tree.outside("never there");
    add_library(&pool, "Watch H Films", &[late.clone(), gone.clone()]).await;
    std::fs::remove_dir(&late).unwrap();
    std::fs::remove_dir(&gone).unwrap();
    // Not forced: each path gets the method its filesystem calls for.
    let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
    watcher.set_poll_interval(Duration::from_millis(100));
    watcher.start();

    let missing = |path: &Path| {
        format!(
            "library monitor: '{}' is missing or unreadable",
            path.display()
        )
    };
    let stopped = |path: &Path| {
        format!(
            "library monitor: stopped '{}' of 'Watch H Films'",
            path.display()
        )
    };
    wait_for_log(&[&missing(&late)]).await;
    wait_for_log(&[&missing(&gone)]).await;
    let monitored = watcher.monitored();
    assert_eq!(monitored.len(), 2, "{monitored:#?}");
    for path in &monitored {
        assert_eq!(
            (
                path.method,
                path.fs_type.as_str(),
                path.reason.as_deref(),
                path.ready
            ),
            (Method::Poll, "unknown", Some("path not found"), false),
            "{path:#?}"
        );
    }

    write(&late.join("Heat (1995)/Heat (1995).mkv"), 10);
    let monitored = wait_monitored(&watcher, "the path that appeared is polled", |m| {
        monitor_of(m, &late).is_some_and(|p| p.ready && p.folders == 2)
    })
    .await;
    assert_eq!(
        monitor_of(&monitored, &late).unwrap().reason.as_deref(),
        Some("path not found"),
        "polled as it was until the next reload"
    );

    watcher.libraries_changed();
    let fs_type = fs_type_of(&std::fs::canonicalize(&late).unwrap()).unwrap();
    let monitored = wait_monitored(&watcher, "the path that appeared is checked again", |m| {
        monitor_of(m, &late).is_some_and(|p| {
            p.ready && p.fs_type == fs_type && p.reason.as_deref() != Some("path not found")
        })
    })
    .await;
    assert_eq!(monitored.len(), 2, "{monitored:#?}");
    assert_eq!(
        log_lines(&[&stopped(&late)]).len(),
        1,
        "its first monitor is stopped"
    );

    several_polls().await;
    let monitored = watcher.monitored();
    let kept = monitor_of(&monitored, &gone).unwrap();
    assert_eq!(
        (kept.fs_type.as_str(), kept.reason.as_deref(), kept.ready),
        ("unknown", Some("path not found"), false)
    );
    assert!(
        log_lines(&[&stopped(&gone)]).is_empty(),
        "a path still missing keeps its monitor"
    );
    assert_eq!(
        log_lines(&[&missing(&gone)]).len(),
        1,
        "and is not warned about again"
    );
    assert!(
        changes.pending().is_empty(),
        "a path that appears is not reported"
    );
    watcher.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_monitoring_off_and_on_stops_and_restarts_the_monitors_of_that_library() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    let films_paths = [tree.outside("films"), tree.outside("more films")];
    let films = add_library(&pool, "Watch D Films", &films_paths).await;
    let shows = add_library(&pool, "Watch D Shows", &[tree.outside("shows")]).await;
    let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
    let all = [
        tree.outside("films"),
        tree.outside("more films"),
        tree.outside("shows"),
    ];
    let monitored =
        wait_monitored(&watcher, "three paths are ready", |m| ready_paths(m) == all).await;
    // Sorted by library name, then path.
    assert_eq!(
        monitored.iter().map(|p| p.library_id).collect::<Vec<_>>(),
        vec![films.id, films.id, shows.id]
    );

    set_monitoring(&pool, &watcher, &films, false).await;
    wait_monitored(&watcher, "only the shows are monitored", |m| {
        m.len() == 1 && ready_paths(m) == [tree.outside("shows")]
    })
    .await;
    for path in &films_paths {
        let stopped = format!(
            "library monitor: stopped '{}' of 'Watch D Films'",
            path.display()
        );
        wait_for_log(&[&stopped]).await;
    }
    tick();
    write(&tree.outside("films/Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.outside("shows/Lost/Season 1/S01E01.mkv"), 10);
    let lost = (shows.id, ScanScope::Folder(tree.outside("shows/Lost")));
    wait_pending(&changes, std::slice::from_ref(&lost)).await;
    several_polls().await;
    assert_eq!(
        changes.pending(),
        vec![lost.clone()],
        "nothing from the films"
    );

    set_monitoring(&pool, &watcher, &films, true).await;
    wait_monitored(&watcher, "the films are monitored again", |m| {
        ready_paths(m) == all
    })
    .await;
    tick();
    write(&tree.outside("more films/Up (2009)/Up (2009).mkv"), 10);
    let up = (
        films.id,
        ScanScope::Folder(tree.outside("more films/Up (2009)")),
    );
    wait_pending(&changes, std::slice::from_ref(&up)).await;
    // What changed while monitoring was off is in the new baseline; the scheduled scan takes it in.
    several_polls().await;
    assert_eq!(changes.pending(), sorted(vec![lost, up]));
    watcher.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_and_removing_libraries_and_paths_changes_what_is_monitored() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    let films = add_library(&pool, "Watch E Films", &[tree.outside("films")]).await;
    let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
    wait_monitored(&watcher, "the films path", |m| {
        ready_paths(m) == [tree.outside("films")]
    })
    .await;

    let shows = add_library(&pool, "Watch E Shows", &[tree.outside("shows")]).await;
    watcher.libraries_changed();
    wait_monitored(&watcher, "the shows library is added", |m| {
        ready_paths(m) == [tree.outside("films"), tree.outside("shows")]
    })
    .await;

    std::fs::create_dir_all(tree.outside("more films")).unwrap();
    let both = [
        path_string(&tree.outside("films")),
        path_string(&tree.outside("more films")),
    ];
    rustyfin_db::repo_library::update_library_paths(&pool, films.id, &both)
        .await
        .unwrap();
    watcher.libraries_changed();
    wait_monitored(&watcher, "a films path is added", |m| {
        ready_paths(m)
            == [
                tree.outside("films"),
                tree.outside("more films"),
                tree.outside("shows"),
            ]
    })
    .await;

    let second = [path_string(&tree.outside("more films"))];
    rustyfin_db::repo_library::update_library_paths(&pool, films.id, &second)
        .await
        .unwrap();
    watcher.libraries_changed();
    wait_monitored(&watcher, "the first films path is removed", |m| {
        m.len() == 2 && ready_paths(m) == [tree.outside("more films"), tree.outside("shows")]
    })
    .await;
    let stopped = format!(
        "library monitor: stopped '{}' of 'Watch E Films'",
        tree.outside("films").display()
    );
    wait_for_log(&[&stopped]).await;

    rustyfin_db::repo_library::delete_library(&pool, shows.id)
        .await
        .unwrap();
    watcher.libraries_changed();
    wait_monitored(&watcher, "the shows library is removed", |m| {
        m.len() == 1 && ready_paths(m) == [tree.outside("more films")]
    })
    .await;
    let stopped = format!(
        "library monitor: stopped '{}' of 'Watch E Shows'",
        tree.outside("shows").display()
    );
    wait_for_log(&[&stopped]).await;

    tick();
    write(&tree.outside("films/Heat (1995)/Heat (1995).mkv"), 10);
    write(&tree.outside("shows/Lost/S01E01.mkv"), 10);
    write(&tree.outside("more films/Up (2009)/Up (2009).mkv"), 10);
    let up = (
        films.id,
        ScanScope::Folder(tree.outside("more films/Up (2009)")),
    );
    wait_pending(&changes, std::slice::from_ref(&up)).await;
    several_polls().await;
    assert_eq!(changes.pending(), vec![up]);
    watcher.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_database_error_keeps_the_current_monitors() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    add_library(&pool, "Watch F Films", &[tree.root()]).await;
    let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
    wait_monitored(&watcher, "the library path is ready", |m| {
        m.len() == 1 && m[0].ready
    })
    .await;

    pool.close().await;
    watcher.libraries_changed();
    wait_for_log(&[
        "library monitor: could not load the libraries (",
        "); keeping the current monitors",
    ])
    .await;
    assert_eq!(ready_paths(&watcher.monitored()), [tree.root()]);
    watcher.stop();
}

/// Fix round 2, item E: a reconcile that fails is tried again a little later, so a library added
/// while the database failed is monitored without another reload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_reconcile_is_retried() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    add_library(&pool, "Watch I Films", &[tree.outside("films")]).await;
    let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
    wait_monitored(&watcher, "the films path", |m| {
        ready_paths(m) == [tree.outside("films")]
    })
    .await;

    watcher.fail_reconciles(1);
    add_library(&pool, "Watch I Shows", &[tree.outside("shows")]).await;
    watcher.libraries_changed();
    wait_for_log(&[
        "library monitor: could not load the libraries (",
        "injected for a test",
    ])
    .await;
    wait_monitored(&watcher, "the shows library is added by a retry", |m| {
        ready_paths(m) == [tree.outside("films"), tree.outside("shows")]
    })
    .await;
    watcher.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_poll_interval_applies_at_once() {
    let _logs = captured_logs();
    let tree = Tree::new();
    let (pool, changes) = manager_fixture(&tree).await;
    let lib = add_library(&pool, "Watch G Films", &[tree.root()]).await;
    let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
    assert_eq!(
        watcher.poll_interval(),
        Duration::from_secs(60),
        "the default"
    );
    watcher.set_force_poll(true);
    watcher.set_poll_interval(Duration::from_secs(3600));
    watcher.start();
    wait_monitored(&watcher, "the baseline", |m| m.len() == 1 && m[0].ready).await;

    tick();
    write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
    several_polls().await;
    assert!(
        changes.pending().is_empty(),
        "the next poll is an hour away"
    );
    watcher.set_poll_interval(Duration::from_millis(100));
    assert_eq!(watcher.poll_interval(), Duration::from_millis(100));
    wait_pending(
        &changes,
        &[(lib.id, ScanScope::Folder(tree.at("Heat (1995)")))],
    )
    .await;
    watcher.stop();
}

/// A poll that hangs, as one of a hard-mounted NFS share that doesn't answer does, doesn't hold up
/// shutdown: dropping the runtime, as the end of the server's `main` does, returns while the poll
/// still waits.
#[test]
fn shutdown_does_not_wait_for_a_poll_that_hangs() {
    let tree = Tree::new();
    let films = tree.at("Films");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (parked_tx, parked) = std::sync::mpsc::channel::<PathBuf>();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let (parked_tx, release_rx) = (Mutex::new(parked_tx), Mutex::new(release_rx));
    let (watcher, pool) = runtime.block_on(async {
        let (pool, changes) = manager_fixture(&tree).await;
        add_library(&pool, "Hanging Films", std::slice::from_ref(&films)).await;
        let watcher = LibraryWatcher::new(pool.clone(), changes, tree.outside("data"));
        watcher.set_force_poll(true);
        watcher.set_poll_interval(Duration::from_millis(100));
        watcher.set_poll_hook(Some(Arc::new(move |path: &Path| {
            let _ = parked_tx.lock().unwrap().send(path.to_path_buf());
            // Until the test lets go, or is gone.
            let _ = release_rx.lock().unwrap().recv();
        })));
        watcher.start();
        (watcher, pool)
    });
    let path = parked.recv_timeout(Duration::from_secs(20));
    assert_eq!(
        path.as_ref().ok(),
        Some(&films),
        "the poll reaches the hook"
    );

    runtime.block_on(async {
        watcher.stop();
        drop(watcher);
        pool.close().await;
    });
    let (dropped_tx, dropped) = std::sync::mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(runtime);
        let _ = dropped_tx.send(());
    });
    let shut_down = dropped.recv_timeout(Duration::from_secs(5));
    // The poll goes on, so that a runtime that waits for it can finish.
    drop(release);
    dropper.join().unwrap();
    assert!(
        shut_down.is_ok(),
        "dropping the runtime waited for the poll that hangs"
    );
}

// ── Kernel watching ──

/// Kernel watching exists only on Linux; elsewhere every path is polled.
#[cfg(target_os = "linux")]
mod kernel {
    use std::future::Future;
    use std::io::Write;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::pin::Pin;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use rustyfin_core::library_monitor::RescanRunner;
    use rustyfin_core::scanner::ScanChanges;

    use super::*;

    /// A runner that records every rescan and changes nothing.
    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<(Uuid, ScanScope)>>,
    }

    impl RescanRunner for Recorder {
        fn rescan(
            &self,
            library: Library,
            scope: ScanScope,
        ) -> Pin<Box<dyn Future<Output = Result<ScanChanges, String>> + Send>> {
            self.calls.lock().unwrap().push((library.id, scope));
            Box::pin(async { Ok(ScanChanges::default()) })
        }
    }

    impl Recorder {
        /// Every rescan so far, in order.
        fn calls(&self) -> Vec<(Uuid, ScanScope)> {
            self.calls.lock().unwrap().clone()
        }

        /// How many times `unit` has been rescanned.
        fn count(&self, unit: &(Uuid, ScanScope)) -> usize {
            self.calls().iter().filter(|call| *call == unit).count()
        }

        /// Waits up to 20 s until `unit` has been rescanned `n` times.
        async fn wait_for(&self, unit: &(Uuid, ScanScope), n: usize) {
            let deadline = Instant::now() + Duration::from_secs(20);
            while self.count(unit) < n {
                assert!(
                    Instant::now() < deadline,
                    "{unit:?} not rescanned {n} times within 20 s: {:?}",
                    self.calls()
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    /// A database in `tree`, and a queue on it that hands each unit to a `Recorder` 200 ms after its
    /// last report.
    async fn kernel_fixture(tree: &Tree) -> (SqlitePool, ChangeQueue, Arc<Recorder>) {
        let (pool, changes) = manager_fixture(tree).await;
        let recorder = Arc::new(Recorder::default());
        changes.set_runner(recorder.clone());
        changes.set_delay(Duration::from_millis(200));
        (pool, changes, recorder)
    }

    /// A started watcher that watches local paths and polls the others every 100 ms.
    fn kernel_watcher(
        pool: &SqlitePool,
        changes: &ChangeQueue,
        data_dir: PathBuf,
    ) -> LibraryWatcher {
        let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), data_dir);
        watcher.set_poll_interval(Duration::from_millis(100));
        watcher.start();
        watcher
    }

    /// The movies every kernel test starts with: 4 folders with the library path.
    fn movie_tree(tree: &Tree) {
        write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
        write(&tree.at("Up (2009)/Up (2009).mkv"), 10);
        write(&tree.at("Up (2009)/extras/Making of.mkv"), 10);
    }

    /// Waits up to 20 s until `path` is watched, ready and holds `watches` watches, and returns it.
    async fn wait_watched(watcher: &LibraryWatcher, path: &Path, watches: usize) -> MonitoredPath {
        let what = format!("{} watched with {watches} watches", path.display());
        let monitored = wait_monitored(watcher, &what, |m| {
            monitor_of(m, path)
                .is_some_and(|p| p.method == Method::Watch && p.ready && p.watches == watches)
        })
        .await;
        monitor_of(&monitored, path).unwrap().clone()
    }

    /// Gives the monitors time to batch, report and rescan what they saw, so that a rescan that
    /// should not come would have: the 500 ms batch, the 200 ms settle delay and some slack.
    async fn quiet_period() {
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    fn folder_unit(lib: &Library, path: PathBuf) -> (Uuid, ScanScope) {
        (lib.id, ScanScope::Folder(path))
    }

    /// The rescans after the first `skip`, each of which must be one of `units`; and each of
    /// `units` must be among them.
    fn assert_rescans_since(recorder: &Recorder, skip: usize, units: &[(Uuid, ScanScope)]) {
        let calls = recorder.calls();
        let since = &calls[skip..];
        assert!(
            since.iter().all(|call| units.contains(call))
                && units.iter().all(|u| since.contains(u)),
            "expected rescans of {units:?}, got {since:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_local_library_path_is_watched_with_a_watch_per_folder() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        add_library(&pool, "Kernel A Films", &[tree.root()]).await;
        let fs_type = fs_type_of(&std::fs::canonicalize(tree.root()).unwrap()).unwrap();
        assert!(
            is_local_fs_type(&fs_type),
            "the kernel tests need the temp dir on a local filesystem, not {fs_type}"
        );
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));

        let path = wait_watched(&watcher, &tree.root(), 4).await;
        assert_eq!(
            (path.fs_type.as_str(), path.reason.as_deref(), path.folders),
            (fs_type.as_str(), None, 4)
        );
        let watching = format!(
            "library monitor: watching '{}' of 'Kernel A Films' ({fs_type}): 4 folders, 4 watches, ready in ",
            tree.root().display()
        );
        wait_for_log(&[&watching]).await;
        quiet_period().await;
        assert!(
            recorder.calls().is_empty(),
            "setting up rescans nothing: {:?}",
            recorder.calls()
        );
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_folder_is_watched_and_rescanned_with_what_it_holds() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel B Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
        let alien = folder_unit(&lib, tree.at("Alien (1979)"));
        recorder.wait_for(&alien, 1).await;
        wait_watched(&watcher, &tree.root(), 5).await;
        quiet_period().await;

        // The new folder's own watch reports what happens in it.
        let seen = recorder.count(&alien);
        std::fs::create_dir(tree.at("Alien (1979)/extras")).unwrap();
        recorder.wait_for(&alien, seen + 1).await;
        wait_watched(&watcher, &tree.root(), 6).await;

        // Folders made at once, each inside the one before: what a walk doesn't list raises an
        // event, and nothing is watched twice.
        std::fs::create_dir_all(tree.at("Brazil (1985)/extras/Deleted scenes/Uncut")).unwrap();
        write(
            &tree.at("Brazil (1985)/extras/Deleted scenes/Scene 1.mkv"),
            10,
        );
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Brazil (1985)")), 1)
            .await;
        let path = wait_watched(&watcher, &tree.root(), 10).await;
        assert_eq!(path.folders, 10);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_file_created_renamed_or_deleted_rescans_its_unit() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel C Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        let heat = folder_unit(&lib, tree.at("Heat (1995)"));
        let up = folder_unit(&lib, tree.at("Up (2009)"));
        let root_files = (lib.id, ScanScope::RootFiles(tree.root()));
        // In a movie folder, in a subfolder of one, and in the library path.
        write(&tree.at("Heat (1995)/Heat (1995).en.srt"), 10);
        write(&tree.at("Up (2009)/extras/Trailer.mkv"), 10);
        write(&tree.at("Solaris (1972).mkv"), 10);
        recorder.wait_for(&heat, 1).await;
        recorder.wait_for(&up, 1).await;
        recorder.wait_for(&root_files, 1).await;
        quiet_period().await;
        assert_rescans_since(
            &recorder,
            0,
            &[heat.clone(), up.clone(), root_files.clone()],
        );

        let before = recorder.calls().len();
        std::fs::rename(
            tree.at("Heat (1995)/Heat (1995).en.srt"),
            tree.at("Heat (1995)/Heat (1995).de.srt"),
        )
        .unwrap();
        std::fs::remove_file(tree.at("Up (2009)/extras/Trailer.mkv")).unwrap();
        std::fs::remove_file(tree.at("Solaris (1972).mkv")).unwrap();
        for unit in [&heat, &up, &root_files] {
            let seen = recorder.calls()[..before]
                .iter()
                .filter(|call| *call == unit)
                .count();
            recorder.wait_for(unit, seen + 1).await;
        }
        quiet_period().await;
        assert_rescans_since(&recorder, before, &[heat, up, root_files]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_renamed_or_moved_folder_is_rescanned_under_both_names_and_watched_under_the_new_one()
    {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        write(&tree.outside("incoming/Alien (1979)/Alien (1979).mkv"), 10);
        write(
            &tree.outside("incoming/Alien (1979)/extras/Trailer.mkv"),
            10,
        );
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel D Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        // Renamed within the library: both names are rescanned, and the folder and its subfolder
        // are watched under the new name.
        let old_up = folder_unit(&lib, tree.at("Up (2009)"));
        let new_up = folder_unit(&lib, tree.at("Up (2009) [1080p]"));
        std::fs::rename(tree.at("Up (2009)"), tree.at("Up (2009) [1080p]")).unwrap();
        recorder.wait_for(&old_up, 1).await;
        recorder.wait_for(&new_up, 1).await;
        quiet_period().await;
        wait_watched(&watcher, &tree.root(), 4).await;
        let seen = recorder.count(&new_up);
        write(&tree.at("Up (2009) [1080p]/extras/Deleted scene.mkv"), 10);
        recorder.wait_for(&new_up, seen + 1).await;
        quiet_period().await;
        assert_eq!(recorder.count(&old_up), 1, "{:?}", recorder.calls());

        // Moved in from outside the library, with a subfolder.
        let alien = folder_unit(&lib, tree.at("Alien (1979)"));
        std::fs::rename(
            tree.outside("incoming/Alien (1979)"),
            tree.at("Alien (1979)"),
        )
        .unwrap();
        recorder.wait_for(&alien, 1).await;
        wait_watched(&watcher, &tree.root(), 6).await;

        // Moved out of the library: rescanned, then no longer watched.
        let heat = folder_unit(&lib, tree.at("Heat (1995)"));
        std::fs::rename(tree.at("Heat (1995)"), tree.outside("Heat (1995)")).unwrap();
        recorder.wait_for(&heat, 1).await;
        wait_watched(&watcher, &tree.root(), 5).await;
        quiet_period().await;
        let before = recorder.calls().len();
        write(&tree.outside("Heat (1995)/Heat (1995).en.srt"), 10);
        quiet_period().await;
        assert_rescans_since(&recorder, before, &[]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deleted_folder_is_rescanned_and_its_watches_are_dropped() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel E Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        std::fs::remove_dir_all(tree.at("Up (2009)")).unwrap();
        let up = folder_unit(&lib, tree.at("Up (2009)"));
        recorder.wait_for(&up, 1).await;
        let path = wait_watched(&watcher, &tree.root(), 2).await;
        assert_eq!(path.folders, 2);
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[up]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_copy_is_rescanned_when_it_starts_and_again_when_it_ends() {
        use std::io::Write;
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel F Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        let heat = folder_unit(&lib, tree.at("Heat (1995)"));
        let mut copy =
            std::fs::File::create(tree.at("Heat (1995)/Heat (1995) - 2160p.mkv")).unwrap();
        copy.write_all(&[b'x'; 1024]).unwrap();
        recorder.wait_for(&heat, 1).await;
        // The copy goes on: writes raise nothing.
        for _ in 0..10 {
            copy.write_all(&[b'x'; 1024]).unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        quiet_period().await;
        assert_eq!(recorder.count(&heat), 1, "{:?}", recorder.calls());
        // It ends: the file is closed after writing, and its folder is rescanned again.
        drop(copy);
        recorder.wait_for(&heat, 2).await;
        quiet_period().await;
        assert_eq!(recorder.count(&heat), 2, "{:?}", recorder.calls());
        assert_rescans_since(&recorder, 0, &[heat]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ignored_folders_and_the_data_dir_get_no_watch_and_raise_nothing() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        write(&tree.at(".@__thumb/Heat (1995).mkv"), 10);
        write(&tree.at("Heat (1995)/metadata/poster.jpg"), 10);
        write(
            &tree.at("Heat (1995)/Heat (1995).trickplay/320 - 10x10/0.jpg"),
            10,
        );
        write(&tree.at("rustyfin-data/library.db"), 10);
        write(&tree.at("rustyfin-data/images/a/poster.jpg"), 10);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel G Films", &[tree.root()]).await;
        // The data dir lies inside the library path.
        let watcher = kernel_watcher(&pool, &changes, tree.at("rustyfin-data"));
        let path = wait_watched(&watcher, &tree.root(), 4).await;
        assert_eq!(path.folders, 4);

        write(&tree.at(".@__thumb/Up (2009).mkv"), 10);
        write(&tree.at("Heat (1995)/metadata/fanart.jpg"), 10);
        write(
            &tree.at("Heat (1995)/Heat (1995).trickplay/320 - 10x10/1.jpg"),
            10,
        );
        write(&tree.at("@eaDir/Up (2009).mkv/SYNOINDEX_MEDIA_INFO"), 10);
        write(&tree.at("rustyfin-data/library.db-wal"), 10);
        write(&tree.at("rustyfin-data/images/b/poster.jpg"), 10);
        write(&tree.at("rustyfin-data/transcodes/x/0.ts"), 10);
        quiet_period().await;
        assert!(recorder.calls().is_empty(), "{:?}", recorder.calls());
        assert_eq!(watcher.monitored()[0].watches, 4);

        // A change next to them is rescanned.
        write(&tree.at("Up (2009)/Up (2009).en.srt"), 10);
        let up = folder_unit(&lib, tree.at("Up (2009)"));
        recorder.wait_for(&up, 1).await;
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[up]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reading_a_file_or_changing_its_times_raises_nothing() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        add_library(&pool, "Kernel H Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        let movie = tree.at("Heat (1995)/Heat (1995).mkv");
        assert_eq!(std::fs::read(&movie).unwrap().len(), 10);
        assert!(std::fs::metadata(&movie).unwrap().is_file());
        assert_eq!(std::fs::read_dir(tree.at("Up (2009)")).unwrap().count(), 2);
        std::fs::File::open(&movie)
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        quiet_period().await;
        assert!(recorder.calls().is_empty(), "{:?}", recorder.calls());
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_queue_overflow_rescans_the_whole_library() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel I Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        watcher.fake_overflow(lib.id);
        let whole = (lib.id, ScanScope::Library);
        recorder.wait_for(&whole, 1).await;
        let overflowed = format!(
            "library monitor: the event queue of '{}' overflowed; rescanning 'Kernel I Films'",
            tree.root().display()
        );
        wait_for_log(&[&overflowed]).await;
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[whole]);
        assert_eq!(
            watcher.monitored()[0].watches,
            4,
            "walking again adds no watch twice"
        );

        // It keeps watching.
        write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Alien (1979)")), 1)
            .await;
        wait_watched(&watcher, &tree.root(), 5).await;
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_symlinked_local_folder_is_watched_and_reported_by_its_path_in_the_library() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        write(
            &tree.outside("elsewhere/Brazil (1985)/Brazil (1985).mkv"),
            10,
        );
        write(
            &tree.outside("elsewhere/Solaris (1972)/Solaris (1972).mkv"),
            10,
        );
        symlink(
            tree.outside("elsewhere/Brazil (1985)"),
            tree.at("Brazil (1985)"),
        )
        .unwrap();
        // A second link to the same folder: it is watched once, under the name that sorts first.
        symlink(
            tree.outside("elsewhere/Brazil (1985)"),
            tree.at("Brazil (1985) copy"),
        )
        .unwrap();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel J Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 5).await;

        write(
            &tree.outside("elsewhere/Brazil (1985)/Brazil (1985).en.srt"),
            10,
        );
        let brazil = folder_unit(&lib, tree.at("Brazil (1985)"));
        recorder.wait_for(&brazil, 1).await;
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[brazil]);

        // A link made later is followed too.
        let before = recorder.calls().len();
        symlink(
            tree.outside("elsewhere/Solaris (1972)"),
            tree.at("Solaris (1972)"),
        )
        .unwrap();
        let solaris = folder_unit(&lib, tree.at("Solaris (1972)"));
        recorder.wait_for(&solaris, 1).await;
        wait_watched(&watcher, &tree.root(), 6).await;
        quiet_period().await;
        let seen = recorder.count(&solaris);
        write(
            &tree.outside("elsewhere/Solaris (1972)/Solaris (1972).en.srt"),
            10,
        );
        recorder.wait_for(&solaris, seen + 1).await;
        quiet_period().await;
        assert_rescans_since(&recorder, before, &[solaris]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_path_over_the_watch_limit_is_polled_and_the_limit_is_warned_about_once() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        // A second path of the library, with 6 folders.
        let more = tree.outside("more");
        write(&more.join("Alien (1979)/extras/Trailer.mkv"), 10);
        write(&more.join("Brazil (1985)/extras/Trailer.mkv"), 10);
        write(&more.join("Casablanca (1942)/Casablanca (1942).mkv"), 10);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel K Films", &[tree.root(), more.clone()]).await;
        let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
        watcher.set_watch_limit(Some(4));
        watcher.set_poll_interval(Duration::from_millis(100));
        watcher.start();

        // The path with 4 folders fits; the one with 6 doesn't, and is polled from the start.
        wait_watched(&watcher, &tree.root(), 4).await;
        let monitored = wait_monitored(&watcher, "the path over the limit polled", |m| {
            monitor_of(m, &more).is_some_and(|p| p.method == Method::Poll && p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &more).unwrap();
        let fs_type = fs_type_of(&std::fs::canonicalize(&more).unwrap()).unwrap();
        assert_eq!(
            (
                polled.fs_type.as_str(),
                polled.reason.as_deref(),
                polled.watches,
                polled.folders
            ),
            (fs_type.as_str(), Some("watch limit reached"), 0, 6)
        );
        let warning = |path: &Path| {
            format!(
                "library monitor: the kernel's inotify watch limit is reached; polling '{}' instead. Raise fs.inotify.max_user_watches to watch it; the next SIGHUP or library change tries again.",
                path.display()
            )
        };
        let polling = |path: &Path, folders: usize| {
            format!(
                "library monitor: polling '{}' of 'Kernel K Films' ({fs_type}, watch limit reached): {folders} folders, every 100ms, baseline in ",
                path.display()
            )
        };
        wait_for_log(&[&warning(&more)]).await;
        wait_for_log(&[&polling(&more, 6)]).await;
        quiet_period().await;
        assert!(
            recorder.calls().is_empty(),
            "falling back during setup reports nothing: {:?}",
            recorder.calls()
        );

        // A new folder takes the watched path over the limit too. The folder is still rescanned, the
        // path is polled from then on, and the limit is not warned about again.
        write(&tree.at("Alien (1979)/Alien (1979).mkv"), 10);
        let alien = folder_unit(&lib, tree.at("Alien (1979)"));
        recorder.wait_for(&alien, 1).await;
        let monitored = wait_monitored(&watcher, "the watched path polled", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.method == Method::Poll && p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &tree.root()).unwrap();
        assert_eq!(
            (polled.reason.as_deref(), polled.watches, polled.folders),
            (Some("watch limit reached"), 0, 5)
        );
        wait_for_log(&[&polling(&tree.root(), 5)]).await;
        assert_eq!(log_lines(&[&warning(&more)]).len(), 1);
        assert!(
            log_lines(&[&warning(&tree.root())]).is_empty(),
            "the limit is warned about once per watcher"
        );

        // Both paths are polled now.
        quiet_period().await;
        tick();
        write(&tree.at("Heat (1995)/Heat (1995).en.srt"), 10);
        write(&more.join("Casablanca (1942)/Casablanca (1942).en.srt"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Heat (1995)")), 1)
            .await;
        recorder
            .wait_for(&folder_unit(&lib, more.join("Casablanca (1942)")), 1)
            .await;
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_path_over_the_watch_limit_is_watched_once_the_limit_is_lifted_and_libraries_change()
    {
        // Fix round 1, item 2: a fallback at a limit is not for good. Once the limit allows it, a
        // library change (a SIGHUP makes one too) sets the path up again, watched. While the
        // limit holds, a change keeps the monitor (fix round 2, item C) and warns no more.
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel L Films", &[tree.root()]).await;
        let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
        watcher.set_watch_limit(Some(2));
        watcher.set_poll_interval(Duration::from_millis(100));
        watcher.start();
        let polling = format!(
            "library monitor: polling '{}' of 'Kernel L Films' (",
            tree.root().display()
        );
        let polled = [
            polling.as_str(),
            ", watch limit reached): 4 folders, every 100ms, baseline in ",
        ];
        wait_for_log(&polled).await;

        // The limit still holds: the monitor is kept at a change, and warned about once.
        watcher.libraries_changed();
        quiet_period().await;
        assert_eq!(
            log_lines(&polled).len(),
            1,
            "the monitor is kept while the limit holds"
        );
        let warning = format!(
            "library monitor: the kernel's inotify watch limit is reached; polling '{}' instead.",
            tree.root().display()
        );
        assert_eq!(log_lines(&[&warning]).len(), 1, "warned about once");

        // Lifted: watched after the next change, and a change is a folder rescan.
        watcher.set_watch_limit(None);
        watcher.libraries_changed();
        wait_watched(&watcher, &tree.root(), 4).await;
        write(&tree.at("Heat (1995)/Heat (1995).en.srt"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Heat (1995)")), 1)
            .await;
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_path_over_the_watch_limit_keeps_its_baseline_when_libraries_change_and_the_limit_does_not()
     {
        // Fix round 2, item C: a library change, a SIGHUP or a mount restart of another path keeps
        // a monitor that polls for a limit that has not changed, so what changed since its last
        // poll is still reported. No poll runs until the change has been handled.
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel R Films", &[tree.root()]).await;
        let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
        watcher.set_watch_limit(Some(2));
        watcher.set_poll_interval(Duration::from_secs(3600));
        watcher.start();
        wait_monitored(&watcher, "the baseline at the watch limit", |m| {
            monitor_of(m, &tree.root())
                .is_some_and(|p| p.ready && p.reason.as_deref() == Some("watch limit reached"))
        })
        .await;

        tick();
        write(&tree.at("Bravo (2002)/Bravo (2002).mkv"), 10);
        watcher.libraries_changed();
        tokio::time::sleep(Duration::from_secs(1)).await;
        watcher.set_poll_interval(Duration::from_millis(100));
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Bravo (2002)")), 1)
            .await;
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deleted_library_path_is_polled_until_it_is_back_and_watched_again_at_the_next_reload()
     {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel M Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        // What is deleted before the path itself is rescanned, but not the whole library.
        std::fs::remove_dir_all(tree.root()).unwrap();
        let heat = folder_unit(&lib, tree.at("Heat (1995)"));
        let up = folder_unit(&lib, tree.at("Up (2009)"));
        recorder.wait_for(&heat, 1).await;
        recorder.wait_for(&up, 1).await;
        let monitored = wait_monitored(&watcher, "the deleted path polled", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.method == Method::Poll)
        })
        .await;
        let polled = monitor_of(&monitored, &tree.root()).unwrap();
        assert_eq!(
            (polled.reason.as_deref(), polled.watches, polled.ready),
            (Some("library path gone"), 0, false)
        );
        let missing = format!(
            "library monitor: '{}' is missing or unreadable",
            tree.root().display()
        );
        wait_for_log(&[&missing]).await;
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[heat.clone(), up]);

        // Back, all at once: the poller takes its baseline, which reports nothing.
        write(&tree.outside("staging/Heat (1995)/Heat (1995).mkv"), 10);
        std::fs::rename(tree.outside("staging"), tree.root()).unwrap();
        let monitored = wait_monitored(&watcher, "the path back and polled", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &tree.root()).unwrap();
        assert_eq!(
            (polled.method, polled.reason.as_deref(), polled.folders),
            (Method::Poll, Some("library path gone"), 2)
        );
        let before = recorder.calls().len();
        several_polls().await;
        assert_rescans_since(&recorder, before, &[]);

        // The next reload watches it again.
        watcher.libraries_changed();
        let path = wait_watched(&watcher, &tree.root(), 2).await;
        assert_eq!(path.reason, None);
        let stopped = format!(
            "library monitor: stopped '{}' of 'Kernel M Films'",
            tree.root().display()
        );
        assert_eq!(
            log_lines(&[&stopped]).len(),
            1,
            "its poll monitor is stopped"
        );
        assert_eq!(log_lines(&[&missing]).len(), 1, "and was warned about once");
        write(&tree.at("Heat (1995)/Heat (1995).en.srt"), 10);
        recorder.wait_for(&heat, 2).await;
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_symlinked_folder_on_a_filesystem_that_is_not_local_makes_its_path_polled() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        // /proc/sys/fs/inotify: a small folder on a filesystem that is not local.
        symlink("/proc/sys/fs/inotify", tree.at("Inotify settings")).unwrap();
        let later = tree.outside("later");
        write(&later.join("Heat (1995)/Heat (1995).mkv"), 10);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel N Films", &[tree.root(), later.clone()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));

        // A link there at the start: the path is polled from the start.
        let monitored = wait_monitored(&watcher, "the path with the link polled", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.method == Method::Poll && p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &tree.root()).unwrap();
        let fs_type = fs_type_of(&std::fs::canonicalize(tree.root()).unwrap()).unwrap();
        assert_eq!(
            (
                polled.fs_type.as_str(),
                polled.reason.as_deref(),
                polled.watches,
                polled.folders
            ),
            (fs_type.as_str(), Some("symlinked folder on proc"), 0, 5)
        );
        wait_watched(&watcher, &later, 2).await;

        // A link made later: it is rescanned, and its path is polled from then on.
        symlink("/proc/sys/fs/inotify", later.join("Inotify settings")).unwrap();
        recorder
            .wait_for(&folder_unit(&lib, later.join("Inotify settings")), 1)
            .await;
        let monitored = wait_monitored(&watcher, "the path with the later link polled", |m| {
            monitor_of(m, &later).is_some_and(|p| p.method == Method::Poll && p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &later).unwrap();
        assert_eq!(
            (polled.reason.as_deref(), polled.watches, polled.folders),
            (Some("symlinked folder on proc"), 0, 3)
        );
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_symlinked_folder_moved_into_a_library_path_is_watched_like_one_made_there() {
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        write(
            &tree.outside("elsewhere/Solaris (1972)/extras/Trailer.mkv"),
            10,
        );
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel P Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        // The link is made outside and moved in, which raises MOVED_TO without ISDIR.
        symlink(
            tree.outside("elsewhere/Solaris (1972)"),
            tree.outside("link"),
        )
        .unwrap();
        std::fs::rename(tree.outside("link"), tree.at("Solaris (1972)")).unwrap();
        let solaris = folder_unit(&lib, tree.at("Solaris (1972)"));
        recorder.wait_for(&solaris, 1).await;
        wait_watched(&watcher, &tree.root(), 6).await;

        // A change in the linked folder is rescanned under its path in the library.
        write(
            &tree.outside("elsewhere/Solaris (1972)/Solaris (1972).en.srt"),
            10,
        );
        recorder.wait_for(&solaris, 2).await;
        quiet_period().await;
        assert_rescans_since(&recorder, 0, &[solaris]);
        watcher.stop();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_library_path_on_a_filesystem_that_is_not_local_is_polled() {
        let _logs = captured_logs();
        let tree = Tree::new();
        let (pool, changes, _recorder) = kernel_fixture(&tree).await;
        let settings = PathBuf::from("/proc/sys/fs/inotify");
        add_library(&pool, "Kernel O Settings", std::slice::from_ref(&settings)).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));

        let monitored = wait_monitored(&watcher, "the proc path polled", |m| {
            monitor_of(m, &settings).is_some_and(|p| p.ready)
        })
        .await;
        let polled = monitor_of(&monitored, &settings).unwrap();
        assert_eq!(
            (
                polled.method,
                polled.fs_type.as_str(),
                polled.reason.as_deref(),
                polled.watches,
                polled.folders
            ),
            (
                Method::Poll,
                "proc",
                Some("network or unknown filesystem"),
                0,
                1
            )
        );
        wait_for_log(&[
        "library monitor: polling '/proc/sys/fs/inotify' of 'Kernel O Settings' (proc, network or unknown filesystem): 1 folders, every 100ms, baseline in ",
    ])
    .await;
        watcher.stop();
    }

    // ── Mounts ──
    // Mounting needs a mount namespace of its own. `mount_changes_in_a_namespace` runs the ignored
    // `mount_ns_` tests again under `unshare -rm`, where they mount tmpfs; nothing is mounted
    // outside it. Where unshare is not allowed (CI's container), it skips, and says so on stderr;
    // `RUSTYFIN_REQUIRE_MOUNT_NS=1` makes it fail there instead. Its notes go to stderr with
    // `writeln!`, which libtest doesn't capture, unlike `eprintln!`.

    /// Set for the run of the `mount_ns_` tests under `unshare -rm`.
    const MOUNT_NS: &str = "RUSTYFIN_TEST_MOUNT_NS";

    /// Set to 1 to make `mount_changes_in_a_namespace` fail where `unshare -rm` is refused,
    /// instead of skipping.
    const REQUIRE_MOUNT_NS: &str = "RUSTYFIN_REQUIRE_MOUNT_NS";

    #[test]
    fn mount_changes_in_a_namespace() {
        let allowed = Command::new("unshare")
            .args(["-rm", "true"])
            .output()
            .is_ok_and(|out| out.status.success());
        if !allowed {
            assert!(
                std::env::var(REQUIRE_MOUNT_NS).as_deref() != Ok("1"),
                "unshare refused; the mount_ns_ tests did not run, and {REQUIRE_MOUNT_NS}=1 requires them"
            );
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED: unshare refused; the mount_ns_ tests did not run ({REQUIRE_MOUNT_NS}=1 makes this fail)"
            );
            return;
        }
        let out = Command::new("unshare")
            .arg("-rm")
            .arg(std::env::current_exe().unwrap())
            .args(["mount_ns_", "--ignored", "--nocapture", "--test-threads=1"])
            .env(MOUNT_NS, "1")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success()
                && text.contains("test result: ok.")
                && !text.contains(" 0 passed"),
            "the mount tests failed under `unshare -rm`:\n{text}"
        );
        if let Some(result) = text.lines().find(|line| line.starts_with("test result:")) {
            let _ = writeln!(
                std::io::stderr(),
                "mount_ns_ tests under `unshare -rm`: {result}"
            );
        }
    }

    /// Fix round 2, item B: where `unshare -rm` is refused, `mount_changes_in_a_namespace` says so
    /// on stderr, which libtest doesn't hide when the test passes, and `RUSTYFIN_REQUIRE_MOUNT_NS=1`
    /// makes it fail instead. An `unshare` that exits 1, first on `PATH`, stands in for the refusal.
    #[test]
    fn a_refused_unshare_is_shown_and_can_be_made_to_fail() {
        let tree = Tree::new();
        let bin = tree.outside("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let unshare = bin.join("unshare");
        std::fs::write(&unshare, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&unshare, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let run = |require: bool| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["kernel::mount_changes_in_a_namespace", "--exact"])
                .env("PATH", &path)
                .env_remove(MOUNT_NS)
                .env_remove(REQUIRE_MOUNT_NS);
            if require {
                command.env(REQUIRE_MOUNT_NS, "1");
            }
            command.output().unwrap()
        };

        let skipped = run(false);
        let stderr = String::from_utf8_lossy(&skipped.stderr);
        assert!(
            skipped.status.success(),
            "a refused unshare skips:\n{stderr}"
        );
        assert!(
            stderr.contains("SKIPPED: unshare refused"),
            "the skip is not shown on stderr:\n{stderr}"
        );
        let required = run(true);
        assert!(
            !required.status.success(),
            "{REQUIRE_MOUNT_NS}=1 doesn't make a refused unshare fail:\n{}",
            String::from_utf8_lossy(&required.stdout)
        );
    }

    /// Whether this is the run under `unshare -rm`; says so where it isn't.
    fn in_mount_namespace() -> bool {
        let inside = std::env::var_os(MOUNT_NS).is_some();
        if !inside {
            let thread = std::thread::current();
            let _ = writeln!(
                std::io::stderr(),
                "SKIPPED: {} mounts, so it runs only under `unshare -rm`, from mount_changes_in_a_namespace",
                thread.name().unwrap_or("this test")
            );
        }
        inside
    }

    /// A tmpfs mounted on a folder, as a share would be; unmounted when dropped unless the test
    /// unmounted it.
    struct Share(Option<PathBuf>);

    impl Share {
        fn mount(at: &Path) -> Self {
            Self::mount_type("tmpfs", at)
        }

        /// Mounts a new filesystem of type `fs_type` on `at`.
        fn mount_type(fs_type: &str, at: &Path) -> Self {
            std::fs::create_dir_all(at).unwrap();
            let status = Command::new("mount")
                .args(["-t", fs_type, fs_type])
                .arg(at)
                .status()
                .unwrap();
            assert!(status.success(), "mounting {fs_type} on {}", at.display());
            Self(Some(at.to_path_buf()))
        }

        /// Mounts the folder `src` on `at` too (a bind mount), as a share that keeps its files
        /// while it is away from `at`.
        fn bind(src: &Path, at: &Path) -> Self {
            std::fs::create_dir_all(at).unwrap();
            let status = Command::new("mount")
                .arg("--bind")
                .arg(src)
                .arg(at)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "bind-mounting {} on {}",
                src.display(),
                at.display()
            );
            Self(Some(at.to_path_buf()))
        }

        /// Unmounts it lazily, as a share that goes away does, also while a poll reads it.
        fn unmount(&mut self) {
            let at = self.0.take().unwrap();
            let status = Command::new("umount").arg("-l").arg(&at).status().unwrap();
            assert!(status.success(), "unmounting {}", at.display());
        }
    }

    impl Drop for Share {
        fn drop(&mut self) {
            // A test may have mounted more on it; each unmount takes the top one.
            if let Some(at) = self.0.take() {
                for _ in 0..8 {
                    let unmounted = Command::new("umount")
                        .arg("-l")
                        .arg(&at)
                        .stderr(std::process::Stdio::null())
                        .status()
                        .is_ok_and(|status| status.success());
                    if !unmounted {
                        break;
                    }
                }
            }
        }
    }

    /// Holds each poll before it reads, while it is closed.
    #[derive(Clone, Default)]
    struct PollGate {
        closed: Arc<AtomicBool>,
        held: Arc<AtomicUsize>,
    }

    impl PollGate {
        /// The poll hook that waits at the gate.
        fn hook(&self) -> Arc<dyn Fn(&Path) + Send + Sync> {
            let gate = self.clone();
            Arc::new(move |_: &Path| {
                if !gate.closed.load(Ordering::SeqCst) {
                    return;
                }
                gate.held.fetch_add(1, Ordering::SeqCst);
                while gate.closed.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                gate.held.fetch_sub(1, Ordering::SeqCst);
            })
        }

        /// Closes it, and waits up to 20 s until a poll is held there.
        async fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(20);
            while self.held.load(Ordering::SeqCst) == 0 {
                assert!(Instant::now() < deadline, "no poll is held within 20 s");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        fn open(&self) {
            self.closed.store(false, Ordering::SeqCst);
        }
    }

    /// The reviewer's case. inotify doesn't see into a filesystem mounted over a watched folder,
    /// so a share mounted on a watched library path sets the path up again, on the share; what
    /// then changes on the share is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_on_a_watched_path_is_watched_after() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount A Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 1).await;
        let point = std::fs::canonicalize(tree.root()).unwrap();

        let _share = Share::mount(&tree.root());
        let watching = format!(
            "library monitor: watching '{}' of 'Mount A Films' (",
            tree.root().display()
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while log_lines(&[&watching]).len() < 2 {
            assert!(
                Instant::now() < deadline,
                "the path is not watched again on the share within 20 s: {:?}",
                watcher.monitored()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        write(&tree.at("Film (2001)/Film (2001).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Film (2001)")), 1)
            .await;
        let mounted = format!(
            "library monitor: '{}' was mounted; setting up '{}' of 'Mount A Films' again",
            point.display(),
            tree.root().display()
        );
        assert_eq!(log_lines(&[&mounted]).len(), 1, "logged once");
        watcher.stop();
    }

    /// A share mounted inside a polled library path goes away. Its folders were not deleted, so
    /// nothing is reported: the path is set up again and takes a new baseline.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_unmounted_from_inside_a_polled_path_reports_nothing() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
        let mut share = Share::mount(&tree.at("Shares"));
        write(&tree.at("Shares/Film (2001)/Film (2001).mkv"), 10);
        let point = std::fs::canonicalize(tree.at("Shares")).unwrap();
        let (pool, changes) = manager_fixture(&tree).await;
        add_library(&pool, "Mount B Films", &[tree.root()]).await;
        let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
        wait_monitored(&watcher, "the baseline with the share", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.ready && p.folders == 4)
        })
        .await;

        // No poll runs from just before the unmount until the path is set up again.
        watcher.set_poll_interval(Duration::from_secs(3600));
        several_polls().await;
        share.unmount();
        tokio::time::sleep(Duration::from_secs(1)).await;
        watcher.set_poll_interval(Duration::from_millis(100));
        several_polls().await;
        assert!(
            changes.pending().is_empty(),
            "the share's folders were not deleted: {:?}",
            changes.pending()
        );
        let unmounted = format!(
            "library monitor: '{}' was unmounted; setting up '{}' of 'Mount B Films' again",
            point.display(),
            tree.root().display()
        );
        assert_eq!(log_lines(&[&unmounted]).len(), 1, "logged once");
        let monitored = watcher.monitored();
        assert_eq!(
            monitor_of(&monitored, &tree.root()).map(|p| (p.ready, p.folders)),
            Some((true, 3)),
            "the new baseline"
        );
        watcher.stop();
    }

    /// A share mounted on a polled library path goes away and leaves the empty folder it was
    /// mounted on. That is an unmounted share, not every folder deleted: the path is warned about
    /// once and keeps its last state, its monitor is kept, and nothing is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_unmounted_from_a_polled_path_keeps_its_last_state() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let mut share = Share::mount(&tree.root());
        write(&tree.at("Film (2001)/Film (2001).mkv"), 10);
        let (pool, changes) = manager_fixture(&tree).await;
        add_library(&pool, "Mount C Films", &[tree.root()]).await;
        let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
        wait_monitored(&watcher, "the baseline", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.ready && p.folders == 2)
        })
        .await;

        share.unmount();
        let missing = format!(
            "library monitor: '{}' is missing or unreadable; keeping its last state until it is back",
            tree.root().display()
        );
        wait_for_log(&[&missing]).await;
        several_polls().await;
        assert_eq!(log_lines(&[&missing]).len(), 1, "warned about once");
        assert!(
            changes.pending().is_empty(),
            "nothing was deleted: {:?}",
            changes.pending()
        );
        let monitored = watcher.monitored();
        assert_eq!(
            monitor_of(&monitored, &tree.root()).map(|p| p.folders),
            Some(2),
            "the last state is kept"
        );
        let again = format!("setting up '{}' of", tree.root().display());
        assert!(log_lines(&[&again]).is_empty(), "the monitor is kept");
        watcher.stop();
    }

    /// The same for a watched library path: the kernel reports the path gone, so it is polled, and
    /// nothing is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_unmounted_from_a_watched_path_reports_nothing() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let mut share = Share::mount(&tree.root());
        write(&tree.at("Film (2001)/Film (2001).mkv"), 10);
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        add_library(&pool, "Mount D Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 2).await;

        share.unmount();
        let monitored = wait_monitored(&watcher, "the unmounted path polled", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.method == Method::Poll && p.ready)
        })
        .await;
        assert_eq!(
            monitor_of(&monitored, &tree.root()).and_then(|p| p.reason.clone()),
            Some("library path gone".to_string())
        );
        quiet_period().await;
        assert!(
            recorder.calls().is_empty(),
            "nothing was deleted: {:?}",
            recorder.calls()
        );
        let again = format!("setting up '{}' of", tree.root().display());
        assert!(log_lines(&[&again]).is_empty(), "the monitor is kept");
        watcher.stop();
    }

    /// A share mounted inside a watched library path goes away. The kernel drops the share's
    /// watches and nothing is reported; the path is set up again, so the folder that the share
    /// covered is watched from then on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_unmounted_from_inside_a_watched_path_reports_nothing() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        write(&tree.at("Heat (1995)/Heat (1995).mkv"), 10);
        let mut share = Share::mount(&tree.at("Shares"));
        write(&tree.at("Shares/Film (2001)/Film (2001).mkv"), 10);
        let point = std::fs::canonicalize(tree.at("Shares")).unwrap();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount E Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 4).await;

        share.unmount();
        // The old monitor keeps 2 watches; the new one also watches the folder under the share.
        wait_watched(&watcher, &tree.root(), 3).await;
        quiet_period().await;
        assert!(
            recorder.calls().is_empty(),
            "nothing was deleted: {:?}",
            recorder.calls()
        );
        write(&tree.at("Shares/Later (2010)/Later (2010).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Shares")), 1)
            .await;
        let unmounted = format!(
            "library monitor: '{}' was unmounted; setting up '{}' of 'Mount E Films' again",
            point.display(),
            tree.root().display()
        );
        assert_eq!(log_lines(&[&unmounted]).len(), 1, "logged once");
        watcher.stop();
    }

    /// The reviewer's case (a), criterion 11. The share on a polled library path goes away, a film
    /// is added on the NAS meanwhile, and the share comes back: the path is compared with its last
    /// state, so the film is reported. Polls are held while the share comes back, so that none
    /// falls between the mount and a restart of the monitor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_again_on_a_polled_path_is_compared_with_its_last_state() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        // The folder under the share must not have the share's modified time, or the first poll
        // after the unmount sees the library path unchanged.
        tick();
        let nas = tree.outside("nas");
        write(&nas.join("Alpha (2001)/Alpha (2001).mkv"), 10);
        let mut share = Share::bind(&nas, &tree.root());
        let (pool, changes) = manager_fixture(&tree).await;
        let lib = add_library(&pool, "Mount F Films", &[tree.root()]).await;
        let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
        let gate = PollGate::default();
        watcher.set_poll_hook(Some(gate.hook()));
        wait_monitored(&watcher, "the baseline with the share", |m| {
            monitor_of(m, &tree.root()).is_some_and(|p| p.ready && p.folders == 2)
        })
        .await;

        share.unmount();
        let missing = format!(
            "library monitor: '{}' is missing or unreadable; keeping its last state until it is back",
            tree.root().display()
        );
        wait_for_log(&[&missing]).await;
        write(&nas.join("Bravo (2002)/Bravo (2002).mkv"), 10);
        gate.close().await;
        let _share = Share::bind(&nas, &tree.root());
        // The follower sees the mount within milliseconds.
        tokio::time::sleep(Duration::from_secs(1)).await;
        gate.open();
        let bravo = (lib.id, ScanScope::Folder(tree.at("Bravo (2002)")));
        wait_pending(&changes, std::slice::from_ref(&bravo)).await;
        several_polls().await;
        assert_eq!(
            changes.pending(),
            vec![bravo],
            "the film added while the share was away, alone"
        );
        let back = format!("library monitor: '{}' is back", tree.root().display());
        assert_eq!(log_lines(&[&back]).len(), 1, "logged once");
        let again = format!("setting up '{}' of", tree.root().display());
        assert!(log_lines(&[&again]).is_empty(), "the monitor is kept");
        watcher.stop();
    }

    /// The reviewer's case (b): an automount that expires between polls is mounted again by each
    /// poll that reaches it. The poll hook does that here, before each poll. The path is still
    /// compared with its last state, so a film added on the share is reported, and its monitor is
    /// never set up again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_by_each_poll_is_still_compared() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let nas = tree.outside("nas");
        write(&nas.join("Alpha (2001)/Alpha (2001).mkv"), 10);
        let _share = Share::bind(&nas, &tree.root());
        let (pool, changes) = manager_fixture(&tree).await;
        let lib = add_library(&pool, "Mount G Films", &[tree.root()]).await;
        let watcher = polling_watcher(&pool, &changes, tree.outside("data"));
        let remounts = Arc::new(AtomicUsize::new(0));
        // The remounts end with the test, also when it fails, before its share and tree go.
        let remounting = Arc::new(());
        let alive = Arc::downgrade(&remounting);
        let (root, src, counted) = (tree.root(), nas.clone(), Arc::clone(&remounts));
        watcher.set_poll_hook(Some(Arc::new(move |_: &Path| {
            if alive.strong_count() == 0 {
                return;
            }
            let _ = Command::new("umount")
                .arg("-l")
                .arg(&root)
                .stderr(std::process::Stdio::null())
                .status();
            let _ = Command::new("mount")
                .arg("--bind")
                .arg(&src)
                .arg(&root)
                .stderr(std::process::Stdio::null())
                .status();
            counted.fetch_add(1, Ordering::SeqCst);
        })));
        let deadline = Instant::now() + Duration::from_secs(20);
        while remounts.load(Ordering::SeqCst) < 3 {
            assert!(Instant::now() < deadline, "fewer than 3 polls within 20 s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        write(&nas.join("Bravo (2002)/Bravo (2002).mkv"), 10);
        let bravo = (lib.id, ScanScope::Folder(tree.at("Bravo (2002)")));
        wait_pending(&changes, std::slice::from_ref(&bravo)).await;
        several_polls().await;
        assert_eq!(changes.pending(), vec![bravo], "the film added, alone");
        let again = format!("setting up '{}' of", tree.root().display());
        assert!(log_lines(&[&again]).is_empty(), "never set up again");
        drop(remounting);
        watcher.stop();
    }

    /// A library path on a filesystem that is not local is polled. Another such filesystem
    /// mounted on it keeps its monitor, which compares with its last state; a local one sets it
    /// up again, watched (a guard). devpts stands in for a network share: it is not local, and a
    /// user namespace may mount it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts devpts and tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_path_on_a_filesystem_that_is_not_local_keeps_its_monitor_when_one_is_mounted_on_it()
     {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let _ptys = Share::mount_type("devpts", &tree.root());
        let (pool, changes, _recorder) = kernel_fixture(&tree).await;
        add_library(&pool, "Mount H Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        let polling = format!(
            "library monitor: polling '{}' of 'Mount H Films' (devpts, network or unknown filesystem): ",
            tree.root().display()
        );
        wait_for_log(&[&polling]).await;

        let _more = Share::mount_type("devpts", &tree.root());
        quiet_period().await;
        let again = format!(
            "setting up '{}' of 'Mount H Films' again",
            tree.root().display()
        );
        assert!(
            log_lines(&[&again]).is_empty(),
            "a filesystem that is not local either keeps the monitor"
        );

        let _local = Share::mount(&tree.root());
        wait_for_log(&[&again]).await;
        let watching = format!(
            "library monitor: watching '{}' of 'Mount H Films' (tmpfs): ",
            tree.root().display()
        );
        wait_for_log(&[&watching]).await;
        watcher.stop();
    }

    /// Fix round 2, item I. A symlinked folder is watched at its target. A share at the target
    /// that goes away is forgotten, and nothing is reported; mounted there again, the library path
    /// is set up again, so what then changes on the share is reported by its path in the library.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_again_at_a_symlinked_folder_is_watched_after() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        // A tmpfs, not a bind mount: inotify drops the watches on a filesystem that is unmounted,
        // but those on a bind mount watch the source, which stays.
        let target = tree.outside("mnt");
        let mut share = Share::mount(&target);
        write(&target.join("Alpha (2001).mkv"), 10);
        symlink(&target, tree.at("Alpha (2001)")).unwrap();
        let point = std::fs::canonicalize(&target).unwrap();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount I Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 5).await;

        share.unmount();
        wait_watched(&watcher, &tree.root(), 4).await;
        let _share = Share::mount(&target);
        wait_watched(&watcher, &tree.root(), 5).await;
        let alpha = folder_unit(&lib, tree.at("Alpha (2001)"));
        assert_eq!(recorder.count(&alpha), 0, "the share's coming and going");
        write(&target.join("Alpha (2001).mkv"), 10);
        recorder.wait_for(&alpha, 1).await;
        let mounted = format!(
            "library monitor: '{}' was mounted; setting up '{}' of 'Mount I Films' again",
            point.display(),
            tree.root().display()
        );
        assert_eq!(log_lines(&[&mounted]).len(), 1, "logged once");
        watcher.stop();
    }

    /// A guard for fix round 2, item I: a library path that is a symlink is matched with the
    /// mount table by its canonical path, so a share mounted at its target sets it up again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_at_the_target_of_a_symlinked_library_path_is_watched_after() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let link = tree.outside("link");
        symlink(tree.root(), &link).unwrap();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount J Films", std::slice::from_ref(&link)).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &link, 1).await;

        let _share = Share::mount(&tree.root());
        let mounted = format!(
            "library monitor: '{}' was mounted; setting up '{}' of 'Mount J Films' again",
            std::fs::canonicalize(tree.root()).unwrap().display(),
            link.display()
        );
        wait_for_log(&[&mounted]).await;
        let watching = format!(
            "library monitor: watching '{}' of 'Mount J Films' (",
            link.display()
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while log_lines(&[&watching]).len() < 2 {
            assert!(
                Instant::now() < deadline,
                "the path is not watched again on the share within 20 s: {:?}",
                watcher.monitored()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        write(&link.join("Film (2001)/Film (2001).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, link.join("Film (2001)")), 1)
            .await;
        watcher.stop();
    }

    /// Fix round 2, item D. A burst of mount changes, as an automounter or a host of containers
    /// makes, is read about once a second, not once for each change.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_burst_of_mount_changes_is_read_about_once_a_second() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let (pool, changes, _recorder) = kernel_fixture(&tree).await;
        add_library(&pool, "Mount K Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 1).await;
        let point = tree.outside("burst");
        std::fs::create_dir_all(&point).unwrap();

        let before = watcher.mount_readings();
        let started = Instant::now();
        let burst = tokio::task::spawn_blocking(move || {
            Command::new("sh")
                .arg("-c")
                .arg(r#"for i in $(seq 200); do mount -t tmpfs tmpfs "$1" && umount "$1" || exit 1; done"#)
                .arg("burst")
                .arg(&point)
                .status()
                .unwrap()
        });
        assert!(burst.await.unwrap().success(), "200 mounts and unmounts");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let readings = watcher.mount_readings() - before;
        let elapsed = started.elapsed();
        assert!(
            readings <= elapsed.as_secs() + 3,
            "{readings} readings of the mount table in {elapsed:?}"
        );
        watcher.stop();
    }

    /// A guard for fix round 2, item D: a few changes in a row are still read one by one, so a
    /// share unmounted and mounted again right after another mount change is seen. Linux gives the
    /// new mount the id and device number that the old one freed: read together, the two changes
    /// would look like none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_share_mounted_again_right_after_another_mount_change_is_seen() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        movie_tree(&tree);
        let target = tree.outside("mnt");
        let mut share = Share::mount(&target);
        symlink(&target, tree.at("Alpha (2001)")).unwrap();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount L Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 5).await;

        let readings = watcher.mount_readings();
        let _other = Share::mount(&tree.outside("other"));
        let deadline = Instant::now() + Duration::from_secs(20);
        while watcher.mount_readings() == readings {
            assert!(
                Instant::now() < deadline,
                "the other mount is not read within 20 s"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        share.unmount();
        wait_watched(&watcher, &tree.root(), 4).await;
        let _share = Share::mount(&target);
        wait_watched(&watcher, &tree.root(), 5).await;
        write(&target.join("Alpha (2001).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Alpha (2001)")), 1)
            .await;
        watcher.stop();
    }

    /// Fix round 2, item E: a failed reading of the mount table doesn't end the follower. It tries
    /// again a little later and compares the table with its last good reading, so a share mounted
    /// on a watched path while the reading failed is still seen.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "mounts tmpfs; mount_changes_in_a_namespace runs it under `unshare -rm`"]
    async fn mount_ns_a_failed_reading_of_the_mount_table_is_retried() {
        if !in_mount_namespace() {
            return;
        }
        let _logs = captured_logs();
        let tree = Tree::new();
        let (pool, changes, recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Mount M Films", &[tree.root()]).await;
        let watcher = kernel_watcher(&pool, &changes, tree.outside("data"));
        wait_watched(&watcher, &tree.root(), 1).await;

        watcher.fail_mount_reads(1);
        let _share = Share::mount(&tree.root());
        let watching = format!(
            "library monitor: watching '{}' of 'Mount M Films' (",
            tree.root().display()
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while log_lines(&[&watching]).len() < 2 {
            assert!(
                Instant::now() < deadline,
                "the path is not watched again on the share within 20 s: {:?}",
                watcher.monitored()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        write(&tree.at("Film (2001)/Film (2001).mkv"), 10);
        recorder
            .wait_for(&folder_unit(&lib, tree.at("Film (2001)")), 1)
            .await;
        let failed = log_lines(&[
            "library monitor: cannot read the mount table (injected for a test); trying again in 1 s",
        ]);
        assert_eq!(failed.len(), 1, "warned once");
        let again = log_lines(&["library monitor: following the mount table again"]);
        assert_eq!(again.len(), 1, "logged once");
        watcher.stop();
    }

    /// Fix round 2, item G: a setup walk whose monitor stops, here because monitoring is turned
    /// off, stops too, instead of adding a watch for each folder of a path no longer monitored.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_setup_walk_stops_when_its_monitor_stops() {
        let _logs = captured_logs();
        let tree = Tree::new();
        for year in 2001..=2020 {
            write(&tree.at(&format!("Film ({year})/Film ({year}).mkv")), 10);
        }
        let (pool, changes, _recorder) = kernel_fixture(&tree).await;
        let lib = add_library(&pool, "Kernel S Films", &[tree.root()]).await;
        let watcher = LibraryWatcher::new(pool.clone(), changes.clone(), tree.outside("data"));
        let gate = PollGate::default();
        let hold = gate.hook();
        let visits = Arc::new(AtomicUsize::new(0));
        let counted = visits.clone();
        watcher.set_walk_hook(Some(Arc::new(move |dir: &Path| {
            counted.fetch_add(1, Ordering::SeqCst);
            hold(dir);
        })));
        gate.closed.store(true, Ordering::SeqCst);
        watcher.start();
        gate.close().await;
        let held_at = visits.load(Ordering::SeqCst);

        set_monitoring(&pool, &watcher, &lib, false).await;
        let stopped = format!(
            "library monitor: stopped '{}' of 'Kernel S Films'",
            tree.root().display()
        );
        wait_for_log(&[&stopped]).await;
        // The abort drops the monitor's task a moment after the line is logged.
        tokio::time::sleep(Duration::from_millis(200)).await;
        gate.open();
        quiet_period().await;
        let visited = visits.load(Ordering::SeqCst);
        assert!(
            visited <= held_at + 1,
            "the walk went on after its monitor stopped: {visited} folders visited, {held_at} before"
        );
        watcher.stop();
    }
}

/// Measures polling a real library, which it only reads:
/// `RUSTYFIN_MEASURE_PATH=/mnt/media cargo test -p rustyfin-core --test library_watch_tests
/// measure -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement; needs RUSTYFIN_MEASURE_PATH"]
fn measure_polling_a_library() {
    let Some(path) = std::env::var_os("RUSTYFIN_MEASURE_PATH") else {
        return;
    };
    let rss_before = rss_kib();
    let mut poller = FolderPoller::new(PathBuf::from(path), None);
    let started = Instant::now();
    assert!(poller.poll().is_empty());
    let baseline = started.elapsed();
    let mut polls = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let reported = poller.poll().len();
        polls.push((started.elapsed(), reported));
    }
    let rss_growth = rss_kib()
        .zip(rss_before)
        .map(|(after, before)| after - before);
    println!(
        "measure: {} folders, baseline in {baseline:?}, polls (time, reported): {polls:?}, \
         resident memory grew by {rss_growth:?} KiB",
        poller.folders()
    );
}

/// Resident memory of this process in KiB, from `/proc/self/status`.
fn rss_kib() -> Option<i64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}
