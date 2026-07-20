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
    State(_state): State<AppState>,
    Query(q): Query<NextUpQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    // Minimal kompatible Antwort: leere Liste – Clients akzeptieren das,
    // bis wir eine echte "next-up"-Logik aus der Wiedergabe-Historie ableiten.
    Ok(Json(QueryResult {
        items: Vec::<MediaItem>::new(),
        total_record_count: 0,
        start_index: q.start_index.unwrap_or(0),
    }))
}

async fn upcoming(
    State(_state): State<AppState>,
    Query(q): Query<NextUpQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
        start_index: q.start_index.unwrap_or(0),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonsQuery {
    #[serde(default)]
    user_id: Option<Uuid>,
}

async fn get_seasons(
    State(state): State<AppState>,
    Path(series_id): Path<Uuid>,
    Query(_q): Query<SeasonsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let children = rustyfin_db::repo_media::get_children(&state.db, series_id).await?;
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
    // 1) wenn season_id angegeben → episodes dieser Season
    // 2) sonst: alle Seasons der Serie sammeln und Episoden abrufen
    let mut episodes: Vec<MediaItem> = Vec::new();

    if let Some(season_id) = q.season_id {
        let children = rustyfin_db::repo_media::get_children(&state.db, season_id).await?;
        episodes.extend(
            children
                .into_iter()
                .filter(|c| c.media_type == MediaType::Episode),
        );
    } else {
        let seasons = rustyfin_db::repo_media::get_children(&state.db, series_id).await?;
        for season in seasons.into_iter().filter(|s| s.media_type == MediaType::Season) {
            if let Some(num) = q.season {
                if season.index_number != Some(num) {
                    continue;
                }
            }
            let ep = rustyfin_db::repo_media::get_children(&state.db, season.id).await?;
            episodes.extend(
                ep.into_iter()
                    .filter(|c| c.media_type == MediaType::Episode),
            );
        }
    }

    // Sortiere nach Season/Index
    episodes.sort_by(|a, b| {
        a.parent_index_number
            .cmp(&b.parent_index_number)
            .then(a.index_number.cmp(&b.index_number))
    });

    let total = episodes.len() as i64;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(i64::MAX).max(0) as usize;
    let end = start.saturating_add(limit).min(episodes.len());
    let page: Vec<MediaItem> = if start < episodes.len() {
        episodes.drain(start..end).collect()
    } else {
        Vec::new()
    };

    Ok(Json(QueryResult {
        items: page,
        total_record_count: total,
        start_index: start as i64,
    }))
}
