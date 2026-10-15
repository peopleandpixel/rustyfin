use serde::Deserialize;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{info, warn};

use sqlx::Row;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Detect theme songs across episodes of a series.
///
/// Uses a simple audio fingerprinting approach: extracts audio from the first 30 seconds
/// of each episode and compares fingerprints to find recurring theme music.
/// Returns the detected theme song start/end times in ticks.
pub async fn detect_theme_song(
    pool: &sqlx::SqlitePool,
    series_id: Uuid,
    _config: &DetectionConfig,
) -> Result<Option<(i64, i64)>, String> {
    use sqlx::Row;

    // Get season IDs for this series
    let series_id_str = series_id.to_string();
    let seasons: Vec<String> =
        sqlx::query("SELECT id FROM media_items WHERE parent_id = ?1 AND media_type = 'Season'")
            .bind(&series_id_str)
            .try_map(|row: sqlx::sqlite::SqliteRow| row.try_get("id"))
            .fetch_all(pool)
            .await
            .map_err(|e| format!("Query error: {}", e))?;

    if seasons.is_empty() {
        return Ok(None);
    }

    // Get episodes belonging to these seasons
    let mut episode_paths: Vec<(String, String)> = Vec::new();
    for season_id in &seasons {
        let rows = sqlx::query(
            "SELECT id, path FROM media_items WHERE parent_id = ?1 AND media_type = 'Episode'",
        )
        .bind(season_id)
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            Ok((
                row.try_get::<String, _>("id")?,
                row.try_get::<String, _>("path")?,
            ))
        })
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        episode_paths.extend(rows);
    }

    if episode_paths.len() < 2 {
        return Ok(None);
    }

    // Extract audio fingerprints from the first 30 seconds of each episode
    let theme_duration_seconds = 30.0;
    let mut fingerprints: Vec<Vec<u8>> = Vec::new();

    for (_ep_id, path) in &episode_paths {
        if let Some(fp) = extract_audio_fingerprint(path, theme_duration_seconds).await {
            fingerprints.push(fp);
        }
    }

    if fingerprints.len() < 2 {
        return Ok(None);
    }

    // Compare fingerprints to find the most common theme
    let mut best_match_count = 0;

    for i in 0..fingerprints.len() {
        let mut match_count = 1;
        for j in (i + 1)..fingerprints.len() {
            if fingerprint_similarity(&fingerprints[i], &fingerprints[j]) > 0.7 {
                match_count += 1;
            }
        }
        if match_count > best_match_count {
            best_match_count = match_count;
        }
    }

    // Only consider it a theme if at least 2 episodes match
    if best_match_count >= 2 {
        let end_ticks = (theme_duration_seconds * 10_000_000.0) as i64;
        tracing::info!(
            "Detected theme song for series {}: {} episodes match",
            series_id,
            best_match_count
        );
        Ok(Some((0, end_ticks)))
    } else {
        Ok(None)
    }
}

/// Extract a simple audio fingerprint from the first N seconds of a video file.
/// Uses FFmpeg to extract audio energy levels per frame as a fingerprint.
async fn extract_audio_fingerprint(path: &str, duration_seconds: f64) -> Option<Vec<u8>> {
    let output = Command::new("ffmpeg")
        .args([
            "-i",
            path,
            "-t",
            &duration_seconds.to_string(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "8000",
            "-f",
            "u8le",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let audio_data = output.stdout;
    if audio_data.is_empty() {
        return None;
    }

    // Create a simple fingerprint by dividing audio into 100ms buckets
    // and computing the average energy per bucket
    let sample_rate = 8000u32;
    let bucket_size = (sample_rate as f64 * 0.1) as usize; // 100ms buckets
    let mut fingerprint = Vec::new();

    for chunk in audio_data.chunks(bucket_size) {
        let mut energy = 0u32;
        for &sample in chunk {
            let val = (sample as i16 - 128) as i32;
            energy += (val * val) as u32;
        }
        let avg = if !chunk.is_empty() {
            energy / chunk.len() as u32
        } else {
            0
        };
        fingerprint.push(avg as u8);
    }

    Some(fingerprint)
}

/// Calculate similarity between two fingerprints using normalized cross-correlation.
fn fingerprint_similarity(a: &[u8], b: &[u8]) -> f64 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    let mut sum = 0.0;
    for i in 0..len {
        let diff = (a[i] as i32 - b[i] as i32).abs() as f64;
        sum += 1.0 - (diff / 255.0);
    }

    sum / len as f64
}

/// Configuration for intro/outro detection.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Maximum intro duration in seconds (default: 120s = 2 minutes)
    pub max_intro_duration: f64,
    /// Maximum outro duration in seconds (default: 240s = 4 minutes)
    pub max_outro_duration: f64,
    /// Minimum silence duration in seconds to consider as outro marker (default: 2s)
    pub min_silence_duration: f64,
    /// Noise threshold in dB for silence detection (default: -40dB)
    pub silence_noise_threshold: f64,
    /// Minimum black frame duration in seconds to consider (default: 2s)
    pub min_black_duration: f64,
    /// Black pixel threshold (0.0 to 1.0, default: 0.1)
    pub black_pixel_threshold: f64,
    /// Maximum intro position as percentage of episode (default: 0.25 = first 25%)
    pub max_intro_position_pct: f64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            max_intro_duration: 120.0,
            max_outro_duration: 240.0,
            min_silence_duration: 2.0,
            silence_noise_threshold: -40.0,
            min_black_duration: 2.0,
            black_pixel_threshold: 0.1,
            max_intro_position_pct: 0.25,
        }
    }
}

/// A detected segment with start and end times in seconds.
#[derive(Debug, Clone)]
pub struct DetectedSegment {
    pub start: f64,
    pub end: f64,
}

/// FFmpeg silencedetect output parsing structures.
#[derive(Debug, Deserialize)]
struct SilenceEvent {
    /// Silence start time in seconds
    silence_start: f64,
    /// Silence end time in seconds (may be absent if silence extends to end)
    silence_end: Option<f64>,
}

/// Detect intro and outro segments for a media file using FFmpeg filters.
///
/// This uses two detection methods:
/// 1. **Silence detection** for outros: Long silence periods near the end of the file indicate credits/outro.
/// 2. **Black frame detection** for intros/outros: Consecutive black frames at start/end indicate intro/outro boundaries.
///
/// Returns a tuple of (intro_segment, outro_segment), where each is None if not detected.
pub async fn detect_segments(
    path: &str,
    duration_seconds: f64,
    config: &DetectionConfig,
) -> Result<(Option<DetectedSegment>, Option<DetectedSegment>), String> {
    // Detect silence periods (for outro detection)
    let silence_events = detect_silence(path, config).await?;

    // Detect black frames (for intro/outro detection)
    let black_segments = detect_black_frames(path, config).await?;

    // Find intro from black frames at the beginning
    let intro = find_intro(&black_segments, duration_seconds, config);

    // Find outro from silence or black frames at the end
    let outro = find_outro(&silence_events, &black_segments, duration_seconds, config);

    Ok((intro, outro))
}

/// Run FFmpeg silencedetect filter and parse the output.
async fn detect_silence(path: &str, config: &DetectionConfig) -> Result<Vec<SilenceEvent>, String> {
    let args = format!(
        "-i {} -af \"silencedetect=noise={}dB:d={},ametadata=mode=print\" -f null -",
        path, config.silence_noise_threshold, config.min_silence_duration
    );

    let output = Command::new("ffmpeg")
        .args(args.split_whitespace())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("FFmpeg not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("FFmpeg silence detection failed: {}", stderr));
    }

    // Parse silence events from stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut events = Vec::new();

    for line in stderr.lines() {
        if line.contains("[silencedetect")
            && let Some(event) = parse_silence_line(line)
        {
            events.push(event);
        }
    }

    Ok(events)
}

/// Parse a single silence detection line from FFmpeg output.
fn parse_silence_line(line: &str) -> Option<SilenceEvent> {
    // FFmpeg output: [silencedetect @ 0x...] silence_start:1234.56
    //                [silencedetect @ 0x...] silence_end:1240.12 | silence_duration:5.56
    let mut silence_start: Option<f64> = None;
    let mut silence_end: Option<f64> = None;

    // Find silence_start value
    if let Some(pos) = line.find("silence_start:") {
        let val_str = &line[pos + 14..];
        let val_str = val_str.trim_start();
        if let Some(end_pos) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.') {
            if let Ok(val) = val_str[..end_pos].trim().parse::<f64>() {
                silence_start = Some(val);
            }
        } else if let Ok(val) = val_str.trim().parse::<f64>() {
            silence_start = Some(val);
        }
    }

    // Find silence_end value
    if let Some(pos) = line.find("silence_end:") {
        let val_str = &line[pos + 12..];
        let val_str = val_str.trim_start();
        if let Some(end_pos) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.') {
            if let Ok(val) = val_str[..end_pos].trim().parse::<f64>() {
                silence_end = Some(val);
            }
        } else if let Ok(val) = val_str.trim().parse::<f64>() {
            silence_end = Some(val);
        }
    }

    // Only return if we have a start time
    silence_start.map(|start| SilenceEvent {
        silence_start: start,
        silence_end,
    })
}

/// Run FFmpeg blackdetect filter and parse the output.
async fn detect_black_frames(
    path: &str,
    config: &DetectionConfig,
) -> Result<Vec<DetectedSegment>, String> {
    let args = format!(
        "-i {} -vf \"blackdetect=d={}:pix_th={}\" -an -f null -",
        path, config.min_black_duration, config.black_pixel_threshold
    );

    let output = Command::new("ffmpeg")
        .args(args.split_whitespace())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("FFmpeg not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("FFmpeg black frame detection failed: {}", stderr));
    }

    // Parse black frame events from stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut segments = Vec::new();
    let mut current_start: Option<f64> = None;

    for line in stderr.lines() {
        if line.contains("[blackdetect") {
            // Example: [blackdetect @ 0x...] black_start:1234.56
            //          [blackdetect @ 0x...] black_end:1240.12
            //          [blackdetect @ 0x...] black_duration:5.56
            if let Some(pos) = line.find("black_start:") {
                let val_str = &line[pos + 12..];
                if let Some(end_pos) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.')
                    && let Ok(start) = val_str[..end_pos].trim().parse::<f64>()
                {
                    current_start = Some(start);
                }
            }
            if let Some(pos) = line.find("black_end:") {
                let val_str = &line[pos + 10..];
                if let Some(end_pos) = val_str.find(|c: char| !c.is_ascii_digit() && c != '.')
                    && let Ok(end) = val_str[..end_pos].trim().parse::<f64>()
                    && let Some(start) = current_start.take()
                {
                    segments.push(DetectedSegment { start, end });
                }
            }
        }
    }

    // If we have an unclosed black segment, it extends to the end
    if let Some(start) = current_start {
        segments.push(DetectedSegment {
            start,
            end: f64::MAX, // Will be clamped later
        });
    }

    Ok(segments)
}

/// Find intro segment from black frames at the beginning of the file.
fn find_intro(
    black_segments: &[DetectedSegment],
    duration: f64,
    config: &DetectionConfig,
) -> Option<DetectedSegment> {
    let max_intro_time = duration * config.max_intro_position_pct;

    // Look for a black segment near the start of the file
    for segment in black_segments {
        // Intro should start near the beginning (within first 5 seconds)
        if segment.start <= 5.0 && segment.end > 0.0 {
            let end = segment
                .end
                .min(max_intro_time)
                .min(config.max_intro_duration);
            if end - segment.start >= 3.0 {
                // At least 3 seconds to be considered an intro
                return Some(DetectedSegment {
                    start: segment.start,
                    end,
                });
            }
        }
    }

    None
}

/// Find outro segment from silence or black frames at the end of the file.
fn find_outro(
    silence_events: &[SilenceEvent],
    black_segments: &[DetectedSegment],
    duration: f64,
    config: &DetectionConfig,
) -> Option<DetectedSegment> {
    // First, try to find a silence period near the end (credits usually have silence before them)
    for event in silence_events {
        let silence_end = event.silence_end.unwrap_or(duration);
        // Check if silence is near the end of the file
        if silence_end >= duration * 0.7 {
            // Silence starts the outro/credits section
            let outro_start = event.silence_start;
            let outro_end = silence_end.min(duration);
            let outro_duration = outro_end - outro_start;

            if outro_duration <= config.max_outro_duration
                && outro_duration >= config.min_silence_duration
            {
                return Some(DetectedSegment {
                    start: outro_start,
                    end: outro_end,
                });
            }
        }
    }

    // If no silence found, look for black frames near the end
    for segment in black_segments.iter().rev() {
        let end = segment.end.min(duration);
        // Black segment near the end indicates credits/outro
        if segment.start >= duration * 0.7 {
            let outro_duration = end - segment.start;
            if outro_duration <= config.max_outro_duration && outro_duration >= 3.0 {
                return Some(DetectedSegment {
                    start: segment.start,
                    end,
                });
            }
        }
    }

    None
}

/// Detect and save segments for a media item to the database.
pub async fn detect_and_save(
    pool: &SqlitePool,
    item_id: Uuid,
    path: &str,
    duration_seconds: f64,
) -> Result<(), String> {
    let config = DetectionConfig::default();

    info!(
        "Detecting segments for item {} (duration: {:.1}s)",
        item_id, duration_seconds
    );

    let (intro, outro) = detect_segments(path, duration_seconds, &config).await?;

    // Save intro if detected
    if let Some(seg) = intro {
        let start_ticks = (seg.start * 10_000_000.0) as i64;
        let end_ticks = (seg.end * 10_000_000.0) as i64;
        rustyfin_db::repo_media_segment::save_media_segment(
            pool,
            item_id,
            "Intro",
            start_ticks,
            end_ticks,
        )
        .await
        .map_err(|e| format!("Failed to save intro segment: {}", e))?;

        // Also update the MediaItem's intro fields
        sqlx::query(
            "UPDATE media_items SET intro_start_time_ticks = ?1, intro_end_time_ticks = ?2 WHERE id = ?3",
        )
        .bind(start_ticks)
        .bind(end_ticks)
        .bind(item_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| format!("Failed to update intro ticks: {}", e))?;

        info!(
            "Item {}: Intro detected {:.1}s - {:.1}s",
            item_id, seg.start, seg.end
        );
    }

    // Save outro if detected
    if let Some(seg) = outro {
        let start_ticks = (seg.start * 10_000_000.0) as i64;
        let end_ticks = (seg.end * 10_000_000.0) as i64;
        rustyfin_db::repo_media_segment::save_media_segment(
            pool,
            item_id,
            "Outro",
            start_ticks,
            end_ticks,
        )
        .await
        .map_err(|e| format!("Failed to save outro segment: {}", e))?;

        info!(
            "Item {}: Outro detected {:.1}s - {:.1}s",
            item_id, seg.start, seg.end
        );
    }

    Ok(())
}

/// Batch detect segments for all episodes in a library.
pub async fn detect_for_all_episodes(pool: &SqlitePool) -> Result<usize, String> {
    // Get all episodes with paths and runtime
    let episodes: Vec<(String, String, Option<i64>)> = sqlx::query(
        "SELECT id, path, runtime_ticks FROM media_items WHERE media_type = 'Episode' AND path IS NOT NULL",
    )
    .try_map(|row: sqlx::sqlite::SqliteRow| {
        let id: String = row.try_get("id")?;
        let path: String = row.try_get("path")?;
        let runtime_ticks: Option<i64> = row.try_get("runtime_ticks")?;
        Ok((id, path, runtime_ticks))
    })
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to fetch episodes: {}", e))?;

    let mut count = 0;
    for (id_str, path, runtime_ticks) in &episodes {
        let item_id = match Uuid::parse_str(id_str) {
            Ok(id) => id,
            Err(e) => {
                warn!("Invalid UUID {}: {}", id_str, e);
                continue;
            }
        };

        // Calculate duration from runtime_ticks or skip if unknown
        let duration = match runtime_ticks {
            Some(ticks) if *ticks > 0 => *ticks as f64 / 10_000_000.0,
            _ => {
                warn!("Skipping {}: no runtime ticks", id_str);
                continue;
            }
        };

        match detect_and_save(pool, item_id, path, duration).await {
            Ok(()) => count += 1,
            Err(e) => warn!("Failed to detect segments for {}: {}", id_str, e),
        }
    }

    info!("Detected segments for {} episodes", count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_silence_line_start() {
        let line = "[silencedetect @ 0x555555555555] silence_start: 1234.56";
        let event = parse_silence_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert!((event.silence_start - 1234.56).abs() < 0.01);
        assert!(event.silence_end.is_none());
    }

    #[test]
    fn test_parse_silence_line_end() {
        let line = "[silencedetect @ 0x555555555555] silence_end: 1240.12 | silence_duration: 5.56";
        let event = parse_silence_line(line);
        // This line has no silence_start, so it returns None
        assert!(event.is_none());
    }

    #[test]
    fn test_find_intro_near_start() {
        let black_segments = vec![DetectedSegment {
            start: 0.0,
            end: 60.0,
        }];
        let config = DetectionConfig::default();
        let intro = find_intro(&black_segments, 2600.0, &config);
        assert!(intro.is_some());
        let intro = intro.unwrap();
        assert!(intro.start <= 5.0);
        assert!(intro.end <= 120.0); // max_intro_duration
    }

    #[test]
    fn test_find_intro_too_short() {
        let black_segments = vec![DetectedSegment {
            start: 0.0,
            end: 2.0,
        }];
        let config = DetectionConfig::default();
        let intro = find_intro(&black_segments, 2600.0, &config);
        // Less than 3 seconds, should not be considered an intro
        assert!(intro.is_none());
    }

    #[test]
    fn test_find_outro_from_silence() {
        let silence_events = vec![SilenceEvent {
            silence_start: 2400.0,
            silence_end: Some(2410.0),
        }];
        let black_segments = vec![];
        let config = DetectionConfig::default();
        let outro = find_outro(&silence_events, &black_segments, 2600.0, &config);
        // Silence at 2400s is after 70% of 2600s (1820s), so it should be detected
        assert!(outro.is_some());
    }

    #[test]
    fn test_find_outro_from_black_frames() {
        let silence_events = vec![];
        let black_segments = vec![DetectedSegment {
            start: 2400.0,
            end: 2500.0,
        }];
        let config = DetectionConfig::default();
        let outro = find_outro(&silence_events, &black_segments, 2600.0, &config);
        // Black frames at 2400s is after 70% of 2600s (1820s)
        assert!(outro.is_some());
    }
}
