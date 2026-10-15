use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use rustyfin_model::search::SearchHintResult;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Search/Hints", get(search_hints))
        .route("/Search", get(full_search))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchQuery {
    pub search_term: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct FullSearchQuery {
    pub term: Option<String>,
    #[serde(default)]
    pub search_filter: Option<String>,
    #[serde(default)]
    pub include_item_types: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: i64,
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default)]
    pub genre_ids: Option<String>,
    #[serde(default)]
    pub artist_ids: Option<String>,
    #[serde(default)]
    pub album_artist_ids: Option<String>,
    #[serde(default)]
    pub album_ids: Option<String>,
    #[serde(default)]
    pub studio_ids: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub featured: Option<bool>,
    #[serde(default)]
    pub is_favorite: Option<bool>,
    #[serde(default)]
    pub is_unplayed: Option<bool>,
    #[serde(default)]
    pub is_played: Option<bool>,
    #[serde(default)]
    pub is_movie: Option<bool>,
    #[serde(default)]
    pub is_series: Option<bool>,
    #[serde(default)]
    pub is_episode: Option<bool>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
}

fn default_search_limit() -> i64 {
    100
}

async fn search_hints(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchHintResult>, ApiError> {
    let term = query.search_term.unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        }));
    }

    // Check cache first
    let cache_key = format!("hints:{}:{}", term.to_lowercase(), query.limit);
    if let Some(cached) = state.cache.search_results.get(&cache_key)
        && let Ok(result) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(result));
    }

    let hints = rustyfin_db::repo_search::search_hints(&state.db_ro, &term, query.limit).await?;
    let count = hints.len() as i64;
    let result = SearchHintResult {
        search_hints: hints,
        total_record_count: count,
    };
    // Cache the result
    if let Ok(json) = serde_json::to_value(&result) {
        state.cache.search_results.insert(cache_key, json);
    }
    Ok(Json(result))
}

async fn full_search(
    State(state): State<AppState>,
    Query(query): Query<FullSearchQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let term = query.term.unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: query.start_index.unwrap_or(0),
        }));
    }

    let start_index = query.start_index.unwrap_or(0) as usize;
    let limit = query.limit as usize;

    // Parse include_item_types
    let item_types: Vec<MediaType> = if let Some(ref types) = query.include_item_types {
        types
            .split(',')
            .filter_map(|t| match t.trim() {
                "Movie" => Some(MediaType::Movie),
                "Series" => Some(MediaType::Series),
                "Episode" => Some(MediaType::Episode),
                "Audio" => Some(MediaType::Audio),
                "Photo" => Some(MediaType::Photo),
                _ => None,
            })
            .collect()
    } else {
        vec![
            MediaType::Movie,
            MediaType::Series,
            MediaType::Episode,
            MediaType::Audio,
        ]
    };

    // Check cache first
    let cache_key = format!(
        "search:{}:{}:{}:{}",
        term.to_lowercase(),
        item_types.len(),
        start_index,
        limit
    );
    if let Some(cached) = state.cache.search_results.get(&cache_key)
        && let Ok(result) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(result));
    }

    let items = rustyfin_db::repo_search::search_items(
        &state.db_ro,
        &term,
        &item_types,
        (start_index + limit) as i64,
    )
    .await?;

    let total = items.len() as i64;
    let end = start_index.saturating_add(limit).min(items.len());
    let page: Vec<MediaItem> = if start_index < items.len() {
        items[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    let result = QueryResult {
        items: page,
        total_record_count: total,
        start_index: start_index as i64,
    };
    // Cache the result
    if let Ok(json) = serde_json::to_value(&result) {
        state.cache.search_results.insert(cache_key, json);
    }
    Ok(Json(result))
}
