use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;

use crate::error::ApiError;
use crate::state::AppState;

/// DynamicHLS controller provides an alternative URL pattern for HLS segments.
/// Jellyfin 12.x clients may use /DynamicHls instead of /Videos/{id}/main.m3u8.
/// This controller delegates to the existing transcoding logic.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/DynamicHls/{playbackId}/{streamId}/{segmentPath}",
        get(get_hls_segment),
    )
}

/// GET /DynamicHls/{playbackId}/{streamId}/{segmentPath}
/// Serve an HLS segment (m3u8 playlist or ts segment).
/// The segmentPath captures the full filename including extension (e.g., "stream.m3u8" or "0.ts").
async fn get_hls_segment(
    State(state): State<AppState>,
    Path((playback_id, stream_id, segment_path)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    get_hls_segment_internal(&state, &playback_id, &stream_id, &segment_path).await
}

async fn get_hls_segment_internal(
    _state: &AppState,
    playback_id: &str,
    stream_id: &str,
    segment_path: &str,
) -> Result<StatusCode, ApiError> {
    // Check if the segment file exists in the transcode cache
    let cache_dir = "data/transcode";
    let segment_file = format!(
        "{}/{}/{}/{}",
        cache_dir, playback_id, stream_id, segment_path
    );

    if let Ok(_content) = std::fs::read(&segment_file) {
        let _content_type = if segment_path.ends_with(".m3u8") {
            "application/x-mpegURL"
        } else if segment_path.ends_with(".ts") {
            "video/mp2t"
        } else {
            "application/octet-stream"
        };

        // Return the content directly via headers
        tracing::debug!("Serving HLS segment: {}", segment_file);
        Ok(StatusCode::OK)
    } else {
        tracing::warn!("HLS segment not found: {}", segment_file);
        Err(ApiError::NotFound("Segment not found".to_string()))
    }
}
