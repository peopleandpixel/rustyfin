//! Chapter images, and the one-time re-read of chapters for videos probed before they were read.
//!
//! Chapter images are made only by the "Extract Chapter Images" task, never on a request, one
//! item at a time, as one image job (`trickplay::image_job`). Each is a frame at the chapter's
//! start, saved as `<data dir>/chapters/<item id>/<start ticks>.jpg`. A chapter whose image fails
//! is marked with an empty path; the mark lasts until the file changes, as a re-probe replaces
//! the chapters.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;

use crate::trickplay::{Cancel, image_job, run_ffmpeg};

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
/// tried again. Waits for an `image_job`. Once `cancel` fires it stops, marking nothing.
pub async fn extract_for_item(
    pool: &SqlitePool,
    data_dir: &Path,
    item_id: Uuid,
    path: &str,
    cancel: &Cancel,
) -> Result<u32, String> {
    let chapters = rustyfin_db::repo_chapter::get_chapters(pool, item_id)
        .await
        .map_err(|e| format!("could not read the chapters: {e}"))?;
    let mut made = 0;
    let mut tried = false;
    let Some(_job) = image_job(cancel).await else {
        return Ok(made);
    };
    for chapter in chapters.iter().filter(|c| c.image_path.is_none()) {
        let file = image_path(data_dir, item_id, chapter.start_position_ticks);
        let made_ok = match extract_one(path, chapter.start_position_ticks, &file, cancel).await {
            Ok(()) => true,
            Err(_) if cancel.is_cancelled() => return Ok(made),
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
        tried = true;
    }
    if tried {
        // A re-probe of this same file keeps the images (and the failure marks).
        let version = crate::trickplay::file_version(path).await;
        rustyfin_db::repo_trickplay::set_image_version(pool, item_id, Some(version))
            .await
            .map_err(|e| format!("could not save the file version: {e}"))?;
    }
    Ok(made)
}

/// Writes the frame at `start_ticks` of the video at `path` to `file`, at most `MAX_WIDTH` wide.
async fn extract_one(
    path: &str,
    start_ticks: i64,
    file: &Path,
    cancel: &Cancel,
) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    }
    let seconds = format!("{:.3}", start_ticks as f64 / 10_000_000.0);
    let scale = format!("scale=w='min(iw,{MAX_WIDTH})':h=-2");
    let out = file.to_string_lossy();
    run_ffmpeg(
        &[
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
        ],
        cancel,
    )
    .await?;
    match fs::metadata(file).await {
        Ok(meta) if meta.len() > 0 => Ok(()),
        _ => Err("ffmpeg wrote no image".to_string()),
    }
}

/// Makes the missing chapter images of the videos in the libraries `library_ids`, one video at a
/// time, and returns how many images it made. Stops between videos once `cancel` fires.
pub async fn extract_for_libraries(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
    cancel: &Cancel,
) -> Result<u32, String> {
    let progress = crate::task_progress::TaskProgress::new();
    extract_for_libraries_with_progress(pool, data_dir, library_ids, cancel, &progress).await
}

/// `extract_for_libraries`, reporting the share of the videos missing chapter images at the
/// start that it has looked at on `progress`.
pub async fn extract_for_libraries_with_progress(
    pool: &SqlitePool,
    data_dir: &Path,
    library_ids: &[String],
    cancel: &Cancel,
    progress: &crate::task_progress::TaskProgress,
) -> Result<u32, String> {
    let mut made = 0;
    let total = rustyfin_db::repo_chapter::items_missing_chapter_images(pool, library_ids, -1)
        .await
        .map_err(|e| format!("could not list videos: {e}"))?
        .len();
    let mut done = 0usize;
    progress.report_count(0, total);
    loop {
        let batch = rustyfin_db::repo_chapter::items_missing_chapter_images(pool, library_ids, 20)
            .await
            .map_err(|e| format!("could not list videos: {e}"))?;
        if batch.is_empty() {
            break;
        }
        for (item_id, path) in batch {
            if cancel.is_cancelled() {
                return Ok(made);
            }
            // Videos added during the run can take it past the total counted at the start
            progress.report_count(done.min(total), total);
            done += 1;
            // Every chapter is marked, made or failed, so the loop moves on.
            match extract_for_item(pool, data_dir, item_id, &path, cancel).await {
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
/// Returns how many videos had chapters. Stops once `cancel` fires (at shutdown), leaving the
/// video being read on the list.
pub async fn reread_chapters(pool: &SqlitePool, cancel: &Cancel) -> Result<u32, String> {
    let mut with_chapters = 0;
    loop {
        let batch = rustyfin_db::repo_chapter::pending_chapter_rereads(pool, 50)
            .await
            .map_err(|e| format!("could not list videos: {e}"))?;
        if batch.is_empty() {
            break;
        }
        for (item_id, path) in batch {
            let chapters = tokio::select! {
                chapters = crate::probe::read_chapters(&path) => chapters,
                _ = cancel.cancelled() => return Ok(with_chapters),
            };
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
