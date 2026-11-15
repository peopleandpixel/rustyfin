//! Trickplay tiles: the seek-bar thumbnails Jellyfin clients show, made in the background.
//!
//! Like Jellyfin, a tile set holds one thumbnail every 10 seconds, 320 pixels wide, laid out 10 by
//! 10 in each JPEG tile, under `<data dir>/trickplay/<item id>/<width> - 10x10/<index>.jpg`. Tiles
//! are only made by the "Generate Trickplay Images" task, never on a request. Every trickplay or
//! chapter-image ffmpeg run holds `ffmpeg_lock`, so one image job runs at a time, alongside any
//! transcodes. A video that fails is recorded with its file's size and time and is not tried
//! again until the file changes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::{Mutex, watch};
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
/// the chapter images no chapter names any more. Holds `ffmpeg_lock`, so no job is writing.
pub async fn sweep_unused_files(pool: &SqlitePool, data_dir: &Path) -> Result<(), String> {
    let _one_at_a_time = ffmpeg_lock().lock().await;
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
        let mut files = fs::read_dir(entry.path())
            .await
            .map_err(|e| e.to_string())?;
        let mut kept = 0;
        while let Some(file) = files.next_entry().await.map_err(|e| e.to_string())? {
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

/// Held around every ffmpeg run that makes trickplay tiles or chapter images, so only one image
/// job runs at a time, whichever task starts it. Transcodes don't take it.
pub(crate) fn ffmpeg_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
/// `cancel` fires, killing ffmpeg. The caller holds `ffmpeg_lock`.
pub(crate) async fn run_ffmpeg(args: &[&str], cancel: &Cancel) -> Result<(), String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
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

/// The JPEG files in `dir`, sorted by name.
async fn jpegs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = fs::read_dir(dir).await.map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jpg") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Makes the tile set of the video at `path`, an item's file, and saves it. The old set, if any,
/// is replaced only once the new one is complete. Waits for `ffmpeg_lock`.
pub async fn generate(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    let _one_at_a_time = ffmpeg_lock().lock().await;
    let item_dir = data_dir.join("trickplay").join(item_id.to_string());
    let work = item_dir.join(format!(".work-{WIDTH}"));
    let thumbs = work.join("thumbs");
    let tiles = work.join("tiles");
    let _ = fs::remove_dir_all(&work).await;
    fs::create_dir_all(&thumbs)
        .await
        .map_err(|e| format!("could not make {}: {e}", thumbs.display()))?;
    fs::create_dir_all(&tiles)
        .await
        .map_err(|e| format!("could not make {}: {e}", tiles.display()))?;

    let result = make_tiles(path, &thumbs, &tiles, cancel).await;
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

/// Makes the thumbnails of `path` in `thumbs`, then the tile images in `tiles`, and returns the
/// set's info.
async fn make_tiles(
    path: &str,
    thumbs: &Path,
    tiles: &Path,
    cancel: &Cancel,
) -> Result<TrickplayInfo, String> {
    // Only key frames are decoded, which is many times faster than every frame; the fps filter
    // repeats the last one where they are further apart than the interval.
    let filter = format!("fps=1000/{INTERVAL_MS},scale={WIDTH}:-2");
    let pattern = thumbs.join("%06d.jpg");
    let pattern = pattern.to_string_lossy();
    run_ffmpeg(
        &[
            "-threads",
            "1",
            "-skip_frame",
            "nokey",
            "-i",
            path,
            "-map",
            "0:V:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filter,
            "-threads",
            "1",
            "-qscale:v",
            "4",
            "-f",
            "image2",
            &pattern,
        ],
        cancel,
    )
    .await?;

    let thumb_files = jpegs(thumbs).await?;
    let count = u32::try_from(thumb_files.len()).unwrap_or(u32::MAX);
    let Some(first) = thumb_files.first() else {
        return Err("ffmpeg made no thumbnails".to_string());
    };
    let (width, height) =
        image::image_dimensions(first).map_err(|e| format!("could not read a thumbnail: {e}"))?;

    let tile = format!("tile={TILE_SIDE}x{TILE_SIDE}");
    let input = thumbs.join("%06d.jpg");
    let output = tiles.join("%d.jpg");
    run_ffmpeg(
        &[
            "-framerate",
            "1",
            "-start_number",
            "1",
            "-i",
            &input.to_string_lossy(),
            "-vf",
            &tile,
            "-threads",
            "1",
            "-qscale:v",
            "4",
            "-start_number",
            "0",
            "-f",
            "image2",
            &output.to_string_lossy(),
        ],
        cancel,
    )
    .await?;

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

/// Makes the missing tile sets of the videos in the libraries `library_ids`, one at a time, and
/// returns how many it made. A video that fails is logged and recorded with its file's version;
/// it is skipped until the file changes. Stops between videos once `cancel` fires.
pub async fn generate_for_libraries(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
    cancel: &Cancel,
) -> Result<u32, String> {
    use rustyfin_db::repo_trickplay::{clear_failure, get_failure, record_failure};
    // Videos this run won't try (again): failed now, or before on the same file
    let mut failed: Vec<Uuid> = Vec::new();
    let mut made = 0u32;
    loop {
        let batch = rustyfin_db::repo_trickplay::items_missing_trickplay(
            pool,
            library_ids,
            WIDTH,
            &failed,
            20,
        )
        .await
        .map_err(|e| format!("could not list videos: {e}"))?;
        if batch.is_empty() {
            break;
        }
        for (item_id, path, _runtime) in batch {
            if cancel.is_cancelled() {
                return Ok(made);
            }
            let version = file_version(&path).await;
            let db = |e: sqlx::Error| format!("could not record a failure: {e}");
            if get_failure(pool, item_id, FAILURE_KIND).await.map_err(db)? == Some(version) {
                failed.push(item_id);
                continue;
            }
            match generate(pool, data_dir, item_id, &path, cancel).await {
                Ok(set) => {
                    made += 1;
                    clear_failure(pool, item_id, FAILURE_KIND)
                        .await
                        .map_err(db)?;
                    info!(item = %item_id, thumbnails = set.thumbnail_count, "made trickplay tiles");
                }
                Err(_) if cancel.is_cancelled() => return Ok(made),
                Err(e) => {
                    warn!(item = %item_id, path = %path, error = %e, "could not make trickplay tiles");
                    record_failure(pool, item_id, FAILURE_KIND, version)
                        .await
                        .map_err(db)?;
                    failed.push(item_id);
                }
            }
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let cancel = Cancel::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stopper.cancel();
        });
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
                "-",
            ],
            &cancel,
        )
        .await;
        assert_eq!(result, Err("cancelled".to_string()));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
