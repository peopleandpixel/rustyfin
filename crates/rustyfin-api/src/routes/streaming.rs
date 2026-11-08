use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path as FilePath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/stream", get(stream_video).head(head_video))
        .route(
            "/Videos/{id}/stream.{container}",
            get(stream_video).head(head_video),
        )
        .route(
            "/Videos/{id}/progressive.stream",
            get(stream_video_progressive),
        )
        .route("/Videos/{id}/PlaybackInfo", get(video_playback_info))
        .route("/Videos/{id}/Universal", get(video_universal))
        .route("/Videos/{id}/Chapters", get(get_chapters))
        .route("/Audio/{id}/stream", get(stream_audio).head(head_stream))
        .route(
            "/Audio/{id}/stream.{container}",
            get(stream_audio).head(head_stream),
        )
        .route("/Audio/{id}/universal", get(audio_universal))
        .route("/Audio/{id}/PlaybackInfo", get(audio_playback_info))
}

// ---------------------------------------------------------------------------
// Video versions
// ---------------------------------------------------------------------------

/// Which version of an item a playback request plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    /// No MediaSourceId, a blank one or the item's own id: the item's file.
    Primary,
    /// The source at this index of the item's sources.
    Source(usize),
}

/// The request's MediaSourceId (name matched in any ASCII case), trimmed. None when absent or
/// blank.
pub(crate) fn requested_source_id(params: &HashMap<String, String>) -> Option<&str> {
    params
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("MediaSourceId"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        // A client sends one name; should several match, the smallest value does not depend on
        // the map's order
        .min()
}

/// The version `requested` names among `sources` of item `item_id`. ApiError::NotFound when it
/// names none.
pub(crate) fn select_version(
    item_id: Uuid,
    sources: &[rustyfin_model::media_source::MediaSourceResponse],
    requested: Option<&str>,
) -> Result<Version, ApiError> {
    let Some(requested) = requested else {
        return Ok(Version::Primary);
    };
    let wanted = Uuid::parse_str(requested).ok();
    if wanted == Some(item_id) {
        return Ok(Version::Primary);
    }
    sources
        .iter()
        .position(|source| {
            source.id == requested
                || (wanted.is_some() && Uuid::parse_str(&source.id).ok() == wanted)
        })
        .map(Version::Source)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Media source {requested} not found for item {item_id}"
            ))
        })
}

/// The path of a video route. `stream.{container}` has a second parameter, which serde ignores.
#[derive(Deserialize)]
struct VideoPath {
    id: Uuid,
}

/// The file that a video request plays: the item's own file, or the file of the version that its
/// `MediaSourceId` names. The sources are read only when an id is given.
async fn video_file(
    state: &AppState,
    id: Uuid,
    params: &HashMap<String, String>,
) -> Result<String, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    let Some(requested) = requested_source_id(params) else {
        return Ok(item.path);
    };
    let sources = rustyfin_db::repo_media_source::get_media_sources(&state.db_ro, &id).await?;
    match select_version(id, &sources, Some(requested))? {
        Version::Primary => Ok(item.path),
        Version::Source(index) => Ok(sources[index].path.clone()),
    }
}

/// `GET /Videos/{id}/stream` and `stream.{container}`: the file of the requested version.
async fn stream_video(
    State(state): State<AppState>,
    Path(VideoPath { id }): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;
    serve_file(file_path, &headers).await
}

/// `HEAD` of `stream` and `stream.{container}`: the headers of the file that `GET` serves.
async fn head_video(
    State(state): State<AppState>,
    Path(VideoPath { id }): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;
    head_file(&file_path).await
}

async fn stream_audio(
    state: State<AppState>,
    path: Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    stream_file(state, path, headers).await
}

async fn head_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    head_file(&item.path).await
}

/// The headers of a `HEAD` for the file at `file_path`: its type, its length, and that byte
/// ranges are accepted.
async fn head_file(file_path: &str) -> Result<Response, ApiError> {
    let metadata = tokio::fs::metadata(file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;

    let content_type = mime_from_path(file_path);
    let file_size = metadata.len();

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, file_size.to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
        ],
    )
        .into_response())
}

async fn stream_file(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    serve_file(item.path, &headers).await
}

/// Serves the file at `file_path` for a request with `headers`: the whole file, or the byte range,
/// 304 or 416 that the headers ask for.
async fn serve_file(file_path: String, headers: &HeaderMap) -> Result<Response, ApiError> {
    let content_type = mime_from_path(&file_path);

    // Open file first, then get metadata from the file handle — avoids a separate stat() syscall
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read file metadata: {e}")))?;
    let file_size = metadata.len();

    // ETag support: use modification time + size as cache key
    let etag = if let Ok(mtime) = metadata.modified() {
        let mtime_secs = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("\"{}-{}\"", mtime_secs, file_size)
    } else {
        format!("\"{}\"", file_size)
    };

    // Check If-None-Match for conditional GET (304 Not Modified)
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(client_etag) = if_none_match.to_str()
        && client_etag == etag
    {
        return Ok((StatusCode::NOT_MODIFIED, ()).into_response());
    }

    let byte_range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map_or(ByteRange::Ignore, |s| parse_range(s, file_size));

    match byte_range {
        ByteRange::Partial(start, end) => {
            let length = end - start + 1;

            // Reopen for range request (need mutable handle for seek)
            let mut file = tokio::fs::File::open(&file_path)
                .await
                .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| ApiError::Internal(format!("Seek failed: {e}")))?;

            let reader = tokio::io::BufReader::new(file.take(length));
            let stream = tokio_util::io::ReaderStream::new(reader);
            let body = axum::body::Body::from_stream(stream);

            Ok((
                StatusCode::PARTIAL_CONTENT,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_LENGTH, length.to_string()),
                    (header::ACCEPT_RANGES, "bytes".to_string()),
                    (
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{file_size}"),
                    ),
                    (header::ETAG, etag),
                ],
                body,
            )
                .into_response())
        }
        ByteRange::Unsatisfiable => Ok((
            StatusCode::RANGE_NOT_SATISFIABLE,
            [
                (header::CONTENT_RANGE, format!("bytes */{file_size}")),
                (header::ACCEPT_RANGES, "bytes".to_string()),
            ],
        )
            .into_response()),
        ByteRange::Ignore => {
            let file = tokio::fs::File::open(&file_path)
                .await
                .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

            let reader = tokio::io::BufReader::new(file);
            let stream = tokio_util::io::ReaderStream::new(reader);
            let body = axum::body::Body::from_stream(stream);

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_LENGTH, file_size.to_string()),
                    (header::ACCEPT_RANGES, "bytes".to_string()),
                    (header::ETAG, etag),
                ],
                body,
            )
                .into_response())
        }
    }
}

/// How to answer a `Range` header (RFC 9110 §14.1.2).
#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    /// 206 with bytes `start..=end`.
    Partial(u64, u64),
    /// 416 with `Content-Range: bytes */<size>`.
    Unsatisfiable,
    /// Not a single byte range we serve: 200 with the whole file.
    Ignore,
}

/// Parse a `Range: bytes=START-END` header for a file of `file_size` bytes.
fn parse_range(range: &str, file_size: u64) -> ByteRange {
    let Some(spec) = range.strip_prefix("bytes=") else {
        return ByteRange::Ignore;
    };
    // Multi-range requests are not served; they get the whole file.
    if spec.contains(',') {
        return ByteRange::Ignore;
    }
    let Some((start_str, end_str)) = spec.split_once('-') else {
        return ByteRange::Ignore;
    };
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if start_str.is_empty() {
        // Suffix range: `bytes=-N` is the last N bytes.
        return match end_str.parse::<u64>() {
            Ok(0) => ByteRange::Unsatisfiable,
            Ok(n) if n > file_size => ByteRange::Ignore,
            Ok(n) => ByteRange::Partial(file_size - n, file_size - 1),
            Err(_) => ByteRange::Ignore,
        };
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return ByteRange::Ignore;
    };
    // `A-` has no explicit end and runs to the last byte of the file.
    let end = if end_str.is_empty() {
        None
    } else {
        let Ok(end) = end_str.parse::<u64>() else {
            return ByteRange::Ignore;
        };
        if end < start {
            return ByteRange::Ignore;
        }
        Some(end)
    };
    if start >= file_size {
        return ByteRange::Unsatisfiable;
    }
    // start < file_size here, so file_size >= 1 and `file_size - 1` cannot underflow.
    let last = match end {
        Some(end) => end.min(file_size - 1),
        None => file_size - 1,
    };
    ByteRange::Partial(start, last)
}

fn mime_from_path(path: &str) -> String {
    let ext = FilePath::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // Video
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" => "video/mp2t",
        "mpg" | "mpeg" => "video/mpeg",
        "3gp" => "video/3gpp",
        // Audio
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "wma" => "audio/x-ms-wma",
        "aiff" | "aif" => "audio/aiff",
        "wv" => "audio/x-wavpack",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Audio Universal Endpoint
// ---------------------------------------------------------------------------

/// Query parameters for the universal audio endpoint
#[derive(Debug, Default)]
#[allow(dead_code)]
struct AudioUniversalQuery {
    max_audio_bitrate: Option<u32>,
    audio_stream_index: Option<u32>,
    start_time_ticks: Option<i64>,
    is_live: bool,
    media_source_id: Option<String>,
    device_id: Option<String>,
    audio_codec: Option<String>,
    enable_remote_media: bool,
    enable_direct_play: bool,
    enable_direct_stream: bool,
}

impl From<HashMap<String, String>> for AudioUniversalQuery {
    fn from(params: HashMap<String, String>) -> Self {
        Self {
            max_audio_bitrate: params.get("MaxAudioBitrate").and_then(|v| v.parse().ok()),
            audio_stream_index: params.get("AudioStreamIndex").and_then(|v| v.parse().ok()),
            start_time_ticks: params.get("StartTimeTicks").and_then(|v| v.parse().ok()),
            is_live: params
                .get("IsLive")
                .map(|v| v.parse().unwrap_or(false))
                .unwrap_or(false),
            media_source_id: params.get("MediaSourceId").cloned(),
            device_id: params.get("DeviceId").cloned(),
            audio_codec: params.get("AudioCodec").cloned(),
            enable_remote_media: params
                .get("EnableRemoteMedia")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_play: params
                .get("EnableDirectPlay")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_stream: params
                .get("EnableDirectStream")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
        }
    }
}

/// `GET /Audio/{id}/universal` — Primary music playback endpoint.
/// Returns a StreamingResponse with media sources and direct play/transcode URLs.
async fn audio_universal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let query = AudioUniversalQuery::from(params);

    // Single query: verify item exists and fetch media sources with streams
    let media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Audio item {id} not found")))?;

    // Select the appropriate media source
    let selected_source = if let Some(ref source_id) = query.media_source_id {
        media_sources
            .iter()
            .find(|s| s.id == *source_id)
            .cloned()
            .or_else(|| media_sources.first().cloned())
    } else {
        media_sources.first().cloned()
    };

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine if direct play is possible
    let can_direct_play = query.enable_direct_play
        && selected_source
            .as_ref()
            .is_some_and(|s| s.supports_direct_play);

    // Build the URL for direct play or transcoding
    let url = if can_direct_play {
        // Direct play URL
        Some(format!(
            "/Audio/{}/stream?MediaSourceId={}",
            id,
            selected_source
                .as_ref()
                .map_or("".to_string(), |s| s.id.clone())
        ))
    } else if query.enable_direct_stream {
        // Direct stream URL
        Some(format!("/Audio/{}/stream", id))
    } else {
        // Transcode URL (HLS)
        let mut url = format!(
            "/Videos/{}/main.m3u8?DeviceId={}",
            id,
            query.device_id.unwrap_or_default()
        );
        if let Some(bitrate) = query.max_audio_bitrate {
            url.push_str(&format!("&MaxAudioBitrate={}", bitrate));
        }
        if let Some(codec) = &query.audio_codec {
            url.push_str(&format!("&AudioCodec={}", codec));
        }
        Some(url)
    };

    // Update media sources with direct play flags based on client capabilities
    let mut response_sources = media_sources.clone();
    for source in &mut response_sources {
        source.supports_direct_play = query.enable_direct_play && source.supports_direct_play;
        source.supports_direct_stream = query.enable_direct_stream && source.supports_direct_stream;
        source.supports_transcoding = true;
    }

    let response = rustyfin_model::playback_info::StreamingResponse {
        media_sources: response_sources,
        url,
        analyze_duration_linear_percent: 100,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Audio/{id}/PlaybackInfo` — Returns playback info for an audio item.
async fn audio_playback_info(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(_params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Audio item {id} not found")))?;

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    let response = rustyfin_model::playback_info::PlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Video Progressive Stream Endpoint
// ---------------------------------------------------------------------------

/// `GET /Videos/{id}/progressive.stream` — Direct progressive download without Range support.
/// Clients use this for progressive playback where the entire file is streamed sequentially.
async fn stream_video_progressive(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;

    let metadata = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;

    let file_size = metadata.len();
    let content_type = mime_from_path(&file_path);

    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

    let reader = tokio::io::BufReader::new(file);
    let stream = tokio_util::io::ReaderStream::new(reader);
    let body = axum::body::Body::from_stream(stream);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, file_size.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "inline; filename=\"{}\"",
                    FilePath::new(&file_path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
            ),
        ],
        body,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Video PlaybackInfo Endpoint
// ---------------------------------------------------------------------------

/// Query parameters for video PlaybackInfo
#[derive(Debug, Default)]
#[allow(dead_code)]
struct VideoPlaybackInfoQuery {
    max_video_bitrate: Option<u32>,
    max_audio_bitrate: Option<u32>,
    video_stream_index: Option<u32>,
    audio_stream_index: Option<u32>,
    subtitle_stream_index: Option<i32>,
    start_time_ticks: Option<i64>,
    is_live: bool,
    device_id: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    transcode_to_subtitles: Option<String>,
    enable_remote_media: bool,
    enable_direct_play: bool,
    enable_direct_stream: bool,
    enable_video_stream_copy: bool,
    enable_audio_stream_copy: bool,
    width: Option<i32>,
    height: Option<i32>,
    max_streaming_bitrate: Option<u32>,
    segment_time: Option<u32>,
}

impl From<HashMap<String, String>> for VideoPlaybackInfoQuery {
    fn from(params: HashMap<String, String>) -> Self {
        Self {
            max_video_bitrate: params.get("MaxVideoBitrate").and_then(|v| v.parse().ok()),
            max_audio_bitrate: params.get("MaxAudioBitrate").and_then(|v| v.parse().ok()),
            video_stream_index: params.get("VideoStreamIndex").and_then(|v| v.parse().ok()),
            audio_stream_index: params.get("AudioStreamIndex").and_then(|v| v.parse().ok()),
            subtitle_stream_index: params
                .get("SubtitleStreamIndex")
                .and_then(|v| v.parse().ok()),
            start_time_ticks: params.get("StartTimeTicks").and_then(|v| v.parse().ok()),
            is_live: params
                .get("IsLive")
                .map(|v| v.parse().unwrap_or(false))
                .unwrap_or(false),
            device_id: params.get("DeviceId").cloned(),
            // Support both singular (VideoCodec) and plural (VideoCodecs) parameter names
            video_codec: params
                .get("VideoCodecs")
                .cloned()
                .or_else(|| params.get("VideoCodec").cloned()),
            audio_codec: params
                .get("AudioCodecs")
                .cloned()
                .or_else(|| params.get("AudioCodec").cloned()),
            transcode_to_subtitles: params.get("TranscodeToSubtitles").cloned(),
            enable_remote_media: params
                .get("EnableRemoteMedia")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_play: params
                .get("EnableDirectPlay")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_stream: params
                .get("EnableDirectStream")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_video_stream_copy: params
                .get("EnableVideoStreamCopy")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_audio_stream_copy: params
                .get("EnableAudioStreamCopy")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            width: params.get("Width").and_then(|v| v.parse().ok()),
            height: params.get("Height").and_then(|v| v.parse().ok()),
            max_streaming_bitrate: params
                .get("MaxStreamingBitrate")
                .and_then(|v| v.parse().ok()),
            segment_time: params.get("SegmentTime").and_then(|v| v.parse().ok()),
        }
    }
}

/// What a transcode must carry for a source, beyond what a direct play or stream would
#[derive(Debug, Default, PartialEq, Eq)]
struct TrackNeeds {
    /// The request names an audio stream other than the default one
    other_audio: bool,
    /// The request names a picture subtitle that the transcode burns in
    burn_in: bool,
}

/// The tracks that `query` names on `source` and that a direct play or stream would not carry
fn track_needs(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> TrackNeeds {
    let audio: Vec<_> = source
        .media_streams
        .iter()
        .filter(|s| is_stream_type(s, "Audio"))
        .collect();
    // The default audio is the first flagged one, else the first one
    let default_audio = audio
        .iter()
        .find(|s| s.is_default)
        .or_else(|| audio.first());
    let other_audio = match (query.audio_stream_index, default_audio) {
        (Some(wanted), Some(default)) => {
            let wanted = i64::from(wanted);
            audio.iter().any(|s| i64::from(s.index) == wanted) && i64::from(default.index) != wanted
        }
        _ => false,
    };
    // Picture subtitles are burned in by the transcode, text ones are drawn by the browser
    let burn_in = match query.subtitle_stream_index {
        Some(wanted) if wanted >= 0 => source.media_streams.iter().any(|s| {
            s.index == wanted
                && is_stream_type(s, "Subtitle")
                && s.codec
                    .as_deref()
                    .is_some_and(rustyfin_core::subtitle::is_image_subtitle)
        }),
        _ => false,
    };
    TrackNeeds {
        other_audio,
        burn_in,
    }
}

/// `GET /Videos/{id}/Universal` — Primary video playback endpoint.
/// Returns a StreamingResponse with media sources and direct play/transcode URLs.
async fn video_universal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // The index of the version the request names; an id that names none is refused
    let index = match select_version(id, &media_sources, requested_source_id(&params))? {
        Version::Primary => 0,
        Version::Source(index) => index,
    };
    let query = VideoPlaybackInfoQuery::from(params);

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // The tracks the request names that a direct play or stream would not carry
    let needs = match media_sources.get(index) {
        Some(source) => track_needs(source, &query),
        None => TrackNeeds::default(),
    };

    // Determine direct play/stream/transcode capability
    let (supports_direct_play, supports_direct_stream, supports_transcoding) =
        if media_sources.is_empty() {
            // No media sources - fallback to transcoding if possible
            (false, false, true)
        } else {
            let source = &media_sources[index];

            let can_direct_play = query.enable_direct_play
                && !needs.other_audio
                && !needs.burn_in
                && check_direct_play(source, &query);
            let can_direct_stream = query.enable_direct_stream
                && !needs.other_audio
                && !needs.burn_in
                && check_direct_stream(source, &query);
            let can_transcode = true;

            (can_direct_play, can_direct_stream, can_transcode)
        };

    // Build the URL based on playback method
    let url = if supports_direct_play {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}&Static=true",
            id, media_sources[index].id
        ))
    } else if supports_direct_stream {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}",
            id, media_sources[index].id
        ))
    } else if supports_transcoding {
        let mut url = format!(
            "/Videos/{}/main.m3u8?DeviceId={}",
            id,
            query.device_id.unwrap_or_default()
        );
        if let Some(source) = media_sources.get(index) {
            url.push_str(&format!("&MediaSourceId={}", source.id));
        }
        if let Some(bitrate) = query.max_video_bitrate {
            url.push_str(&format!("&VideoBitrate={}", bitrate));
        }
        if let Some(bitrate) = query.max_audio_bitrate {
            url.push_str(&format!("&AudioBitrate={}", bitrate));
        }
        if let Some(ref codec) = query.video_codec {
            url.push_str(&format!("&VideoCodec={}", codec));
        }
        if let Some(ref codec) = query.audio_codec {
            url.push_str(&format!("&AudioCodec={}", codec));
        }
        if let Some(width) = query.width {
            url.push_str(&format!("&MaxWidth={}", width));
        }
        if let Some(height) = query.height {
            url.push_str(&format!("&MaxHeight={}", height));
        }
        if let Some(ticks) = query.start_time_ticks {
            url.push_str(&format!("&StartTimeTicks={}", ticks));
        }
        if let Some(audio) = query.audio_stream_index {
            url.push_str(&format!("&AudioStreamIndex={}", audio));
        }
        if let Some(subtitle) = query.subtitle_stream_index.filter(|&i| i >= 0) {
            url.push_str(&format!("&SubtitleStreamIndex={}", subtitle));
        }
        if needs.burn_in {
            url.push_str("&SubtitleMethod=Encode");
        }
        if let Some(seg_time) = query.segment_time {
            url.push_str(&format!("&SegmentTime={}", seg_time));
        }
        url.push_str(&format!("&PlaySessionId={}", play_session_id));
        Some(url)
    } else {
        None
    };

    // Update media sources with direct play flags based on client capabilities
    for (i, source) in media_sources.iter_mut().enumerate() {
        source.supports_direct_play = i == index && supports_direct_play;
        source.supports_direct_stream = i == index && supports_direct_stream;
        source.supports_transcoding = i == index && supports_transcoding;
    }

    let response = rustyfin_model::playback_info::StreamingResponse {
        media_sources,
        url,
        analyze_duration_linear_percent: 100,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Videos/{id}/PlaybackInfo` — Returns playback info for a video item.
/// Includes direct play detection based on client codec support.
async fn video_playback_info(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // The subtitle files beside each video are listed after its streams
    for source in media_sources.iter_mut() {
        let external = crate::routes::subtitle::external_streams(source).await;
        source.media_streams.extend(external);
    }

    // The index of the version the request names; an id that names none is refused
    let index = match select_version(id, &media_sources, requested_source_id(&params))? {
        Version::Primary => 0,
        Version::Source(index) => index,
    };
    let query = VideoPlaybackInfoQuery::from(params);

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine direct play/stream/transcode capability based on client codec support
    let (supports_direct_play, supports_direct_stream, supports_transcoding) = if media_sources
        .is_empty()
    {
        // No media sources - fallback to transcoding if possible
        (false, false, true)
    } else {
        let source = &media_sources[index];

        // Check if client can direct play
        let can_direct_play = query.enable_direct_play && check_direct_play(source, &query);

        // Check if client can direct stream (re-mux without transcoding)
        let can_direct_stream = query.enable_direct_stream && check_direct_stream(source, &query);

        // Transcoding is always possible if FFmpeg is available
        let can_transcode = true;

        (can_direct_play, can_direct_stream, can_transcode)
    };

    // Update media sources with direct play flags based on client capabilities
    for (i, source) in media_sources.iter_mut().enumerate() {
        source.supports_direct_play = i == index && supports_direct_play;
        source.supports_direct_stream = i == index && supports_direct_stream;
        source.supports_transcoding = i == index && supports_transcoding;
    }

    let response = rustyfin_model::playback_info::VideoPlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
        can_play: true,
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Videos/{id}/Chapters` — Returns chapter markers for a video item.
async fn get_chapters(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let chapters = rustyfin_db::repo_chapter::get_chapters(&state.db_ro, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&chapters)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// Whether the first video and audio streams use a codec from the client's lists.
/// A list without entries falls back to the codecs a client is assumed to play.
fn codecs_playable(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // Comma-separated, e.g. "h264,hevc,vp9"
    let codec_list = |list: Option<&str>, fallback: &[&str]| -> Vec<String> {
        let entries: Vec<String> = list
            .unwrap_or_default()
            .split(',')
            .map(|c| c.trim().to_lowercase())
            .filter(|c| !c.is_empty())
            .collect();
        if entries.is_empty() {
            fallback.iter().map(|c| c.to_string()).collect()
        } else {
            entries
        }
    };
    let video_codecs = codec_list(query.video_codec.as_deref(), &["h264", "avc1"]);
    let audio_codecs = codec_list(query.audio_codec.as_deref(), &["aac", "mp4a", "mp3"]);

    // A stream without a codec passes
    let listed = |codec_type: &str, codecs: &[String]| {
        source
            .media_streams
            .iter()
            .find(|s| is_stream_type(s, codec_type))
            .and_then(|s| s.codec.as_ref())
            .is_none_or(|codec| {
                let codec = codec.to_lowercase();
                codecs.iter().any(|c| codec.contains(c.as_str()))
            })
    };
    listed("Video", &video_codecs) && listed("Audio", &audio_codecs)
}

/// Whether `stream` is of `kind` ("Video", "Audio"). The probe stores ffprobe's lowercase types.
fn is_stream_type(stream: &rustyfin_model::media_source::MediaStream, kind: &str) -> bool {
    stream.codec_type.eq_ignore_ascii_case(kind)
}

/// Check if a media source can be directly played by the client.
/// Direct play requires the client to support all codecs without any transcoding.
fn check_direct_play(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // If no media streams are available, we cannot verify codec compatibility.
    // Reject direct play to avoid Format errors in the browser.
    let has_video_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Video"));
    let has_audio_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Audio"));
    if !has_video_stream && !has_audio_stream {
        // No stream info at all - cannot verify compatibility, reject direct play
        return false;
    }
    // If there's a video item but no video stream info, reject direct play
    if !has_video_stream && source.video_bitrate.is_some() {
        return false;
    }

    // Browsers can only play video in MP4/M4V/WebM containers natively.
    // MKV can sometimes be direct-streamed (remuxed) if codecs are compatible.
    let container = source.container.as_ref().map(|s| s.to_lowercase());
    let browser_compatible_containers = ["mp4", "m4v", "webm"];
    let remux_compatible_containers = ["mkv", "mka"];
    if let Some(ref cont) = container {
        if browser_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            // Native browser container - OK for direct play
        } else if remux_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            // MKV/MKA: allow direct stream (remux) but not direct play
            // This is handled by check_direct_stream separately
            return false;
        } else {
            return false;
        }
    } else {
        // If container is unknown, check the file extension
        let path_lower = source.path.to_lowercase();
        let browser_compatible_extensions = [".mp4", ".m4v", ".webm"];
        if !browser_compatible_extensions
            .iter()
            .any(|ext| path_lower.ends_with(ext))
        {
            return false;
        }
    }

    // Codecs must be in the client's lists, or in the fallback lists if it sent none
    if !codecs_playable(source, query) {
        return false;
    }
    // Without any list, the codec check is the whole answer
    let sent_list = [&query.video_codec, &query.audio_codec]
        .into_iter()
        .flatten()
        .any(|list| list.split(',').any(|c| !c.trim().is_empty()));
    if !sent_list {
        return true;
    }

    // Check bitrate constraints
    if let Some(max_bitrate) = query.max_video_bitrate
        && let Some(video_bitrate) = source.video_bitrate
        && video_bitrate > max_bitrate
    {
        return false;
    }

    if let Some(max_audio_bitrate) = query.max_audio_bitrate
        && let Some(audio_bitrate) = source.audio_bitrate
        && audio_bitrate > max_audio_bitrate
    {
        return false;
    }

    true
}

/// Check if a media source can be directly streamed (re-muxed without transcoding).
/// Direct stream allows container conversion but no codec transcoding.
fn check_direct_stream(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // If direct stream is not enabled, return false
    if !query.enable_direct_stream {
        return false;
    }

    // If no media streams are available, we cannot verify codec compatibility.
    // Reject direct stream to avoid Format errors in the browser.
    let has_video_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Video"));
    let has_audio_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Audio"));
    if !has_video_stream && !has_audio_stream {
        return false;
    }

    // Browsers can only play video in MP4/M4V containers natively.
    // MKV, AVI, TS, etc. require transcoding to a browser-compatible format.
    let container = source.container.as_ref().map(|s| s.to_lowercase());
    let browser_compatible_containers = ["mp4", "m4v", "webm"];
    if let Some(ref cont) = container {
        if !browser_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            return false;
        }
    } else {
        // If container is unknown, check the file extension
        let path_lower = source.path.to_lowercase();
        let browser_compatible_extensions = [".mp4", ".m4v", ".webm"];
        if !browser_compatible_extensions
            .iter()
            .any(|ext| path_lower.ends_with(ext))
        {
            return false;
        }
    }

    // Codecs must be in the client's lists, or in the fallback lists if it sent none
    if !codecs_playable(source, query) {
        return false;
    }

    // If video stream copy is not enabled and there's a video stream, need transcoding
    if !query.enable_video_stream_copy && has_video_stream {
        return false;
    }

    // If audio stream copy is not enabled and there's an audio stream, need transcoding
    if !query.enable_audio_stream_copy && has_audio_stream {
        return false;
    }

    // Check bitrate constraints — direct stream may still need to respect bitrate limits
    // via container-level bitrate reduction, which is not always possible
    if let Some(max_bitrate) = query.max_streaming_bitrate
        && let Some(total_bitrate) = source.bitrate
        && total_bitrate > max_bitrate
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_range ──

    #[test]
    fn test_parse_range_full_range() {
        assert_eq!(parse_range("bytes=0-999", 1000), ByteRange::Partial(0, 999));
    }

    #[test]
    fn test_parse_range_partial() {
        assert_eq!(
            parse_range("bytes=100-199", 1000),
            ByteRange::Partial(100, 199)
        );
    }

    #[test]
    fn test_parse_range_open_ended() {
        assert_eq!(
            parse_range("bytes=500-", 1000),
            ByteRange::Partial(500, 999)
        );
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(
            parse_range("bytes=-100", 1000),
            ByteRange::Partial(900, 999)
        );
    }

    #[test]
    fn test_parse_range_clamp_end_to_file_size() {
        assert_eq!(
            parse_range("bytes=0-9999", 1000),
            ByteRange::Partial(0, 999)
        );
    }

    #[test]
    fn test_parse_range_invalid_no_bytes_prefix() {
        assert_eq!(parse_range("0-999", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_start_beyond_file() {
        assert_eq!(
            parse_range("bytes=1000-1100", 1000),
            ByteRange::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_start_greater_than_end() {
        assert_eq!(parse_range("bytes=500-100", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_empty_string() {
        assert_eq!(parse_range("", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_invalid_numbers() {
        assert_eq!(parse_range("bytes=abc-def", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_suffix_larger_than_file() {
        assert_eq!(parse_range("bytes=-1001", 1000), ByteRange::Ignore);
    }

    // RFC 9110 §14.1.2: a suffix of length 0 and a first byte at or past the end are unsatisfiable (416).

    #[test]
    fn test_parse_range_zero_suffix_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_start_far_beyond_file_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=999999999-", 1000),
            ByteRange::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_start_at_file_size_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=1000-", 1000), ByteRange::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_huge_numbers_do_not_overflow() {
        assert_eq!(
            parse_range("bytes=18446744073709551615-", 1000),
            ByteRange::Unsatisfiable
        );
        assert_eq!(
            parse_range("bytes=0-18446744073709551615", 1000),
            ByteRange::Partial(0, 999)
        );
        assert_eq!(
            parse_range("bytes=-18446744073709551615", 1000),
            ByteRange::Ignore
        );
        assert_eq!(
            parse_range("bytes=18446744073709551616-", 1000),
            ByteRange::Ignore
        );
    }

    #[test]
    fn test_parse_range_empty_file() {
        assert_eq!(parse_range("bytes=0-", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=-5", 0), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_single_bytes() {
        assert_eq!(parse_range("bytes=0-0", 1000), ByteRange::Partial(0, 0));
        assert_eq!(
            parse_range("bytes=999-", 1000),
            ByteRange::Partial(999, 999)
        );
        assert_eq!(parse_range("bytes=-1", 1000), ByteRange::Partial(999, 999));
    }

    #[test]
    fn test_parse_range_suffix_of_whole_file() {
        assert_eq!(parse_range("bytes=-1000", 1000), ByteRange::Partial(0, 999));
    }

    #[test]
    fn test_parse_range_multiple_ranges_are_ignored() {
        assert_eq!(parse_range("bytes=0-0,5-9", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=-0,0-9", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_without_numbers_is_ignored() {
        assert_eq!(parse_range("bytes=-", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=12", 1000), ByteRange::Ignore);
    }

    // ── mime_from_path ──

    #[test]
    fn test_mime_mp4() {
        assert_eq!(mime_from_path("/path/to/video.mp4"), "video/mp4");
    }

    #[test]
    fn test_mime_mkv() {
        assert_eq!(mime_from_path("/path/to/video.mkv"), "video/x-matroska");
    }

    #[test]
    fn test_mime_avi() {
        assert_eq!(mime_from_path("/path/to/video.avi"), "video/x-msvideo");
    }

    #[test]
    fn test_mime_webm() {
        assert_eq!(mime_from_path("/path/to/video.webm"), "video/webm");
    }

    #[test]
    fn test_mime_mov() {
        assert_eq!(mime_from_path("/path/to/video.mov"), "video/quicktime");
    }

    #[test]
    fn test_mime_wmv() {
        assert_eq!(mime_from_path("/path/to/video.wmv"), "video/x-ms-wmv");
    }

    #[test]
    fn test_mime_ts() {
        assert_eq!(mime_from_path("/path/to/video.ts"), "video/mp2t");
    }

    #[test]
    fn test_mime_mpeg() {
        assert_eq!(mime_from_path("/path/to/video.mpeg"), "video/mpeg");
    }

    #[test]
    fn test_mime_mpg() {
        assert_eq!(mime_from_path("/path/to/video.mpg"), "video/mpeg");
    }

    #[test]
    fn test_mime_mp3() {
        assert_eq!(mime_from_path("/path/to/song.mp3"), "audio/mpeg");
    }

    #[test]
    fn test_mime_flac() {
        assert_eq!(mime_from_path("/path/to/song.flac"), "audio/flac");
    }

    #[test]
    fn test_mime_ogg() {
        assert_eq!(mime_from_path("/path/to/song.ogg"), "audio/ogg");
    }

    #[test]
    fn test_mime_opus() {
        assert_eq!(mime_from_path("/path/to/song.opus"), "audio/opus");
    }

    #[test]
    fn test_mime_wav() {
        assert_eq!(mime_from_path("/path/to/song.wav"), "audio/wav");
    }

    #[test]
    fn test_mime_aac() {
        assert_eq!(mime_from_path("/path/to/song.aac"), "audio/aac");
    }

    #[test]
    fn test_mime_m4a() {
        assert_eq!(mime_from_path("/path/to/song.m4a"), "audio/mp4");
    }

    #[test]
    fn test_mime_unknown_extension() {
        assert_eq!(
            mime_from_path("/path/to/file.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_mime_no_extension() {
        assert_eq!(mime_from_path("/path/to/file"), "application/octet-stream");
    }

    #[test]
    fn test_mime_case_insensitive_extension() {
        assert_eq!(mime_from_path("/path/to/video.MP4"), "video/mp4");
    }

    #[test]
    fn test_mime_uppercase_extension() {
        assert_eq!(mime_from_path("/path/to/video.MKV"), "video/x-matroska");
    }

    #[test]
    fn test_mime_m4v() {
        assert_eq!(mime_from_path("/path/to/video.m4v"), "video/mp4");
    }

    #[test]
    fn test_mime_wma() {
        assert_eq!(mime_from_path("/path/to/song.wma"), "audio/x-ms-wma");
    }

    #[test]
    fn test_mime_aiff() {
        assert_eq!(mime_from_path("/path/to/song.aiff"), "audio/aiff");
    }

    #[test]
    fn test_mime_3gp() {
        assert_eq!(mime_from_path("/path/to/video.3gp"), "video/3gpp");
    }

    // ── check_direct_play / check_direct_stream ──

    const LISTS: &str = "VideoCodecs=h264,vp8,vp9,av1&AudioCodecs=aac,mp3,opus,flac";

    /// A source in `container` with one video and one audio stream and a 4 Mbit/s video bitrate.
    fn source_with(
        container: &str,
        video: &str,
        audio: &str,
    ) -> rustyfin_model::media_source::MediaSourceResponse {
        serde_json::from_value(serde_json::json!({
            "Id": "source",
            "Uid": "source",
            "Container": container,
            "Protocol": "File",
            "Path": format!("/media/movie.{container}"),
            "VideoBitrate": 4_000_000,
            "MediaStreams": [
                { "Index": 0, "Codec": video, "CodecType": "Video" },
                { "Index": 1, "Codec": audio, "CodecType": "Audio" },
            ],
        }))
        .unwrap()
    }

    /// The query of a client that sends `params` (`Key=value&...`).
    fn query_from(params: &str) -> VideoPlaybackInfoQuery {
        let params: HashMap<String, String> = params
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        VideoPlaybackInfoQuery::from(params)
    }

    #[test]
    fn test_direct_play_and_stream_need_listed_codecs() {
        let query = query_from(LISTS);
        // The browser plays the video but not the AC3 audio, or the audio but not the HEVC video
        for (video, audio) in [("h264", "ac3"), ("hevc", "aac")] {
            let source = source_with("mp4", video, audio);
            assert!(!check_direct_play(&source, &query), "{video} + {audio}");
            assert!(!check_direct_stream(&source, &query), "{video} + {audio}");
        }
        let source = source_with("mp4", "h264", "aac");
        assert!(check_direct_play(&source, &query));
        assert!(check_direct_stream(&source, &query));
    }

    #[test]
    fn test_direct_play_without_lists_takes_h264_with_aac_or_mp3() {
        let query = query_from("");
        for (video, audio, playable) in [
            ("h264", "aac", true),
            ("h264", "mp3", true),
            ("h264", "ac3", false),
            ("h264", "eac3", false),
            ("h264", "opus", false),
            ("hevc", "aac", false),
            ("vp9", "aac", false),
        ] {
            let source = source_with("mp4", video, audio);
            assert_eq!(
                check_direct_play(&source, &query),
                playable,
                "{video} + {audio}"
            );
            assert_eq!(
                check_direct_stream(&source, &query),
                playable,
                "{video} + {audio}"
            );
        }
    }

    #[test]
    fn test_direct_play_falls_back_for_a_missing_or_empty_list() {
        // Only an audio list: the video must still be H.264
        let query = query_from("AudioCodecs=aac,ac3");
        assert!(!check_direct_play(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(!check_direct_stream(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(check_direct_play(
            &source_with("mp4", "h264", "ac3"),
            &query
        ));
        // Empty lists count as none
        let query = query_from("VideoCodecs=&AudioCodecs=");
        assert!(!check_direct_play(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(!check_direct_play(
            &source_with("mp4", "h264", "ac3"),
            &query
        ));
        assert!(check_direct_play(
            &source_with("mp4", "h264", "aac"),
            &query
        ));
    }

    #[test]
    fn test_direct_play_keeps_container_and_bitrate_rules() {
        let query = query_from(LISTS);
        for container in ["mkv", "avi"] {
            let source = source_with(container, "h264", "aac");
            assert!(!check_direct_play(&source, &query), "{container}");
            assert!(!check_direct_stream(&source, &query), "{container}");
        }
        let query = query_from(&format!("{LISTS}&MaxVideoBitrate=1000000"));
        assert!(!check_direct_play(
            &source_with("mp4", "h264", "aac"),
            &query
        ));
    }

    #[test]
    fn test_probed_lowercase_stream_types_count() {
        // The probe stores ffprobe's stream types, "video" and "audio"
        let probed = |video: &str, audio: &str| {
            let mut source = source_with("mp4", video, audio);
            for stream in &mut source.media_streams {
                stream.codec_type = stream.codec_type.to_lowercase();
            }
            source
        };
        let query = query_from(LISTS);
        assert!(check_direct_play(&probed("h264", "aac"), &query));
        assert!(check_direct_stream(&probed("h264", "aac"), &query));
        for (video, audio) in [("h264", "ac3"), ("hevc", "aac")] {
            let source = probed(video, audio);
            assert!(!check_direct_play(&source, &query), "{video} + {audio}");
            assert!(!check_direct_stream(&source, &query), "{video} + {audio}");
        }
        for copy in ["EnableVideoStreamCopy", "EnableAudioStreamCopy"] {
            let query = query_from(&format!("{LISTS}&{copy}=false"));
            assert!(
                !check_direct_stream(&probed("h264", "aac"), &query),
                "{copy}"
            );
        }
    }

    // ── B16: version selection ──

    /// One H.264 + AAC MP4 source per id, in this order.
    fn sources_with_ids(ids: &[&str]) -> Vec<rustyfin_model::media_source::MediaSourceResponse> {
        ids.iter()
            .map(|id| {
                let mut source = source_with("mp4", "h264", "aac");
                source.id = id.to_string();
                source.path = format!("/media/{id}.mp4");
                source
            })
            .collect()
    }

    fn params_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn test_requested_source_id() {
        assert_eq!(requested_source_id(&params_of(&[])), None);
        assert_eq!(requested_source_id(&params_of(&[("Static", "true")])), None);
        // A blank id is no id
        for blank in ["", "  "] {
            let params = params_of(&[("MediaSourceId", blank)]);
            assert_eq!(requested_source_id(&params), None, "{blank:?}");
        }
        // Clients send the name in either case
        for name in ["MediaSourceId", "mediaSourceId", "MEDIASOURCEID"] {
            let params = params_of(&[(name, "abc")]);
            assert_eq!(requested_source_id(&params), Some("abc"), "{name}");
        }
        let params = params_of(&[("MediaSourceId", " abc ")]);
        assert_eq!(requested_source_id(&params), Some("abc"));
    }

    #[test]
    fn test_select_version() {
        let item = Uuid::new_v4();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let sources = sources_with_ids(&[&a.to_string(), &b.to_string()]);
        let select = |requested: Option<&str>| select_version(item, &sources, requested);

        // No id, or the item's own id in any form: the primary
        for requested in [
            None,
            Some(item.to_string()),
            Some(item.simple().to_string()),
            Some(item.to_string().to_uppercase()),
        ] {
            let version = select(requested.as_deref()).unwrap();
            assert_eq!(version, Version::Primary, "{requested:?}");
        }
        // A version's id, in any form
        for (id, index) in [(a, 0), (b, 1)] {
            for requested in [
                id.to_string(),
                id.simple().to_string(),
                id.to_string().to_uppercase(),
            ] {
                let version = select(Some(&requested)).unwrap();
                assert_eq!(version, Version::Source(index), "{requested}");
            }
        }
        // Anything else names no version of this item
        for requested in [Uuid::new_v4().to_string(), "nope".to_string()] {
            let selected = select(Some(&requested));
            assert!(
                matches!(selected, Err(ApiError::NotFound(_))),
                "{requested}: {selected:?}"
            );
        }

        // A movie that was never probed has no version but itself
        let version = select_version(item, &[], None).unwrap();
        assert_eq!(version, Version::Primary);
        let version = select_version(item, &[], Some(&item.to_string())).unwrap();
        assert_eq!(version, Version::Primary);
        let selected = select_version(item, &[], Some(&a.to_string()));
        assert!(
            matches!(selected, Err(ApiError::NotFound(_))),
            "{selected:?}"
        );

        // A source id that is not a UUID matches as it is
        let odd = sources_with_ids(&["odd-source"]);
        let version = select_version(item, &odd, Some("odd-source")).unwrap();
        assert_eq!(version, Version::Source(0));
    }
}
