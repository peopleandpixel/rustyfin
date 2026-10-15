use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Shows/NextUp", get(next_up))
        .route("/Shows/Upcoming", get(upcoming))
        .route("/Shows/{seriesId}/Seasons", get(get_seasons))
        .route("/Shows/{seriesId}/Episodes", get(get_episodes))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct NextUpQuery {
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    series_id: Option<Uuid>,
    #[serde(default)]
    parent_id: Option<Uuid>,
    #[serde(default)]
    start_index: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn next_up(
    State(state): State<AppState>,
    Query(q): Query<NextUpQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = q.user_id;
    let series_id = q.series_id;
    let start_index = q.start_index.unwrap_or(0) as usize;
    let limit = q.limit.unwrap_or(20) as usize;

    // Get episodes that haven't been fully watched
    let episodes = if let Some(sid) = series_id {
        // For a specific series, get unwatched episodes
        rustyfin_db::repo_media::get_next_up_for_series(&state.db_ro, user_id, sid).await?
    } else {
        // Get all unwatched episodes across all series
        rustyfin_db::repo_media::get_next_up(&state.db_ro, user_id).await?
    };

    let total = episodes.len() as i64;
    let end = start_index.saturating_add(limit).min(episodes.len());
    let items: Vec<MediaItem> = if start_index < episodes.len() {
        episodes[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: start_index as i64,
    }))
}

async fn upcoming(
    State(state): State<AppState>,
    Query(q): Query<NextUpQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = q.user_id;
    let series_id = q.series_id;
    let start_index = q.start_index.unwrap_or(0) as usize;
    let limit = q.limit.unwrap_or(20) as usize;

    let episodes = if let Some(sid) = series_id {
        rustyfin_db::repo_media::get_upcoming_for_series(&state.db_ro, user_id, sid).await?
    } else {
        rustyfin_db::repo_media::get_upcoming(&state.db_ro, user_id).await?
    };

    let total = episodes.len() as i64;
    let end = start_index.saturating_add(limit).min(episodes.len());
    let items: Vec<MediaItem> = if start_index < episodes.len() {
        episodes[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: start_index as i64,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct SeasonsQuery {
    #[serde(default)]
    user_id: Option<Uuid>,
}

async fn get_seasons(
    State(state): State<AppState>,
    Path(series_id): Path<Uuid>,
    Query(_q): Query<SeasonsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let children = rustyfin_db::repo_media::get_children(&state.db_ro, series_id).await?;
    let seasons: Vec<MediaItem> = children
        .into_iter()
        .filter(|c| c.media_type == MediaType::Season)
        .collect();
    let total = seasons.len() as i64;
    Ok(Json(QueryResult {
        items: seasons,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct EpisodesQuery {
    #[serde(default)]
    season_id: Option<Uuid>,
    #[serde(default)]
    season: Option<i32>,
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    start_index: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn get_episodes(
    State(state): State<AppState>,
    Path(series_id): Path<Uuid>,
    Query(q): Query<EpisodesQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    // Single-query fetch: get all episodes for the series using a subquery
    let episodes = if let Some(season_id) = q.season_id {
        // If season_id is specified, fetch children of that season directly
        let children = rustyfin_db::repo_media::get_children(&state.db, season_id).await?;
        children
            .into_iter()
            .filter(|c| c.media_type == MediaType::Episode)
            .collect()
    } else {
        // Use single-query subquery to fetch all episodes for the series
        rustyfin_db::repo_media::get_episodes_for_series(&state.db_ro, series_id, q.season).await?
    };

    let total = episodes.len() as i64;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(i64::MAX).max(0) as usize;
    let end = start.saturating_add(limit).min(episodes.len());
    let page: Vec<MediaItem> = if start < episodes.len() {
        episodes[start..end].to_vec()
    } else {
        Vec::new()
    };

    Ok(Json(QueryResult {
        items: page,
        total_record_count: total,
        start_index: start as i64,
    }))
}
