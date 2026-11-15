//! Trickplay tiles: the seek-bar thumbnails Jellyfin clients show, made in the background.
//!
//! Like Jellyfin, a tile set holds one thumbnail every 10 seconds, 320 pixels wide, laid out 10 by
//! 10 in each JPEG tile, under `<data dir>/trickplay/<item id>/<width> - 10x10/<index>.jpg`. Tiles
//! are only made by the "Generate Trickplay Images" task, never on a request, and every ffmpeg
//! run here and in `chapter_images` holds `ffmpeg_lock`, so at most one runs at a time.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::Mutex;
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

/// Held around every ffmpeg run that makes trickplay tiles or chapter images, so only one runs at
/// a time, whichever task starts it.
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

/// Runs ffmpeg with `args`, and fails on a non-zero exit or after `FFMPEG_TIMEOUT`. The caller
/// holds `ffmpeg_lock`.
pub(crate) async fn run_ffmpeg(args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(format!("ffmpeg could not start: {e}")),
        Err(_) => return Err("ffmpeg took too long and was stopped".to_string()),
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

    let result = make_tiles(path, &thumbs, &tiles).await;
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
async fn make_tiles(path: &str, thumbs: &Path, tiles: &Path) -> Result<TrickplayInfo, String> {
    // Only key frames are decoded, which is many times faster than every frame; the fps filter
    // repeats the last one where they are further apart than the interval.
    let filter = format!("fps=1000/{INTERVAL_MS},scale={WIDTH}:-2");
    let pattern = thumbs.join("%06d.jpg");
    let pattern = pattern.to_string_lossy();
    run_ffmpeg(&[
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
    ])
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
    run_ffmpeg(&[
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
    ])
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
/// returns how many it made. A video that fails is logged and left for the next run.
pub async fn generate_for_libraries(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
) -> Result<u32, String> {
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
            match generate(pool, data_dir, item_id, &path).await {
                Ok(set) => {
                    made += 1;
                    info!(item = %item_id, thumbnails = set.thumbnail_count, "made trickplay tiles");
                }
                Err(e) => {
                    warn!(item = %item_id, path = %path, error = %e, "could not make trickplay tiles");
                    failed.push(item_id);
                }
            }
        }
    }
    Ok(made)
}
