use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::path::Path as FilePath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/stream", get(stream_video).head(head_stream))
        .route("/Videos/{id}/stream.{container}", get(stream_video).head(head_stream))
        .route("/Audio/{id}/stream", get(stream_audio).head(head_stream))
        .route("/Audio/{id}/stream.{container}", get(stream_audio).head(head_stream))
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
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
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
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let file_path = item.path.clone();
    let metadata = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;

    let file_size = metadata.len();
    let content_type = mime_from_path(&file_path);

    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_range(s, file_size));

    match range_header {
        Some((start, end)) => {
            let length = end - start + 1;

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
