//! Trickplay tiles: the seek-bar thumbnails Jellyfin clients show, made in the background.
//!
//! Like Jellyfin, a tile set holds one thumbnail every 10 seconds, 320 pixels wide, laid out 10 by
//! 10 in each JPEG tile, under `<data dir>/trickplay/<item id>/<width> - 10x10/<index>.jpg`. Tiles
//! are only made by the "Generate Trickplay Images" task, never on a request. Every trickplay or
//! chapter-image job holds an `image_job`, so at most `image_jobs` run at once, at a low CPU
//! priority so transcodes come first. A video that fails is recorded with its file's size and
//! time and is not tried again until the file changes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::{RwLock, RwLockReadGuard, Semaphore, SemaphorePermit, watch};
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

use rustyfin_model::trickplay::TrickplayInfo;

/// Thumbnail width, in pixels.
pub const WIDTH: u32 = 320;
/// Milliseconds between thumbnails.
pub const INTERVAL_MS: u32 = 10_000;
/// Thumbnails across and down each tile image.
pub const TILE_SIDE: u32 = 10;
/// The longest one ffmpeg run may take before it is stopped.
pub(crate) const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The failure record kind of trickplay tiles.
pub const FAILURE_KIND: &str = "trickplay";

/// Stops a running image task: it checks between videos, and the ffmpeg running is killed.
#[derive(Clone)]
pub struct Cancel(Arc<watch::Sender<bool>>);

impl Default for Cancel {
    fn default() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Returns once `cancel` has been called.
    pub async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }
}

/// The version of the file at `path` that a failure is recorded against: its size and modified
/// time in seconds, or zeros when it can't be read (so a missing file isn't retried either).
pub async fn file_version(path: &str) -> (i64, i64) {
    match fs::metadata(path).await {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            (meta.len() as i64, mtime)
        }
        Err(_) => (0, 0),
    }
}

/// Removes item `item_id`'s tiles and chapter images from the data dir.
pub async fn remove_item_files(data_dir: &Path, item_id: Uuid) {
    for kind in ["trickplay", "chapters"] {
        let _ = fs::remove_dir_all(data_dir.join(kind).join(item_id.to_string())).await;
    }
}

/// Removes the tile folders of items with no tile set (deleted items, and files that changed) and
/// the chapter images no chapter names any more. Waits for the running image jobs to end and
/// keeps new ones from starting, so no job is writing.
pub async fn sweep_unused_files(pool: &SqlitePool, data_dir: &Path) -> Result<(), String> {
    let _no_jobs = image_job_locks().1.write().await;
    let mut trickplay = fs::read_dir(data_dir.join("trickplay")).await.ok();
    while let Some(dir) = trickplay.as_mut() {
        let Some(entry) = dir.next_entry().await.map_err(|e| e.to_string())? else {
            break;
        };
        let Some(item_id) = entry
            .file_name()
            .to_str()
            .and_then(|n| Uuid::parse_str(n).ok())
        else {
            continue;
        };
        let sets = rustyfin_db::repo_trickplay::get_trickplay(pool, item_id)
            .await
            .map_err(|e| e.to_string())?;
        if sets.is_empty() {
            let _ = fs::remove_dir_all(entry.path()).await;
        }
    }

    let mut chapters = fs::read_dir(data_dir.join("chapters")).await.ok();
    while let Some(dir) = chapters.as_mut() {
        let Some(entry) = dir.next_entry().await.map_err(|e| e.to_string())? else {
            break;
        };
        let Some(item_id) = entry
            .file_name()
            .to_str()
            .and_then(|n| Uuid::parse_str(n).ok())
        else {
            continue;
        };
        let named: std::collections::HashSet<PathBuf> =
            rustyfin_db::repo_chapter::get_chapters(pool, item_id)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter_map(|c| c.image_path.filter(|p| !p.is_empty()).map(PathBuf::from))
                .collect();
        // A stray entry (a file named like an item, say) is skipped, not the whole sweep.
        let mut files = match fs::read_dir(entry.path()).await {
            Ok(files) => files,
            Err(e) => {
                warn!(path = %entry.path().display(), error = %e, "skipped an unexpected entry");
                continue;
            }
        };
        let mut kept = 0;
        while let Ok(Some(file)) = files.next_entry().await {
            if named.contains(&file.path()) {
                kept += 1;
            } else {
                let _ = fs::remove_file(file.path()).await;
            }
        }
        if kept == 0 {
            let _ = fs::remove_dir(entry.path()).await;
        }
    }
    Ok(())
}

fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// How many image jobs (a video's trickplay tiles, an item's chapter images) may run at once:
/// `RUSTYFIN_IMAGE_JOBS`, else half the CPUs, from 1 to 4.
pub fn image_jobs() -> usize {
    static JOBS: OnceLock<usize> = OnceLock::new();
    *JOBS.get_or_init(|| {
        std::env::var("RUSTYFIN_IMAGE_JOBS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| (cpus() / 2).clamp(1, 4))
    })
}

/// The decoder threads of each image job's ffmpeg: the CPUs shared out among the jobs, 1 to 4.
fn decode_threads() -> usize {
    (cpus() / image_jobs()).clamp(1, 4)
}

/// Held by a running image job: one of the `image_jobs` permits, and a shared hold on the lock
/// that `sweep_unused_files` takes alone. Transcodes don't take it.
pub(crate) struct ImageJob {
    _permit: SemaphorePermit<'static>,
    _shared: RwLockReadGuard<'static, ()>,
}

fn image_job_locks() -> &'static (Semaphore, RwLock<()>) {
    static LOCKS: OnceLock<(Semaphore, RwLock<()>)> = OnceLock::new();
    LOCKS.get_or_init(|| (Semaphore::new(image_jobs()), RwLock::new(())))
}

/// Waits for a free image job, and for any sweep to end.
pub(crate) async fn image_job() -> ImageJob {
    let (permits, sweep) = image_job_locks();
    let _permit = permits
        .acquire()
        .await
        .expect("the semaphore is never closed");
    let _shared = sweep.read().await;
    ImageJob { _permit, _shared }
}

/// The folder of an item's tile set at `width`.
pub fn tiles_dir(data_dir: &Path, item_id: Uuid, width: u32, tile_side: u32) -> PathBuf {
    data_dir
        .join("trickplay")
        .join(item_id.to_string())
        .join(format!("{width} - {tile_side}x{tile_side}"))
}

/// The file of tile image `index` of `info`, an item's tile set.
pub fn tile_path(data_dir: &Path, item_id: Uuid, info: &TrickplayInfo, index: u32) -> PathBuf {
    tiles_dir(data_dir, item_id, info.width, info.tile_width).join(format!("{index}.jpg"))
}

/// Runs ffmpeg with `args`, and fails on a non-zero exit, after `FFMPEG_TIMEOUT`, or when
/// `cancel` fires, killing ffmpeg. The caller holds an `image_job`. ffmpeg runs at a low CPU
/// priority (nice 10), so playback transcodes come first.
pub(crate) async fn run_ffmpeg(args: &[&str], cancel: &Cancel) -> Result<(), String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    // SAFETY: the hook only makes the nice(2) system call, which is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::nice(10);
            Ok(())
        });
    }
    // Dropping the output future kills ffmpeg (kill_on_drop).
    let output = tokio::select! {
        out = tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output()) => match out {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("ffmpeg could not start: {e}")),
            Err(_) => return Err("ffmpeg took too long and was stopped".to_string()),
        },
        _ = cancel.cancelled() => return Err("cancelled".to_string()),
    };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("ffmpeg failed: {}", stderr.trim()))
    }
}

/// The JPEG files in `dir`.
async fn jpegs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = fs::read_dir(dir).await.map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jpg") {
            files.push(path);
        }
    }
    Ok(files)
}

/// How many times `generate` has started on the file at `path` since the server started. It
/// lets tests see that a failed file is not tried again.
pub fn generate_attempts(path: &str) -> u32 {
    attempts().lock().unwrap().get(path).copied().unwrap_or(0)
}

fn attempts() -> &'static std::sync::Mutex<std::collections::HashMap<String, u32>> {
    static ATTEMPTS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
        OnceLock::new();
    ATTEMPTS.get_or_init(Default::default)
}

/// Makes the tile set of the video at `path`, an item's file, and saves it. The old set, if any,
/// is replaced only once the new one is complete. Waits for an `image_job`.
pub async fn generate(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    let _job = image_job().await;
    *attempts()
        .lock()
        .unwrap()
        .entry(path.to_string())
        .or_default() += 1;
    let item_dir = data_dir.join("trickplay").join(item_id.to_string());
    let work = item_dir.join(format!(".work-{WIDTH}"));
    let tiles = work.join("tiles");
    let _ = fs::remove_dir_all(&work).await;
    fs::create_dir_all(&tiles)
        .await
        .map_err(|e| format!("could not make {}: {e}", tiles.display()))?;

    let result = make_tiles(path, &work, &tiles, cancel).await;
    let made = match result {
        Ok(made) => made,
        Err(e) => {
            let _ = fs::remove_dir_all(&work).await;
            return Err(e);
        }
    };

    let dest = tiles_dir(data_dir, item_id, WIDTH, TILE_SIDE);
    let _ = fs::remove_dir_all(&dest).await;
    let moved = fs::rename(&tiles, &dest).await;
    let _ = fs::remove_dir_all(&work).await;
    moved.map_err(|e| format!("could not move the tiles to {}: {e}", dest.display()))?;

    rustyfin_db::repo_trickplay::save_trickplay(pool, item_id, &made)
        .await
        .map_err(|e| format!("could not save the tile set: {e}"))?;
    Ok(made)
}

/// The packet filter that thins a video's key frames before they are decoded. Of the key frames
/// in each interval-long window centred on a thumbnail's time, it keeps the first, which the fps
/// filter then shows there, and drops the rest and every other packet. Most videos have a key
/// frame every few seconds, so this decodes a few times fewer frames for the same thumbnails.
/// Every key frame of the first interval and from `keep_from` seconds on is kept, as before: the
/// first and last ones set where the thumbnails start and stop, and some HEVC decoders hand the
/// first frames out of order when the ones between are missing. A key frame without a time is
/// kept too. Register 0 holds the next window's centre in seconds, 4 whether a frame was kept
/// yet, and 5 the first key frame's time.
fn thin_key_frames(keep_from: f64) -> String {
    let i = f64::from(INTERVAL_MS) / 1000.0;
    let next = format!("st(0\\,(round(pts*tb/{i})+1)*{i})");
    format!(
        "noise=drop=if(isnan(pts)+not(key)\\,not(key)\\,\
         if(not(ld(4))\\,st(4\\,1);st(5\\,pts*tb);{next};0\\,\
         if(gte(pts*tb\\,{keep_from})+lt(pts*tb\\,ld(5)+{i})\\,0\\,\
         if(gte(pts*tb\\,ld(0)-{i}/2)\\,{next};0\\,1))))"
    )
}

/// The length of the video at `path` in seconds, from ffprobe, if it can tell.
async fn duration(path: &str) -> Option<f64> {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(60), probe)
        .await
        .ok()?
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Makes the tile images of `path` in `tiles`, in one ffmpeg run that also lists the thumbnails
/// in `work`, and returns the set's info.
async fn make_tiles(
    path: &str,
    work: &Path,
    tiles: &Path,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    let mut count = tile_run(path, work, tiles, true, cancel).await?;
    if count == 0 {
        // A file whose packets carry no key frame flags loses them all to the thinning
        count = tile_run(path, work, tiles, false, cancel).await?;
    }
    if count == 0 {
        return Err("ffmpeg made no thumbnails".to_string());
    }
    // The tile filter pads the last image, so every image is a full grid of thumbnails
    let first = tiles.join("0.jpg");
    let (tile_w, tile_h) =
        image::image_dimensions(&first).map_err(|e| format!("could not read a tile: {e}"))?;
    let (width, height) = (tile_w / TILE_SIDE, tile_h / TILE_SIDE);

    let mut info = TrickplayInfo {
        width,
        height,
        tile_width: TILE_SIDE,
        tile_height: TILE_SIDE,
        thumbnail_count: count,
        interval: INTERVAL_MS,
        bandwidth: 0,
    };
    let tile_files = jpegs(tiles).await?;
    if tile_files.len() as u32 != info.tile_count() {
        return Err(format!(
            "ffmpeg made {} tile images for {} thumbnails",
            tile_files.len(),
            count
        ));
    }
    // Jellyfin's bandwidth: the most bits per second of play that any tile image takes to load.
    let seconds_per_tile = f64::from(info.thumbnails_per_tile()) * f64::from(INTERVAL_MS) / 1000.0;
    for file in &tile_files {
        let size = fs::metadata(file).await.map_err(|e| e.to_string())?.len();
        let bits_per_second = (size as f64 * 8.0 / seconds_per_tile).ceil() as u32;
        info.bandwidth = info.bandwidth.max(bits_per_second);
    }
    Ok(info)
}

/// Runs ffmpeg once over `path`: one thumbnail per interval, scaled and laid out straight into
/// the tile images in `tiles`, and a line per thumbnail in `work`'s `frames.crc`. Returns how many
/// thumbnails it made. Only key frames are decoded, thinned first when `thin` is set; the fps
/// filter repeats the last one where they are further apart than the interval, and from time 0
/// (ffmpeg starts every input there), as the old JPEG-per-thumbnail output did.
async fn tile_run(
    path: &str,
    work: &Path,
    tiles: &Path,
    thin: bool,
    cancel: &Cancel,
) -> Result<u32, String> {
    let threads = decode_threads().to_string();
    // Without a length every key frame is kept, as without thinning, rather than risk the count
    let keep_from = if thin { duration(path).await } else { None }
        .map_or(-1e9, |d| d - 2.0 * f64::from(INTERVAL_MS) / 1000.0);
    let thinning = thin_key_frames(keep_from);
    let graph = format!(
        "[0:V:0]fps=1000/{INTERVAL_MS}:start_time=0,scale={WIDTH}:-2,split[t][n];\
         [t]tile={TILE_SIDE}x{TILE_SIDE}[tiles]"
    );
    let output = tiles.join("%d.jpg");
    let output = output.to_string_lossy();
    let frames = work.join("frames.crc");
    let frames_arg = frames.to_string_lossy();
    // The loop filter only smooths block edges, which the scaling down hides anyway
    let mut args = vec![
        "-threads",
        &threads,
        "-skip_frame",
        "nokey",
        "-skip_loop_filter",
        "all",
    ];
    if thin {
        args.extend(["-bsf:v", &thinning]);
    }
    args.extend([
        "-i",
        path,
        "-filter_complex_threads",
        "1",
        "-filter_complex",
        &graph,
        "-map",
        "[tiles]",
        "-threads",
        "1",
        "-qscale:v",
        "4",
        "-start_number",
        "0",
        "-f",
        "image2",
        &output,
        "-map",
        "[n]",
        "-f",
        "framecrc",
        &frames_arg,
    ]);
    for stale in jpegs(tiles).await? {
        let _ = fs::remove_file(stale).await;
    }
    run_ffmpeg(&args, cancel).await?;
    let listed = fs::read_to_string(&frames)
        .await
        .map_err(|e| format!("could not read {}: {e}", frames.display()))?;
    let count = listed.lines().filter(|l| !l.starts_with('#')).count();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Makes the missing tile sets of the videos in the libraries `library_ids`, up to `image_jobs`
/// at a time, and returns how many it made. A video that fails is logged and recorded with its
/// file's version; it is skipped until the file changes. Once `cancel` fires it starts no more
/// videos and the running ones are stopped.
pub async fn generate_for_libraries(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
    cancel: &Cancel,
) -> Result<u32, String> {
    let progress = crate::task_progress::TaskProgress::new();
    generate_for_libraries_with_progress(pool, data_dir, library_ids, cancel, &progress).await
}

/// `generate_for_libraries`, reporting the share of the videos missing tiles at the start that
/// it has looked at on `progress`.
pub async fn generate_for_libraries_with_progress(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
    cancel: &Cancel,
    progress: &crate::task_progress::TaskProgress,
) -> Result<u32, String> {
    use rustyfin_db::repo_trickplay::{
        clear_failure, get_failure, record_failure, set_image_version,
    };
    // Videos this run won't try (again): failed now, or before on the same file
    let mut failed: Vec<Uuid> = Vec::new();
    let mut made = 0u32;
    let total =
        rustyfin_db::repo_trickplay::items_missing_trickplay(pool, library_ids, WIDTH, &[], -1)
            .await
            .map_err(|e| format!("could not list videos: {e}"))?
            .len();
    let mut done = 0usize;
    progress.report_count(0, total);
    let db = |e: sqlx::Error| format!("could not record a failure: {e}");
    let mut queue = std::collections::VecDeque::new();
    // Up to `image_jobs` videos at once; each task's id maps to its video, path and file version
    let mut running = JoinSet::new();
    let mut started = std::collections::HashMap::new();
    loop {
        while running.len() < image_jobs() && !cancel.is_cancelled() {
            if queue.is_empty() {
                // The videos being made have no tile set yet, so they are skipped like failures
                let skip: Vec<Uuid> = failed
                    .iter()
                    .chain(started.values().map(|(id, _, _)| id))
                    .copied()
                    .collect();
                queue.extend(
                    rustyfin_db::repo_trickplay::items_missing_trickplay(
                        pool,
                        library_ids,
                        WIDTH,
                        &skip,
                        20,
                    )
                    .await
                    .map_err(|e| format!("could not list videos: {e}"))?,
                );
            }
            let Some((item_id, path, _runtime)) = queue.pop_front() else {
                break;
            };
            let version = file_version(&path).await;
            if get_failure(pool, item_id, FAILURE_KIND).await.map_err(db)? == Some(version) {
                failed.push(item_id);
                done += 1;
                progress.report_count(done.min(total), total);
                continue;
            }
            let (pool, data_dir, cancel, file) = (
                pool.clone(),
                data_dir.to_path_buf(),
                cancel.clone(),
                path.clone(),
            );
            let task = running
                .spawn(async move { generate(&pool, &data_dir, item_id, &file, &cancel).await });
            started.insert(task.id(), (item_id, path, version));
        }
        let Some(joined) = running.join_next_with_id().await else {
            break;
        };
        let (result, task) = match joined {
            Ok((task, result)) => (result, task),
            Err(e) => (Err(format!("the tile task failed: {e}")), e.id()),
        };
        let Some((item_id, path, version)) = started.remove(&task) else {
            continue;
        };
        // Videos added during the run can take it past the total counted at the start
        done += 1;
        progress.report_count(done.min(total), total);
        match result {
            Ok(set) => {
                made += 1;
                clear_failure(pool, item_id, FAILURE_KIND)
                    .await
                    .map_err(db)?;
                set_image_version(pool, item_id, Some(version))
                    .await
                    .map_err(db)?;
                info!(item = %item_id, thumbnails = set.thumbnail_count, "made trickplay tiles");
            }
            // The other running videos are stopped too, and end up here
            Err(_) if cancel.is_cancelled() => {}
            Err(e) => {
                warn!(item = %item_id, path = %path, error = %e, "could not make trickplay tiles");
                record_failure(pool, item_id, FAILURE_KIND, version)
                    .await
                    .map_err(db)?;
                set_image_version(pool, item_id, Some(version))
                    .await
                    .map_err(db)?;
                failed.push(item_id);
            }
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg(test: &str) -> bool {
        let here = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !here {
            eprintln!("SKIPPED {test}: ffmpeg is not on PATH");
        }
        here
    }

    #[tokio::test]
    async fn r3trickplay_thinning_keeps_the_thumbnails() {
        if !have_ffmpeg("r3trickplay_thinning_keeps_the_thumbnails") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // A key frame every 2 s, so the thinning drops four of every five
        let clip = dir.join("clip.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=95:size=640x360:rate=10")
            .args(["-c:v", "mpeg4", "-g", "20"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success());
        let clip = clip.to_string_lossy();
        let mut made = Vec::new();
        for thin in [false, true] {
            let tiles = dir.join(format!("tiles-{thin}"));
            std::fs::create_dir_all(&tiles).unwrap();
            let count = tile_run(&clip, &dir, &tiles, thin, &Cancel::new())
                .await
                .unwrap();
            let size = image::image_dimensions(tiles.join("0.jpg")).unwrap();
            made.push((count, size, std::fs::read_dir(&tiles).unwrap().count()));
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            made[0], made[1],
            "the same thumbnails with and without thinning"
        );
        assert_eq!(made[1], (9, (3200, 1800), 1));
    }

    #[tokio::test]
    async fn r3trickplay_sweep_waits_for_running_jobs() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let dir = std::env::temp_dir().join(format!("r3trickplay-sweep-{}", Uuid::new_v4()));
        let job = image_job().await;
        let sweep = tokio::spawn(async move { sweep_unused_files(&pool, &dir).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!sweep.is_finished(), "the sweep waits for the running job");
        drop(job);
        tokio::time::timeout(Duration::from_secs(5), sweep)
            .await
            .expect("the sweep runs once the job ends")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn r2chapters_cancel_kills_the_running_ffmpeg() {
        let have_ffmpeg = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !have_ffmpeg {
            eprintln!("SKIPPED r2chapters_cancel_kills_the_running_ffmpeg: ffmpeg is not on PATH");
            return;
        }
        // The null muxer ignores its file name, so a unique one finds this ffmpeg in /proc.
        let marker = format!("/nonexistent/r2chapters-{}", Uuid::new_v4());
        let running = {
            let marker = marker.clone();
            move || {
                std::fs::read_dir("/proc").unwrap().flatten().any(|p| {
                    std::fs::read(p.path().join("cmdline"))
                        .is_ok_and(|c| String::from_utf8_lossy(&c).contains(&marker))
                })
            }
        };
        let cancel = Cancel::new();
        let stopper = cancel.clone();
        let seen = {
            let running = running.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let seen = running();
                stopper.cancel();
                seen
            })
        };
        let started = std::time::Instant::now();
        // An endless input: only the cancel ends it.
        let result = run_ffmpeg(
            &[
                "-re",
                "-f",
                "lavfi",
                "-i",
                "testsrc=rate=5",
                "-f",
                "null",
                &marker,
            ],
            &cancel,
        )
        .await;
        assert!(seen.await.unwrap(), "ffmpeg was running before the cancel");
        // Killed and reaped: a zombie has an empty cmdline, so poll briefly.
        let gone_by = std::time::Instant::now() + Duration::from_secs(3);
        while running() && std::time::Instant::now() < gone_by {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!running(), "the ffmpeg child is gone");
        assert_eq!(result, Err("cancelled".to_string()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
