use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use serde::Serialize;

use crate::state::AppState;

/// LiveTV stub endpoints for Jellyfin client compatibility.
/// These return empty results as LiveTV is not yet implemented.

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T> {
    items: Vec<T>,
    #[serde(rename = "TotalRecordCount")]
    total_record_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "StartIndex")]
    start_index: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LiveTvChannel {
    id: String,
    name: String,
    #[serde(rename = "ChannelNumber")]
    channel_number: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LiveTvRecording {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LiveTvGuideEntry {
    id: String,
    #[serde(rename = "ChannelId")]
    channel_id: String,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LiveTvSeriesTimer {
    id: String,
    name: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/LiveTv/Channels", get(get_channels))
        .route("/LiveTv/Recordings", get(get_recordings))
        .route("/LiveTv/Guides", get(get_guides))
        .route("/LiveTv/SeriesTimers", get(get_series_timers))
}

async fn get_channels(
    State(_state): State<AppState>,
) -> (StatusCode, Json<QueryResult<LiveTvChannel>>) {
    (
        StatusCode::OK,
        Json(QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: None,
        }),
    )
}

async fn get_recordings(
    State(_state): State<AppState>,
) -> (StatusCode, Json<QueryResult<LiveTvRecording>>) {
    (
        StatusCode::OK,
        Json(QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: None,
        }),
    )
}

async fn get_guides(
    State(_state): State<AppState>,
) -> (StatusCode, Json<QueryResult<LiveTvGuideEntry>>) {
    (
        StatusCode::OK,
        Json(QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: None,
        }),
    )
}

async fn get_series_timers(
    State(_state): State<AppState>,
) -> (StatusCode, Json<QueryResult<LiveTvSeriesTimer>>) {
    (
        StatusCode::OK,
        Json(QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: None,
        }),
    )
}
