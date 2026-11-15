//! Chapter images, and the one-time re-read of chapters for videos probed before they were read.
//!
//! Chapter images are made only by the "Extract Chapter Images" task, never on a request, one
//! ffmpeg at a time (`trickplay::ffmpeg_lock`). Each is a frame at the chapter's start, saved as
//! `<data dir>/chapters/<item id>/<start ticks>.jpg`.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;

use crate::trickplay::{ffmpeg_lock, run_ffmpeg};

/// The widest a chapter image is made, in pixels.
const MAX_WIDTH: u32 = 640;

/// The file of the image of the chapter that starts at `start_ticks` in item `item_id`.
pub fn image_path(data_dir: &Path, item_id: Uuid, start_ticks: i64) -> PathBuf {
    data_dir
        .join("chapters")
        .join(item_id.to_string())
        .join(format!("{start_ticks}.jpg"))
}

/// Makes the images of item `item_id`'s chapters that have none tried yet, from its file at
/// `path`, and returns how many it made. A chapter whose image can't be made is marked so it isn't
/// tried again. Waits for `ffmpeg_lock`.
pub async fn extract_for_item(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
) -> Result<u32, String> {
    let chapters = rustyfin_db::repo_chapter::get_chapters(pool, item_id)
        .await
        .map_err(|e| format!("could not read the chapters: {e}"))?;
    let mut made = 0;
    let _one_at_a_time = ffmpeg_lock().lock().await;
    for chapter in chapters.iter().filter(|c| c.image_path.is_none()) {
        let file = image_path(data_dir, item_id, chapter.start_position_ticks);
        let made_ok = match extract_one(path, chapter.start_position_ticks, &file).await {
            Ok(()) => true,
            Err(e) => {
                warn!(item = %item_id, chapter = chapter.chapter_number, error = %e,
                      "could not make a chapter image");
                false
            }
        };
        let (stored, date) = if made_ok {
            made += 1;
            (
                file.to_string_lossy().into_owned(),
                Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            )
        } else {
            (String::new(), None)
        };
        rustyfin_db::repo_chapter::set_chapter_image(
            pool,
            item_id,
            chapter.chapter_number,
            &stored,
            date.as_deref(),
        )
        .await
        .map_err(|e| format!("could not save a chapter image: {e}"))?;
    }
    Ok(made)
}

/// Writes the frame at `start_ticks` of the video at `path` to `file`, at most `MAX_WIDTH` wide.
async fn extract_one(path: &str, start_ticks: i64, file: &Path) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    let seconds = format!("{:.3}", start_ticks as f64 / 10_000_000.0);
    let scale = format!("scale=w='min(iw,{MAX_WIDTH})':h=-2");
    let out = file.to_string_lossy();
    run_ffmpeg(&[
        "-threads",
        "1",
        "-ss",
        &seconds,
        "-i",
        path,
        "-map",
        "0:V:0",
        "-an",
        "-sn",
        "-dn",
        "-frames:v",
        "1",
        "-vf",
        &scale,
        "-qscale:v",
        "2",
        "-f",
        "image2",
        &out,
    ])
    .await?;
    match fs::metadata(file).await {
        Ok(meta) if meta.len() > 0 => Ok(()),
        _ => Err("ffmpeg wrote no image".to_string()),
    }
}

/// Makes the missing chapter images of the videos in the libraries `library_ids`, one video at a
/// time, and returns how many images it made.
pub async fn extract_for_libraries(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
) -> Result<u32, String> {
    let mut made = 0;
    loop {
        let batch = rustyfin_db::repo_chapter::items_missing_chapter_images(pool, library_ids, 20)
            .await
            .map_err(|e| format!("could not list videos: {e}"))?;
        if batch.is_empty() {
            break;
        }
        for (item_id, path) in batch {
            // Every chapter it doesn't fail on is marked, so the loop moves on.
            match extract_for_item(pool, data_dir, item_id, &path).await {
                Ok(n) => made += n,
                Err(e) => {
                    warn!(item = %item_id, error = %e, "could not make chapter images");
                    return Err(e);
                }
            }
        }
    }
    if made > 0 {
        info!(images = made, "made chapter images");
    }
    Ok(made)
}

/// Reads the chapters of the videos on the re-read list, one at a time, and takes each off the
/// list. Videos probed before chapters could be read get theirs this way without a rescan.
/// Returns how many videos had chapters.
pub async fn reread_chapters(pool: &SqlitePool) -> Result<u32, String> {
    let mut with_chapters = 0;
    loop {
        let batch = rustyfin_db::repo_chapter::pending_chapter_rereads(pool, 50)
            .await
            .map_err(|e| format!("could not list videos: {e}"))?;
        if batch.is_empty() {
            break;
        }
        for (item_id, path) in batch {
            let chapters = crate::probe::read_chapters(&path).await;
            if !chapters.is_empty() {
                with_chapters += 1;
                // A probe since the list was made has saved them already.
                let stored = rustyfin_db::repo_chapter::get_chapters(pool, item_id)
                    .await
                    .map_err(|e| format!("could not read the chapters: {e}"))?;
                if stored.is_empty() {
                    crate::probe::save_chapters(pool, &item_id, chapters).await;
                }
            }
            rustyfin_db::repo_chapter::finish_chapter_reread(pool, item_id)
                .await
                .map_err(|e| format!("could not update the re-read list: {e}"))?;
        }
    }
    if with_chapters > 0 {
        info!(
            videos = with_chapters,
            "read the chapters of videos probed before"
        );
    }
    Ok(with_chapters)
}
