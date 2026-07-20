use axum::extract::Path;
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
    Path(_item_id): Path<Uuid>,
) -> Result<Json<MediaSegmentsResult>, ApiError> {
    // No persisted segments yet (intros, outros, recaps, previews, commercials).
    Ok(Json(MediaSegmentsResult {
        items: Vec::new(),
        total_record_count: 0,
        start_index: 0,
    }))
}
