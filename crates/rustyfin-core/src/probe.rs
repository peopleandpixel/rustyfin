use serde::Deserialize;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::scan_sync;

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
    /// Codec name of the attached picture (cover art) stream, e.g. `mjpeg` or `png`
    pub attached_picture_codec: Option<String>,
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
    /// The codec level; ffprobe writes -99 when it has none.
    #[serde(default, deserialize_with = "deserialize_optional_u32")]
    level: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    display_aspect_ratio: Option<String>,
    field_order: Option<String>,
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
    /// Tags such as `language` and `title`. ffprobe nests a stream's language and title here.
    #[serde(default)]
    tags: Option<HashMap<String, String>>,
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
    #[serde(default, deserialize_with = "deserialize_optional_u8")]
    attached_pic: Option<u8>,
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

/// One entry of ffprobe's `-show_chapters` output. ffprobe writes `id`, `start` and `end` as
/// JSON numbers and the times as strings, so only the string fields are read; the times carry
/// the same positions as `start` and `end` over `time_base`.
#[derive(Debug, Deserialize)]
struct FfprobeChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    tags: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe a media file using ffprobe and return the parsed result. One ffprobe run reads the
/// format, the streams and the chapters, at a low CPU priority.
pub async fn probe_file(path: &str) -> Result<ProbeResult, ProbeError> {
    let mut cmd = Command::new("ffprobe");
    cmd.args([
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-i",
        path,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    crate::media_reads::lower_priority(&mut cmd);
    let output = cmd
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

    let container = container_name(
        path,
        format.format_name.as_deref(),
        // Cover art is a video stream too, but doesn't make the file a video
        video_streams
            .iter()
            .any(|s| s.disposition.as_ref().and_then(|d| d.attached_pic) != Some(1)),
    );

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
    let chapters = parse_chapters(&stdout);

    // The codec of the attached picture (cover art) stream, if the file has one.
    let attached_picture_codec = probe
        .streams
        .iter()
        .find(|s| {
            s.disposition
                .as_ref()
                .and_then(|d| d.attached_pic)
                .unwrap_or(0)
                > 0
        })
        .and_then(|s| s.codec_name.clone());

    Ok(ProbeResult {
        source,
        audio_tags,
        chapters,
        attached_picture_codec,
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

/// Reads the chapter markers of a media file with ffprobe; none when it can't.
pub(crate) async fn read_chapters(path: &str) -> Vec<Chapter> {
    read_chapters_with("ffprobe", path, CHAPTER_READ_TIMEOUT).await
}

/// The longest one chapter read may take, so a stalled file can't hold up the re-read list.
const CHAPTER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `read_chapters` with the ffprobe `program` and a `timeout`, after which ffprobe is killed and
/// no chapters are given.
async fn read_chapters_with(
    program: &str,
    path: &str,
    timeout: std::time::Duration,
) -> Vec<Chapter> {
    let run = Command::new(program)
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
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(timeout, run).await {
        Ok(Ok(o)) if o.status.success() => o,
        Ok(_) => return Vec::new(),
        Err(_) => {
            warn!(
                path,
                "reading the chapters took too long; ffprobe was stopped"
            );
            return Vec::new();
        }
    };

    parse_chapters(&String::from_utf8_lossy(&output.stdout))
}

/// Parses ffprobe's `-show_chapters` JSON into chapters, like Jellyfin: positions are rounded to
/// the millisecond, and a chapter without a title, or titled with a time ("00:05:00.000", as some
/// rippers write), is named "Chapter N".
fn parse_chapters(json: &str) -> Vec<Chapter> {
    let chapters: FfprobeChapters = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(e) => {
            warn!("could not read ffprobe's chapters: {}", e);
            return Vec::new();
        }
    };
    let ticks = |t: &Option<String>| {
        t.as_deref()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map(|s| (s * 1000.0).round() as i64 * 10_000)
            .unwrap_or(0)
    };

    chapters
        .chapters
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let title = ch.tags.as_ref().and_then(|tags| {
                tags.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("title"))
                    .map(|(_, v)| v.trim())
            });
            let name = match title {
                Some(t) if !t.is_empty() && !looks_like_time(t) => t.to_string(),
                _ => format!("Chapter {}", i + 1),
            };
            Chapter {
                id: uuid::Uuid::new_v4().to_string(),
                item_id: String::new(), // will be set when saving
                chapter_number: (i + 1) as i32,
                name,
                start_position_ticks: ticks(&ch.start_time),
                end_position_ticks: ticks(&ch.end_time),
                image_path: None,
                image_date_modified: None,
            }
        })
        .collect()
}

/// Whether a chapter title is a time span such as "00:05:00.000" or "12", the titles Jellyfin
/// replaces because .NET's `TimeSpan.TryParse` accepts them.
fn looks_like_time(title: &str) -> bool {
    let body = title.strip_prefix('-').unwrap_or(title);
    let mut parts = body.split(':');
    let count = body.split(':').count();
    count <= 4
        && parts.all(|p| {
            let (whole, frac) = p.split_once('.').unwrap_or((p, "0"));
            !whole.is_empty()
                && whole.bytes().all(|b| b.is_ascii_digit())
                && frac.bytes().all(|b| b.is_ascii_digit())
                && !frac.is_empty()
        })
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

/// Looks up an ffprobe tag by key, ignoring ASCII case in the key. An exact match wins over a
/// case-insensitive one.
fn tag<'a>(tags: Option<&'a HashMap<String, String>>, key: &str) -> Option<&'a str> {
    let tags = tags?;
    tags.get(key)
        .or_else(|| {
            tags.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v)
        })
        .map(String::as_str)
}

/// Jellyfin's name for an ffprobe format (ProbeResultNormalizer.NormalizeFormat).
fn jellyfin_format(name: &str) -> &str {
    match name {
        "mpegvideo" => "mpeg",
        "mpegts" => "ts",
        "matroska" => "mkv",
        other => other,
    }
}

/// The container of the file at `path`, given ffprobe's `format_name` (a comma-separated list,
/// "mov,mp4,m4a,3gp,3g2,mj2"). It is the lowercased extension, with MPEG-TS's other extensions
/// (m2ts, mts) named "ts" as Jellyfin names them. A video file whose extension is not among the
/// formats ffprobe found (an MPEG program stream named .avi) gets the first format instead, as
/// Jellyfin does. Rustyfin keeps the extension where it is one of the formats ("mp4", where
/// Jellyfin says "mov"), since clients' direct play profiles list containers by extension.
fn container_name(path: &str, format_name: Option<&str>, is_video: bool) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());
    let ext = match ext.as_deref() {
        Some("m2ts" | "mts" | "m2t") => Some("ts".to_string()),
        _ => ext,
    };
    let formats: Vec<&str> = format_name
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(jellyfin_format)
        .collect();
    let Some(&first) = formats.first() else {
        return ext;
    };
    match ext {
        Some(ext) if !is_video || formats.contains(&ext.as_str()) => Some(ext),
        // The MP4 family's extensions are all among its formats except m4v
        Some(ext) if ext == "m4v" && formats.contains(&"mp4") => Some(ext),
        _ => Some(first.to_string()),
    }
}

fn stream_to_model(s: FfprobeStream) -> MediaStream {
    let is_default = s.disposition.as_ref().and_then(|d| d.default).unwrap_or(0) > 0;
    let is_forced = s.disposition.as_ref().and_then(|d| d.forced).unwrap_or(0) > 0;

    // ffprobe nests language and title under `tags`; the top-level fields are the fallback
    let language = tag(s.tags.as_ref(), "language")
        .map(str::to_string)
        .or(s.language);
    let title = tag(s.tags.as_ref(), "title")
        .map(str::to_string)
        .or(s.title);

    let stream_type = MediaStream::stream_type(
        s.codec_type.as_deref().unwrap_or(""),
        s.codec_name.as_deref(),
    );

    // As in Jellyfin, only the disposition makes a subtitle forced; a title or codec name
    // that says "forced" does not
    let is_forced_sub = s.codec_type.as_deref() == Some("subtitle") && is_forced;

    // Level, bit depth, pixel shape and scan type of a video stream, as Jellyfin's probe reads them
    let is_video = s.codec_type.as_deref() == Some("video");
    let bit_depth = s
        .bits_per_raw_sample
        .as_ref()
        .and_then(|b| b.parse().ok())
        .filter(|depth| *depth > 0)
        .or_else(|| {
            is_video
                .then(|| s.pix_fmt.as_deref().and_then(pixel_format_bit_depth))
                .flatten()
        });
    // A Matroska stream has no bit rate of its own; its statistics tags carry one
    let is_audio = s.codec_type.as_deref() == Some("audio");
    let bit_rate = s.bit_rate.filter(|rate| *rate > 0).or_else(|| {
        (is_video || is_audio)
            .then(|| tag_bit_rate(s.tags.as_ref()))
            .flatten()
    });
    let level = s
        .level
        .filter(|level| is_video && *level > 0)
        .map(f64::from);
    let is_anamorphic = is_video.then(|| {
        is_anamorphic(
            s.sample_aspect_ratio.as_deref(),
            s.display_aspect_ratio.as_deref(),
        )
    });
    let is_interlaced = is_video
        && s.field_order.as_deref().is_some_and(|order| {
            !order.trim().is_empty() && !order.eq_ignore_ascii_case("progressive")
        });

    MediaStream {
        index: s.index,
        codec: s.codec_name,
        codec_type: s.codec_type.unwrap_or_default(),
        language,
        time_base: s.time_base,
        codec_time_base: s.codec_time_base,
        codec_tag: s.codec_tag,
        is_external: s.external.unwrap_or(false),
        path: s.path,
        width: s.width,
        height: s.height,
        average_frame_rate: s.avg_frame_rate,
        real_frame_rate: s.r_frame_rate,
        bit_rate,
        bit_depth,
        channels: s.channels,
        channel_layout: s.channel_layout,
        sample_rate: s
            .sample_rate
            .as_ref()
            .and_then(|r| r.replace(" Hz", "").parse().ok()),
        profile: s.profile,
        r#type: stream_type,
        aspect_ratio_text: None,
        aspect_ratio: None,
        title,
        color_space: s.color_space,
        color_transfer: s.color_transfer,
        color_primaries: s.color_primaries,
        color_range: None,
        localized_uri: None,
        is_default,
        is_forced: is_forced_sub,
        is_interlaced,
        embedded_bitmaps: false,
        level,
        is_anamorphic,
    }
}

/// A stream's bit rate from its Matroska statistics tags, as Jellyfin reads it: `BPS`, else
/// `NUMBER_OF_BYTES` over `DURATION` when the stream lasts a second or more. The `-eng` variants
/// that mkvmerge writes come first.
fn tag_bit_rate(tags: Option<&HashMap<String, String>>) -> Option<u32> {
    let either = |name: &str| tag(tags, &format!("{name}-eng")).or_else(|| tag(tags, name));
    if let Some(bps) = either("BPS").and_then(|bps| bps.trim().parse::<u32>().ok()) {
        return Some(bps).filter(|bps| *bps > 0);
    }
    let bytes: u64 = either("NUMBER_OF_BYTES")?.trim().parse().ok()?;
    let seconds = either("DURATION").and_then(duration_seconds)?;
    if seconds < 1.0 {
        return None;
    }
    let bps = (bytes as f64 * 8.0 / seconds).round();
    (bps > 0.0 && bps <= f64::from(u32::MAX)).then_some(bps as u32)
}

/// Seconds in a Matroska `DURATION` tag, `HH:MM:SS.fraction`.
fn duration_seconds(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// The bit depth that Jellyfin reads from a pixel format when ffprobe gives no sample size.
fn pixel_format_bit_depth(pix_fmt: &str) -> Option<u32> {
    match pix_fmt.to_ascii_lowercase().as_str() {
        "yuv420p" | "yuv444p" => Some(8),
        "yuv420p10le" | "yuv444p10le" => Some(10),
        "yuv420p12le" | "yuv444p12le" => Some(12),
        _ => None,
    }
}

/// Whether a video's pixels are not square, by Jellyfin's rules: a sample aspect ratio that is
/// set, not near 1:1 and not 0:1 (unknown). A missing or unknown ratio is square.
fn is_anamorphic(sample_aspect_ratio: Option<&str>, display_aspect_ratio: Option<&str>) -> bool {
    let sar = sample_aspect_ratio.unwrap_or_default();
    if sar.is_empty() && display_aspect_ratio.unwrap_or_default().is_empty() {
        return false;
    }
    let near_square = match sar.split_once(':') {
        Some((num, den)) => match (num.parse::<f64>(), den.parse::<f64>()) {
            (Ok(num), Ok(den)) if den > 0.0 => (num / den - 1.0).abs() <= 0.01,
            _ => sar == "1:1",
        },
        None => sar == "1:1",
    };
    !near_square && !sar.is_empty() && sar != "0:1"
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

/// Probes `path`, one of `files`, the files of the item's probe plan in order, and writes what it
/// gave with `write_probe_outcome`. As the primary, a probe that read the file saves the item's
/// runtime and chapters, and a failed probe that kept the file's source sets the item's runtime
/// from that source. Returns the probe's result, and whether the item has a source at `path` now.
async fn probe_and_write(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    files: &[String],
    path: &str,
    as_primary: bool,
    stamp: Option<(i64, i64)>,
) -> (Result<(), ProbeError>, bool) {
    let probe = probe_file(path).await;
    let outcome = match &probe {
        Ok(result) => rustyfin_db::repo_media_source::ProbeOutcome::Probed(&result.source),
        Err(_) => rustyfin_db::repo_media_source::ProbeOutcome::Failed { stamp },
    };
    let has_source = match rustyfin_db::repo_media_source::write_probe_outcome(
        pool, item_id, files, path, outcome,
    )
    .await
    {
        Ok(has_source) => has_source,
        // The transaction rolled back, so nothing changed and nothing must be promoted.
        Err(e) => return (Err(ProbeError::Db(e)), true),
    };
    match probe {
        Ok(result) => {
            if as_primary {
                save_runtime_and_chapters(
                    pool,
                    item_id,
                    path,
                    result.source.run_time_ticks,
                    result.chapters,
                )
                .await;
            }
            (Ok(()), true)
        }
        Err(e) => {
            if as_primary && has_source {
                let updated = sqlx::query(
                    "UPDATE media_items SET runtime_ticks = COALESCE((SELECT run_time_ticks \
                     FROM media_sources WHERE item_id = ?1 AND path = ?2 ORDER BY rowid \
                     LIMIT 1), runtime_ticks) WHERE id = ?1",
                )
                .bind(item_id.to_string())
                .bind(path)
                .execute(pool)
                .await;
                if let Err(e) = updated {
                    warn!("failed to set the runtime of '{}': {}", path, e);
                }
            }
            (Err(e), has_source)
        }
    }
}

/// Probe a file and save it as the primary (only) media source for an item. A probe that reads the
/// file replaces the item's sources and saves its runtime and chapters. A probe that fails keeps
/// the source of an unchanged file and deletes any other source. Each source records
/// [`rustyfin_db::repo_media_source::PROBE_VERSION`], which tells a scan when to read
/// the file again.
/// Unlike a scan's probe step, this records no probe failure and stores no stamp.
pub async fn probe_and_save(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
) -> Result<(), ProbeError> {
    probe_and_write(
        pool,
        item_id,
        &[path.to_string()],
        path,
        true,
        scan_sync::file_stamp(path),
    )
    .await
    .0
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

/// Runs one probe step: takes the file's stamp, probes and writes with `probe_and_write`, clears
/// the file's probe failure when the probe read it, records one for an ffprobe or parse error, so
/// that later scans skip the file until it changes, and stores the stamp on the file's source when
/// it has one (a new source, or a kept one that was never stamped).
async fn run_probe_step(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    files: &[String],
    path: &str,
    as_primary: bool,
) -> (Result<(), ProbeError>, bool) {
    // Taken before the probe, so that a file that changes during it is probed again.
    let stamp = scan_sync::file_stamp(path);
    let (result, has_source) = probe_and_write(pool, item_id, files, path, as_primary, stamp).await;
    // Keep the failure record in step with the probe. A DB error here is only logged.
    let recorded = match &result {
        Ok(()) => scan_sync::clear_probe_failure(pool, path).await,
        Err(ProbeError::FfprobeFailed(_) | ProbeError::ParseError(_)) => {
            scan_sync::record_probe_failure(pool, path, stamp).await
        }
        Err(_) => Ok(()),
    };
    if let Err(e) = recorded {
        warn!("failed to record the probe of '{}': {}", path, e);
    }
    if has_source
        && !matches!(result, Err(ProbeError::Db(_)))
        && let Err(e) = scan_sync::store_stamp(pool, &item_id.to_string(), path, stamp).await
    {
        warn!("failed to store the stamp of '{}': {}", path, e);
    }
    (result, has_source)
}

/// `probe_file` under a permit of the global probe limit.
pub async fn probe_file_bounded(path: String) -> Result<ProbeResult, ProbeError> {
    let _permit = probe_semaphore().acquire().await;
    probe_file(&path).await
}

/// The global limit on ffprobe runs at once (`RUSTYFIN_MAX_PROBES`).
pub fn max_probes() -> usize {
    get_max_probes()
}

/// Spawn a bounded probe task that respects the global concurrency limit.
/// Returns a JoinHandle so the caller can await it if needed. The probe is the primary step of its
/// file (see `run_probe_step`): a failure is logged, and an unchanged file keeps its source.
pub fn spawn_probe_and_save(
    pool: sqlx::SqlitePool,
    item_id: uuid::Uuid,
    path: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Acquire a permit from the semaphore to limit concurrent probes
        let _permit = probe_semaphore().acquire().await;
        let (result, _) =
            run_probe_step(&pool, &item_id, std::slice::from_ref(&path), &path, true).await;
        if let Err(e) = result {
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

/// One file of a probe plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeStep {
    /// Probes the file as the item's primary source: when the probe reads the file, its source
    /// replaces the item's source at that file, and the item's runtime and chapters come from it.
    Primary(String),
    /// Probes the file and adds it to the item as another source, replacing the item's source at
    /// that file.
    Alternate(String),
}

/// Runs `steps` for one item, in order, under one probe permit. Each step runs with
/// `run_probe_step`, and the files of all the steps are the plan's files: a step writes the source
/// of its own file and keeps the sources of the others in order. A step that fails is logged, and
/// the next step still runs. A failed step keeps the source of an unchanged file, and records its
/// failure for an ffprobe or parse error, so that later scans skip the file until it changes.
///
/// A primary step that leaves the item without a source at its file makes the next step run as the
/// primary instead. When that step leaves a source, its file becomes the item's path, and the
/// steps after it are alternates again. A primary that keeps its source keeps the item's place, so
/// the alternates do not take it over.
pub fn spawn_probe_plan(
    pool: sqlx::SqlitePool,
    item_id: uuid::Uuid,
    steps: Vec<ProbeStep>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // One permit for the whole plan, so that its steps run in order
        let _permit = probe_semaphore().acquire().await;
        let files: Vec<String> = steps
            .iter()
            .map(|step| match step {
                ProbeStep::Primary(path) | ProbeStep::Alternate(path) => path.clone(),
            })
            .collect();
        // Whether the item has no primary: true after a primary step leaves no source at its file.
        // It is cleared when a later step leaves a source.
        let mut needs_primary = false;
        for step in steps {
            let (path, is_primary) = match &step {
                ProbeStep::Primary(path) => (path, true),
                ProbeStep::Alternate(path) => (path, false),
            };
            let (result, has_source) =
                run_probe_step(&pool, &item_id, &files, path, is_primary || needs_primary).await;
            if let Err(e) = &result {
                warn!("failed to probe '{}': {}", path, e);
            }
            if is_primary {
                needs_primary = !has_source;
            } else if needs_primary && has_source {
                let moved = sqlx::query("UPDATE media_items SET path = ?1 WHERE id = ?2")
                    .bind(path.as_str())
                    .bind(item_id.to_string())
                    .execute(&pool)
                    .await;
                if let Err(e) = moved {
                    warn!("failed to set the path of '{}': {}", path, e);
                }
                needs_primary = false;
            }
        }
    })
}

/// True when `ffprobe -version` runs and exits 0.
pub async fn ffprobe_runs() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

/// Adds `path` to an item as another source: probes the file and inserts it with its streams.
/// Unlike `probe_and_add_source`, it leaves the item's runtime and chapters alone.
pub async fn probe_and_add_alternate(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
) -> Result<(), ProbeError> {
    let result = probe_file(path).await?;
    let _source_id = rustyfin_db::repo_media_source::insert_media_source_with_streams(
        pool,
        item_id,
        &result.source,
    )
    .await?;
    Ok(())
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

    save_runtime_and_chapters(pool, item_id, path, source.run_time_ticks, result.chapters).await;

    Ok(())
}

/// Internal: saves a probed file's runtime and chapters for an item.
async fn save_runtime_and_chapters(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    path: &str,
    run_time_ticks: Option<i64>,
    mut chapters: Vec<Chapter>,
) {
    // Update the media item's runtime_ticks if we got them from probing
    if let Some(ticks) = run_time_ticks {
        let _ = sqlx::query("UPDATE media_items SET runtime_ticks = ? WHERE id = ?")
            .bind(ticks)
            .bind(item_id.to_string())
            .execute(pool)
            .await;
    }

    // The tile set, chapter images and failed image jobs belong to the file version they were
    // made from. When this file's size or time differs, they go (the image tasks remove the old
    // files from disk); when it is the same file probed again, they stay.
    use rustyfin_db::repo_trickplay as tp;
    let now = crate::trickplay::file_version(path).await;
    match tp::get_image_version(pool, *item_id).await {
        Ok(Some(made_from)) if made_from != now => {
            let dropped = async {
                tp::delete_trickplay(pool, *item_id).await?;
                tp::clear_failures(pool, *item_id).await?;
                tp::set_image_version(pool, *item_id, None).await
            };
            if let Err(e) = dropped.await {
                warn!("failed to drop the old trickplay tiles: {}", e);
            }
        }
        Ok(_) => {
            // The same file: chapters at the same start keep their image, or their failure mark.
            if let Ok(old) = rustyfin_db::repo_chapter::get_chapters(pool, *item_id).await {
                for ch in &mut chapters {
                    if let Some(o) = old
                        .iter()
                        .find(|o| o.start_position_ticks == ch.start_position_ticks)
                    {
                        ch.image_path.clone_from(&o.image_path);
                        ch.image_date_modified.clone_from(&o.image_date_modified);
                    }
                }
            }
        }
        Err(e) => warn!("failed to read the version images were made from: {}", e),
    }
    save_chapters(pool, item_id, chapters).await;
}

/// Saves a probed file's chapters for an item, replacing its old ones; none found keeps them.
pub(crate) async fn save_chapters(
    pool: &sqlx::SqlitePool,
    item_id: &uuid::Uuid,
    mut chapters: Vec<Chapter>,
) {
    if !chapters.is_empty() {
        let item_id_str = item_id.to_string();
        // Set the item_id on each chapter
        for ch in &mut chapters {
            ch.item_id = item_id_str.clone();
        }
        if let Err(e) = rustyfin_db::repo_chapter::save_chapters(pool, *item_id, chapters).await {
            warn!("failed to save chapters: {}", e);
        }
    }
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

    #[test]
    fn r1play_container_names() {
        let mp4 = Some("mov,mp4,m4a,3gp,3g2,mj2");
        let mkv = Some("matroska,webm");
        let cases = [
            ("/m/a.mkv", mkv, true, "mkv"),
            ("/m/a.webm", mkv, true, "webm"),
            ("/m/a.mp4", mp4, true, "mp4"),
            ("/m/a.M4V", mp4, true, "m4v"),
            ("/m/a.m2ts", Some("mpegts"), true, "ts"),
            ("/m/a.mts", Some("mpegts"), true, "ts"),
            ("/m/a.ts", Some("mpegts"), true, "ts"),
            // An MPEG program stream named .avi
            ("/m/a.avi", Some("mpeg"), true, "mpeg"),
            ("/m/a.avi", Some("avi"), true, "avi"),
            ("/m/a.mpg", Some("mpegvideo"), true, "mpeg"),
            // Audio keeps its extension
            ("/m/a.opus", Some("ogg"), false, "opus"),
            // No format: the extension
            ("/m/a.m2ts", None, true, "ts"),
        ];
        for (path, format, video, want) in cases {
            assert_eq!(
                container_name(path, format, video).as_deref(),
                Some(want),
                "{path} {format:?}"
            );
        }
        assert_eq!(
            container_name("/m/noext", Some("matroska,webm"), true).as_deref(),
            Some("mkv")
        );
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

    /// `ffprobe -show_streams -of json` output (ffprobe 7.1) for an MKV episode, trimmed to the
    /// fields that matter here. ffprobe gives a stream's language and title under `tags`.
    const TAGGED_STREAMS_JSON: &str = r#"{
        "streams": [
            {
                "index": 1,
                "codec_name": "flac",
                "codec_type": "audio",
                "channels": 2,
                "channel_layout": "stereo",
                "sample_rate": "48000",
                "disposition": { "default": 1, "forced": 0, "hearing_impaired": 0 },
                "tags": { "language": "jpn" }
            },
            {
                "index": 2,
                "codec_name": "flac",
                "codec_type": "audio",
                "channels": 2,
                "channel_layout": "stereo",
                "sample_rate": "48000",
                "disposition": { "default": 0, "forced": 0, "hearing_impaired": 0 },
                "tags": { "language": "eng" }
            },
            {
                "index": 3,
                "codec_name": "hdmv_pgs_subtitle",
                "codec_type": "subtitle",
                "disposition": { "default": 0, "forced": 0, "hearing_impaired": 0 },
                "tags": { "language": "eng", "title": "English PGS" }
            },
            {
                "index": 6,
                "codec_name": "subrip",
                "codec_type": "subtitle",
                "disposition": { "default": 1, "forced": 1, "hearing_impaired": 0 },
                "tags": { "language": "eng", "title": "English Subrip" }
            }
        ]
    }"#;

    /// Each stream's (index, language, title) after `stream_to_model`.
    fn languages_and_titles(json: &str) -> Vec<(i32, Option<String>, Option<String>)> {
        let parsed: FfprobeOutput = serde_json::from_str(json).expect("should parse");
        parsed
            .streams
            .into_iter()
            .map(stream_to_model)
            .map(|s| (s.index, s.language, s.title))
            .collect()
    }

    fn some(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn test_stream_language_and_title_come_from_tags() {
        assert_eq!(
            languages_and_titles(TAGGED_STREAMS_JSON),
            vec![
                (1, some("jpn"), None),
                (2, some("eng"), None),
                (3, some("eng"), some("English PGS")),
                (6, some("eng"), some("English Subrip")),
            ]
        );
    }

    #[test]
    fn test_stream_tag_keys_match_without_case() {
        // Some muxers write the tag keys in capitals
        let json = r#"{"streams": [{"index": 0, "codec_name": "ac3", "codec_type": "audio",
            "tags": {"LANGUAGE": "ger", "Title": "Deutsch"}}]}"#;
        assert_eq!(
            languages_and_titles(json),
            vec![(0, some("ger"), some("Deutsch"))]
        );
    }

    #[test]
    fn test_stream_top_level_language_and_title_are_the_fallback() {
        let json = r#"{"streams": [
            {"index": 0, "codec_name": "aac", "codec_type": "audio",
                "language": "fre", "title": "Commentaire"},
            {"index": 1, "codec_name": "aac", "codec_type": "audio",
                "language": "fre", "title": "Commentaire", "tags": {"language": "eng"}},
            {"index": 2, "codec_name": "aac", "codec_type": "audio", "tags": {}}
        ]}"#;
        assert_eq!(
            languages_and_titles(json),
            vec![
                (0, some("fre"), some("Commentaire")),
                (1, some("eng"), some("Commentaire")),
                (2, None, None),
            ]
        );
    }

    #[test]
    fn test_subtitle_is_forced_only_by_its_disposition() {
        // As in Jellyfin, a title or a codec name that says "forced" does not make a subtitle
        // forced, and the title is still kept
        let json = r#"{"streams": [
            {"index": 4, "codec_name": "subrip", "codec_type": "subtitle",
                "disposition": {"default": 0, "forced": 0},
                "tags": {"language": "eng", "title": "english forced"}},
            {"index": 5, "codec_name": "hdmv_pgs_subtitle", "codec_type": "subtitle",
                "disposition": {"default": 0, "forced": 1},
                "tags": {"language": "eng", "title": "English"}},
            {"index": 6, "codec_name": "forced", "codec_type": "subtitle"}
        ]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).expect("should parse");
        let streams: Vec<(i32, Option<String>, bool)> = parsed
            .streams
            .into_iter()
            .map(stream_to_model)
            .map(|s| (s.index, s.title, s.is_forced))
            .collect();
        assert_eq!(
            streams,
            vec![
                (4, some("english forced"), false),
                (5, some("English"), true),
                (6, None, false),
            ]
        );
    }

    /// A video stream's level, bit depth, anamorphic and interlaced flags, as Jellyfin's probe
    /// reads them: the level when ffprobe gives one (not -99), the bit depth from the pixel format
    /// without a sample size, non-square pixels, and any field order but progressive.
    #[test]
    fn r3decisions_video_level_depth_shape_and_scan() {
        let json = r#"{"streams": [
            {"index": 0, "codec_name": "hevc", "codec_type": "video", "profile": "Main 10",
                "level": 153, "pix_fmt": "yuv420p10le", "sample_aspect_ratio": "1:1",
                "display_aspect_ratio": "16:9", "field_order": "progressive"},
            {"index": 1, "codec_name": "h264", "codec_type": "video", "level": 40,
                "bits_per_raw_sample": "8", "pix_fmt": "yuv420p10le",
                "sample_aspect_ratio": "32:27", "display_aspect_ratio": "4:3", "field_order": "tt"},
            {"index": 2, "codec_name": "mpeg4", "codec_type": "video", "level": -99,
                "pix_fmt": "yuv420p", "sample_aspect_ratio": "0:1", "display_aspect_ratio": "0:1"},
            {"index": 3, "codec_name": "aac", "codec_type": "audio", "level": 2}
        ]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).expect("should parse");
        let streams: Vec<_> = parsed
            .streams
            .into_iter()
            .map(stream_to_model)
            .map(|s| (s.level, s.bit_depth, s.is_anamorphic, s.is_interlaced))
            .collect();
        assert_eq!(
            streams,
            vec![
                (Some(153.0), Some(10), Some(false), false),
                (Some(40.0), Some(8), Some(true), true),
                (None, Some(8), Some(false), false),
                (None, None, None, false),
            ]
        );
    }

    /// A Matroska stream's bit rate comes from its `BPS` tag, else from its byte count over its
    /// duration, as Jellyfin reads them; ffprobe's own bit rate wins.
    #[test]
    fn r3decisions_matroska_streams_take_their_bit_rate_from_tags() {
        let json = r#"{"streams": [
            {"index": 0, "codec_name": "hevc", "codec_type": "video",
                "tags": {"BPS-eng": "48082772", "BPS": "1"}},
            {"index": 1, "codec_name": "dts", "codec_type": "audio",
                "tags": {"NUMBER_OF_BYTES": "1000000", "DURATION": "00:00:08.000000000"}},
            {"index": 2, "codec_name": "ac3", "codec_type": "audio", "bit_rate": "640000",
                "tags": {"BPS": "1"}},
            {"index": 3, "codec_name": "aac", "codec_type": "audio",
                "tags": {"NUMBER_OF_BYTES": "10", "DURATION": "00:00:00.500000000"}},
            {"index": 4, "codec_name": "subrip", "codec_type": "subtitle", "tags": {"BPS": "90"}}
        ]}"#;
        let parsed: FfprobeOutput = serde_json::from_str(json).expect("should parse");
        let rates: Vec<_> = parsed
            .streams
            .into_iter()
            .map(stream_to_model)
            .map(|s| s.bit_rate)
            .collect();
        assert_eq!(
            rates,
            vec![Some(48_082_772), Some(1_000_000), Some(640_000), None, None]
        );
    }

    // Chapters

    /// ffprobe 7's `-show_chapters` output: `id`, `start` and `end` are numbers.
    const R2CHAPTERS_FFPROBE: &str = r#"{"chapters": [
        {"id": 7566774747248968120, "time_base": "1/1000000000", "start": 0,
         "start_time": "0.000000", "end": 433350000000, "end_time": "433.350000",
         "tags": {"title": "Opening"}},
        {"id": -8933400255973827885, "time_base": "1/1000000000", "start": 433350000000,
         "start_time": "433.350400", "end": 774107000000, "end_time": "774.107000",
         "tags": {"TITLE": "00:07:13.350"}},
        {"id": 3, "time_base": "1/1000", "start": 774107, "start_time": "774.107000",
         "end": 900000, "end_time": "900.000000"}
    ]}"#;

    #[test]
    fn r2chapters_parses_numeric_ids_and_positions() {
        let got = parse_chapters(R2CHAPTERS_FFPROBE);
        let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Opening", "Chapter 2", "Chapter 3"]);
        let starts: Vec<_> = got.iter().map(|c| c.start_position_ticks).collect();
        // Rounded to the millisecond, like Jellyfin.
        assert_eq!(starts, [0, 4_333_500_000, 7_741_070_000]);
        assert_eq!(got[2].end_position_ticks, 9_000_000_000);
        let numbers: Vec<_> = got.iter().map(|c| c.chapter_number).collect();
        assert_eq!(numbers, [1, 2, 3]);
    }

    #[tokio::test]
    async fn r2chapters_a_stalled_chapter_read_times_out() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rustyfin_stall_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("ffprobe");
        std::fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let started = std::time::Instant::now();
        let got = read_chapters_with(
            fake.to_str().unwrap(),
            "/no/such.mkv",
            std::time::Duration::from_millis(300),
        )
        .await;
        assert!(got.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn r2chapters_no_chapters_or_bad_json_gives_none() {
        assert!(parse_chapters(r#"{"chapters": []}"#).is_empty());
        assert!(parse_chapters("not json").is_empty());
    }

    #[test]
    fn r2chapters_time_titles_are_renamed() {
        // .NET reads a bare number as days, so Jellyfin renames "1984" too.
        for t in [
            "00:05:00.000",
            "5",
            "1984",
            "1:02",
            "-00:00:01",
            "1.02:03:04",
        ] {
            assert!(looks_like_time(t), "{t}");
        }
        for t in ["Chapter 01", "Part 1: Intro", "12:", "Act 2", ""] {
            assert!(!looks_like_time(t), "{t}");
        }
    }
}
