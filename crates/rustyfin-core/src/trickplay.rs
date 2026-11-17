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

use rustyfin_db::busy::retry_busy_until;
use sqlx::SqlitePool;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::{RwLock, RwLockReadGuard, Semaphore, SemaphorePermit, watch};
use tokio::task::JoinSet;
use tracing::{info, warn};
use uuid::Uuid;

use rustyfin_model::trickplay::TrickplayInfo;

use crate::key_frames::{self, Codec, KeyFrameStream};
use crate::mkv_index::{self, MkvIndex};

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
/// is replaced only once the new one is complete. Waits for an `image_job` (`make_tiles`). Tiles
/// are decoded on a device as `decoding` says.
pub async fn generate(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
    decoding: &TileDecoding,
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

    let result = make_tiles(path, &work, &tiles, Reading::Auto, decoding, cancel).await;
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

    retry_busy_until(
        "saving a trickplay tile set",
        || cancel.cancelled(),
        || rustyfin_db::repo_trickplay::save_trickplay(pool, item_id, &made),
    )
    .await
    .map_err(|e| format!("{SAVE_FAILED}: {e}"))?;
    Ok(made)
}

/// The start of the error when the tile set was made but the database would not take it. That
/// is no fault of the video, so it is tried again on the next run.
pub const SAVE_FAILED: &str = "could not save the tile set";

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
#[derive(Clone, Debug)]
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
    /// Where the video's key frames can be read as a raw stream (`key_frames`): H.264 or HEVC,
    /// and not rotated, as a raw stream can't say so.
    raw: Option<RawVideo>,
}

/// What a raw stream of a video's key frames needs from its container.
#[derive(Clone, Debug)]
struct RawVideo {
    codec: Codec,
    /// The decoder configuration (`avcC`, `hvcC`).
    config: Vec<u8>,
    /// The container's sample aspect ratio, as `n/d`, which the raw stream may not carry.
    sar: Option<String>,
    /// The container's cropping (Matroska PixelCrop, MP4 `clap`), in pixels from the top,
    /// bottom, left and right, which ffmpeg applies to the file but can't see in a raw stream.
    crop: Option<[u64; 4]>,
    /// A Matroska file's Cues, which find the key frames without ffprobe.
    index: Option<Arc<MkvIndex>>,
    /// More than 8 bits a sample.
    deep: bool,
    /// The device that decodes it, where the admin chose one (`TileDecoding`).
    vaapi: Option<Arc<Vaapi>>,
}

impl RawVideo {
    /// The filters that give a raw stream's frames what the container would have: its
    /// cropping, as ffmpeg's own applies it, then its aspect ratio. Each ends with a comma.
    fn container_filters(&self) -> String {
        let mut filters = String::new();
        if let Some([top, bottom, left, right]) = self.crop {
            filters.push_str(&format!(
                "crop=w=iw-{left}-{right}:h=ih-{top}-{bottom}:x={left}:y={top},"
            ));
        }
        if let Some(sar) = &self.sar {
            filters.push_str(&format!("setsar={sar},"));
        }
        filters
    }
}

/// How seek mode reads a tile's key frames.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Seeking {
    /// One ffprobe opening the file once and seeking to each key frame, its packets piped to one
    /// ffmpeg as a raw stream.
    Raw,
    /// One ffmpeg through the concat demuxer, opening the file again for each key frame.
    Concat,
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

    /// How seek mode reads this video, where seeking to each thumbnail's key frame reads at most
    /// a third of what reading the whole file does; `None` where it doesn't. Each opening of the
    /// file reads its header and its index (at the end, after the media, in some MP4 files),
    /// and each seek about a second of media around the key frame. `Raw` opens the file once per
    /// tile, `Concat` once per thumbnail.
    fn seeking(&self) -> Option<Seeking> {
        #[cfg(test)]
        if tests::SEEK_PAYS.get() {
            return Some(if self.raw.is_some() {
                Seeking::Raw
            } else {
                Seeking::Concat
            });
        }
        [Seeking::Raw, Seeking::Concat]
            .into_iter()
            .find(|&how| self.seek_pays(how))
    }

    fn seek_pays(&self, how: Seeking) -> bool {
        let count = self.count();
        if !self.indexed || count == 0 || self.duration <= 0.0 || self.size <= 0.0 {
            return false;
        }
        let openings = match how {
            Seeking::Raw if self.raw.is_none() => return false,
            Seeking::Raw => count.div_ceil(TILE_SIDE * TILE_SIDE),
            Seeking::Concat => count,
        };
        let rate = self.size / self.duration;
        let after_last_key = (self.duration - (self.last_key - self.start)).max(0.0) * rate;
        let index = (self.size - self.last_pos - after_last_key).max(0.0);
        let read =
            f64::from(openings) * (self.first_pos + index) + f64::from(count) * (SEEK_SLACK + rate);
        self.size >= 3.0 * read
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
            "-show_data",
            "-show_entries",
            "format=start_time,duration,size,format_name:stream=codec_name,avg_frame_rate,\
             width,height,pix_fmt,sample_aspect_ratio,extradata:stream_side_data=rotation,crop_top,\
             crop_bottom,crop_left,crop_right:packet=pts_time,pos",
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
    let side_data = stream["side_data_list"].as_array();
    let rotated = side_data.is_some_and(|list| {
        list.iter()
            .any(|d| d["rotation"].as_i64().is_some_and(|r| r % 360 != 0))
    });
    let crop = side_data
        .and_then(|list| list.iter().find(|d| d.get("crop_top").is_some()))
        .map(|d| {
            ["crop_top", "crop_bottom", "crop_left", "crop_right"]
                .map(|side| d[side].as_u64().unwrap_or_default())
        })
        .filter(|crop| crop.iter().any(|&px| px > 0));
    let sar = stream["sample_aspect_ratio"]
        .as_str()
        .and_then(|r| r.split_once(':'))
        .and_then(|(n, d)| Some((n.parse::<u32>().ok()?, d.parse::<u32>().ok()?)))
        .filter(|&(n, d)| n > 0 && d > 0)
        .map(|(n, d)| format!("{n}/{d}"));
    let raw = stream["codec_name"]
        .as_str()
        .and_then(Codec::from_name)
        .filter(|_| !rotated)
        .map(|codec| RawVideo {
            codec,
            config: key_frames::hex_dump(stream["extradata"].as_str().unwrap_or_default()),
            sar,
            crop,
            index: None,
            deep: stream["pix_fmt"]
                .as_str()
                .is_some_and(|f| ["p10", "p12", "p14", "p16"].iter().any(|d| f.contains(d))),
            vaapi: None,
        })
        .filter(|raw| KeyFrameStream::new(raw.codec, &raw.config).is_some());
    let mut raw = raw;
    if let Some(raw) = raw.as_mut()
        && format_name.contains("matroska")
    {
        let file = PathBuf::from(path);
        raw.index = tokio::task::spawn_blocking(move || MkvIndex::read(&file))
            .await
            .ok()
            .flatten()
            .map(Arc::new);
    }
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
            raw,
        })
    })();
    Ok(probe)
}

/// Makes the tile images of `path` in `tiles` by seeking to each thumbnail's key frame, a tile
/// image at a time (`seek_tile`), read as `how` says. The key frame of a thumbnail is the last
/// one before the middle of its interval, as the unthinned streaming pass's fps filter picked
/// (the thinned pass takes the first key frame of the interval instead; both are within half an
/// interval). Tiles run `SEEK_TILES_AT_ONCE` at once, and `seek_tile_slots` over all videos. They
/// are made in `work` and moved into `tiles` once all are right. Returns the thumbnail count.
/// Fails where the decoder doesn't hand out exactly one frame per key frame in order, and with
/// `key_frames::NOT_ALONE` where `Raw` meets a key frame it can't decode alone.
async fn seek_tiles(
    path: &str,
    probe: &Probe,
    how: Seeking,
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
            let raw = match how {
                Seeking::Raw => probe.raw.clone(),
                Seeking::Concat => None,
            };
            running.spawn(async move {
                let tile = Tile {
                    path: &path,
                    index: next,
                    work: &work,
                    tiles: &tiles,
                };
                seek_tile(tile, start, slots, raw, &cancel).await
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

/// A tile image that `seek_tile` makes: of video `path`, number `index`, in folder `tiles`,
/// with `work` for lists.
struct Tile<'a> {
    path: &'a str,
    index: u32,
    work: &'a Path,
    tiles: &'a Path,
}

/// Makes tile image `tile` of the thumbnails `slots` for `seek_tiles`, of a video whose time
/// line starts at `start`: from a raw stream of its key frames where `raw` holds the video's
/// codec and decoder configuration, else through the concat demuxer.
async fn seek_tile(
    tile: Tile<'_>,
    start: f64,
    slots: std::ops::Range<u32>,
    raw: Option<RawVideo>,
    cancel: &Cancel,
) -> Result<(), String> {
    let _slot = tokio::select! {
        slot = seek_tile_slots().acquire() => slot.expect("the semaphore is never closed"),
        _ = cancel.cancelled() => return Err("cancelled".to_string()),
    };
    let interval = f64::from(INTERVAL_MS) / 1000.0;
    // Just before the middle: a key frame right on it is the next thumbnail's to the fps
    // filter. A millisecond, as Matroska's times are in milliseconds and a seek rounds to them.
    let targets: Vec<f64> = slots
        .map(|k| start + f64::from(k) * interval + interval / 2.0 - 1e-3)
        .collect();
    let intervals: Vec<String> = targets.iter().map(|t| format!("{t:.6}%+#1")).collect();
    let intervals = intervals.join(",");
    match raw {
        Some(raw) => {
            if let Some(index) = &raw.index {
                let from = PacketSource::Index {
                    index,
                    targets: &targets,
                };
                match raw_tile(&tile, from, targets.len(), &raw, cancel).await {
                    Err(e) if e.starts_with(mkv_index::INDEX_FAILED) => {
                        info!(path = tile.path, error = %e, "trickplay asks ffprobe for the key frames");
                    }
                    done => return done,
                }
            }
            let from = PacketSource::Probe(&intervals);
            raw_tile(&tile, from, targets.len(), &raw, cancel).await
        }
        None => concat_tile(&tile, &intervals, targets.len(), cancel).await,
    }
}

/// How many tiles are decoded on a device at once over all videos: one per image job.
fn device_tile_slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(image_jobs() + 1))
}

/// How many tiles may fail on the device before a run makes the rest in software: one.
const HW_FAILURES_MOST: usize = 1;

/// Whether trickplay decodes tiles on a device: as the transcoder does, by the encoding settings
/// in effect (`encoding::current`: the administrator's choice, or the detected device while the
/// type is automatic), VA-API only (its device, the codecs it decodes and the 10-bit HEVC
/// switch), where ffmpeg has the VA-API hwaccel and scaler. Read once a run. Tiles decoded on
/// a device show the same frames as software ones but aren't byte-identical: the device's
/// decoder doesn't skip the loop filter.
#[derive(Clone, Debug, Default)]
pub struct TileDecoding {
    vaapi: Option<Arc<Vaapi>>,
}

/// The VA-API device tiles are decoded on, and the codecs the admin lets it decode.
#[derive(Debug)]
struct Vaapi {
    device: String,
    codecs: Vec<String>,
    hevc_10bit: bool,
    failures: std::sync::atomic::AtomicUsize,
}

impl TileDecoding {
    /// The encoding settings in effect.
    pub async fn current() -> Self {
        Self::from_settings(crate::encoding::current()).await
    }

    async fn from_settings(settings: crate::encoding::EncodingSettings) -> Self {
        if settings.hardware_acceleration != crate::encoding::HardwareAccelerationType::Vaapi {
            return Self::default();
        }
        let caps = crate::encoding::probe_capabilities(Path::new("ffmpeg")).await;
        if !caps.has_hwaccel("vaapi") || !caps.has_filter("scale_vaapi") {
            warn!("ffmpeg has no VA-API hwaccel or scaler: trickplay decodes in software");
            return Self::default();
        }
        Self {
            vaapi: Some(Arc::new(Vaapi {
                device: settings.vaapi_device,
                codecs: settings.hardware_decoding_codecs,
                hevc_10bit: settings.enable_decoding_color_depth_10_hevc,
                failures: Default::default(),
            })),
        }
    }

    /// The device for a video in `codec`, more than 8 bits a sample where `deep`, if the
    /// admin lets it decode that: as the transcoder, never 10-bit H.264, and 10-bit HEVC only
    /// with its switch.
    fn device_for(&self, codec: Codec, deep: bool) -> Option<Arc<Vaapi>> {
        let vaapi = self.vaapi.as_ref()?;
        let allowed = vaapi.codecs.iter().any(|c| c == codec.format())
            && !(deep && codec == Codec::H264)
            && !(deep && codec == Codec::Hevc && !vaapi.hevc_10bit);
        allowed.then(|| vaapi.clone())
    }
}

impl Vaapi {
    /// Whether tiles still go to the device this run: not once one failed there.
    fn usable(&self) -> bool {
        self.failures.load(std::sync::atomic::Ordering::Relaxed) < HW_FAILURES_MOST
    }

    fn failed(&self, error: &str) {
        let failed = 1 + self
            .failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if failed == HW_FAILURES_MOST {
            warn!(device = %self.device, error, "trickplay stops decoding on the device this run");
        }
    }

    /// The input arguments that open the device and decode on it, as the transcoder writes
    /// them.
    fn input_args(&self) -> Vec<String> {
        [
            "-init_hw_device",
            &format!("vaapi=va:{}", self.device),
            "-filter_hw_device",
            "va",
            "-hwaccel",
            "vaapi",
            "-hwaccel_output_format",
            "vaapi",
            "-hwaccel_device",
            "va",
        ]
        .map(String::from)
        .to_vec()
    }

    /// The filters from the device's frames to software ones `WIDTH` wide: made 8-bit on the
    /// device and scaled there to 3 times `WIDTH`, brought down, then scaled as in software.
    /// The device's scaler alone (`scale_vaapi`, whose modes the driver may ignore) leaves fine
    /// detail such as credits jagged going straight to `WIDTH`; brought down whole, a 2160p
    /// frame takes more memory. A container that crops has its frames brought down whole and
    /// cropped and scaled as in software.
    fn filters(raw: &RawVideo) -> String {
        let device = if raw.crop.is_some() {
            "scale_vaapi=format=nv12".to_string()
        } else {
            format!("scale_vaapi=w='min(iw,{})':h=-2:format=nv12", 3 * WIDTH)
        };
        format!(
            "{device},hwdownload,format=nv12,{}scale={WIDTH}:-2",
            raw.container_filters()
        )
    }
}

/// Where `raw_tile` gets a tile's key frame packets.
#[derive(Clone, Copy)]
enum PacketSource<'a> {
    /// From ffprobe, which seeks to each of these intervals (`-read_intervals`).
    Probe(&'a str),
    /// From the file's Matroska index: the key frame each of the times seeks to.
    Index {
        index: &'a Arc<MkvIndex>,
        targets: &'a [f64],
    },
}

/// The ffmpeg arguments that decode one input into tile image `tile`, and list each frame
/// it decoded in `frames` (framecrc), after the input arguments `input`. Frames go through the
/// filters `chain`, which end with them `WIDTH` wide, before they are laid out.
fn tile_args(input: Vec<String>, chain: &str, tile: &Tile<'_>, frames: &Path) -> Vec<String> {
    let output = tile.tiles.join(format!("{}.jpg", tile.index));
    let graph = format!("[0:V:0]{chain},split[t][n];[t]tile={TILE_SIDE}x{TILE_SIDE}[tiles]");
    let mut args = input;
    args.extend(
        [
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
        ]
        .map(String::from),
    );
    args
}

/// The presentation times of the frames ffmpeg listed in `frames` (framecrc).
async fn frame_times(frames: &Path) -> Result<Vec<i64>, String> {
    // framecrc lines: stream, dts, pts, duration, size, checksum
    let listed = fs::read_to_string(frames)
        .await
        .map_err(|e| format!("could not read {}: {e}", frames.display()))?;
    Ok(listed
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split(',').nth(2)?.trim().parse().ok())
        .collect())
}

/// `seek_tile` from a raw stream: the `wanted` key frames' packets come `from` the Matroska
/// index or from ffprobe, and a `KeyFrameStream` turns them into the input of one ffmpeg. The
/// order is the stream's own (see `key_frames`), so this checks that each key frame gave one
/// frame, and that the decoder met no error: ffmpeg conceals a damaged slice and still hands
/// out a frame, so a decode error stops it (`-xerror`) and any error it prints fails the tile
/// (`pipe_key_frames`). Where the video has a device (`RawVideo::vaapi`), it decodes and scales
/// the tile first; any failure there is counted against the device and the tile made again in
/// software.
async fn raw_tile(
    tile: &Tile<'_>,
    from: PacketSource<'_>,
    wanted: usize,
    raw: &RawVideo,
    cancel: &Cancel,
) -> Result<(), String> {
    if let Some(vaapi) = raw.vaapi.as_deref().filter(|v| v.usable()) {
        // One tile on the device per image job: it decodes one at a time anyway, and each
        // ffmpeg on it holds more memory than one in software
        let _slot = tokio::select! {
            slot = device_tile_slots().acquire() => slot.expect("the semaphore is never closed"),
            _ = cancel.cancelled() => return Err("cancelled".to_string()),
        };
        match raw_tile_on(tile, from, wanted, raw, Some(vaapi), cancel).await {
            Ok(()) => return Ok(()),
            Err(e) if cancel.is_cancelled() => return Err(e),
            Err(e) => {
                vaapi.failed(&e);
                info!(path = tile.path, error = %e, "trickplay decodes the tile in software");
            }
        }
    }
    raw_tile_on(tile, from, wanted, raw, None, cancel).await
}

/// `raw_tile` on `vaapi`'s device, or in software.
async fn raw_tile_on(
    tile: &Tile<'_>,
    from: PacketSource<'_>,
    wanted: usize,
    raw: &RawVideo,
    vaapi: Option<&Vaapi>,
    cancel: &Cancel,
) -> Result<(), String> {
    let stream = KeyFrameStream::new(raw.codec, &raw.config)
        .ok_or("the decoder configuration can't be read")?;
    let frames = tile.work.join(format!("seek-{}.crc", tile.index));
    let mut input = vec!["-xerror".to_string()];
    let chain = match vaapi {
        Some(vaapi) => {
            input.extend(vaapi.input_args());
            Vaapi::filters(raw)
        }
        None => {
            input.extend(["-skip_loop_filter", "all"].map(String::from));
            format!("{}scale={WIDTH}:-2", raw.container_filters())
        }
    };
    input.extend(["-threads", "1", "-f", raw.codec.format(), "-i", "pipe:0"].map(String::from));
    let ffmpeg_args = tile_args(input, &chain, tile, &frames);
    let fed = pipe_key_frames(from, tile.path, &ffmpeg_args, stream, cancel).await?;
    if fed != wanted {
        return Err(format!("ffprobe found {fed} of {wanted} key frames"));
    }
    let times = frame_times(&frames).await?;
    if times.len() != wanted {
        return Err(format!(
            "the decoder gave {} frames, not {wanted}, for tile {}",
            times.len(),
            tile.index
        ));
    }
    Ok(())
}

/// Turns key frame packets into ffmpeg's input, through a `KeyFrameStream`.
struct Feed {
    stream: KeyFrameStream,
    input: tokio::process::ChildStdin,
    out: Vec<u8>,
    fed: usize,
}

impl Feed {
    async fn send(&mut self, packet: &[u8]) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;
        self.out.clear();
        self.stream.push(packet, &mut self.out)?;
        self.fed += 1;
        self.input
            .write_all(&self.out)
            .await
            .map_err(|e| format!("ffmpeg stopped reading the key frames: {e}"))
    }

    /// Sends the key frames ffprobe prints (`-show_data`) with `probe_args`. Dropping it kills
    /// ffprobe.
    async fn from_probe(&mut self, probe_args: &[&str]) -> Result<(), String> {
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut probe = tool("ffprobe");
        probe.args(probe_args).stdin(Stdio::null());
        let mut probe = probe
            .spawn()
            .map_err(|e| format!("ffprobe could not start: {e}"))?;
        let packets = probe.stdout.take().ok_or("ffprobe has no pipe")?;
        let read = async {
            let mut lines = BufReader::with_capacity(1 << 16, packets);
            // One buffer for every line: a 4K key frame takes thousands
            let mut line = Vec::with_capacity(128);
            let (mut packet, mut key, mut seen) = (Vec::new(), false, false);
            // Each packet: flags=K__, then data=, its hex dump and an empty line
            loop {
                line.clear();
                let read = lines
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(|e| e.to_string())?;
                let text = line.trim_ascii_end();
                let next = text.strip_prefix(b"flags=");
                if (read == 0 || next.is_some()) && seen {
                    if !key {
                        return Err("ffprobe landed on a frame that isn't a key frame".to_string());
                    }
                    self.send(&packet).await?;
                    packet.clear();
                }
                if read == 0 {
                    return Ok(());
                }
                if let Some(flags) = next {
                    key = flags.starts_with(b"K");
                    seen = true;
                } else if !text.is_empty()
                    && text != b"data="
                    && !key_frames::hex_line(text, &mut packet)
                {
                    return Err(format!(
                        "ffprobe printed {:?}",
                        String::from_utf8_lossy(text)
                    ));
                }
            }
        };
        tokio::try_join!(read, async {
            exited("ffprobe", probe.wait_with_output().await, false)
        })
        .map(drop)
    }

    /// Sends the key frame each of `targets` seeks to in `index`, from the file at `path`, each
    /// read while the one before goes to ffmpeg.
    async fn from_index(
        &mut self,
        index: &Arc<MkvIndex>,
        path: &str,
        targets: &[f64],
    ) -> Result<(), String> {
        let file = std::fs::File::open(path)
            .map(Arc::new)
            .map_err(|e| format!("{}: {e}", mkv_index::INDEX_FAILED))?;
        let read = |target: f64| {
            let (index, file) = (index.clone(), file.clone());
            tokio::task::spawn_blocking(move || match index.pick(target) {
                Some(cue) => index.frame(&file, cue),
                // Before the first cue or past the last, ffmpeg doesn't seek by the index
                None => Err(format!("{} at {target:.3} s", mkv_index::INDEX_FAILED)),
            })
        };
        let mut targets = targets.iter();
        let mut next = targets.next().map(|&t| read(t));
        while let Some(reading) = next.take() {
            let packet = reading.await.map_err(|e| e.to_string())??;
            next = targets.next().map(|&t| read(t));
            self.send(&packet).await?;
        }
        Ok(())
    }
}

/// A `run_tool`-like command for `program` whose output is piped.
fn tool(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(["-hide_banner", "-loglevel", "error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::media_reads::lower_priority(&mut cmd);
    cmd
}

/// `Ok` where `program` exited well, and where `quiet`, printed nothing (it runs at the error
/// log level).
fn exited(
    program: &str,
    output: std::io::Result<std::process::Output>,
    quiet: bool,
) -> Result<(), String> {
    let output = output.map_err(|e| format!("{program} failed: {e}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        Err(format!("{program} failed: {}", stderr.trim()))
    } else if quiet && !stderr.trim().is_empty() {
        Err(format!("{program} met an error: {}", stderr.trim()))
    } else {
        Ok(())
    }
}

/// Runs ffmpeg with `ffmpeg_args`, which reads the key frame packets of the video at `path`
/// that come `from` the index or ffprobe, as `stream` turns them out. ffmpeg and ffprobe run as
/// `run_tool` runs one, and fail as it does; ffmpeg fails too where it printed an error. Returns
/// how many packets went through.
async fn pipe_key_frames(
    from: PacketSource<'_>,
    path: &str,
    ffmpeg_args: &[String],
    stream: KeyFrameStream,
    cancel: &Cancel,
) -> Result<usize, String> {
    let mut ffmpeg = tool("ffmpeg");
    ffmpeg
        .args(["-nostdin", "-y"])
        .args(ffmpeg_args)
        .stdin(Stdio::piped());
    let mut ffmpeg = ffmpeg
        .spawn()
        .map_err(|e| format!("ffmpeg could not start: {e}"))?;
    let input = ffmpeg.stdin.take().ok_or("ffmpeg has no pipe")?;
    let mut feed = Feed {
        stream,
        input,
        out: Vec::new(),
        fed: 0,
    };
    let feeding = async {
        match from {
            PacketSource::Probe(intervals) => {
                let probe_args = [
                    "-probesize",
                    PROBE_BYTES,
                    "-analyzeduration",
                    "0",
                    "-select_streams",
                    // The first video that isn't cover art, as ffmpeg's 0:V:0
                    "V:0",
                    "-read_intervals",
                    intervals,
                    "-show_data",
                    "-show_entries",
                    "packet=flags,data",
                    "-of",
                    "default=noprint_wrappers=1",
                    path,
                ];
                feed.from_probe(&probe_args).await?;
            }
            PacketSource::Index { index, targets } => {
                feed.from_index(index, path, targets).await?;
            }
        }
        // ffmpeg's input ends here
        let Feed { fed, input, .. } = feed;
        drop(input);
        Ok::<_, String>(fed)
    };
    let both = async {
        // The first to fail drops the other, which kills ffprobe and ffmpeg (kill_on_drop)
        tokio::try_join!(feeding, async {
            exited("ffmpeg", ffmpeg.wait_with_output().await, true)
        })
    };
    tokio::select! {
        out = tokio::time::timeout(FFMPEG_TIMEOUT, both) => match out {
            Ok(out) => out.map(|(fed, ())| fed),
            Err(_) => Err("ffprobe and ffmpeg took too long and were stopped".to_string()),
        },
        _ = cancel.cancelled() => Err("cancelled".to_string()),
    }
}

/// `seek_tile` through the concat demuxer: one ffprobe run finds each of the `wanted` key
/// frames (`intervals`), and one ffmpeg run reads and decodes just those, opening the file
/// again for each. Fails where the decoder doesn't hand out exactly one frame per key frame in
/// order (open-GOP HEVC can't decode its key frames alone that way).
async fn concat_tile(
    tile: &Tile<'_>,
    intervals: &str,
    wanted: usize,
    cancel: &Cancel,
) -> Result<(), String> {
    let (path, index) = (tile.path, tile.index);
    let interval = f64::from(INTERVAL_MS) / 1000.0;
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
            intervals,
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
    if keys.len() != wanted {
        return Err(format!(
            "ffprobe found {} of {wanted} key frames",
            keys.len()
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
    let list_path = tile.work.join(format!("seek-{index}.ffconcat"));
    fs::write(&list_path, list)
        .await
        .map_err(|e| format!("could not write {}: {e}", list_path.display()))?;
    let frames = tile.work.join(format!("seek-{index}.crc"));
    let input = [
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
    ]
    .map(String::from)
    .to_vec();
    let args = tile_args(input, &format!("scale={WIDTH}:-2"), tile, &frames);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run_ffmpeg(&args, cancel).await?;
    let times = frame_times(&frames).await?;
    if times.len() != keys.len() || times.windows(2).any(|w| w[0] >= w[1]) {
        return Err(format!(
            "the decoder gave {} frames, not {} in order, for tile {index}",
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
    /// Seek as given, failing where that can't make the thumbnails.
    #[cfg(test)]
    Seek(Seeking),
    /// Read the whole file.
    #[cfg(test)]
    Stream,
}

/// Makes the tile images of `path` in `tiles`, using `work` for lists, and returns the set's
/// info. A video in a container with an index (Matroska, MP4) that seeking reads much less of is
/// read a key frame at a time (`seek_tiles`), from a raw stream where its codec allows, else
/// through the concat demuxer, and through that too where a key frame doesn't decode alone in
/// a raw stream. Others, and any that seeking fails on, are read whole (`tile_run`). All make
/// the same thumbnails.
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
    decoding: &TileDecoding,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    let mut probed = probe(path, cancel).await?;
    if let Some(raw) = probed.as_mut().and_then(|p| p.raw.as_mut()) {
        raw.vaapi = decoding.device_for(raw.codec, raw.deep);
    }
    let mut count = 0;
    let seek = probed.as_ref().and_then(|p| match reading {
        Reading::Auto => Some((p, p.seeking()?)),
        #[cfg(test)]
        Reading::Seek(how) => {
            (p.indexed && (how == Seeking::Concat || p.raw.is_some())).then_some((p, how))
        }
        #[cfg(test)]
        Reading::Stream => None,
    });
    if let Some((p, how)) = seek {
        let Some(_job) = image_job(cancel).await else {
            return Err("cancelled".to_string());
        };
        let mut result = seek_tiles(path, p, how, work, tiles, cancel).await;
        if let Err(e) = &result
            && how == Seeking::Raw
            && e.starts_with(key_frames::NOT_ALONE)
            && reading == Reading::Auto
            && p.seek_pays(Seeking::Concat)
        {
            info!(path, error = %e, "trickplay seeks through the concat demuxer instead");
            result = seek_tiles(path, p, Seeking::Concat, work, tiles, cancel).await;
        }
        match result {
            Ok(n) => count = n,
            Err(e) if cancel.is_cancelled() => return Err(e),
            #[cfg(test)]
            Err(e) if reading != Reading::Auto => return Err(e),
            Err(e) => {
                info!(path, error = %e, "seeking can't make the trickplay tiles; reading all")
            }
        }
    }
    #[cfg(test)]
    if matches!(reading, Reading::Seek(_)) && seek.is_none() {
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
    // Every video the task covers, and those already done: with a tile set, or a failure on
    // record. A video with a failure is counted done here, so it isn't counted again when it
    // is skipped or tried again. The bar so starts where the last run left it.
    // Whether tiles are decoded on a device, as the transcoder's are
    let decoding = TileDecoding::current().await;
    let (total, done) =
        rustyfin_db::repo_trickplay::trickplay_progress(pool, library_ids, WIDTH, FAILURE_KIND)
            .await
            .map_err(|e| format!("could not count videos: {e}"))?;
    let total = usize::try_from(total).unwrap_or(usize::MAX);
    let mut done = usize::try_from(done).unwrap_or(usize::MAX).min(total);
    progress.report_count(done, total);
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
                        .chain(started.values().map(|(id, _, _, _)| id))
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
                let failure = get_failure(pool, item_id, FAILURE_KIND).await.map_err(db)?;
                Ok::<_, String>(Some((item_id, path, version, failure)))
            }
            .await;
            let (item_id, path, version, failure) = match next {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            };
            // Counted done at the start
            if failure == Some(version) {
                failed.push(item_id);
                continue;
            }
            let counted = failure.is_some();
            let (pool, data_dir, cancel, file, decoding) = (
                pool.clone(),
                data_dir.to_path_buf(),
                cancel.clone(),
                path.clone(),
                decoding.clone(),
            );
            let task = running.spawn(async move {
                generate(&pool, &data_dir, item_id, &file, &decoding, &cancel).await
            });
            started.insert(task.id(), (item_id, path, version, counted));
        }
        let Some(joined) = running.join_next_with_id().await else {
            break;
        };
        let (result, task) = match joined {
            Ok((task, result)) => (result, task),
            Err(e) => (Err(format!("the tile task failed: {e}")), e.id()),
        };
        let Some((item_id, path, version, counted)) = started.remove(&task) else {
            continue;
        };
        // Videos added during the run can take it past the total counted at the start
        if !counted {
            done += 1;
            progress.report_count(done.min(total), total);
        }
        let recorded = async {
            match result {
                Ok(set) => {
                    made += 1;
                    retry_busy_until("clearing a trickplay failure", || cancel.cancelled(), || {
                        clear_failure(pool, item_id, FAILURE_KIND)
                    })
                    .await
                    .map_err(db)?;
                    retry_busy_until("storing a trickplay file version", || cancel.cancelled(), || {
                        set_image_version(pool, item_id, Some(version))
                    })
                    .await
                    .map_err(db)?;
                    info!(item = %item_id, thumbnails = set.thumbnail_count, "made trickplay tiles");
                }
                // The other running videos are stopped too, and end up here
                Err(_) if cancel.is_cancelled() => {}
                // Not the video's fault: nothing is recorded, so it is tried once ffmpeg is newer
                Err(e) if e.starts_with(FFMPEG_TOO_OLD) => return Err(e),
                // Not the video's fault either: nothing is recorded, so the next run tries again
                Err(e) if e.starts_with(SAVE_FAILED) => {
                    warn!(item = %item_id, path = %path, error = %e, "could not make trickplay tiles");
                    failed.push(item_id);
                }
                Err(e) => {
                    warn!(item = %item_id, path = %path, error = %e, "could not make trickplay tiles");
                    retry_busy_until("recording a trickplay failure", || cancel.cancelled(), || {
                        record_failure(pool, item_id, FAILURE_KIND, version)
                    })
                    .await
                    .map_err(db)?;
                    retry_busy_until("storing a trickplay file version", || cancel.cancelled(), || {
                        set_image_version(pool, item_id, Some(version))
                    })
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

    /// No device: tiles decoded in software, as most tests want them.
    const SOFTWARE: TileDecoding = TileDecoding { vaapi: None };

    thread_local! {
        /// Set by a test: seeking pays for every video (`Probe::seeking`), so `Reading::Auto`
        /// seeks a small clip.
        pub(super) static SEEK_PAYS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

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
        // Key frames that aren't IDR frames, as on Blu-rays
        let x264_open = [
            "-x264-params",
            "keyint=20:min-keyint=20:scenecut=0:open-gop=1",
        ];
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
            ("libx265", &x265, "seek.mp4"),
        ] {
            if !have_encoder(encoder) {
                eprintln!("SKIPPED {test} for {encoder}: ffmpeg can't encode with it");
                continue;
            }
            let clip = timed_clip(&dir, encoder, params, &[], name);
            let clip = clip.to_string_lossy();
            let mut made = Vec::new();
            for reading in [
                Reading::Stream,
                Reading::Seek(Seeking::Raw),
                Reading::Seek(Seeking::Concat),
            ] {
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
                    make_tiles(&clip, &work, &tiles, reading, &SOFTWARE, &Cancel::new())
                        .await
                        .map(|i| i.thumbnail_count)
                };
                made.push(info.map(|count| (count, thumbnail_times(&tiles, count))));
            }
            let old = made[0].as_ref().unwrap();
            for (how, new) in ["raw", "concat"].iter().zip(&made[1..]) {
                match new {
                    Ok(new) => {
                        assert_eq!(old.0, new.0, "{encoder} {name} {how}: the same count");
                        for (k, (a, b)) in old.1.iter().zip(&new.1).enumerate() {
                            assert!(
                                (a - b).abs() <= 1.0,
                                "{encoder} {name} {how}: thumbnail {k} {a} {b}"
                            );
                        }
                    }
                    // The concat demuxer fails on open-GOP HEVC; never with wrong frames
                    Err(e) if *how == "concat" && encoder == "libx265" => {
                        assert!(e.contains("not 9 in order"), "{e}");
                    }
                    Err(e) => panic!("{encoder} {name} {how}: {e}"),
                }
            }
            checked += 1;
        }
        if have_encoder("libx264") {
            // Open-GOP H.264, whose key frames after the first are non-IDR I frames, as on
            // Blu-rays: the raw stream shows the frames the closed-GOP encode's original pass
            // does, as both have a key frame every 2 s
            let open = timed_clip(&dir, "libx264", &x264_open, &[], "open.mkv");
            let closed = dir.join("libx264-seek.mkv");
            let mut made = Vec::new();
            for (clip, raw) in [(&closed, false), (&open, true)] {
                let work = dir.join(format!("work-open-{raw}"));
                let tiles = work.join("tiles");
                std::fs::create_dir_all(&tiles).unwrap();
                let clip = clip.to_string_lossy();
                let count = if raw {
                    let raw = Reading::Seek(Seeking::Raw);
                    make_tiles(&clip, &work, &tiles, raw, &SOFTWARE, &Cancel::new())
                        .await
                        .unwrap()
                        .thumbnail_count
                } else {
                    tile_run(&clip, &work, &tiles, false, 1, &Cancel::new())
                        .await
                        .unwrap()
                };
                made.push(thumbnail_times(&tiles, count));
            }
            assert_eq!(made[0].len(), made[1].len());
            for (k, (a, b)) in made[0].iter().zip(&made[1]).enumerate() {
                assert!((a - b).abs() <= 1.0, "open GOP: thumbnail {k} {a} {b}");
            }
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

    /// A clip at `rate` fps for `seconds`, H.264 (or HEVC, for a name starting with `hevc`, in
    /// x265's open GOPs) with a key frame every `keyint` frames, whose left half's brightness is
    /// 16 + t / 5.5 and right half's 16 + (t mod 10) × 20, so each frame of it looks different.
    /// `filter` is added to the source, `options` to the output.
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
        let x = if name.starts_with("hevc") {
            "x265"
        } else {
            "x264"
        };
        let params = format!("keyint={keyint}:min-keyint={keyint}:scenecut=0:log-level=error");
        let params = if x == "x264" {
            params.replace(":log-level=error", "")
        } else {
            params
        };
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
                &format!("lib{x}"),
                "-pix_fmt",
                "yuv420p",
                &format!("-{x}-params"),
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
    async fn r3trickplay_a_damaged_raw_stream_falls_back() {
        let test = "r3trickplay_a_damaged_raw_stream_falls_back";
        if !have_ffmpeg(test) {
            return;
        }
        if !have_encoder("libx264") {
            eprintln!("SKIPPED {test}: ffmpeg can't encode H.264, so this test checked nothing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-damage-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let x264 = ["-x264-params", "keyint=20:min-keyint=20:scenecut=0"];
        let clip = timed_clip(&dir, "libx264", &x264, &[], "damaged.mkv");
        let clip = clip.to_string_lossy();
        let run = |name: &str| {
            let work = dir.join(name);
            let tiles = work.join("tiles");
            std::fs::create_dir_all(&tiles).unwrap();
            (work, tiles)
        };
        // Every key frame's slice is scrambled on its way to ffmpeg, which still hands out a
        // frame for each
        key_frames::DAMAGE.set(true);
        SEEK_PAYS.set(true);
        let (work, tiles) = run("raw");
        let raw = make_tiles(
            &clip,
            &work,
            &tiles,
            Reading::Seek(Seeking::Raw),
            &SOFTWARE,
            &Cancel::new(),
        )
        .await
        .unwrap_err();
        assert!(raw.contains("ffmpeg"), "{raw}");
        // So the video is read whole, and its tiles are the streaming pass's
        let (work, auto) = run("auto");
        let info = make_tiles(
            &clip,
            &work,
            &auto,
            Reading::Auto,
            &SOFTWARE,
            &Cancel::new(),
        )
        .await
        .unwrap();
        key_frames::DAMAGE.set(false);
        SEEK_PAYS.set(false);
        let (work, streamed) = run("stream");
        let count = tile_run(&clip, &work, &streamed, true, 1, &Cancel::new())
            .await
            .unwrap();
        assert_eq!(info.thumbnail_count, count);
        assert_eq!(
            std::fs::read(auto.join("0.jpg")).unwrap(),
            std::fs::read(streamed.join("0.jpg")).unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Matroska EBML number at `at`: its value (with the length marker kept, as for an
    /// element id, when `marker`) and its length in bytes.
    fn ebml_number(data: &[u8], at: usize, marker: bool) -> (u64, usize) {
        let len = data[at].leading_zeros() as usize + 1;
        let first = if marker {
            u64::from(data[at])
        } else {
            u64::from(data[at]) & (0xff >> len)
        };
        let value = data[at + 1..at + len]
            .iter()
            .fold(first, |v, b| v << 8 | u64::from(*b));
        (value, len)
    }

    /// The elements between `start` and `end`: id, offset, id and size length, body size.
    fn ebml_children(data: &[u8], start: usize, end: usize) -> Vec<(u64, usize, usize, usize)> {
        let mut out = Vec::new();
        let mut at = start;
        while at < end {
            let (id, id_len) = ebml_number(data, at, true);
            let (size, size_len) = ebml_number(data, at + id_len, false);
            out.push((id, at, id_len + size_len, size as usize));
            at += id_len + size_len + size as usize;
        }
        out
    }

    /// Writes a copy of the ffmpeg-made Matroska file `from` to `to` with PixelCrop top,
    /// bottom, left and right `crop` on its video track, which ffmpeg can't write. The 16 bytes
    /// come out of the Void element ffmpeg leaves before Info and Tracks, so the clusters and
    /// the index keep their offsets; the SeekHead entries of Info and Tracks move with them.
    fn with_pixel_crop(from: &Path, to: &Path, crop: [u8; 4]) {
        let mut d = std::fs::read(from).unwrap();
        let ids = [0x54BB_u16, 0x54AA, 0x54CC, 0x54DD];
        let added: Vec<u8> = ids
            .iter()
            .zip(crop)
            .flat_map(|(id, px)| [(id >> 8) as u8, *id as u8, 0x81, px])
            .collect();
        let n = added.len();
        let find = |d: &[u8], el: (u64, usize, usize, usize), id: u64| {
            ebml_children(d, el.1 + el.2, el.1 + el.2 + el.3)
                .into_iter()
                .find(|c| c.0 == id)
                .unwrap()
        };
        let segment = ebml_children(&d, 0, d.len())
            .into_iter()
            .find(|c| c.0 == 0x1853_8067)
            .unwrap();
        let void = find(&d, segment, 0xEC);
        let tracks = find(&d, segment, 0x1654_AE6B);
        let seek_head = find(&d, segment, 0x114D_9B74);
        let entry = find(&d, tracks, 0xAE);
        let video = find(&d, entry, 0xE0);
        assert!(void.1 < tracks.1 && void.3 >= n);
        let set_size =
            |d: &mut [u8], (id, at, head, size): (u64, usize, usize, usize), to: usize| {
                let id_len = ebml_number(d, at, true).1;
                let size_len = head - id_len;
                assert_eq!(ebml_number(d, at, true).0, id);
                let raw = (1u64 << (7 * size_len)) | (to as u64);
                let bytes = raw.to_be_bytes();
                d[at + id_len..at + head].copy_from_slice(&bytes[8 - size_len..]);
                assert!(to + n >= size);
            };
        let video_end = video.1 + video.2 + video.3;
        d.splice(video_end..video_end, added);
        for el in [video, entry, tracks] {
            set_size(&mut d, el, el.3 + n);
        }
        set_size(&mut d, void, void.3 - n);
        let void_end = void.1 + void.2 + void.3;
        d.drain(void_end - n..void_end);
        // Seek entries: SeekID then SeekPosition, relative to the segment's body
        for seek in ebml_children(
            &d,
            seek_head.1 + seek_head.2,
            seek_head.1 + seek_head.2 + seek_head.3,
        ) {
            let fields = ebml_children(&d, seek.1 + seek.2, seek.1 + seek.2 + seek.3);
            let value = |f: &(u64, usize, usize, usize)| {
                d[f.1 + f.2..f.1 + f.2 + f.3]
                    .iter()
                    .fold(0u64, |v, b| v << 8 | u64::from(*b))
            };
            let target = fields.iter().find(|f| f.0 == 0x53AB).map(value);
            let pos = fields.iter().find(|f| f.0 == 0x53AC).copied();
            if let (Some(0x1549_A966 | 0x1654_AE6B), Some(pos)) = (target, pos) {
                let moved = value(&pos) - n as u64;
                let bytes = moved.to_be_bytes();
                d[pos.1 + pos.2..pos.1 + pos.2 + pos.3].copy_from_slice(&bytes[8 - pos.3..]);
            }
        }
        std::fs::write(to, d).unwrap();
    }

    #[tokio::test]
    async fn r3trickplay_the_index_finds_the_frames_ffprobe_does() {
        let test = "r3trickplay_the_index_finds_the_frames_ffprobe_does";
        if !have_ffmpeg(test) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-index-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let x264 = ["-x264-params", "keyint=20:min-keyint=20:scenecut=0"];
        let x265 = [
            "-x265-params",
            "keyint=20:min-keyint=20:scenecut=0:log-level=error",
        ];
        let mut checked = 0;
        for (encoder, params, audio) in [
            ("libx264", &x264, false),
            ("libx265", &x265, false),
            // Audio interleaved, and clusters cut every 0.7 s, so key frames sit inside them
            ("libx264", &x264, true),
        ] {
            if !have_encoder(encoder) {
                eprintln!("SKIPPED {test} for {encoder}: ffmpeg can't encode with it");
                continue;
            }
            let clip = if audio {
                let clip = dir.join(format!("{encoder}-audio.mkv"));
                let status = std::process::Command::new("ffmpeg")
                    .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
                    .arg("nullsrc=size=320x180:rate=10:duration=95,geq=lum='16+N/5':cb=128:cr=128")
                    .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=95"])
                    .args(["-c:v", encoder, "-pix_fmt", "yuv420p"])
                    .args(params)
                    .args(["-c:a", "pcm_s16le", "-cluster_time_limit", "700"])
                    .args(["-cluster_size_limit", "20000"])
                    .arg(&clip)
                    .status()
                    .unwrap();
                assert!(status.success());
                clip
            } else {
                timed_clip(&dir, encoder, params, &[], "index.mkv")
            };
            let clip = clip.to_string_lossy();
            let probed = probe(&clip, &Cancel::new()).await.unwrap().unwrap();
            let index = probed.raw.as_ref().unwrap().index.clone();
            let index = index.expect("ffmpeg writes cues with the block's position");
            if audio {
                assert!(
                    index.key_frames_inside_clusters() > 0,
                    "some key frames don't start their cluster"
                );
            }
            // The index's packets, read with and without the blocks' positions, are the ones
            // ffprobe prints, for every thumbnail the index covers
            let targets: Vec<f64> = (0..probed.count())
                .map(|k| probed.start + f64::from(k) * 10.0 + 5.0 - 1e-3)
                .collect();
            let printed = probe_packets(&clip, &targets);
            let file = std::fs::File::open(&*clip).unwrap();
            for index in [index.clone(), Arc::new(index.without_positions())] {
                let mut compared = 0;
                for (target, printed) in targets.iter().zip(&printed) {
                    if let Some(cue) = index.pick(*target) {
                        assert_eq!(&index.frame(&file, cue).unwrap(), printed, "{target}");
                        compared += 1;
                    }
                }
                assert!(
                    compared + 1 >= targets.len(),
                    "{compared} of {}",
                    targets.len()
                );
            }
            // Through the index; through ffprobe; through an index without the blocks'
            // positions, which reads each cluster from its start; and through an index whose
            // clusters are all in the wrong place, which falls back to ffprobe
            let mut made = Vec::new();
            for (name, index) in [
                ("index", Some(index.clone())),
                ("ffprobe", None),
                ("scan", Some(Arc::new(index.without_positions()))),
                ("broken", Some(Arc::new(index.shifted(7)))),
            ] {
                let mut p = probed.clone();
                p.raw.as_mut().unwrap().index = index;
                let work = dir.join(format!("work-{encoder}-{name}"));
                let tiles = work.join("tiles");
                std::fs::create_dir_all(&tiles).unwrap();
                let count = seek_tiles(&clip, &p, Seeking::Raw, &work, &tiles, &Cancel::new())
                    .await
                    .unwrap_or_else(|e| panic!("{encoder} {name}: {e}"));
                made.push((count, std::fs::read(tiles.join("0.jpg")).unwrap()));
            }
            assert!(
                made[0] == made[1],
                "{encoder}: the index's tile is ffprobe's"
            );
            assert!(
                made[2] == made[1],
                "{encoder}: scanning the clusters finds them too"
            );
            assert!(
                made[3] == made[1],
                "{encoder}: a broken index falls back to ffprobe"
            );
            checked += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
        if checked == 0 {
            eprintln!("SKIPPED {test}: no H.264 or HEVC encoder, so this test checked nothing");
        }
    }

    /// The packets ffprobe prints for a seek to each of `targets` in `clip` (`-show_data`).
    fn probe_packets(clip: &str, targets: &[f64]) -> Vec<Vec<u8>> {
        let intervals: Vec<String> = targets.iter().map(|t| format!("{t:.6}%+#1")).collect();
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "V:0", "-read_intervals"])
            .arg(intervals.join(","))
            .args(["-show_data", "-show_entries", "packet=flags,data"])
            .args(["-of", "default=noprint_wrappers=1", clip])
            .output()
            .unwrap();
        let mut packets: Vec<Vec<u8>> = Vec::new();
        for line in out.stdout.split(|&b| b == b'\n') {
            if line.starts_with(b"flags=") {
                packets.push(Vec::new());
            } else if let Some(packet) = packets.last_mut() {
                key_frames::hex_line(line, packet);
            }
        }
        assert_eq!(packets.len(), targets.len());
        packets
    }

    /// Decoding on the VA-API device `device`, for H.264 and HEVC (10-bit too).
    fn on_device(device: &str) -> TileDecoding {
        TileDecoding {
            vaapi: Some(Arc::new(Vaapi {
                device: device.to_string(),
                codecs: vec!["h264".into(), "hevc".into()],
                hevc_10bit: true,
                failures: Default::default(),
            })),
        }
    }

    #[tokio::test]
    async fn r3trickplay_only_the_settings_in_effect_decode_on_a_device() {
        use crate::encoding::{EncodingSettings, HardwareAccelerationType};
        // None, as by default, or another acceleration: software
        let settings = |accel| EncodingSettings {
            hardware_acceleration: accel,
            hardware_decoding_codecs: vec!["h264".into()],
            ..EncodingSettings::default()
        };
        for accel in [
            HardwareAccelerationType::None,
            HardwareAccelerationType::Nvenc,
        ] {
            let decoding = TileDecoding::from_settings(settings(accel)).await;
            assert!(decoding.vaapi.is_none(), "{accel:?}");
        }
        // VA-API in effect: its device and codecs, where ffmpeg has VA-API
        let vaapi = TileDecoding::from_settings(settings(HardwareAccelerationType::Vaapi)).await;
        if let Some(vaapi) = &vaapi.vaapi {
            assert_eq!(vaapi.codecs, ["h264"]);
            assert_eq!(vaapi.device, EncodingSettings::default().vaapi_device);
        }
        // The codecs and bit depths the settings let the device decode, as the transcoder
        let device = TileDecoding {
            vaapi: Some(Arc::new(Vaapi {
                device: "/dev/dri/renderD128".to_string(),
                codecs: vec!["h264".into()],
                hevc_10bit: false,
                failures: Default::default(),
            })),
        };
        assert!(device.device_for(Codec::H264, false).is_some());
        assert!(
            device.device_for(Codec::H264, true).is_none(),
            "10-bit H.264"
        );
        assert!(
            device.device_for(Codec::Hevc, false).is_none(),
            "not a listed codec"
        );
        let hevc = on_device("/dev/dri/renderD128");
        assert!(hevc.device_for(Codec::Hevc, true).is_some());
        // A device that fails is given up on for the run
        let vaapi = hevc.vaapi.as_ref().unwrap();
        for _ in 0..HW_FAILURES_MOST {
            assert!(vaapi.usable());
            vaapi.failed("test");
        }
        assert!(!vaapi.usable());
    }

    #[tokio::test]
    async fn r3trickplay_device_tiles_show_the_frames_and_fall_back() {
        let test = "r3trickplay_device_tiles_show_the_frames_and_fall_back";
        if !have_ffmpeg(test) || !have_encoder("libx264") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-va-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let x264 = ["-x264-params", "keyint=20:min-keyint=20:scenecut=0"];
        let clip = timed_clip(&dir, "libx264", &x264, &[], "va.mkv");
        let clip = clip.to_string_lossy();
        let raw = Reading::Seek(Seeking::Raw);
        let run = async |name: &str, decoding: &TileDecoding| {
            let work = dir.join(name);
            let tiles = work.join("tiles");
            std::fs::create_dir_all(&tiles).unwrap();
            let info = make_tiles(&clip, &work, &tiles, raw, decoding, &Cancel::new())
                .await
                .unwrap();
            (info, tiles)
        };
        let (soft, soft_tiles) = run("soft", &SOFTWARE).await;
        // A device that can't be opened: every tile is made again in software, the same
        let broken = on_device("/nonexistent/renderD999");
        let (info, tiles) = run("broken", &broken).await;
        assert_eq!(info, soft);
        assert_eq!(
            std::fs::read(tiles.join("0.jpg")).unwrap(),
            std::fs::read(soft_tiles.join("0.jpg")).unwrap()
        );
        assert!(
            broken
                .vaapi
                .as_ref()
                .unwrap()
                .failures
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0
        );
        // A real device, where there is one: the same frames, scaled there
        let device = "/dev/dri/renderD128";
        if std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(device)
            .is_ok()
        {
            let real = on_device(device);
            let (info, tiles) = run("device", &real).await;
            assert_eq!(info.thumbnail_count, soft.thumbnail_count);
            assert_eq!((info.width, info.height), (soft.width, soft.height));
            let failures = real
                .vaapi
                .as_ref()
                .unwrap()
                .failures
                .load(std::sync::atomic::Ordering::Relaxed);
            if failures == 0 {
                let (a, b) = (
                    thumbnail_times(&soft_tiles, soft.thumbnail_count),
                    thumbnail_times(&tiles, info.thumbnail_count),
                );
                for (k, (a, b)) in a.iter().zip(&b).enumerate() {
                    assert!((a - b).abs() <= 1.0, "thumbnail {k}: {a} {b}");
                }
            } else {
                eprintln!("SKIPPED {test} on the device: it failed there, and fell back");
            }
        } else {
            eprintln!("SKIPPED {test} on a device: {device} can't be opened");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3trickplay_raw_tiles_keep_the_containers_cropping() {
        let test = "r3trickplay_raw_tiles_keep_the_containers_cropping";
        if !have_ffmpeg(test) {
            return;
        }
        if !have_encoder("libx264") {
            eprintln!("SKIPPED {test}: ffmpeg can't encode H.264, so this test checked nothing");
            return;
        }
        let dir = std::env::temp_dir().join(format!("r3trickplay-crop-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let x264 = ["-x264-params", "keyint=20:min-keyint=20:scenecut=0"];
        let plain = timed_clip(&dir, "libx264", &x264, &[], "plain.mkv");
        // Odd sizes, which a 4:2:0 crop has to round the same way ffmpeg's own does
        let cropped = dir.join("cropped.mkv");
        with_pixel_crop(&plain, &cropped, [21, 9, 31, 40]);
        let cropped = cropped.to_string_lossy();
        let mut made = Vec::new();
        for how in [Seeking::Raw, Seeking::Concat] {
            let work = dir.join(format!("work-{how:?}"));
            let tiles = work.join("tiles");
            std::fs::create_dir_all(&tiles).unwrap();
            let info = make_tiles(
                &cropped,
                &work,
                &tiles,
                Reading::Seek(how),
                &SOFTWARE,
                &Cancel::new(),
            )
            .await
            .unwrap();
            made.push((info, std::fs::read(tiles.join("0.jpg")).unwrap()));
        }
        // ffmpeg crops the file to 249 x 150, which scales to 320 x 194, not 320 x 180
        assert_eq!((made[0].0.width, made[0].0.height), (320, 194));
        assert_eq!(made[0].0, made[1].0);
        assert!(
            made[0].1 == made[1].1,
            "the raw stream's tile is the concat demuxer's"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let mut clips = vec![
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
        if have_encoder("libx265") {
            // Open-GOP HEVC whose chosen CRA key frames are 250 frames apart: in a raw stream,
            // each one's picture order count would be 6 below the last's, on a frame the last
            // one's references made up, but for the rewrite (`key_frames`)
            clips.push(marked_clip(&dir, "hevc-open.mkv", (25, 120, 50), "", &[]));
        } else {
            eprintln!("SKIPPED {test} for HEVC: ffmpeg can't encode with libx265");
        }
        for clip in &clips {
            let name = clip.file_name().unwrap().to_string_lossy().into_owned();
            let clip = clip.to_string_lossy();
            let mut readings = vec![Reading::Stream, Reading::Seek(Seeking::Raw)];
            if !name.starts_with("hevc") {
                readings.push(Reading::Seek(Seeking::Concat));
            }
            let mut made = Vec::new();
            for reading in readings {
                let work = dir.join(format!("work-{name}-{reading:?}"));
                let tiles = work.join("tiles");
                std::fs::create_dir_all(&tiles).unwrap();
                let count = if reading == Reading::Stream {
                    // The pass with every key frame, whose frames seeking picks
                    tile_run(&clip, &work, &tiles, false, 1, &Cancel::new())
                        .await
                        .unwrap()
                } else {
                    let info = make_tiles(&clip, &work, &tiles, reading, &SOFTWARE, &Cancel::new())
                        .await
                        .unwrap_or_else(|e| panic!("{name}: {e}"));
                    let tile_count = std::fs::read_dir(&tiles).unwrap().count();
                    assert_eq!(tile_count as u32, info.tile_count(), "{name}");
                    info.thumbnail_count
                };
                made.push((count, thumbnail_marks(&tiles, count)));
            }
            for (how, new) in made.iter().enumerate().skip(1) {
                assert_eq!(made[0].0, new.0, "{name} {how}: the same count");
                for (k, (a, b)) in made[0].1.iter().zip(&new.1).enumerate() {
                    // A frame 2 s on is 40 levels brighter on the right; 10 s on, 1.8 on the left
                    assert!(
                        (a.0 - b.0).abs() <= 1.2 && (a.1 - b.1).abs() <= 8.0,
                        "{name} {how}: thumbnail {k} differs: {a:?} {b:?}"
                    );
                }
            }
        }
        assert_eq!(
            thumbnail_marks(&dir.join("work-long.mkv-Seek(Raw)/tiles"), 111).len(),
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
    async fn r3trickplay_progress_resumes_where_the_last_run_left_it() {
        use rustyfin_db::repo_trickplay::{record_failure, save_trickplay};
        use rustyfin_model::library::{Library, LibraryContentType};

        let dir = std::env::temp_dir().join(format!("r3trickplay-progress-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = rustyfin_db::init_db(dir.join("db.sqlite").to_str().unwrap())
            .await
            .unwrap();
        let lib = Library {
            id: Uuid::new_v4(),
            name: "Films".to_string(),
            content_type: LibraryContentType::Movies,
            paths: vec![dir.to_string_lossy().into_owned()],
        };
        rustyfin_db::repo_library::create_library(&pool, &lib)
            .await
            .unwrap();
        // Two made, one failed on the file as it is, one failed on an older file, one to do;
        // none of the files is there, so the last two fail
        let items: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
        for (k, id) in items.iter().enumerate() {
            sqlx::query(
                "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
                 VALUES (?1, ?2, 'Movie', ?3, ?4, 600000000)",
            )
            .bind(id.to_string())
            .bind(format!("Film {k}"))
            .bind(
                dir.join(format!("missing-{k}.mkv"))
                    .to_string_lossy()
                    .into_owned(),
            )
            .bind(lib.id.to_string())
            .execute(&pool)
            .await
            .unwrap();
        }
        let set = TrickplayInfo {
            width: WIDTH,
            height: 180,
            tile_width: TILE_SIDE,
            tile_height: TILE_SIDE,
            thumbnail_count: 10,
            interval: INTERVAL_MS,
            bandwidth: 1,
        };
        save_trickplay(&pool, items[0], &set).await.unwrap();
        save_trickplay(&pool, items[1], &set).await.unwrap();
        record_failure(&pool, items[2], FAILURE_KIND, (0, 0))
            .await
            .unwrap();
        record_failure(&pool, items[3], FAILURE_KIND, (1, 2))
            .await
            .unwrap();
        let libs = [lib.id.to_string()];
        let data = dir.join("data");

        // A run stopped before any video starts still shows the 4 of 5 done
        let progress = crate::task_progress::TaskProgress::new();
        let cancel = Cancel::new();
        cancel.cancel();
        generate_for_libraries_with_progress(&pool, &data, &libs, &cancel, &progress)
            .await
            .unwrap();
        assert_eq!(progress.get(), Some(80.0));

        // A whole run ends at 100%: the older failure, counted done already, isn't counted again
        let progress = crate::task_progress::TaskProgress::new();
        let made =
            generate_for_libraries_with_progress(&pool, &data, &libs, &Cancel::new(), &progress)
                .await
                .unwrap();
        assert_eq!(made, 0);
        assert_eq!(progress.get(), Some(100.0));
        // And the next run starts there
        let progress = crate::task_progress::TaskProgress::new();
        generate_for_libraries_with_progress(&pool, &data, &libs, &cancel, &progress)
            .await
            .unwrap();
        assert_eq!(progress.get(), Some(100.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3trickplay_a_failed_save_is_not_the_videos_failure() {
        let test = "r3trickplay_a_failed_save_is_not_the_videos_failure";
        if !have_ffmpeg(test) {
            return;
        }
        use rustyfin_model::library::{Library, LibraryContentType};

        let dir = std::env::temp_dir().join(format!("r3trickplay-save-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = marked_clip(&dir, "clip.mkv", (10, 25, 20), "", &[]);
        let pool = rustyfin_db::init_db(dir.join("db.sqlite").to_str().unwrap())
            .await
            .unwrap();
        let lib = Library {
            id: Uuid::new_v4(),
            name: "Films".to_string(),
            content_type: LibraryContentType::Movies,
            paths: vec![dir.to_string_lossy().into_owned()],
        };
        rustyfin_db::repo_library::create_library(&pool, &lib)
            .await
            .unwrap();
        let item = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
             VALUES (?1, 'Film', 'Movie', ?2, ?3, 250000000)",
        )
        .bind(item.to_string())
        .bind(clip.to_string_lossy().into_owned())
        .bind(lib.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        // The tiles are made, but the database won't take them, as when its pool timed out
        sqlx::query(
            "CREATE TRIGGER no_saves BEFORE INSERT ON trickplay_infos
             BEGIN SELECT RAISE(ABORT, 'the database is down'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        let libs = [lib.id.to_string()];
        let made = generate_for_libraries(&pool, &dir.join("data"), &libs, &Cancel::new())
            .await
            .unwrap();
        assert_eq!(made, 0);
        let failure = rustyfin_db::repo_trickplay::get_failure(&pool, item, FAILURE_KIND)
            .await
            .unwrap();
        assert_eq!(failure, None, "a database error isn't the video's failure");

        // So the next run, with the database back, makes the set
        sqlx::query("DROP TRIGGER no_saves")
            .execute(&pool)
            .await
            .unwrap();
        let made = generate_for_libraries(&pool, &dir.join("data"), &libs, &Cancel::new())
            .await
            .unwrap();
        assert_eq!(made, 1);
        let _ = std::fs::remove_dir_all(&dir);
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
