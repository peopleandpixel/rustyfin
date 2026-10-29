use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::collections::HashMap;
use std::path::Path as FilePath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/stream", get(stream_video).head(head_stream))
        .route(
            "/Videos/{id}/stream.{container}",
            get(stream_video).head(head_stream),
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

async fn stream_video(
    state: State<AppState>,
    path: Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    stream_file(state, path, headers).await
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

    let file_path = &item.path;
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

    let file_path = item.path.clone();
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

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, file_size));

    match range_header {
        Some((start, end)) => {
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
        None => {
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

/// Parse a `Range: bytes=START-END` header, returning inclusive (start, end).
fn parse_range(range: &str, file_size: u64) -> Option<(u64, u64)> {
    let range = range.strip_prefix("bytes=")?;
    let mut parts = range.splitn(2, '-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();

    if start_str.is_empty() {
        // suffix range: bytes=-500
        let suffix: u64 = end_str.parse().ok()?;
        let start = file_size.checked_sub(suffix)?;
        Some((start, file_size - 1))
    } else {
        let start: u64 = start_str.parse().ok()?;
        let end = if end_str.is_empty() {
            file_size - 1
        } else {
            end_str.parse().ok()?
        };
        if start > end || start >= file_size {
            return None;
        }
        Some((start, end.min(file_size - 1)))
    }
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
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    let file_path = item.path.clone();
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
    media_source_id: Option<String>,
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
            media_source_id: params.get("MediaSourceId").cloned(),
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

/// `GET /Videos/{id}/Universal` — Primary video playback endpoint.
/// Returns a StreamingResponse with media sources and direct play/transcode URLs.
async fn video_universal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let query = VideoPlaybackInfoQuery::from(params.clone());

    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // Select the appropriate media source
    let selected_source_index = if let Some(ref source_id) = query.media_source_id {
        media_sources
            .iter()
            .position(|s| s.id == *source_id)
            .unwrap_or(0)
    } else {
        0
    };

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine direct play/stream/transcode capability
    let (supports_direct_play, supports_direct_stream, supports_transcoding) = if media_sources
        .is_empty()
    {
        // No media sources - fallback to transcoding if possible
        (false, false, true)
    } else {
        let source = &media_sources[selected_source_index];

        let can_direct_play = query.enable_direct_play && check_direct_play(source, &query);
        let can_direct_stream = query.enable_direct_stream && check_direct_stream(source, &query);
        let can_transcode = true;

        (can_direct_play, can_direct_stream, can_transcode)
    };

    // Build the URL based on playback method
    let url = if supports_direct_play {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}&Static=true",
            id,
            media_sources
                .first()
                .map_or("".to_string(), |s| s.id.clone())
        ))
    } else if supports_direct_stream {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}",
            id,
            media_sources
                .first()
                .map_or("".to_string(), |s| s.id.clone())
        ))
    } else if supports_transcoding {
        let mut url = format!(
            "/Videos/{}/main.m3u8?DeviceId={}",
            id,
            query.device_id.unwrap_or_default()
        );
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
        if let Some(seg_time) = query.segment_time {
            url.push_str(&format!("&SegmentTime={}", seg_time));
        }
        Some(url)
    } else {
        None
    };

    // Update media sources with direct play flags based on client capabilities
    for (i, source) in media_sources.iter_mut().enumerate() {
        source.supports_direct_play = i == selected_source_index && supports_direct_play;
        source.supports_direct_stream = i == selected_source_index && supports_direct_stream;
        source.supports_transcoding = i == selected_source_index && supports_transcoding;
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
    let query = VideoPlaybackInfoQuery::from(params);

    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // Select the appropriate media source
    let selected_source_index = if let Some(ref source_id) = query.media_source_id {
        media_sources
            .iter()
            .position(|s| s.id == *source_id)
            .unwrap_or(0)
    } else {
        0
    };

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine direct play/stream/transcode capability based on client codec support
    let (supports_direct_play, supports_direct_stream, supports_transcoding) = if media_sources
        .is_empty()
    {
        // No media sources - fallback to transcoding if possible
        (false, false, true)
    } else {
        let source = &media_sources[selected_source_index];

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
        source.supports_direct_play = i == selected_source_index && supports_direct_play;
        source.supports_direct_stream = i == selected_source_index && supports_direct_stream;
        source.supports_transcoding = i == selected_source_index && supports_transcoding;
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

/// Check if a media source can be directly played by the client.
/// Direct play requires the client to support all codecs without any transcoding.
fn check_direct_play(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // If no media streams are available, we cannot verify codec compatibility.
    // Reject direct play to avoid Format errors in the browser.
    let has_video_stream = source.media_streams.iter().any(|s| s.codec_type == "Video");
    let has_audio_stream = source.media_streams.iter().any(|s| s.codec_type == "Audio");
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

    // Parse codec lists (comma-separated, e.g. "h264,hevc,vp9")
    let video_codecs: Vec<String> = query
        .video_codec
        .as_ref()
        .map(|s| s.split(',').map(|c| c.trim().to_lowercase()).collect())
        .unwrap_or_default();
    let audio_codecs: Vec<String> = query
        .audio_codec
        .as_ref()
        .map(|s| s.split(',').map(|c| c.trim().to_lowercase()).collect())
        .unwrap_or_default();

    // If no codec preferences are specified, check browser compatibility
    if video_codecs.is_empty() && audio_codecs.is_empty() {
        // Check if video codec is browser-compatible (H.264/AVC is the safest)
        let browser_compatible_video_codecs =
            ["h264", "avc1", "hevc", "h265", "vp8", "vp9", "av01"];
        if let Some(video_stream) = source
            .media_streams
            .iter()
            .find(|s| s.codec_type == "Video")
            && let Some(ref codec) = video_stream.codec
        {
            let codec_lower = codec.to_lowercase();
            if !browser_compatible_video_codecs
                .iter()
                .any(|c| codec_lower.contains(c))
            {
                return false;
            }
        }
        // Check if audio codec is browser-compatible
        let browser_compatible_audio_codecs = ["mp3", "aac", "opus", "ac3", "mp4a"];
        if let Some(audio_stream) = source
            .media_streams
            .iter()
            .find(|s| s.codec_type == "Audio")
            && let Some(ref codec) = audio_stream.codec
        {
            let codec_lower = codec.to_lowercase();
            if !browser_compatible_audio_codecs
                .iter()
                .any(|c| codec_lower.contains(c))
            {
                return false;
            }
        }
        return true;
    }

    // Check video codec compatibility
    if !video_codecs.is_empty() {
        let video_stream = source
            .media_streams
            .iter()
            .find(|s| s.codec_type == "Video");
        if let Some(stream) = video_stream
            && let Some(ref codec) = stream.codec
        {
            let codec_lower = codec.to_lowercase();
            // Video codec must be in the list of supported codecs
            if !video_codecs
                .iter()
                .any(|c| codec_lower.contains(c.as_str()))
            {
                return false;
            }
        }
    }

    // Check audio codec compatibility
    if !audio_codecs.is_empty() {
        let audio_stream = source
            .media_streams
            .iter()
            .find(|s| s.codec_type == "Audio");
        if let Some(stream) = audio_stream
            && let Some(ref codec) = stream.codec
        {
            let codec_lower = codec.to_lowercase();
            // Audio codec must be in the list of supported codecs
            if !audio_codecs
                .iter()
                .any(|c| codec_lower.contains(c.as_str()))
            {
                return false;
            }
        }
    } else if let Some(audio_stream) = source
        .media_streams
        .iter()
        .find(|s| s.codec_type == "Audio")
    {
        // If no audio codec preference is specified, check if the audio codec is browser-compatible
        let browser_compatible_audio_codecs = ["mp3", "aac", "opus", "ac3", "mp4a"];
        if let Some(ref codec) = audio_stream.codec {
            let codec_lower = codec.to_lowercase();
            if !browser_compatible_audio_codecs
                .iter()
                .any(|c| codec_lower.contains(c))
            {
                return false;
            }
        }
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
    let has_video_stream = source.media_streams.iter().any(|s| s.codec_type == "Video");
    let has_audio_stream = source.media_streams.iter().any(|s| s.codec_type == "Audio");
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

    // If video stream copy is not enabled and there's a video stream, need transcoding
    if !query.enable_video_stream_copy {
        let has_video = source.media_streams.iter().any(|s| s.codec_type == "Video");
        if has_video {
            return false;
        }
    }

    // If audio stream copy is not enabled and there's an audio stream, need transcoding
    if !query.enable_audio_stream_copy {
        let has_audio = source.media_streams.iter().any(|s| s.codec_type == "Audio");
        if has_audio {
            return false;
        }
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
