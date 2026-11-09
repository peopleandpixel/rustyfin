//! The library monitor's change queue (B18): which unit of which library a reported path rescans,
//! the settle delay, the limit that turns many pending units into one full scan, and a worker that
//! goes on after failed and panicking rescans.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustyfin_core::library_monitor::{ChangeQueue, FULL_SCAN_LIMIT, RescanRunner, unit_for};
use rustyfin_core::scanner::{self, ScanChanges, ScanScope};
use rustyfin_core::scheduler::TaskScheduler;
use rustyfin_model::library::{Library, LibraryContentType};
use sqlx::SqlitePool;
use tokio::sync::Semaphore;
use uuid::Uuid;

/// A temp dir holding a database and the test media; removed on drop.
struct Fixture {
    pool: SqlitePool,
    dir: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_monitor_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
        Self { pool, dir }
    }

    /// Creates the library `name` whose paths are `<temp dir>/<root>` for each of `roots`.
    async fn library(
        &self,
        name: &str,
        content_type: LibraryContentType,
        roots: &[&str],
    ) -> Library {
        let paths = roots
            .iter()
            .map(|root| {
                let path = self.dir.join(root);
                std::fs::create_dir_all(&path).unwrap();
                path.to_str().unwrap().to_string()
            })
            .collect();
        let lib = Library {
            id: Uuid::new_v4(),
            name: name.to_string(),
            content_type,
            paths,
        };
        rustyfin_db::repo_library::create_library(&self.pool, &lib)
            .await
            .unwrap();
        lib
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The library's first path.
fn root(lib: &Library) -> PathBuf {
    PathBuf::from(&lib.paths[0])
}

/// Creates an empty file at `<root>/<rel>`, with its parent folders.
fn touch(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"").unwrap();
    path
}

fn sorted(mut units: Vec<(Uuid, ScanScope)>) -> Vec<(Uuid, ScanScope)> {
    units.sort();
    units
}

/// One rescan that the worker started.
#[derive(Clone, Debug)]
struct Call {
    library: Uuid,
    scope: ScanScope,
    at: Instant,
}

/// A runner that records each rescan and changes nothing, unless told to wait, fail or panic.
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Call>>,
    /// When set, every rescan takes one permit before it ends.
    gate: Option<Arc<Semaphore>>,
    /// Rescans of these scopes return an error.
    fail: Vec<ScanScope>,
    /// Rescans of these scopes panic in their future.
    panic: Vec<ScanScope>,
    /// Rescans of these scopes panic before they return a future.
    panic_now: Vec<ScanScope>,
}

impl RescanRunner for Recorder {
    fn rescan(
        &self,
        library: Library,
        scope: ScanScope,
    ) -> Pin<Box<dyn Future<Output = Result<ScanChanges, String>> + Send>> {
        self.calls.lock().unwrap().push(Call {
            library: library.id,
            scope: scope.clone(),
            at: Instant::now(),
        });
        if self.panic_now.contains(&scope) {
            panic!("the test runner panics at once on {scope:?}");
        }
        let gate = self.gate.clone();
        let fail = self.fail.contains(&scope);
        let panic = self.panic.contains(&scope);
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            if panic {
                panic!("the test runner panics on {scope:?}");
            }
            if fail {
                return Err(format!("the test runner fails on {scope:?}"));
            }
            Ok(ScanChanges::default())
        })
    }
}

impl Recorder {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// The library and scope of every rescan so far, sorted.
    fn units(&self) -> Vec<(Uuid, ScanScope)> {
        sorted(
            self.calls()
                .into_iter()
                .map(|call| (call.library, call.scope))
                .collect(),
        )
    }

    /// Waits up to 20 s until `n` rescans have started, and returns them.
    async fn wait_for(&self, n: usize) -> Vec<Call> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let calls = self.calls();
            if calls.len() >= n {
                return calls;
            }
            assert!(
                Instant::now() < deadline,
                "{} of {n} rescans started within 20 s: {calls:?}",
                calls.len()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// A queue on `fx`'s database with `runner` and the settle delay `delay`.
fn queue(fx: &Fixture, runner: &Arc<Recorder>, delay: Duration) -> ChangeQueue {
    let queue = ChangeQueue::new(fx.pool.clone());
    queue.set_runner(runner.clone());
    queue.set_delay(delay);
    queue
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

/// Waits up to 10 s for a log line that contains every one of `needles`.
async fn wait_for_log(needles: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(line) = log_lines(needles).into_iter().next() {
            return line;
        }
        assert!(
            Instant::now() < deadline,
            "no log line with {needles:?} within 10 s"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ── Units ──

#[test]
fn unit_for_maps_a_path_to_the_top_level_entry_under_its_library_path() {
    let dir = std::env::temp_dir().join(format!("rustyfin_test_monitor_{}", Uuid::new_v4()));
    let films = dir.join("films");
    touch(&films, "Heat (1995)/Heat (1995).mkv");
    touch(&films, "Loose (2001).mkv");
    std::fs::create_dir_all(films.join("Odd.Folder.2001")).unwrap();
    let roots = vec![films.to_str().unwrap().to_string()];
    let unit = |rel: &str| unit_for(&roots, &films.join(rel));
    let folder = |rel: &str| Some(ScanScope::Folder(films.join(rel)));
    let root_files = Some(ScanScope::RootFiles(films.clone()));

    assert_eq!(unit_for(&roots, &films), Some(ScanScope::Library));
    let slash = format!("{}/", films.to_str().unwrap());
    assert_eq!(
        unit_for(&roots, Path::new(&slash)),
        Some(ScanScope::Library),
        "the library path with a trailing /"
    );
    assert_eq!(unit("Heat (1995)"), folder("Heat (1995)"), "a folder");
    assert_eq!(
        unit("Heat (1995)/"),
        folder("Heat (1995)"),
        "a folder with a trailing /"
    );
    assert_eq!(
        unit("Heat (1995)/Heat (1995).mkv"),
        folder("Heat (1995)"),
        "a file in a folder"
    );
    assert_eq!(
        unit("Gone (1990)/Gone (1990).mkv"),
        folder("Gone (1990)"),
        "a gone file in a gone folder"
    );
    assert_eq!(
        unit("Show/Season 1/Show S01E01.mkv"),
        folder("Show"),
        "a file two folders down"
    );
    assert_eq!(
        unit("Odd.Folder.2001"),
        folder("Odd.Folder.2001"),
        "a folder whose name has dots"
    );
    assert_eq!(
        unit("Loose (2001).mkv"),
        root_files,
        "a file in the library path"
    );
    assert_eq!(
        unit("Gone (1990).MKV"),
        root_files,
        "a gone video file in the library path"
    );
    assert_eq!(
        unit("Gone Song.flac"),
        root_files,
        "a gone audio file in the library path"
    );
    assert_eq!(
        unit("Gone Folder"),
        folder("Gone Folder"),
        "a gone entry without an extension"
    );
    assert_eq!(
        unit("Gone.Folder.2001"),
        folder("Gone.Folder.2001"),
        "a gone entry whose extension is no media extension"
    );

    for rel in [
        "Heat (1995)/.actors/Al Pacino.jpg",
        "@eaDir/Heat (1995).mkv",
        "Heat (1995)/Sample/Heat (1995)-sample.mkv",
        "Heat (1995)/extrafanart",
        ".hidden.mkv",
        "#recycle/Old (1980)/Old (1980).mkv",
        "Heat (1995).trickplay/320 - 10x10/0.jpg",
        "../films2/Heat (1995)/Heat (1995).mkv",
    ] {
        assert_eq!(unit(rel), None, "{rel}");
    }
    assert_eq!(
        unit_for(&roots, &dir.join("films2/Heat (1995)/Heat (1995).mkv")),
        None,
        "a sibling folder whose name starts with the library path's"
    );
    assert_eq!(
        unit_for(&roots, &dir),
        None,
        "the folder above the library path"
    );
    assert_eq!(
        unit_for(&roots, Path::new("films/Heat (1995)/Heat (1995).mkv")),
        None,
        "a relative path"
    );
    assert_eq!(unit_for(&[], &films.join("Heat (1995)")), None, "no paths");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unit_for_takes_the_longest_library_path_that_holds_the_path() {
    let roots = vec![
        "/srv/media".to_string(),
        "/srv/media/Kids".to_string(),
        "/srv/other/".to_string(),
    ];
    let folder = |path: &str| Some(ScanScope::Folder(PathBuf::from(path)));
    assert_eq!(
        unit_for(&roots, Path::new("/srv/media/Kids/Up (2009)/Up (2009).mkv")),
        folder("/srv/media/Kids/Up (2009)")
    );
    assert_eq!(
        unit_for(&roots, Path::new("/srv/media/Kids")),
        Some(ScanScope::Library)
    );
    assert_eq!(
        unit_for(&roots, Path::new("/srv/media/Heat (1995)/Heat (1995).mkv")),
        folder("/srv/media/Heat (1995)")
    );
    assert_eq!(
        unit_for(
            &roots,
            Path::new("/srv/other/Show/Season 1/Show S01E01.mkv")
        ),
        folder("/srv/other/Show"),
        "a library path with a trailing /"
    );
}

// ── Reports ──

#[tokio::test]
async fn report_queues_the_unit_for_every_library_that_holds_the_path() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let both = fx
        .library(
            "films and more",
            LibraryContentType::Movies,
            &["more", "films"],
        )
        .await;
    let music = fx
        .library("music", LibraryContentType::Music, &["music"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::from_secs(3600));

    let heat = fx.dir.join("films/Heat (1995)");
    assert!(queue.report(&heat.join("Heat (1995).mkv")).await);
    assert!(
        queue
            .report(&fx.dir.join("music/Artist/Album/01 Song.flac"))
            .await
    );
    assert!(
        queue.report(&heat.join("Heat (1995).nfo")).await,
        "a second report of a pending unit"
    );
    assert_eq!(
        queue.pending(),
        sorted(vec![
            (films.id, ScanScope::Folder(heat.clone())),
            (both.id, ScanScope::Folder(heat.clone())),
            (music.id, ScanScope::Folder(fx.dir.join("music/Artist"))),
        ])
    );
    assert!(rec.calls().is_empty());
}

#[tokio::test]
async fn report_drops_paths_in_no_library_and_in_ignored_folders() {
    captured_logs();
    let fx = Fixture::new().await;
    fx.library("films", LibraryContentType::Movies, &["films"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::ZERO);

    let outside = fx.dir.join("elsewhere/Heat (1995)/Heat (1995).mkv");
    assert!(!queue.report(&outside).await, "a path in no library");
    assert!(
        !queue
            .report(&fx.dir.join("films/Heat (1995)/@eaDir/Heat (1995).mkv.jpg"))
            .await,
        "a path in an ignored folder"
    );
    assert!(
        !queue.report(&fx.dir.join("films2/Heat (1995)")).await,
        "a sibling of the library path"
    );
    assert_eq!(queue.pending(), vec![]);
    assert!(
        !log_lines(&["DEBUG", outside.to_str().unwrap()]).is_empty(),
        "the dropped path is logged at debug"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rec.calls().is_empty());
}

#[tokio::test]
async fn report_all_answers_for_each_path_and_queues_their_units() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let shows = fx
        .library("shows", LibraryContentType::TvShows, &["shows"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::from_secs(3600));

    let heat = root(&films).join("Heat (1995)");
    let show = root(&shows).join("Show");
    let answers = queue
        .report_all(&[
            heat.join("Heat (1995).mkv"),
            fx.dir.join("elsewhere/Other (2001)/Other (2001).mkv"),
            show.join("Season 1/Show S01E01.mkv"),
            heat.join("@eaDir/Heat (1995).mkv.jpg"),
            heat.join("Heat (1995).nfo"),
        ])
        .await;
    assert_eq!(answers, vec![true, false, true, false, true]);
    assert_eq!(
        queue.pending(),
        sorted(vec![
            (films.id, ScanScope::Folder(heat)),
            (shows.id, ScanScope::Folder(show)),
        ])
    );
    assert_eq!(queue.report_all(&[]).await, Vec::<bool>::new());
    assert!(rec.calls().is_empty());
}

/// B18 fix round 2, item 5: a report reads nothing on disk, so a hung mount cannot hold up
/// `Media/Updated` or the Webhook. While the runtime's only blocking thread is busy, as a read on a
/// hung NFS mount keeps it, `report_all` answers and queues the units under the reported names.
#[test]
fn report_all_returns_while_the_blocking_pool_is_busy() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let fx = Fixture::new().await;
        let lib = fx
            .library("films", LibraryContentType::Movies, &["films"])
            .await;
        let films = root(&lib);
        touch(&films, "Heat (1995)/Heat (1995).mkv");
        let rec = Arc::new(Recorder::default());
        let queue = queue(&fx, &rec, Duration::from_secs(3600));

        let (busy, started) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let blocker = tokio::task::spawn_blocking(move || {
            busy.send(()).unwrap();
            let _ = held.recv();
        });
        started.await.unwrap();
        let answers = tokio::time::timeout(
            Duration::from_secs(5),
            queue.report_all(&[
                films.join("Heat (1995)/Heat (1995).mkv"),
                films.join("Loose (2001).mkv"),
            ]),
        )
        .await;
        release.send(()).unwrap();
        blocker.await.unwrap();

        assert_eq!(
            answers.expect("report_all waited for the blocking pool"),
            [true, true]
        );
        assert_eq!(
            queue.pending(),
            sorted(vec![
                (lib.id, ScanScope::Folder(films.join("Heat (1995)"))),
                (lib.id, ScanScope::RootFiles(films.clone())),
            ])
        );
    });
}

/// A file directly in the library path that is no video or audio file, such as a subtitle, is
/// rescanned with the root files.
#[tokio::test]
async fn a_reported_file_of_another_type_in_the_library_path_rescans_the_root_files() {
    let fx = Fixture::new().await;
    let lib = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let films = root(&lib);
    let subtitle = touch(&films, "Loose (2001).en.srt");
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::ZERO);

    assert!(queue.report(&subtitle).await);
    let calls = rec.wait_for(1).await;
    assert_eq!(
        (calls[0].library, calls[0].scope.clone()),
        (lib.id, ScanScope::RootFiles(films))
    );
}

#[tokio::test]
async fn a_report_of_the_library_path_turns_its_pending_units_into_one_full_scan() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let shows = fx
        .library("shows", LibraryContentType::TvShows, &["shows"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::from_secs(3600));

    assert!(queue.report(&root(&films).join("Heat (1995)")).await);
    assert!(queue.report(&root(&films).join("Loose (2001).mkv")).await);
    assert!(
        queue
            .report(&root(&shows).join("Show/Season 1/Show S01E01.mkv"))
            .await
    );
    assert!(queue.report(&root(&films)).await);
    let expected = sorted(vec![
        (films.id, ScanScope::Library),
        (shows.id, ScanScope::Folder(root(&shows).join("Show"))),
    ]);
    assert_eq!(queue.pending(), expected);
    assert!(
        queue.report(&root(&films).join("Alien (1979)")).await,
        "a report that the pending full scan takes in"
    );
    assert_eq!(queue.pending(), expected);
}

// ── Settle delay ──

#[tokio::test]
async fn repeated_reports_within_the_delay_make_one_rescan_after_the_last() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let rec = Arc::new(Recorder::default());
    let delay = Duration::from_secs(3);
    let queue = queue(&fx, &rec, delay);

    let heat = root(&films).join("Heat (1995)");
    let mut last = Instant::now();
    for name in [
        "Heat (1995).mkv",
        "Heat (1995).nfo",
        "poster.jpg",
        "Heat (1995).mkv",
    ] {
        last = Instant::now();
        assert!(queue.report(&heat.join(name)).await);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let calls = rec.wait_for(1).await;
    assert_eq!(
        (calls[0].library, calls[0].scope.clone()),
        (films.id, ScanScope::Folder(heat))
    );
    assert!(
        calls[0].at >= last + delay,
        "the rescan started {:?} after the last report, before the delay of {delay:?} ended",
        calls[0].at.saturating_duration_since(last)
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rec.calls().len(), 1, "one rescan for four reports");
    assert_eq!(queue.pending(), vec![]);
}

#[tokio::test]
async fn reports_that_a_pending_full_scan_takes_in_do_not_postpone_it() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let rec = Arc::new(Recorder::default());
    let delay = Duration::from_secs(1);
    let queue = queue(&fx, &rec, delay);

    let pending_since = Instant::now();
    assert!(queue.report(&root(&films)).await);
    // Changes every 150 ms for 2.5 s, longer than the delay, with the library path once more.
    for n in 0..17 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let path = if n == 8 {
            root(&films)
        } else {
            root(&films).join(format!("Film {n} (2000)/Film {n} (2000).mkv"))
        };
        assert!(queue.report(&path).await);
    }
    let calls = rec.wait_for(1).await;
    assert_eq!(
        (calls[0].library, calls[0].scope.clone()),
        (films.id, ScanScope::Library)
    );
    assert!(
        calls[0].at < pending_since + 2 * delay,
        "the full scan started {:?} after it became pending, with a delay of {delay:?}",
        calls[0].at.saturating_duration_since(pending_since)
    );
}

#[tokio::test]
async fn a_report_during_a_rescan_queues_the_unit_again() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let gate = Arc::new(Semaphore::new(0));
    let rec = Arc::new(Recorder {
        gate: Some(gate.clone()),
        ..Default::default()
    });
    let queue = queue(&fx, &rec, Duration::ZERO);

    let heat = root(&films).join("Heat (1995)");
    assert!(queue.report(&heat.join("Heat (1995).mkv")).await);
    rec.wait_for(1).await;
    assert_eq!(queue.pending(), vec![], "a running rescan is not pending");
    assert!(queue.report(&heat.join("Heat (1995).mkv")).await);
    assert_eq!(
        queue.pending(),
        vec![(films.id, ScanScope::Folder(heat.clone()))]
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rec.calls().len(), 1, "one rescan at a time");

    gate.add_permits(1);
    let calls = rec.wait_for(2).await;
    gate.add_permits(1);
    assert_eq!(calls[1].scope, ScanScope::Folder(heat));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rec.calls().len(), 2);
    assert_eq!(queue.pending(), vec![]);
}

// ── Limit ──

#[tokio::test]
async fn more_than_fifty_pending_units_of_a_library_become_one_full_scan() {
    assert_eq!(FULL_SCAN_LIMIT, 50);
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    let shows = fx
        .library("shows", LibraryContentType::TvShows, &["shows"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::from_secs(3600));

    let films_root = root(&films);
    for i in 0..FULL_SCAN_LIMIT - 1 {
        let film = format!("Film {i:02} (2000)/Film {i:02} (2000).mkv");
        assert!(queue.report(&films_root.join(film)).await);
    }
    assert!(
        queue.report(&films_root.join("Loose (2001).mkv")).await,
        "the files in the library path count as a unit"
    );
    let show = ScanScope::Folder(root(&shows).join("Show"));
    assert!(
        queue
            .report(&root(&shows).join("Show/Season 1/Show S01E01.mkv"))
            .await
    );
    assert_eq!(
        queue.pending().len(),
        FULL_SCAN_LIMIT + 1,
        "50 units of a library stay units"
    );

    assert!(queue.report(&films_root.join("Film 50 (2000)")).await);
    let expected = sorted(vec![(films.id, ScanScope::Library), (shows.id, show)]);
    assert_eq!(queue.pending(), expected, "the 51st unit");
    assert!(queue.report(&films_root.join("Film 51 (2000)")).await);
    assert_eq!(
        queue.pending(),
        expected,
        "the full scan takes in later reports"
    );

    // A new delay applies to the pending units too.
    queue.set_delay(Duration::ZERO);
    rec.wait_for(2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rec.units(), expected);
    assert_eq!(queue.pending(), vec![]);
}

// ── Worker ──

#[tokio::test]
async fn failed_and_panicking_rescans_are_logged_and_the_queue_goes_on() {
    captured_logs();
    let fx = Fixture::new().await;
    let lib = fx
        .library("b18 failing films", LibraryContentType::Movies, &["films"])
        .await;
    let films = root(&lib);
    let unit = |name: &str| ScanScope::Folder(films.join(name));
    let rec = Arc::new(Recorder {
        fail: vec![unit("Fails (2001)")],
        panic: vec![unit("Panics (2002)")],
        panic_now: vec![unit("Panics At Once (2003)")],
        ..Default::default()
    });
    let queue = queue(&fx, &rec, Duration::ZERO);

    let names = [
        "Fails (2001)",
        "Panics (2002)",
        "Panics At Once (2003)",
        "Works (2004)",
    ];
    for name in names {
        assert!(queue.report(&films.join(name)).await);
    }
    rec.wait_for(names.len()).await;
    assert!(
        queue.report(&films.join("Later (2005)")).await,
        "a report after the failures"
    );
    rec.wait_for(names.len() + 1).await;
    let expected = names
        .iter()
        .chain(&["Later (2005)"])
        .map(|name| (lib.id, unit(name)))
        .collect();
    assert_eq!(rec.units(), sorted(expected));

    let of = format!("of '{}'", lib.name);
    for name in names.iter().chain(&["Later (2005)"]) {
        wait_for_log(&[&format!(
            "library monitor: rescanning {:?} {of}",
            unit(name)
        )])
        .await;
    }
    let fails = unit("Fails (2001)");
    wait_for_log(&[
        " WARN ",
        &format!(
            "library monitor: rescan of {fails:?} {of} failed: the test runner fails on {fails:?}"
        ),
    ])
    .await;
    for name in ["Panics (2002)", "Panics At Once (2003)"] {
        let panicked = format!("library monitor: rescan of {:?} {of} panicked", unit(name));
        wait_for_log(&[" WARN ", &panicked]).await;
    }
    for name in ["Works (2004)", "Later (2005)"] {
        let done = format!("library monitor: rescanned {:?} {of} in ", unit(name));
        wait_for_log(&[" INFO ", &done, " ms: 0 added, 0 removed, 0 re-read"]).await;
    }
}

#[tokio::test]
async fn a_library_deleted_before_its_rescan_is_skipped() {
    let fx = Fixture::new().await;
    let gone = fx
        .library("gone", LibraryContentType::Movies, &["gone"])
        .await;
    let kept = fx
        .library("kept", LibraryContentType::Movies, &["kept"])
        .await;
    let rec = Arc::new(Recorder::default());
    let queue = queue(&fx, &rec, Duration::from_millis(500));

    assert!(queue.report(&root(&gone).join("Heat (1995)")).await);
    rustyfin_db::repo_library::delete_library(&fx.pool, gone.id)
        .await
        .unwrap();
    assert!(queue.report(&root(&kept).join("Ronin (1998)")).await);
    rec.wait_for(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rec.units(),
        vec![(kept.id, ScanScope::Folder(root(&kept).join("Ronin (1998)")))]
    );
    assert_eq!(queue.pending(), vec![]);
}

#[tokio::test]
async fn the_scheduler_owns_one_queue_with_a_delay_of_60_seconds() {
    let fx = Fixture::new().await;
    let films = fx
        .library("films", LibraryContentType::Movies, &["films"])
        .await;
    assert_eq!(
        ChangeQueue::new(fx.pool.clone()).delay(),
        Duration::from_secs(60)
    );
    let scheduler = TaskScheduler::new(fx.pool.clone(), Arc::new(fx.dir.clone()));
    assert_eq!(scheduler.changes().delay(), Duration::from_secs(60));
    let rec = Arc::new(Recorder::default());
    scheduler.changes().set_runner(rec.clone());

    let clone = scheduler.clone();
    let heat = root(&films).join("Heat (1995)");
    assert!(clone.changes().report(&heat).await);
    assert_eq!(
        scheduler.changes().pending(),
        vec![(films.id, ScanScope::Folder(heat))]
    );
    clone.changes().set_delay(Duration::from_secs(5));
    assert_eq!(scheduler.changes().delay(), Duration::from_secs(5));
}

#[tokio::test]
async fn the_default_runner_rescans_the_reported_folder() {
    captured_logs();
    let fx = Fixture::new().await;
    let lib = fx
        .library(
            "b18 monitored films",
            LibraryContentType::Movies,
            &["films"],
        )
        .await;
    let films = root(&lib);
    touch(&films, "Heat (1995)/Heat (1995).mkv");
    scanner::scan_library(&fx.pool, &lib).await.unwrap();
    scanner::wait_for_idle().await;
    touch(&films, "Ronin (1998)/Ronin (1998).mkv");
    touch(&films, "Alien (1979)/Alien (1979).mkv");

    let queue = ChangeQueue::new(fx.pool.clone());
    queue.set_delay(Duration::ZERO);
    assert!(
        queue
            .report(&films.join("Ronin (1998)/Ronin (1998).mkv"))
            .await
    );
    let ronin = ScanScope::Folder(films.join("Ronin (1998)"));
    let done = format!("library monitor: rescanned {ronin:?} of '{}' in ", lib.name);
    wait_for_log(&[&done, " ms: 1 added, 0 removed, 0 re-read"]).await;
    scanner::wait_for_idle().await;

    let mut paths: Vec<String> = sqlx::query_scalar(
        "SELECT path FROM media_items WHERE library_id = ?1 AND media_type = 'Movie'",
    )
    .bind(lib.id.to_string())
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    paths.sort();
    let expected: Vec<String> = [
        "Heat (1995)/Heat (1995).mkv",
        "Ronin (1998)/Ronin (1998).mkv",
    ]
    .iter()
    .map(|rel| films.join(rel).to_str().unwrap().to_string())
    .collect();
    assert_eq!(
        paths, expected,
        "the folder that was not reported stays out"
    );
}

/// Whether the names in `dir` ignore case, as on a casefold or an SMB mount.
fn ignores_case(dir: &Path) -> bool {
    std::fs::write(dir.join("CaseProbe"), b"").unwrap();
    let ignores = dir.join("caseprobe").exists();
    std::fs::remove_file(dir.join("CaseProbe")).unwrap();
    ignores
}

/// The id and path of each movie of `lib`, by path.
async fn stored_movies(fx: &Fixture, lib: &Library) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT id, path FROM media_items WHERE library_id = ?1 AND media_type = 'Movie'
         ORDER BY path",
    )
    .bind(lib.id.to_string())
    .fetch_all(&fx.pool)
    .await
    .unwrap()
}

// Runs only where the temp dir ignores case, e.g. with TMPDIR on a tmpfs mounted with `casefold`
// whose folder has `chattr +F`.
#[tokio::test]
async fn a_path_reported_in_another_case_rescans_the_folder_by_its_name_on_disk() {
    captured_logs();
    let fx = Fixture::new().await;
    if !ignores_case(&fx.dir) {
        eprintln!(
            "skipped: {} tells case apart; set TMPDIR to a folder that ignores case",
            fx.dir.display()
        );
        return;
    }
    let lib = fx
        .library("b18 casefold films", LibraryContentType::Movies, &["films"])
        .await;
    let films = root(&lib);
    touch(&films, "The Matrix (1999)/The Matrix (1999).mkv");
    scanner::scan_library(&fx.pool, &lib).await.unwrap();
    scanner::wait_for_idle().await;
    let before = stored_movies(&fx, &lib).await;
    assert_eq!(before.len(), 1, "{before:?}");
    let on_disk = ScanScope::Folder(films.join("The Matrix (1999)"));
    let reported = films.join("the matrix (1999)/The Matrix (1999).mkv");

    let monitor = ChangeQueue::new(fx.pool.clone());
    monitor.set_delay(Duration::ZERO);
    assert!(monitor.report(&reported).await);
    let done = wait_for_log(&[
        "library monitor: rescanned ",
        &format!(" of '{}' in ", lib.name),
    ])
    .await;
    scanner::wait_for_idle().await;
    assert!(
        done.contains(&format!("rescanned {on_disk:?} of ")),
        "{done}"
    );
    assert_eq!(
        stored_movies(&fx, &lib).await,
        before,
        "no second item for the folder's other case"
    );
    scanner::scan_library(&fx.pool, &lib).await.unwrap();
    scanner::wait_for_idle().await;
    assert_eq!(
        stored_movies(&fx, &lib).await,
        before,
        "a full scan keeps the item"
    );

    // Each rescan names the folder as it is on disk, whatever case the report used.
    let rec = Arc::new(Recorder::default());
    let recorded = queue(&fx, &rec, Duration::ZERO);
    assert!(recorded.report(&reported).await);
    assert!(recorded.report(&films.join("THE MATRIX (1999)")).await);
    let calls = rec.wait_for(2).await;
    for call in calls {
        assert_eq!((call.library, call.scope), (lib.id, on_disk.clone()));
    }
}
