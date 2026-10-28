use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::warn;

/// Global semaphore to limit concurrent ffprobe process spawns.
/// Configurable via RUSTYFIN_MAX_PROBES env var; defaults to num_cpus * 2 (min 4, max 16).
static PROBE_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn get_max_probes() -> usize {
    std::env::var("RUSTYFIN_MAX_PROBES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            (cores * 2).clamp(4, 16)
        })
}

fn probe_semaphore() -> &'static Semaphore {
    PROBE_SEMAPHORE.get_or_init(|| Semaphore::const_new(get_max_probes()))
}

use rustyfin_model::chapter::Chapter;
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

/// Result of probing a media file with ffprobe.
#[derive(Debug)]
pub struct ProbeResult {
    pub source: MediaSourceResponse,
    /// Audio metadata tags extracted from the file (ID3, Vorbis, etc.)
    pub audio_tags: Option<AudioTags>,
    /// Chapter markers extracted from the file
    pub chapters: Vec<Chapter>,
}

/// Audio metadata tags extracted from ID3, Vorbis, APEv2, etc.
#[derive(Debug, Clone)]
pub struct AudioTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<String>,
    pub date: Option<String>,
    pub date_recorded: Option<String>,
    pub composer: Option<String>,
    pub encoded_by: Option<String>,
    pub copyright: Option<String>,
    pub comment: Option<String>,
    /// Base64-encoded album art cover (JPEG/PNG) if embedded
    pub album_art: Option<Vec<u8>>,
    /// MIME type of the album art
    pub album_art_mime: Option<String>,
}

// ---------------------------------------------------------------------------
// FFprobe JSON output structures (subset of what ffprobe -of json returns)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize, Clone)]
struct FfprobeStream {
    #[serde(default, deserialize_with = "deserialize_i32_flexible")]
    index: i32,
    codec_name: Option<String>,
    codec_type: Option<String>,
    language: Option<String>,
    #[allow(dead_code)]
    codec_tag_string: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    width: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    height: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    bit_rate: Option<u32>,
    bits_per_raw_sample: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    channels: Option<u32>,
    channel_layout: Option<String>,
    sample_rate: Option<String>,
    profile: Option<String>,
    disposition: Option<Disposition>,
    #[allow(dead_code)]
    r#type: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    time_base: Option<String>,
    codec_time_base: Option<String>,
    codec_tag: Option<String>,
    external: Option<bool>,
    path: Option<String>,
    title: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    /// Side data for attached pictures (album art)
    side_data_list: Option<Vec<SideData>>,
}

#[derive(Debug, Deserialize, Clone)]
struct SideData {
    side_data_type: Option<String>,
    attachment: Option<String>, // base64-encoded image data
    mime_type: Option<String>,
    filename: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Disposition {
    #[serde(default, deserialize_with = "deserialize_optional_u8")]
    default: Option<u8>,
    #[serde(default, deserialize_with = "deserialize_optional_u8")]
    forced: Option<u8>,
}

fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        U32(u32),
        I64(i64),
        F64(f64),
        Str(String),
        Null,
    }

    match Option::<NumOrStr>::deserialize(deserializer)? {
        Some(NumOrStr::U32(n)) => Ok(Some(n)),
        Some(NumOrStr::I64(n)) => Ok(if n >= 0 { Some(n as u32) } else { None }),
        Some(NumOrStr::F64(f)) => Ok(if f >= 0.0 { Some(f as u32) } else { None }),
        Some(NumOrStr::Str(s)) => {
            let s = s.trim();
            if s.is_empty() || s.eq_ignore_ascii_case("N/A") || s.eq_ignore_ascii_case("null") {
                Ok(None)
            } else if let Ok(n) = s.parse::<u32>() {
                Ok(Some(n))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(if f >= 0.0 { Some(f as u32) } else { None })
            } else {
                Ok(None)
            }
        }
        Some(NumOrStr::Null) | None => Ok(None),
    }
}

fn deserialize_optional_u8<'de, D>(deserializer: D) -> Result<Option<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStrOrBool {
        U8(u8),
        I64(i64),
        Bool(bool),
        Str(String),
        Null,
    }

    match Option::<NumOrStrOrBool>::deserialize(deserializer)? {
        Some(NumOrStrOrBool::U8(n)) => Ok(Some(n)),
        Some(NumOrStrOrBool::I64(n)) => Ok(if n > 0 { Some(1) } else { Some(0) }),
        Some(NumOrStrOrBool::Bool(b)) => Ok(Some(if b { 1 } else { 0 })),
        Some(NumOrStrOrBool::Str(s)) => {
            let s = s.trim();
            if s == "1" || s.eq_ignore_ascii_case("true") {
                Ok(Some(1))
            } else if s == "0" || s.eq_ignore_ascii_case("false") {
                Ok(Some(0))
            } else {
                Ok(None)
            }
        }
        Some(NumOrStrOrBool::Null) | None => Ok(None),
    }
}

fn deserialize_i32_flexible<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        I32(i32),
        F64(f64),
        Str(String),
    }

    match NumOrStr::deserialize(deserializer) {
        Ok(NumOrStr::I32(n)) => Ok(n),
        Ok(NumOrStr::F64(f)) => Ok(f as i32),
        Ok(NumOrStr::Str(s)) => Ok(s.trim().parse::<i32>().unwrap_or(0)),
        Err(_) => Ok(0),
    }
}

#[derive(Debug, Deserialize, Default)]
struct FfprobeFormat {
    duration: Option<String>,
    bit_rate: Option<String>,
    size: Option<String>,
    #[allow(dead_code)]
    format_name: Option<String>,
    #[allow(dead_code)]
    format_long_name: Option<String>,
    /// Metadata tags (ID3, Vorbis, APEv2, etc.)
    tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct FfprobeChapters {
    chapters: Vec<FfprobeChapter>,
}

#[derive(Debug, Deserialize)]
struct FfprobeChapter {
    #[allow(dead_code)]
    id: Option<String>,
    time_base: Option<String>,
    start: Option<String>,
    start_time: Option<String>,
    end: Option<String>,
    end_time: Option<String>,
    tags: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe a media file using ffprobe and return the parsed result.
pub async fn probe_file(path: &str) -> Result<ProbeResult, ProbeError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-i",
            path,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ProbeError::FfprobeNotFound(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProbeError::FfprobeFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let probe: FfprobeOutput =
        serde_json::from_str(&stdout).map_err(|e| ProbeError::ParseError(e.to_string()))?;

    let format = probe.format.unwrap_or_default();

    // Calculate duration in ticks (1 tick = 100 ns)
    let run_time_ticks = format
        .duration
        .as_ref()
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| (d * 10_000_000.0) as i64);

    // Calculate bitrates
    let total_bitrate = format.bit_rate.as_ref().and_then(|b| b.parse::<u32>().ok());

    let video_streams: Vec<&FfprobeStream> = probe
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("video"))
        .collect();
    let audio_streams: Vec<&FfprobeStream> = probe
        .streams
        .iter()
        .filter(|s| s.codec_type.as_deref() == Some("audio"))
        .collect();

    let video_bitrate: u32 = video_streams.iter().filter_map(|s| s.bit_rate).sum();
    let video_bitrate = if video_bitrate == 0 {
        None
    } else {
        Some(video_bitrate)
    };

    let audio_bitrate: u32 = audio_streams.iter().filter_map(|s| s.bit_rate).sum();
    let audio_bitrate = if audio_bitrate == 0 {
        None
    } else {
        Some(audio_bitrate)
    };

    // Get video dimensions from the first video stream
    let (width, height): (Option<u32>, Option<u32>) = video_streams
        .first()
        .and_then(|s| s.width.zip(s.height))
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));

    // Calculate aspect ratio
    let aspect_ratio = if let (Some(w), Some(h)) = (width, height) {
        if h > 0 {
            let gcd = gcd(w as u64, h as u64);
            Some(format!("{}/{}", w / gcd as u32, h / gcd as u32))
        } else {
            None
        }
    } else {
        None
    };

    // Container is the file extension
    let container = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());

    // Build media streams
    let media_streams: Vec<MediaStream> =
        probe.streams.iter().cloned().map(stream_to_model).collect();

    let source = MediaSourceResponse {
        id: uuid::Uuid::new_v4().to_string(),
        uid: uuid::Uuid::new_v4().to_string(),
        container,
        protocol: "File".to_string(),
        path: path.to_string(),
        r#type: "Default".to_string(),
        run_time_ticks,
        size: format.size.as_ref().and_then(|s| s.parse::<i64>().ok()),
        bitrate: total_bitrate,
        video_bitrate,
        audio_bitrate,
        width,
        height,
        aspect_ratio,
        is_remote: false,
        media_state: None,
        media_streams,
        supports_transcoding: true,
        supports_direct_stream: true,
        supports_direct_play: true,
    };

    // Extract audio metadata tags (ID3, Vorbis, APEv2, etc.)
    let audio_tags = extract_audio_tags(&format.tags, &probe.streams);

    // Extract chapters
    let chapters = extract_chapters(path).await;

    Ok(ProbeResult {
        source,
        audio_tags,
        chapters,
    })
}

/// Extract audio metadata tags from ffprobe format tags and stream side data.
fn extract_audio_tags(
    format_tags: &Option<HashMap<String, String>>,
    streams: &[FfprobeStream],
) -> Option<AudioTags> {
    let tags = match format_tags {
        Some(t) if !t.is_empty() => t,
        _ => return None,
    };

    let get = |key: &str| -> Option<String> {
        // ffprobe normalizes tag keys to lowercase
        tags.get(key)
            .or_else(|| {
                // Try case-insensitive lookup for common tag name variations
                tags.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(key))
                    .map(|(_, v)| v)
            })
            .cloned()
    };

    // Extract track number, which may include total (e.g., "1/12")
    let track_str = get("track").or_else(|| get("tracknumber"));
    let (track_number, track_total) = track_str.as_deref().map_or((None, None), |s| {
        if let Some((num, total)) = s.split_once('/') {
            (num.trim().parse().ok(), total.trim().parse().ok())
        } else {
            (s.trim().parse().ok(), None)
        }
    });

    // Extract disc number, which may include total (e.g., "1/2")
    let disc_str = get("disc").or_else(|| get("discnumber"));
    let (disc_number, disc_total) = disc_str.as_deref().map_or((None, None), |s| {
        if let Some((num, total)) = s.split_once('/') {
            (num.trim().parse().ok(), total.trim().parse().ok())
        } else {
            (s.trim().parse().ok(), None)
        }
    });

    // Extract album art from attached picture streams
    let (album_art, album_art_mime) = extract_album_art(streams);

    Some(AudioTags {
        title: get("title"),
        artist: get("artist"),
        album: get("album"),
        album_artist: get("albumartist").or_else(|| get("album_artist")),
        genre: get("genre"),
        track_number,
        track_total,
        disc_number,
        disc_total,
        year: get("year"),
        date: get("date"),
        date_recorded: get("date_recorded"),
        composer: get("composer"),
        encoded_by: get("encoded_by").or_else(|| get("encodedby")),
        copyright: get("copyright"),
        comment: get("comment"),
        album_art,
        album_art_mime,
    })
}

/// Extract album art from data streams with attached picture side data.
fn extract_album_art(streams: &[FfprobeStream]) -> (Option<Vec<u8>>, Option<String>) {
    for stream in streams {
        if let Some(ref side_data) = stream.side_data_list {
            for data in side_data {
                if let Some(ref side_type) = data.side_data_type
                    && side_type == "ATTACHMENT"
                    && let Some(ref b64) = data.attachment
                {
                    // Decode base64 attachment
                    if let Ok(decoded) = base64_decode(b64) {
                        return (
                            Some(decoded),
                            data.mime_type.clone().or_else(|| {
                                // Detect MIME from filename extension
                                data.filename.as_ref().and_then(|fn_| {
                                    if fn_.ends_with(".jpg") || fn_.ends_with(".jpeg") {
                                        Some("image/jpeg".to_string())
                                    } else if fn_.ends_with(".png") {
                                        Some("image/png".to_string())
                                    } else {
                                        None
                                    }
                                })
                            }),
                        );
                    }
                }
            }
        }
    }
    (None, None)
}

/// Extract chapter markers from a media file using ffprobe.
async fn extract_chapters(path: &str) -> Vec<Chapter> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_chapters",
            "-i",
            path,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let chapters: FfprobeChapters = match serde_json::from_str(&stdout) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();
    for (i, ch) in chapters.chapters.iter().enumerate() {
        // Parse start time from time_base and start, or from start_time directly
        let start_seconds = ch
            .start_time
            .as_ref()
            .and_then(|t| t.parse::<f64>().ok())
            .or_else(|| {
                ch.time_base.as_ref().and_then(|tb| {
                    let (_, denom) = tb.split_once('/')?;
                    let denom = denom.parse::<f64>().ok()?;
                    ch.start
                        .as_ref()
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(|s| s / denom)
                })
            })
            .unwrap_or(0.0);

        // Parse end time
        let end_seconds = ch
            .end_time
            .as_ref()
            .and_then(|t| t.parse::<f64>().ok())
            .or_else(|| {
                ch.time_base.as_ref().and_then(|tb| {
                    let (_, denom) = tb.split_once('/')?;
                    let denom = denom.parse::<f64>().ok()?;
                    ch.end
                        .as_ref()
                        .and_then(|e| e.parse::<f64>().ok())
                        .map(|e| e / denom)
                })
            })
            .unwrap_or(0.0);

        // Chapter name from tags
        let name = ch
            .tags
            .as_ref()
            .and_then(|tags| {
                tags.get("title").or_else(|| {
                    tags.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("title"))
                        .map(|(_, v)| v)
                })
            })
            .cloned()
            .unwrap_or_else(|| format!("Chapter {}", i + 1));

        result.push(Chapter {
            id: uuid::Uuid::new_v4().to_string(),
            item_id: String::new(), // will be set when saving
            chapter_number: (i + 1) as i32,
            name,
            start_position_ticks: (start_seconds * 10_000_000.0) as i64,
            end_position_ticks: (end_seconds * 10_000_000.0) as i64,
            image_path: None,
        });
    }

    result
}

/// Simple base64 decoder (avoids adding a dependency).
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(input.len() * 3 / 4);

    let bytes: Vec<u8> = input
        .bytes()
        .filter(|&b| b != b'\n' && b != b'\r' && b != b' ')
        .collect();
    let chunks = bytes.chunks(4);

    for chunk in chunks {
        let mut buf = [0u8; 4];
        let mut len = 0u32;
        for (i, &b) in chunk.iter().enumerate() {
            let val = alphabet.iter().position(|&c| c == b).unwrap_or(64); // '=' padding
            buf[i] = val as u8;
            len += 1;
        }
        // Combine 4 6-bit values into 3 bytes
        output.push(buf[0] << 2 | buf[1] >> 4);
        if len > 2 {
            output.push(buf[1] << 4 | buf[2] >> 2);
        }
        if len > 3 {
            output.push(buf[2] << 6 | buf[3]);
        }
    }

    Ok(output)
}

fn stream_to_model(s: FfprobeStream) -> MediaStream {
    let is_default = s.disposition.as_ref().and_then(|d| d.default).unwrap_or(0) > 0;
    let is_forced = s.disposition.as_ref().and_then(|d| d.forced).unwrap_or(0) > 0;

    // Determine subtitle type
    let subtitle_type = if s.codec_type.as_deref() == Some("subtitle") {
        if s.path.is_some() || s.external.unwrap_or(false) {
            Some("External".to_string())
        } else {
            Some("Embedded".to_string())
        }
    } else {
        None
    };

    // Determine if forced for subtitles
    let is_forced_sub = if s.codec_type.as_deref() == Some("subtitle") {
        is_forced
            || s.codec_name
                .as_ref()
                .map(|c| c.contains("forced"))
                .unwrap_or(false)
            || s.title
                .as_ref()
                .map(|t| t.contains("forced"))
                .unwrap_or(false)
    } else {
        false
    };

    MediaStream {
        index: s.index,
        codec: s.codec_name,
        codec_type: s.codec_type.unwrap_or_default(),
        language: s.language,
        time_base: s.time_base,
        codec_time_base: s.codec_time_base,
        codec_tag: s.codec_tag,
        is_external: s.external.unwrap_or(false),
        path: s.path,
        width: s.width,
        height: s.height,
        average_frame_rate: s.avg_frame_rate,
        real_frame_rate: s.r_frame_rate,
        bit_rate: s.bit_rate,
        bit_depth: s.bits_per_raw_sample.as_ref().and_then(|b| b.parse().ok()),
        channels: s.channels,
        channel_layout: s.channel_layout,
        sample_rate: s
            .sample_rate
            .as_ref()
            .and_then(|r| r.replace(" Hz", "").parse().ok()),
        profile: s.profile,
        r#type: subtitle_type,
        aspect_ratio_text: None,
        aspect_ratio: None,
        title: s.title,
        color_space: s.color_space,
        color_transfer: s.color_transfer,
        color_primaries: s.color_primaries,
        color_range: None,
        localized_uri: None,
        is_default,
        is_forced: is_forced_sub,
        is_interlaced: false,
        embedded_bitmaps: false,
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("ffprobe not found or failed to start: {0}")]
    FfprobeNotFound(String),

    #[error("ffprobe failed: {0}")]
    FfprobeFailed(String),

    #[error("Failed to parse ffprobe output: {0}")]
    ParseError(String),

    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),
}

// ---------------------------------------------------------------------------
// Convenience: probe and save to DB
// ---------------------------------------------------------------------------

/// Probe a file and save it as the primary (only) media source for an item.
/// Deletes any existing sources first.
pub async fn probe_and_save(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
) -> Result<(), ProbeError> {
    // Delete any existing sources for this item
    let _ = rustyfin_db::repo_media_source::delete_media_sources(pool, item_id).await;

    probe_and_add_source_internal(pool, item_id, path).await
}

/// Probe a file and add it as an additional media source for an item.
/// Does NOT delete existing sources — used for multiple versions of the same title.
pub async fn probe_and_add_source(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
) -> Result<(), ProbeError> {
    probe_and_add_source_internal(pool, item_id, path).await
}

/// Spawn a bounded probe task that respects the global concurrency limit.
/// Returns a JoinHandle so the caller can await it if needed.
pub fn spawn_probe_and_save(
    pool: sqlx::SqlitePool,
    item_id: uuid::Uuid,
    path: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Acquire a permit from the semaphore to limit concurrent probes
        let _permit = probe_semaphore().acquire().await;
        if let Err(e) = probe_and_save(&pool, &item_id, &path).await {
            tracing::warn!("failed to probe '{}': {}", path, e);
        }
    })
}

/// Spawn a bounded probe-and-add-source task that respects the global concurrency limit.
pub fn spawn_probe_and_add_source(
    pool: sqlx::SqlitePool,
    item_id: uuid::Uuid,
    path: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Acquire a permit from the semaphore to limit concurrent probes
        let _permit = probe_semaphore().acquire().await;
        if let Err(e) = probe_and_add_source(&pool, &item_id, &path).await {
            tracing::warn!("failed to probe '{}': {}", path, e);
        }
    })
}

/// Internal: probe a file and insert it as a media source for an item.
/// Uses a single transaction for source + streams + runtime update + chapters.
async fn probe_and_add_source_internal(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
) -> Result<(), ProbeError> {
    let result = probe_file(path).await?;
    let source = result.source;

    // Insert source and all streams in a single transaction
    let _source_id =
        rustyfin_db::repo_media_source::insert_media_source_with_streams(pool, item_id, &source)
            .await?;

    // Update the media item's runtime_ticks if we got them from probing
    if let Some(ticks) = source.run_time_ticks {
        let _ = sqlx::query("UPDATE media_items SET runtime_ticks = ? WHERE id = ?")
            .bind(ticks)
            .bind(item_id.to_string())
            .execute(pool)
            .await;
    }

    // Save chapters if any were found
    if !result.chapters.is_empty() {
        let item_id_str = item_id.to_string();
        let mut chapters = result.chapters;
        // Set the item_id on each chapter
        for ch in &mut chapters {
            ch.item_id = item_id_str.clone();
        }
        if let Err(e) = rustyfin_db::repo_chapter::save_chapters(pool, *item_id, chapters).await {
            warn!("failed to save chapters: {}", e);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gcd() {
        assert_eq!(gcd(1920, 1080), 120);
        assert_eq!(gcd(1280, 720), 80);
        assert_eq!(gcd(3840, 2160), 240);
        assert_eq!(gcd(480, 270), 30);
        assert_eq!(gcd(100, 0), 100);
        assert_eq!(gcd(0, 50), 50);
    }

    #[tokio::test]
    async fn test_probe_nonexistent_file() {
        let result = probe_file("/nonexistent/file.mp4").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_ffprobe_with_string_bitrate() {
        let json_data = r#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "hevc",
                    "codec_type": "video",
                    "width": "3840",
                    "height": "2160",
                    "bit_rate": "256000",
                    "channels": 6,
                    "disposition": {
                        "default": 1,
                        "forced": "0"
                    }
                },
                {
                    "index": "1",
                    "codec_name": "eac3",
                    "codec_type": "audio",
                    "bit_rate": "958433",
                    "channels": "6",
                    "disposition": {
                        "default": "true",
                        "forced": false
                    }
                },
                {
                    "index": 2,
                    "codec_name": "subrip",
                    "codec_type": "subtitle",
                    "bit_rate": "N/A"
                }
            ],
            "format": {
                "duration": "7200.5",
                "bit_rate": "1214433",
                "size": "1093029302"
            }
        }"#;

        let parsed: FfprobeOutput =
            serde_json::from_str(json_data).expect("should parse successfully");
        assert_eq!(parsed.streams.len(), 3);
        assert_eq!(parsed.streams[0].width, Some(3840));
        assert_eq!(parsed.streams[0].height, Some(2160));
        assert_eq!(parsed.streams[0].bit_rate, Some(256000));
        assert_eq!(parsed.streams[0].channels, Some(6));
        assert_eq!(
            parsed.streams[0].disposition.as_ref().unwrap().default,
            Some(1)
        );
        assert_eq!(
            parsed.streams[0].disposition.as_ref().unwrap().forced,
            Some(0)
        );

        assert_eq!(parsed.streams[1].index, 1);
        assert_eq!(parsed.streams[1].bit_rate, Some(958433));
        assert_eq!(parsed.streams[1].channels, Some(6));
        assert_eq!(
            parsed.streams[1].disposition.as_ref().unwrap().default,
            Some(1)
        );
        assert_eq!(
            parsed.streams[1].disposition.as_ref().unwrap().forced,
            Some(0)
        );

        assert_eq!(parsed.streams[2].bit_rate, None);
    }
}
