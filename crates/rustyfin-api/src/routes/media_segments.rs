use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/MediaSegments/{itemId}", get(get_media_segments))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct MediaSegmentsResult {
    items: Vec<serde_json::Value>,
    total_record_count: i64,
    start_index: i64,
}

async fn get_media_segments(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<Json<MediaSegmentsResult>, ApiError> {
    let segments = rustyfin_db::repo_media_segment::get_media_segments(&state.db_ro, item_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<serde_json::Value> = segments
        .iter()
        .map(|seg| {
            serde_json::json!({
                "ItemId": seg.item_id,
                "Type": seg.segment_type,
                "SegmentStartTicks": seg.segment_start_ticks,
                "SegmentEndTicks": seg.segment_end_ticks,
            })
        })
        .collect();

    Ok(Json(MediaSegmentsResult {
        items,
        total_record_count: segments.len() as i64,
        start_index: 0,
    }))
}
