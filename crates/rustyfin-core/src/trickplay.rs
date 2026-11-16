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
/// `RUSTYFIN_IMAGE_JOBS`, from 1 to the CPU count, else half the CPUs, from 2 to 4. The CPU
/// count is what the process may use: a container's CPU limit counts.
pub fn image_jobs() -> usize {
    static JOBS: OnceLock<usize> = OnceLock::new();
    *JOBS.get_or_init(|| {
        let most = cpus().min(Semaphore::MAX_PERMITS);
        // At least 2 where there are 2 CPUs: a job on a network share mostly waits for it
        let default = (cpus() / 2).clamp(2, 4).min(most);
        match std::env::var("RUSTYFIN_IMAGE_JOBS") {
            Err(_) => default,
            Ok(v) => match v.trim().parse::<usize>() {
                Ok(n) => n.clamp(1, most),
                Err(_) => {
                    warn!(value = %v, default, "RUSTYFIN_IMAGE_JOBS is not a whole number");
                    default
                }
            },
        }
    })
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

/// Waits for a free image job, and for any sweep to end. `None` once `cancel` fires first.
pub(crate) async fn image_job(cancel: &Cancel) -> Option<ImageJob> {
    let (permits, sweep) = image_job_locks();
    let wait = async {
        let _permit = permits
            .acquire()
            .await
            .expect("the semaphore is never closed");
        let _shared = sweep.read().await;
        ImageJob { _permit, _shared }
    };
    tokio::select! {
        job = wait => Some(job),
        _ = cancel.cancelled() => None,
    }
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
    run_tool("ffmpeg", &["-nostdin", "-y"], args, cancel)
        .await
        .map(drop)
}

/// Runs ffprobe with `args`, as `run_ffmpeg` runs ffmpeg, and returns what it printed.
async fn run_ffprobe(args: &[&str], cancel: &Cancel) -> Result<String, String> {
    let out = run_tool("ffprobe", &[], args, cancel).await?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// `run_ffmpeg` for `program` (ffmpeg or ffprobe), with `first` before `args`. Returns stdout.
async fn run_tool(
    program: &str,
    first: &[&str],
    args: &[&str],
    cancel: &Cancel,
) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(program);
    cmd.args(["-hide_banner", "-loglevel", "error"])
        .args(first)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::media_reads::lower_priority(&mut cmd);
    // Dropping the output future kills ffmpeg (kill_on_drop).
    let output = tokio::select! {
        out = tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output()) => match out {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(format!("{program} could not start: {e}")),
            Err(_) => return Err(format!("{program} took too long and was stopped")),
        },
        _ = cancel.cancelled() => return Err("cancelled".to_string()),
    };
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{program} failed: {}", stderr.trim()))
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
/// is replaced only once the new one is complete. Waits for an `image_job` (`make_tiles`).
pub async fn generate(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
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

    let result = make_tiles(path, &work, &tiles, Reading::Auto, cancel).await;
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
/// Every key frame of the first interval is kept, as before: some HEVC decoders hand the first
/// frames out of order when the ones between are missing, and the thumbnails start at the first
/// one. A key frame without a time is kept too. The filter sees the same times as the fps filter,
/// which ffmpeg starts at 0 for every input. Register 0 holds the next window's centre in seconds,
/// 4 whether a frame was kept yet, and 5 the first key frame's time.
///
/// It runs on the input side (`-bsf:v` before `-i`), which needs FFmpeg 7.0 or newer.
fn thin_key_frames() -> String {
    let i = f64::from(INTERVAL_MS) / 1000.0;
    let next = format!("st(0\\,(round(pts*tb/{i})+1)*{i})");
    format!(
        "noise=drop=if(isnan(pts)+not(key)\\,not(key)\\,\
         if(not(ld(4))\\,st(4\\,1);st(5\\,pts*tb);{next};0\\,\
         if(lt(pts*tb\\,ld(5)+{i})\\,0\\,\
         if(gte(pts*tb\\,ld(0)-{i}/2)\\,{next};0\\,1))))"
    )
}

/// The start of the error when ffmpeg can't run the key frame thinning.
pub const FFMPEG_TOO_OLD: &str = "ffmpeg is too old for trickplay images: FFmpeg 7.0 or newer \
                                  is needed, for bitstream filters on an input";

/// Whether ffmpeg's error `e` means it rejected the thinning: before 7.0, `-bsf` is an output
/// option only, and ffmpeg says the option "cannot be applied to input url".
fn rejects_input_bsf(e: &str) -> bool {
    e.contains("bsf") && e.contains("cannot be applied to input")
}

/// What `make_tiles` knows of a video before reading it, from one short ffprobe run.
#[derive(Debug)]
struct Probe {
    /// Where ffmpeg starts the video's time line, in the file's seconds.
    start: f64,
    duration: f64,
    size: f64,
    /// One frame's length, in seconds.
    frame: f64,
    width: u32,
    height: u32,
    /// Where the first video packet is: the header that every opening of the file reads.
    first_pos: f64,
    /// The last key frame's time, in the file's seconds, and where it is.
    last_key: f64,
    last_pos: f64,
    /// A Matroska or MP4 file: its index lets ffmpeg seek straight to a key frame.
    indexed: bool,
}

/// How many tiles of a video `seek_tiles` makes at once, 2 above 1080p. Not from the CPU count:
/// its ffmpeg runs mostly wait for small reads, which a share answers better several at a time.
const SEEK_TILES_AT_ONCE: usize = 4;

/// How many `seek_tiles` tiles run at once over all videos: 2 per image job. It bounds their
/// memory, as an ffmpeg decoding 4K HEVC holds about 180 MB, so it follows `image_jobs`, which
/// follows the CPUs a container may use.
fn seek_tile_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(2 * image_jobs()))
}

/// The bytes that opening a file for a seek may probe. Each opening otherwise probes up to 5 MB
/// from the start of the file, for streams whose details the header leaves out; seek mode only
/// needs the video's key frames, whose details Matroska and MP4 headers hold. A 4K MKV at
/// 67 Mbps with a 7.1 TrueHD track read 5.0 MB per seek with the default and 2.2 MB with this.
const PROBE_BYTES: &str = "100000";

/// The bytes a seek reads beyond the video's header and index: the key frame, and what is
/// interleaved with it.
const SEEK_SLACK: f64 = 64.0 * 1024.0;

impl Probe {
    /// How many thumbnails the video has: one per interval up to the end of its last key frame,
    /// which is where the streaming pass's fps filter stops.
    fn count(&self) -> u32 {
        let interval = f64::from(INTERVAL_MS) / 1000.0;
        let n = ((self.last_key - self.start + self.frame) / interval).round();
        if n.is_finite() && n >= 1.0 {
            n.min(f64::from(u32::MAX)) as u32
        } else {
            0
        }
    }

    /// Whether seeking to each thumbnail's key frame reads at most a third of what reading the
    /// whole file does. Each seek reopens the file, so it reads the header, the index (at the
    /// end, after the media), and about a second of media around the key frame.
    fn seek_pays(&self) -> bool {
        let count = self.count();
        if !self.indexed || count == 0 || self.duration <= 0.0 || self.size <= 0.0 {
            return false;
        }
        let rate = self.size / self.duration;
        let after_last_key = (self.duration - (self.last_key - self.start)).max(0.0) * rate;
        let index = (self.size - self.last_pos - after_last_key).max(0.0);
        let per_seek = self.first_pos + index + SEEK_SLACK + rate;
        self.size / f64::from(count) >= 3.0 * per_seek
    }

    /// The decoder threads for reading this video whole: at most 2 above 1080p, as each frame
    /// thread holds frames of its own and 4K ones are large.
    fn threads(&self) -> usize {
        let threads = crate::media_reads::decode_threads();
        if self.above_1080p() {
            threads.min(2)
        } else {
            threads
        }
    }

    fn above_1080p(&self) -> bool {
        u64::from(self.width) * u64::from(self.height) > 1920 * 1088
    }
}

/// Probes the video at `path` for `make_tiles`: its first and last key frames and its format.
/// `Ok(None)` when ffprobe can't tell; `Err` only once `cancel` fires.
async fn probe(path: &str, cancel: &Cancel) -> Result<Option<Probe>, String> {
    let out = run_ffprobe(
        &[
            "-select_streams",
            // The first video that isn't cover art, as ffmpeg's 0:V:0
            "V:0",
            "-show_entries",
            "format=start_time,duration,size,format_name:stream=avg_frame_rate,width,height:\
             packet=pts_time,pos",
            // The first packet, and the last key frame, which a seek past the end lands on
            "-read_intervals",
            "%+#1,99999999%+#1",
            "-of",
            "json",
            path,
        ],
        cancel,
    )
    .await;
    let out = match out {
        Ok(out) => out,
        Err(e) if cancel.is_cancelled() => return Err(e),
        Err(_) => return Ok(None),
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&out) else {
        return Ok(None);
    };
    let num = |v: &serde_json::Value| -> Option<f64> {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .filter(|n: &f64| n.is_finite())
    };
    let format = &json["format"];
    let stream = &json["streams"][0];
    let packets = json["packets"].as_array();
    let (Some(first), Some(last)) = (
        packets.and_then(|p| p.first()),
        packets.and_then(|p| p.last()),
    ) else {
        return Ok(None);
    };
    let frame = stream["avg_frame_rate"]
        .as_str()
        .and_then(|r| r.split_once('/'))
        .and_then(|(n, d)| Some(d.parse::<f64>().ok()? / n.parse::<f64>().ok()?))
        .filter(|f| f.is_finite() && *f > 0.0);
    let format_name = format["format_name"].as_str().unwrap_or_default();
    let probe = (|| {
        Some(Probe {
            start: num(&format["start_time"])?,
            duration: num(&format["duration"])?,
            size: num(&format["size"])?,
            frame: frame?,
            width: u32::try_from(stream["width"].as_u64()?).ok()?,
            height: u32::try_from(stream["height"].as_u64()?).ok()?,
            first_pos: num(&first["pos"])?,
            last_key: num(&last["pts_time"])?,
            last_pos: num(&last["pos"])?,
            indexed: format_name.contains("matroska") || format_name.contains("mp4"),
        })
    })();
    Ok(probe)
}

/// Makes the tile images of `path` in `tiles` by seeking: per tile image, one ffprobe run finds
/// each of its thumbnails' key frames, and one ffmpeg run reads and decodes just those, through
/// the concat demuxer. The key frame of a thumbnail is the last one before the middle of its
/// interval, as the unthinned streaming pass's fps filter picked (the thinned pass takes the
/// first key frame of the interval instead; both are within half an interval). Tiles run
/// `SEEK_TILES_AT_ONCE` at once, and `seek_tile_slots` over all videos. They are made in `work`
/// and moved into `tiles` once all are right. Returns the thumbnail count. Fails where the
/// decoder doesn't hand out exactly one frame per key frame in order (open-GOP HEVC can't
/// decode its key frames alone that way).
async fn seek_tiles(
    path: &str,
    probe: &Probe,
    work: &Path,
    tiles: &Path,
    cancel: &Cancel,
) -> Result<u32, String> {
    if path.contains(['\n', '\r']) || path.contains("://") {
        return Err("the path can't go in a concat list".to_string());
    }
    let count = probe.count();
    let per_tile = TILE_SIDE * TILE_SIDE;
    // Their own folder, so a fallback to streaming never meets a tile written late
    let made = work.join("seek-tiles");
    let _ = fs::remove_dir_all(&made).await;
    fs::create_dir_all(&made)
        .await
        .map_err(|e| format!("could not make {}: {e}", made.display()))?;
    let mut running = JoinSet::new();
    let mut next = 0;
    loop {
        let at_once = if probe.above_1080p() {
            SEEK_TILES_AT_ONCE.min(2)
        } else {
            SEEK_TILES_AT_ONCE
        };
        while running.len() < at_once && next * per_tile < count {
            let slots = next * per_tile..((next + 1) * per_tile).min(count);
            let (path, work, tiles, cancel) = (
                path.to_string(),
                work.to_path_buf(),
                made.clone(),
                cancel.clone(),
            );
            let start = probe.start;
            running.spawn(async move {
                seek_tile(&path, start, next, slots, &work, &tiles, &cancel).await
            });
            next += 1;
        }
        let failed = match running.join_next().await {
            None => break,
            Some(Ok(Ok(()))) => continue,
            Some(Ok(Err(e))) => e,
            Some(Err(e)) => format!("a tile task failed: {e}"),
        };
        // Stops the other tiles' ffmpeg, and waits until their tasks are gone
        running.shutdown().await;
        return Err(failed);
    }
    for stale in jpegs(tiles).await? {
        let _ = fs::remove_file(stale).await;
    }
    for tile in jpegs(&made).await? {
        let to = tiles.join(tile.file_name().unwrap_or_default());
        fs::rename(&tile, &to)
            .await
            .map_err(|e| format!("could not move {}: {e}", tile.display()))?;
    }
    Ok(count)
}

/// Makes tile image `tile` of the thumbnails `slots` for `seek_tiles`.
async fn seek_tile(
    path: &str,
    start: f64,
    tile: u32,
    slots: std::ops::Range<u32>,
    work: &Path,
    tiles: &Path,
    cancel: &Cancel,
) -> Result<(), String> {
    let _slot = tokio::select! {
        slot = seek_tile_slots().acquire() => slot.expect("the semaphore is never closed"),
        _ = cancel.cancelled() => return Err("cancelled".to_string()),
    };
    let interval = f64::from(INTERVAL_MS) / 1000.0;
    let intervals: Vec<String> = slots
        .clone()
        // Just before the middle: a key frame right on it is the next thumbnail's to the fps
        // filter. A millisecond, as Matroska's times are in milliseconds and a seek rounds to them.
        .map(|k| {
            format!(
                "{:.6}%+#1",
                start + f64::from(k) * interval + interval / 2.0 - 1e-3
            )
        })
        .collect();
    let keys = run_ffprobe(
        &[
            "-probesize",
            PROBE_BYTES,
            "-analyzeduration",
            "0",
            "-select_streams",
            // The first video that isn't cover art, as ffmpeg's 0:V:0
            "V:0",
            "-read_intervals",
            &intervals.join(","),
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
            path,
        ],
        cancel,
    )
    .await?;
    let keys: Vec<f64> = keys
        .lines()
        .map(|l| l.trim().trim_end_matches(',').parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("ffprobe gave a key frame without a time: {e}"))?;
    if keys.len() != slots.len() {
        return Err(format!(
            "ffprobe found {} of {} key frames",
            keys.len(),
            slots.len()
        ));
    }
    // Each entry reads one key frame: in at its time (a microsecond on, past ffprobe's rounding),
    // out a millisecond later. Laid out an interval apart, so their order shows in the times.
    // Each opens the file again; the probing it does there is cut short (`PROBE_BYTES`).
    let quoted = path.replace('\'', "'\\''");
    let mut list = String::from("ffconcat version 1.0\n");
    for key in &keys {
        list.push_str(&format!(
            "file '{quoted}'\noption probesize {PROBE_BYTES}\noption analyzeduration 0\n\
             inpoint {:.6}\noutpoint {:.6}\nduration {interval}\n",
            key + 1e-6,
            key + 1e-3
        ));
    }
    let list_path = work.join(format!("seek-{tile}.ffconcat"));
    fs::write(&list_path, list)
        .await
        .map_err(|e| format!("could not write {}: {e}", list_path.display()))?;
    let frames = work.join(format!("seek-{tile}.crc"));
    let output = tiles.join(format!("{tile}.jpg"));
    let graph =
        format!("[0:V:0]scale={WIDTH}:-2,split[t][n];[t]tile={TILE_SIDE}x{TILE_SIDE}[tiles]");
    run_ffmpeg(
        &[
            "-threads",
            "1",
            "-skip_frame",
            "nokey",
            "-skip_loop_filter",
            "all",
            "-safe",
            "0",
            "-f",
            "concat",
            "-i",
            &list_path.to_string_lossy(),
            "-filter_complex_threads",
            "1",
            "-filter_complex",
            &graph,
            "-map",
            "[tiles]",
            "-frames:v",
            "1",
            "-threads",
            "1",
            "-qscale:v",
            "4",
            "-update",
            "1",
            &output.to_string_lossy(),
            "-map",
            "[n]",
            "-fps_mode",
            "passthrough",
            "-f",
            "framecrc",
            &frames.to_string_lossy(),
        ],
        cancel,
    )
    .await?;
    // framecrc lines: stream, dts, pts, duration, size, checksum
    let listed = fs::read_to_string(&frames)
        .await
        .map_err(|e| format!("could not read {}: {e}", frames.display()))?;
    let times: Vec<i64> = listed
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split(',').nth(2)?.trim().parse().ok())
        .collect();
    if times.len() != keys.len() || times.windows(2).any(|w| w[0] >= w[1]) {
        return Err(format!(
            "the decoder gave {} frames, not {} in order, for tile {tile}",
            times.len(),
            keys.len()
        ));
    }
    Ok(())
}

/// How `make_tiles` reads a video.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Reading {
    /// Seek to each thumbnail's key frame where that reads much less than the whole file.
    Auto,
    /// Seek, failing where seeking can't make the thumbnails.
    #[cfg(test)]
    Seek,
    /// Read the whole file.
    #[cfg(test)]
    Stream,
}

/// Makes the tile images of `path` in `tiles`, using `work` for lists, and returns the set's
/// info. A video in a container with an index (Matroska, MP4) that seeking reads much less of is
/// read a key frame at a time (`seek_tiles`); others, and any that seeking fails on, are read
/// whole (`tile_run`). Both make the same thumbnails.
///
/// Seeking holds an `image_job`. Reading whole first takes a network read
/// (`media_reads::whole_file_read`, for a file on a network share), then an `image_job`: the
/// order intro detection takes them in, so neither waits on the other while holding what the
/// other needs. Seeking needs no network read: it reads a small part of the file, and its tiles
/// are bounded by `seek_tile_slots`.
async fn make_tiles(
    path: &str,
    work: &Path,
    tiles: &Path,
    reading: Reading,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    let probed = probe(path, cancel).await?;
    let mut count = 0;
    let seek = match reading {
        Reading::Auto => probed.as_ref().filter(|p| p.seek_pays()),
        #[cfg(test)]
        Reading::Seek => probed.as_ref().filter(|p| p.indexed),
        #[cfg(test)]
        Reading::Stream => None,
    };
    if let Some(p) = seek {
        let Some(_job) = image_job(cancel).await else {
            return Err("cancelled".to_string());
        };
        match seek_tiles(path, p, work, tiles, cancel).await {
            Ok(n) => count = n,
            Err(e) if cancel.is_cancelled() => return Err(e),
            #[cfg(test)]
            Err(e) if reading == Reading::Seek => return Err(e),
            Err(e) => {
                info!(path, error = %e, "seeking can't make the trickplay tiles; reading all")
            }
        }
    }
    #[cfg(test)]
    if reading == Reading::Seek && seek.is_none() {
        return Err("the video can't be seeked".to_string());
    }
    if count == 0 {
        // The seek's job, if any, is given back first: see above for the order
        let _read = tokio::select! {
            read = crate::media_reads::whole_file_read(Path::new(path)) => read,
            _ = cancel.cancelled() => return Err("cancelled".to_string()),
        };
        let Some(_job) = image_job(cancel).await else {
            return Err("cancelled".to_string());
        };
        let threads = probed
            .as_ref()
            .map_or(crate::media_reads::decode_threads(), Probe::threads);
        count = tile_run(path, work, tiles, true, threads, cancel).await?;
        if count == 0 {
            // A file whose packets carry no key frame flags loses them all to the thinning
            count = tile_run(path, work, tiles, false, threads, cancel).await?;
        }
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
    threads: usize,
    cancel: &Cancel,
) -> Result<u32, String> {
    let threads = threads.to_string();
    let thinning = thin_key_frames();
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
    run_ffmpeg(&args, cancel).await.map_err(|e| {
        if thin && rejects_input_bsf(&e) {
            format!("{FFMPEG_TOO_OLD} ({e})")
        } else {
            e
        }
    })?;
    let listed = fs::read_to_string(&frames)
        .await
        .map_err(|e| format!("could not read {}: {e}", frames.display()))?;
    let count = listed.lines().filter(|l| !l.starts_with('#')).count();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Makes the missing tile sets of the videos in the libraries `library_ids`, up to `image_jobs`
/// at a time, and returns how many it made. A video that fails is logged and recorded with its
/// file's version; it is skipped until the file changes. Once `cancel` fires it starts no more
/// videos and the running ones are stopped. A database error, or ffmpeg too old for the thinning
/// (`FFMPEG_TOO_OLD`, recorded against no video), ends the run once the running videos end.
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
    // After an error no more videos start, and the running ones end before it is returned, so
    // none is stopped while it swaps its tile set in
    let mut error: Option<String> = None;
    loop {
        while error.is_none() && running.len() < image_jobs() && !cancel.is_cancelled() {
            let next = async {
                if queue.is_empty() {
                    // The videos being made have no tile set yet, so they are skipped like
                    // failures
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
                    return Ok(None);
                };
                let version = file_version(&path).await;
                let known = get_failure(pool, item_id, FAILURE_KIND).await.map_err(db)?;
                Ok::<_, String>(Some((item_id, path, version, known == Some(version))))
            }
            .await;
            let (item_id, path, version, known) = match next {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            };
            if known {
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
        let recorded = async {
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
                // Not the video's fault: nothing is recorded, so it is tried once ffmpeg is newer
                Err(e) if e.starts_with(FFMPEG_TOO_OLD) => return Err(e),
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
            Ok(())
        }
        .await;
        if let Err(e) = recorded {
            error.get_or_insert(e);
        }
    }
    error.map_or(Ok(made), Err)
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
            let count = tile_run(&clip, &dir, &tiles, thin, 1, &Cancel::new())
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

    /// Whether ffmpeg has the encoder `name`.
    fn have_encoder(name: &str) -> bool {
        std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(name))
    }

    /// 95 s at 10 fps whose brightness tells the time (luma 16 + frame / 5), with a key frame
    /// every 2 s, encoded with `encoder` and `params` into `dir/<encoder>-<name>`, with the
    /// `container` options.
    fn timed_clip(
        dir: &Path,
        encoder: &str,
        params: &[&str],
        container: &[&str],
        name: &str,
    ) -> PathBuf {
        let source = "nullsrc=size=320x180:rate=10:duration=95,geq=lum='16+N/5':cb=128:cr=128";
        let clip = dir.join(format!("{encoder}-{name}"));
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                source,
            ])
            .args(["-c:v", encoder, "-pix_fmt", "yuv420p"])
            .args(params)
            .args(container)
            .arg(&clip)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "ffmpeg could not write {}",
            clip.display()
        );
        clip
    }

    /// The tile images in `tiles` that hold `count` thumbnails, decoded once: a debug build
    /// takes about a second over each.
    fn tile_images(tiles: &Path, count: u32) -> Vec<image::GrayImage> {
        (0..count.div_ceil(TILE_SIDE * TILE_SIDE))
            .map(|t| {
                image::open(tiles.join(format!("{t}.jpg")))
                    .unwrap()
                    .to_luma8()
            })
            .collect()
    }

    /// The time each of the `count` thumbnails in `tiles` shows, from a `timed_clip`.
    fn thumbnail_times(tiles: &Path, count: u32) -> Vec<f64> {
        let per_tile = TILE_SIDE * TILE_SIDE;
        let images = tile_images(tiles, count);
        (0..count)
            .map(|k| {
                let tile = &images[(k / per_tile) as usize];
                // The mean luma of the thumbnail's middle, back from the JPEG's full range
                let cell = k % per_tile;
                let (x0, y0) = (
                    (cell % TILE_SIDE) * 320 + 120,
                    (cell / TILE_SIDE) * 180 + 60,
                );
                let mut sum = 0.0;
                for y in y0..y0 + 60 {
                    for x in x0..x0 + 80 {
                        sum += f64::from(tile.get_pixel(x, y)[0]);
                    }
                }
                let luma = sum / (60.0 * 80.0) * 219.0 / 255.0;
                luma * 5.0 / 10.0
            })
            .collect()
    }

    #[tokio::test]
    async fn r3trickplay_seeking_shows_the_same_frames() {
        let test = "r3trickplay_seeking_shows_the_same_frames";
        if !have_ffmpeg(test) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-seek-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let x264 = ["-x264-params", "keyint=20:min-keyint=20:scenecut=0"];
        // x265 makes open GOPs by default: key frames that need the frames before them in order
        let x265 = [
            "-x265-params",
            "keyint=20:min-keyint=20:scenecut=0:log-level=error",
        ];
        let mut checked = 0;
        for (encoder, params, name) in [
            ("libx264", &x264, "seek.mkv"),
            ("libx264", &x264, "seek.mp4"),
            ("libx265", &x265, "seek.mkv"),
        ] {
            if !have_encoder(encoder) {
                eprintln!("SKIPPED {test} for {encoder}: ffmpeg can't encode with it");
                continue;
            }
            let clip = timed_clip(&dir, encoder, params, &[], name);
            let clip = clip.to_string_lossy();
            let mut made = Vec::new();
            for reading in [Reading::Stream, Reading::Seek] {
                let work = dir.join(format!("work-{encoder}-{name}-{reading:?}"));
                let tiles = work.join("tiles");
                std::fs::create_dir_all(&tiles).unwrap();
                let info = if reading == Reading::Stream {
                    // The old pass, with every key frame: what seeking must show
                    let count = tile_run(&clip, &work, &tiles, false, 1, &Cancel::new())
                        .await
                        .unwrap();
                    Ok(count)
                } else {
                    make_tiles(&clip, &work, &tiles, reading, &Cancel::new())
                        .await
                        .map(|i| i.thumbnail_count)
                };
                made.push(info.map(|count| (count, thumbnail_times(&tiles, count))));
            }
            let old = made[0].as_ref().unwrap();
            match &made[1] {
                Ok(new) => {
                    assert_eq!(old.0, new.0, "{encoder} {name}: the same count");
                    for (k, (a, b)) in old.1.iter().zip(&new.1).enumerate() {
                        assert!(
                            (a - b).abs() <= 1.0,
                            "{encoder} {name}: thumbnail {k} {a} {b}"
                        );
                    }
                }
                // Only where the key frames can't be decoded alone; never wrong frames
                Err(e) => {
                    assert_eq!(encoder, "libx265", "{name}: {e}");
                    assert!(e.contains("not 9 in order"), "{e}");
                }
            }
            checked += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
        if checked == 0 {
            eprintln!("SKIPPED {test}: no H.264 or HEVC encoder, so this test checked nothing");
        }
    }

    /// Each of the `count` thumbnails in `tiles` as two brightnesses, of its left and right
    /// quarters: on a `marked_clip`, they tell which frame it is.
    fn thumbnail_marks(tiles: &Path, count: u32) -> Vec<(f64, f64)> {
        let per_tile = TILE_SIDE * TILE_SIDE;
        let images = tile_images(tiles, count);
        (0..count)
            .map(|k| {
                let tile = &images[(k / per_tile) as usize];
                let cell = k % per_tile;
                let (x0, y0) = ((cell % TILE_SIDE) * 320, (cell / TILE_SIDE) * 180 + 60);
                let mean = |from: u32| {
                    let mut sum = 0.0;
                    for y in y0..y0 + 60 {
                        for x in x0 + from..x0 + from + 80 {
                            sum += f64::from(tile.get_pixel(x, y)[0]);
                        }
                    }
                    sum / (60.0 * 80.0)
                };
                (mean(40), mean(200))
            })
            .collect()
    }

    /// A clip at `rate` fps for `seconds`, H.264 with a key frame every `keyint` frames, whose
    /// left half's brightness is 16 + t / 5.5 and right half's 16 + (t mod 10) × 20, so each
    /// frame of it looks different. `filter` is added to the source, `options` to the output.
    fn marked_clip(
        dir: &Path,
        name: &str,
        (rate, seconds, keyint): (u32, u32, u32),
        filter: &str,
        options: &[&str],
    ) -> PathBuf {
        let source = format!(
            "nullsrc=size=320x180:rate={rate}:duration={seconds},\
             geq=lum='if(lt(X,W/2),16+T/5.5,16+mod(T,10)*20)':cb=128:cr=128{filter}"
        );
        let params = format!("keyint={keyint}:min-keyint={keyint}:scenecut=0");
        let clip = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                &source,
            ])
            .args([
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                &params,
            ])
            .args(options)
            .arg(&clip)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "ffmpeg could not write {}",
            clip.display()
        );
        clip
    }

    #[tokio::test]
    async fn r3trickplay_seeking_handles_many_tiles_late_starts_and_vfr() {
        let test = "r3trickplay_seeking_handles_many_tiles_late_starts_and_vfr";
        if !have_ffmpeg(test) {
            return;
        }
        if !have_encoder("libx264") {
            eprintln!("SKIPPED {test}: ffmpeg can't encode H.264, so this test checked nothing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-seek2-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let clips = [
            // 1,105 s at 1 fps: 111 thumbnails, so a full tile and one with 11
            marked_clip(&dir, "long.mkv", (1, 1105, 2), "", &[]),
            // Starts at 1,000 s, as a recording cut from a broadcast does
            marked_clip(
                &dir,
                "late.mkv",
                (10, 95, 20),
                "",
                &["-output_ts_offset", "1000"],
            ),
            // Variable frame rate: 5 of every 30 frames dropped, the times kept
            marked_clip(
                &dir,
                "vfr.mkv",
                (10, 95, 20),
                ",select='not(between(mod(n\\,30)\\,10\\,14))'",
                &["-fps_mode", "vfr"],
            ),
        ];
        for clip in &clips {
            let name = clip.file_name().unwrap().to_string_lossy().into_owned();
            let clip = clip.to_string_lossy();
            let mut made = Vec::new();
            for reading in [Reading::Stream, Reading::Seek] {
                let work = dir.join(format!("work-{name}-{reading:?}"));
                let tiles = work.join("tiles");
                std::fs::create_dir_all(&tiles).unwrap();
                let count = if reading == Reading::Stream {
                    // The pass with every key frame, whose frames seeking picks
                    tile_run(&clip, &work, &tiles, false, 1, &Cancel::new())
                        .await
                        .unwrap()
                } else {
                    let info = make_tiles(&clip, &work, &tiles, reading, &Cancel::new())
                        .await
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    let tile_count = std::fs::read_dir(&tiles).unwrap().count();
                    assert_eq!(tile_count as u32, info.tile_count(), "{name}");
                    info.thumbnail_count
                };
                made.push((count, thumbnail_marks(&tiles, count)));
            }
            assert_eq!(made[0].0, made[1].0, "{name}: the same count");
            for (k, (a, b)) in made[0].1.iter().zip(&made[1].1).enumerate() {
                // A frame 2 s on is 40 levels brighter on the right; 10 s on, 1.8 on the left
                assert!(
                    (a.0 - b.0).abs() <= 1.2 && (a.1 - b.1).abs() <= 8.0,
                    "{name}: thumbnail {k} differs: {a:?} {b:?}"
                );
            }
        }
        assert_eq!(
            thumbnail_marks(&dir.join("work-long.mkv-Seek/tiles"), 111).len(),
            111
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3trickplay_thumbnails_come_from_their_interval() {
        let test = "r3trickplay_thumbnails_come_from_their_interval";
        if !have_ffmpeg(test) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-times-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Clips whose brightness tells the time; the MPEG-TS copies start at 1000 s, as
        // broadcast recordings start late
        let codecs = [
            (
                "libx264",
                vec!["-x264-params", "keyint=20:min-keyint=20:scenecut=0"],
            ),
            (
                "libx265",
                vec![
                    "-x265-params",
                    "keyint=20:min-keyint=20:scenecut=0:log-level=error",
                ],
            ),
        ];
        let mut checked = 0;
        for (encoder, params) in &codecs {
            if !have_encoder(encoder) {
                eprintln!("SKIPPED {test} for {encoder}: ffmpeg can't encode with it");
                continue;
            }
            for (name, container) in [
                ("plain.mkv", &[][..]),
                ("late.ts", &["-output_ts_offset", "1000"][..]),
            ] {
                let clip = timed_clip(&dir, encoder, params, container, name);
                let tiles = dir.join(format!("tiles-{encoder}-{name}"));
                std::fs::create_dir_all(&tiles).unwrap();
                let count = tile_run(
                    &clip.to_string_lossy(),
                    &dir,
                    &tiles,
                    true,
                    1,
                    &Cancel::new(),
                )
                .await
                .unwrap();
                assert_eq!(count, 9, "{encoder} {name}");
                for (k, seconds) in thumbnail_times(&tiles, count).into_iter().enumerate() {
                    let nominal = k as f64 * f64::from(INTERVAL_MS) / 1000.0;
                    // Half an interval, and a second for the JPEG's rounding
                    assert!(
                        (seconds - nominal).abs() <= 6.0,
                        "{encoder} {name}: thumbnail {k} is from {seconds:.1} s"
                    );
                }
                checked += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        if checked == 0 {
            eprintln!("SKIPPED {test}: no H.264 or HEVC encoder, so this test checked nothing");
        }
    }

    #[test]
    fn r3trickplay_names_the_ffmpeg_version_it_needs() {
        // What FFmpeg 4.4 to 6.1 print for `-bsf:v` before an input
        let old = "ffmpeg failed: Option bsf:v (A comma-separated list of bitstream filters) \
                   cannot be applied to input url /media/a.mkv -- you are trying to apply an \
                   input option to an output file or vice versa. Move this option before the \
                   file it belongs to.";
        assert!(rejects_input_bsf(old));
        assert!(!rejects_input_bsf(
            "ffmpeg failed: Invalid data found when processing input"
        ));
        assert!(FFMPEG_TOO_OLD.contains("FFmpeg 7.0"));
    }

    #[tokio::test]
    async fn r3trickplay_sweep_waits_for_running_jobs() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let dir = std::env::temp_dir().join(format!("r3trickplay-sweep-{}", Uuid::new_v4()));
        let job = image_job(&Cancel::new()).await.unwrap();
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
