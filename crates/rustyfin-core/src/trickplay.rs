use std::path::Path;
use std::process::Stdio;
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

use sqlx::SqlitePool;
use uuid::Uuid;

use rustyfin_model::trickplay::TrickplayInfo;

/// Configuration for trickplay thumbnail generation.
#[derive(Debug, Clone)]
pub struct TrickplayConfig {
    /// Thumbnail width (default: 80)
    pub width: u32,
    /// Thumbnail height (default: 45)
    pub height: u32,
    /// Interval between thumbnails in seconds (default: 5s)
    pub interval_seconds: f64,
    /// Base directory for storing trickplay thumbnails
    pub base_directory: String,
}

impl Default for TrickplayConfig {
    fn default() -> Self {
        Self {
            width: 80,
            height: 45,
            interval_seconds: 5.0,
            base_directory: "data/trickplay".to_string(),
        }
    }
}

/// Generate trickplay thumbnails for a media file.
///
/// Uses FFmpeg to extract frames at regular intervals and saves them as numbered PNG files
/// in a dedicated directory. The directory path and metadata are saved to the database.
pub async fn generate_trickplay(
    pool: &SqlitePool,
    item_id: Uuid,
    path: &str,
    duration_seconds: f64,
    config: &TrickplayConfig,
) -> Result<TrickplayInfo, String> {
    // Create a unique directory for this item's trickplay
    let dir_name = format!("{}_{}", item_id, uuid::Uuid::new_v4().hyphenated());
    let trickplay_dir = format!("{}/{}", config.base_directory, dir_name);

    // Ensure the base directory exists
    fs::create_dir_all(&config.base_directory)
        .await
        .map_err(|e| format!("Failed to create trickplay directory: {}", e))?;

    // Calculate the number of thumbnails
    let thumbnail_count = (duration_seconds / config.interval_seconds).ceil() as u32;
    if thumbnail_count == 0 {
        return Err("Duration too short for trickplay".to_string());
    }

    // Generate thumbnails using FFmpeg
    let filter = format!(
        "scale={}:-2,fps=1/{}",
        config.width, config.interval_seconds as u32
    );

    let output_pattern = format!("{}/%04d.png", trickplay_dir);

    let output = Command::new("ffmpeg")
        .args([
            "-i",
            path,
            "-vf",
            &filter,
            "-qscale:v",
            "2",
            "-start_number",
            "0",
            "-frames:v",
            &thumbnail_count.to_string(),
            &output_pattern,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("FFmpeg not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up the directory if it was created
        let _ = fs::remove_dir_all(&trickplay_dir).await;
        return Err(format!("FFmpeg trickplay generation failed: {}", stderr));
    }

    // Count actual generated thumbnails
    let actual_count = count_png_files(&trickplay_dir).await.unwrap_or(0);
    if actual_count == 0 {
        let _ = fs::remove_dir_all(&trickplay_dir).await;
        return Err("No thumbnails generated".to_string());
    }

    // Calculate interval in ticks
    let interval_ticks = (config.interval_seconds * 10_000_000.0) as i64;

    let info = TrickplayInfo {
        item_id: item_id.to_string(),
        width: config.width,
        height: config.height,
        interval_ticks,
        thumbnail_count: actual_count,
        directory_path: trickplay_dir.clone(),
    };

    // Save to database
    if let Err(e) = rustyfin_db::repo_trickplay::save_trickplay(pool, item_id, &info).await {
        warn!("Failed to save trickplay info: {}", e);
    }

    info!(
        "Generated {} trickplay thumbnails for item {} at {}",
        actual_count, item_id, trickplay_dir
    );

    Ok(info)
}

/// Count PNG files in a directory.
async fn count_png_files(dir: &str) -> Result<u32, std::io::Error> {
    let mut entries = fs::read_dir(dir).await?;
    let mut count = 0u32;
    while let Some(entry) = entries.next_entry().await? {
        if let Some(ext) = entry.file_name().to_str()
            && ext.ends_with(".png")
        {
            count += 1;
        }
    }
    Ok(count)
}

/// Get the trickplay thumbnail file path for a given seek position.
///
/// The client requests a specific width and seek position (in ticks).
/// We find the matching trickplay directory and return the appropriate thumbnail file.
pub async fn get_trickplay_thumbnail(
    pool: &SqlitePool,
    item_id: Uuid,
    _width: u32,
    seek_ticks: i64,
) -> Result<Option<(String, String)>, String> {
    let trickplay = rustyfin_db::repo_trickplay::get_trickplay(pool, item_id)
        .await
        .map_err(|e| format!("Database error: {}", e))?;

    let trickplay = match trickplay {
        Some(t) => t,
        None => return Ok(None),
    };

    // Calculate which thumbnail index corresponds to the seek position
    let index = if trickplay.interval_ticks > 0 {
        (seek_ticks as f64 / trickplay.interval_ticks as f64).floor() as u32
    } else {
        0
    };

    // Clamp to available thumbnails
    if index >= trickplay.thumbnail_count {
        return Ok(None);
    }

    // Build the file path (thumbnails are named 0000.png, 0001.png, etc.)
    let file_path = format!("{}/{:04}.png", trickplay.directory_path, index);

    // Check if the file exists
    if Path::new(&file_path).exists() {
        Ok(Some((file_path, "image/png".to_string())))
    } else {
        Ok(None)
    }
}

/// Generate trickplay for all video items in the database.
pub async fn generate_for_all_videos(pool: &SqlitePool) -> Result<u32, String> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT id, path, runtime_ticks FROM media_items
         WHERE media_type = 'Movie' OR media_type = 'Episode'
         AND runtime_ticks IS NOT NULL AND runtime_ticks > 0",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Query error: {}", e))?;

    let config = TrickplayConfig::default();
    let mut count = 0u32;

    for row in &rows {
        let id_str: String = row.try_get("id").map_err(|e| e.to_string())?;
        let path: String = row.try_get("path").map_err(|e| e.to_string())?;
        let runtime_ticks: i64 = row.try_get("runtime_ticks").map_err(|e| e.to_string())?;

        let item_id = match Uuid::parse_str(&id_str) {
            Ok(id) => id,
            Err(_) => continue,
        };

        let duration_seconds = runtime_ticks as f64 / 10_000_000.0;

        // Skip if trickplay already exists
        if let Ok(Some(_)) = rustyfin_db::repo_trickplay::get_trickplay(pool, item_id).await {
            continue;
        }

        if let Err(e) = generate_trickplay(pool, item_id, &path, duration_seconds, &config).await {
            warn!("Failed to generate trickplay for {}: {}", id_str, e);
        } else {
            count += 1;
        }
    }

    Ok(count)
}
