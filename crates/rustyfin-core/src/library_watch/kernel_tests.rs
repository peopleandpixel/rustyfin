// Fix round 1, item 5: the open file limit of the process is told apart from the kernel's limit on
// inotify instances. Both make `Inotify::init` fail with EMFILE.

use std::fs::File;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;

use super::{EMFILE, fallback_reason, setup};
use crate::library_monitor::ChangeQueue;
use crate::library_watch::LibraryWatcher;

/// Set for the child process that may open only 64 files.
const LOW_FILE_LIMIT: &str = "RUSTYFIN_TEST_LOW_FILE_LIMIT";

#[test]
fn the_open_file_limit_is_told_apart_from_the_instance_limit() {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new("sh")
        .args(["-c", r#"ulimit -n 64 && exec "$0" "$@""#])
        .arg(&exe)
        .args([
            "--exact",
            "library_watch::kernel::tests::with_64_open_files",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(LOW_FILE_LIMIT, "1")
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && text.contains("1 passed"),
        "the test with 64 open files failed:\n{text}"
    );
}

/// Run by `the_open_file_limit_is_told_apart_from_the_instance_limit` in a process that may open
/// 64 files.
#[tokio::test]
#[ignore = "run under `ulimit -n 64` by the_open_file_limit_is_told_apart_from_the_instance_limit"]
async fn with_64_open_files() {
    if std::env::var_os(LOW_FILE_LIMIT).is_none() {
        eprintln!("skipped: the_open_file_limit_is_told_apart_from_the_instance_limit runs it");
        return;
    }
    let root = Folder::new();
    let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
    let watcher = LibraryWatcher::new(pool.clone(), ChangeQueue::new(pool), root.0.join("data"));

    // Every file this process may open is open while the kernel watching is set up.
    let mut open = Vec::new();
    let full = loop {
        match File::open("/dev/null") {
            Ok(file) => open.push(file),
            Err(err) => break err,
        }
    };
    let fallback = setup(
        &root.0,
        &root.0.join("data"),
        None,
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .err();
    drop(open);
    assert_eq!(full.raw_os_error(), Some(EMFILE), "{full}");
    let fallback = fallback.expect("no inotify instance without a free file");

    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || LogWriter(sink.clone()))
        .finish();
    let reason = tracing::subscriber::with_default(subscriber, || {
        fallback_reason(&watcher.inner, &root.0, fallback)
    });
    let logs = String::from_utf8_lossy(&logs.lock().unwrap()).to_string();
    assert_eq!(reason, "open file limit reached");
    assert!(
        logs.contains("the open file limit of the process is reached")
            && logs.contains("ulimit -n")
            && !logs.contains("max_user_instances"),
        "the warning names the open file limit: {logs}"
    );
    // A raised ulimit, LimitNOFILE= or --ulimit reaches only a new process, and a SIGHUP tries
    // again only when a limit changed, so the advice names the restart and the live way.
    assert!(
        logs.contains("restart Rustyfin") && logs.contains("prlimit --pid"),
        "the warning says to restart Rustyfin, or to raise the limit live with prlimit --pid: {logs}"
    );
}

/// An empty temporary folder, removed when dropped.
struct Folder(PathBuf);

impl Folder {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("rustyfin_test_kernel_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
