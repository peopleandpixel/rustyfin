use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::Router;
use serde::Deserialize;
use uuid::Uuid;

use rustyfin_core::transcode::{HwAccel, TranscodeProfile};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/master.m3u8", get(hls_master_playlist))
        .route(
            "/Videos/{id}/hls/{segment}",
            get(hls_segment),
        )
        .route("/Videos/{id}/Transcode", delete(stop_transcode))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodeQuery {
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hw_accel: Option<String>,
    pub segment_duration: Option<u32>,
    pub subtitle_stream_index: Option<u32>,
    pub burn_subtitles: Option<bool>,
}

impl TranscodeQuery {
    fn into_profile(self) -> TranscodeProfile {
        let mut p = TranscodeProfile::default();
        if let Some(v) = self.video_bitrate {
            p.max_video_bitrate_kbps = v;
        }
        if let Some(v) = self.audio_bitrate {
            p.max_audio_bitrate_kbps = v;
        }
        if let Some(v) = self.max_width {
            p.max_width = v;
        }
        if let Some(v) = self.max_height {
            p.max_height = v;
        }
        if let Some(v) = self.video_codec {
            p.video_codec = v;
        }
        if let Some(v) = self.audio_codec {
            p.audio_codec = v;
        }
        if let Some(v) = self.hw_accel {
            p.hw_accel = HwAccel::from_str(&v);
        }
        if let Some(v) = self.segment_duration {
            p.segment_duration = v;
        }
        if let Some(v) = self.subtitle_stream_index {
            p.subtitle_stream_index = Some(v);
        }
        if let Some(v) = self.burn_subtitles {
            p.burn_subtitles = v;
        }
        p
    }
}

async fn hls_master_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<TranscodeQuery>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let profile = query.into_profile();

    let playlist_path = state
        .transcode_manager
        .start_transcode(id, &item.path, &profile)
        .await
        .map_err(|e| ApiError::Internal(format!("Transcode failed: {e}")))?;

    // Read and return the playlist
    let content = tokio::fs::read_to_string(&playlist_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read playlist: {e}")))?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        content,
    )
        .into_response())
}

async fn hls_segment(
    State(state): State<AppState>,
    Path((id, segment)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let segment_path = state.transcode_manager.get_segment_path(id, &segment);

    if !segment_path.exists() {
        return Err(ApiError::NotFound(format!(
            "Segment {segment} not found for item {id}"
        )));
    }

    let data = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?;

    let content_type = if segment.ends_with(".ts") {
        "video/mp2t"
    } else if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        data,
    )
        .into_response())
}

async fn stop_transcode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    state.transcode_manager.stop_transcode(id).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
