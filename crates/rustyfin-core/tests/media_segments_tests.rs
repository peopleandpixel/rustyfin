//! Intro and outro detection (R1-28): ffmpeg gets one argument per element, so paths with
//! spaces and quotes work, and running detection again does not add segment rows.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustyfin_core::media_segments::{self, DetectionConfig};
use rustyfin_core::trickplay::Cancel;
use rustyfin_model::library::{Library, LibraryContentType};
use sqlx::SqlitePool;
use uuid::Uuid;

/// A directory and file name with spaces and both quote kinds.
const QUOTED: &str = "Bob's \"Quoted\" Show";

/// A temporary directory that is removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_{label}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Runs ffmpeg on lavfi `inputs` and writes `path` with MPEG-4 video and AAC audio.
fn write_clip(inputs: &[&str], path: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(inputs)
        .args(["-c:v", "mpeg4", "-c:a", "aac"])
        .arg(path)
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not write {}",
        path.display()
    );
}

/// Writes a 60 s clip: black 0-6 s, a test pattern 6-50 s, black 50-60 s;
/// a tone 0-50 s and silence 50-60 s.
fn make_clip(path: &Path) {
    write_clip(
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=160x120:r=10:d=6",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x120:r=10:d=44",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=160x120:r=10:d=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:d=50",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0:s=48000:d=10",
            "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0[v];[3:a][4:a]concat=n=2:v=0:a=1[a]",
            "-map",
            "[v]",
            "-map",
            "[a]",
        ],
        path,
    );
}

/// Writes a 60 s clip with a test pattern and a tone throughout: no intro, no outro.
fn make_plain_clip(path: &Path) {
    write_clip(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=160x120:r=10:d=60",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:d=60",
            "-map",
            "0:v",
            "-map",
            "1:a",
        ],
        path,
    );
}

/// Makes `<dir>/Bob's "Quoted" Show/Season 1/Bob's "Quoted" Show - S01E01 - Pilot.mkv`.
fn make_quoted_clip(dir: &Path) -> String {
    let season = dir.join(QUOTED).join("Season 1");
    std::fs::create_dir_all(&season).unwrap();
    let clip = season.join(format!("{QUOTED} - S01E01 - Pilot.mkv"));
    make_clip(&clip);
    clip.to_str().unwrap().to_string()
}

/// Opens a new database in `dir` holding one TV library and one Episode item for `clip`.
async fn episode_db(dir: &Path, clip: &str) -> (SqlitePool, Uuid) {
    let db = dir.join("test.db");
    let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
    let lib = Library {
        id: Uuid::new_v4(),
        name: "Small".to_string(),
        content_type: LibraryContentType::TvShows,
        paths: vec![dir.to_str().unwrap().to_string()],
    };
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
         VALUES (?1, 'Pilot', 'Episode', ?2, ?3, 600000000)",
    )
    .bind(item_id.to_string())
    .bind(clip)
    .bind(lib.id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    (pool, item_id)
}

fn quoted_path() -> String {
    format!("/media/tv/{QUOTED}/Season 1/{QUOTED} - S01E01 - Pilot.mkv")
}

#[test]
fn silence_args_keep_path_whole() {
    let path = quoted_path();
    let args = media_segments::silence_detect_args(&path, &DetectionConfig::default());
    assert_eq!(
        args,
        [
            "-i",
            path.as_str(),
            "-af",
            "silencedetect=noise=-40dB:d=2,ametadata=mode=print",
            "-f",
            "null",
            "-",
        ]
    );

    let config = DetectionConfig {
        silence_noise_threshold: -35.5,
        min_silence_duration: 1.5,
        ..DetectionConfig::default()
    };
    let args = media_segments::silence_detect_args(&path, &config);
    assert_eq!(
        args[3],
        "silencedetect=noise=-35.5dB:d=1.5,ametadata=mode=print"
    );
}

#[test]
fn black_args_keep_path_whole() {
    let path = quoted_path();
    let args = media_segments::black_detect_args(&path, &DetectionConfig::default());
    assert_eq!(
        args,
        [
            "-i",
            path.as_str(),
            "-vf",
            "blackdetect=d=2:pix_th=0.1",
            "-an",
            "-f",
            "null",
            "-",
        ]
    );

    let config = DetectionConfig {
        min_black_duration: 0.5,
        black_pixel_threshold: 0.05,
        ..DetectionConfig::default()
    };
    let args = media_segments::black_detect_args(&path, &config);
    assert_eq!(args[3], "blackdetect=d=0.5:pix_th=0.05");
}

#[tokio::test]
async fn detect_segments_on_quoted_path() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);

    let (intro, outro) = media_segments::detect_segments(&clip, 60.0, &DetectionConfig::default())
        .await
        .expect("detection runs on a path with spaces and quotes");

    let intro = intro.expect("an intro is found");
    assert!(
        intro.start < 1.0 && intro.end > 5.5 && intro.end < 6.5,
        "intro {intro:?}, expected about 0-6 s"
    );
    let outro = outro.expect("an outro is found");
    assert!(
        outro.start > 49.5 && outro.start < 50.5 && outro.end > 59.5,
        "outro {outro:?}, expected about 50-60 s"
    );
}

#[tokio::test]
async fn detect_and_save_twice_keeps_one_of_each() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);
    let (pool, item_id) = episode_db(&dir.0, &clip).await;

    for run in 1..=2 {
        media_segments::detect_and_save(&pool, item_id, &clip, 60.0)
            .await
            .unwrap_or_else(|e| panic!("run {run}: {e}"));
    }

    let segments = rustyfin_db::repo_media_segment::get_media_segments(&pool, item_id)
        .await
        .unwrap();
    let types: Vec<&str> = segments.iter().map(|s| s.segment_type.as_str()).collect();
    assert_eq!(
        types,
        ["Intro", "Outro"],
        "segments after two runs: {segments:?}"
    );
    pool.close().await;
}

#[tokio::test]
async fn detect_and_save_clears_stale_segments() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = dir.0.join(format!("{QUOTED} - S01E02 - Plain.mkv"));
    make_plain_clip(&clip);
    let clip = clip.to_str().unwrap().to_string();
    let (pool, item_id) = episode_db(&dir.0, &clip).await;
    for kind in ["Intro", "Outro"] {
        rustyfin_db::repo_media_segment::save_media_segment(&pool, item_id, kind, 0, 50_000_000)
            .await
            .unwrap();
    }

    media_segments::detect_and_save(&pool, item_id, &clip, 60.0)
        .await
        .expect("detection runs");

    let segments = rustyfin_db::repo_media_segment::get_media_segments(&pool, item_id)
        .await
        .unwrap();
    assert!(
        segments.is_empty(),
        "a run that finds no intro or outro kept {segments:?}"
    );
    pool.close().await;
}

// Background task speed: intro detection decodes each file once.

/// The silencedetect and blackdetect results ffmpeg logs when run with `args`, without the
/// filter instance addresses, in order.
fn detect_events(args: &[String]) -> Vec<String> {
    let out = Command::new("ffmpeg")
        .arg("-nostdin")
        .args(args)
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg failed: {args:?}");
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter(|l| l.contains("[silencedetect") || l.contains("[blackdetect"))
        .map(|l| {
            let kind = if l.contains("[silencedetect") {
                "silence"
            } else {
                "black"
            };
            format!("{kind} {}", l.split_once("] ").map_or(l, |(_, rest)| rest))
        })
        .collect()
}

#[test]
fn r3perf_one_pass_finds_what_the_two_passes_found() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);
    let config = DetectionConfig::default();

    let mut two: Vec<String> = detect_events(&media_segments::silence_detect_args(&clip, &config))
        .into_iter()
        .filter(|e| e.starts_with("silence"))
        .collect();
    two.extend(detect_events(&media_segments::black_detect_args(
        &clip, &config,
    )));
    let mut one = detect_events(&media_segments::segment_detect_args(&clip, &config, 2));
    two.sort();
    one.sort();
    assert!(
        two.iter().any(|e| e.starts_with("silence")) && two.iter().any(|e| e.starts_with("black")),
        "the clip has both kinds: {two:?}"
    );
    assert_eq!(one, two);
}

#[test]
fn r3perf_one_pass_args_run_both_filters_on_the_default_streams() {
    let path = quoted_path();
    let args = media_segments::segment_detect_args(&path, &DetectionConfig::default(), 3);
    assert_eq!(
        args,
        [
            "-threads",
            "3",
            "-nostdin",
            "-i",
            path.as_str(),
            "-af",
            "silencedetect=noise=-40dB:d=2,ametadata=mode=print",
            "-vf",
            "blackdetect=d=2:pix_th=0.1",
            "-f",
            "null",
            "-",
        ]
    );
    let args = media_segments::segment_detect_args(&path, &DetectionConfig::default(), 0);
    assert_eq!(args[..3], ["-nostdin", "-i", path.as_str()]);
}

#[tokio::test]
async fn r3perf_a_file_without_video_still_fails_detection() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = dir.0.join("audio only.mka");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:sample_rate=48000:d=20")
        .args(["-c:a", "aac"])
        .arg(&clip)
        .status()
        .expect("run ffmpeg");
    assert!(status.success());

    let got =
        media_segments::detect_segments(clip.to_str().unwrap(), 20.0, &DetectionConfig::default())
            .await;
    assert!(got.is_err(), "an audio-only file gave {got:?}");
}

#[tokio::test]
async fn r3perf_all_episodes_are_detected_and_counted() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);
    let (pool, first) = episode_db(&dir.0, &clip).await;
    // Two more episodes of the same file, and one without a runtime, which is skipped.
    let library: String = sqlx::query_scalar("SELECT library_id FROM media_items LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut ids = vec![first];
    for (n, runtime) in [(2, Some(600_000_000i64)), (3, Some(600_000_000)), (4, None)] {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
             VALUES (?1, ?2, 'Episode', ?3, ?4, ?5)",
        )
        .bind(id.to_string())
        .bind(format!("Episode {n}"))
        .bind(format!("{clip}#{n}"))
        .bind(&library)
        .bind(runtime)
        .execute(&pool)
        .await
        .unwrap();
        // The paths must differ; they all name the same file through a link.
        let link = format!("{clip}#{n}");
        std::os::unix::fs::symlink(&clip, &link).unwrap();
        if runtime.is_some() {
            ids.push(id);
        }
    }

    let progress = rustyfin_core::task_progress::TaskProgress::new();
    let count =
        media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &Cancel::new())
            .await
            .unwrap();
    assert_eq!(count, 3);
    for id in ids {
        let segments = rustyfin_db::repo_media_segment::get_media_segments(&pool, id)
            .await
            .unwrap();
        let types: Vec<&str> = segments.iter().map(|s| s.segment_type.as_str()).collect();
        assert_eq!(types, ["Intro", "Outro"], "episode {id}");
    }
    pool.close().await;
}

#[tokio::test]
async fn r3perf_an_unchanged_episode_is_not_detected_again() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);
    let (pool, item_id) = episode_db(&dir.0, &clip).await;
    let progress = rustyfin_core::task_progress::TaskProgress::new();
    let intro_start = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT segment_start_ticks FROM media_segments
             WHERE item_id = ?1 AND segment_type = 'Intro'",
        )
        .bind(item_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    let count =
        media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &Cancel::new())
            .await
            .unwrap();
    assert_eq!(count, 1);
    let detected = intro_start().await;

    // A mark on the stored intro survives the next run: the unchanged file isn't read again.
    sqlx::query("UPDATE media_segments SET segment_start_ticks = 12345 WHERE item_id = ?1")
        .bind(item_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let count =
        media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &Cancel::new())
            .await
            .unwrap();
    assert_eq!(count, 1, "an unchanged episode counts as processed");
    assert_eq!(intro_start().await, 12345);

    // A changed file is detected again.
    std::fs::File::options()
        .write(true)
        .open(&clip)
        .unwrap()
        .set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000),
        )
        .unwrap();
    media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &Cancel::new())
        .await
        .unwrap();
    assert_eq!(intro_start().await, detected);

    // An unchanged file detected by an older detection is detected again.
    sqlx::query("UPDATE media_segments SET segment_start_ticks = 12345 WHERE item_id = ?1")
        .bind(item_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE media_segment_scans SET detector_version = ?1")
        .bind(media_segments::DETECTOR_VERSION - 1)
        .execute(&pool)
        .await
        .unwrap();
    media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &Cancel::new())
        .await
        .unwrap();
    assert_eq!(intro_start().await, detected);
    pool.close().await;
}

#[tokio::test]
async fn r3perf_a_cancelled_run_starts_no_episode_and_records_nothing() {
    if !have_ffmpeg() {
        eprintln!("skip: ffmpeg not found");
        return;
    }
    let dir = TempDir::new("segments");
    let clip = make_quoted_clip(&dir.0);
    let (pool, item_id) = episode_db(&dir.0, &clip).await;
    let cancel = Cancel::new();
    cancel.cancel();
    let progress = rustyfin_core::task_progress::TaskProgress::new();
    let count = media_segments::detect_for_all_episodes_with_progress(&pool, &progress, &cancel)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        rustyfin_db::repo_media_segment::detected_versions(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    let segments = rustyfin_db::repo_media_segment::get_media_segments(&pool, item_id)
        .await
        .unwrap();
    assert!(segments.is_empty(), "{segments:?}");
    pool.close().await;
}
