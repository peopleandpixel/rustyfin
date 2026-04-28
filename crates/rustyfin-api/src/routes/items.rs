use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use rustyfin_model::media::{FiltersResult, MediaItem, QueryResult};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Items", get(get_items))
        .route("/Items/{id}", get(get_item))
        .route("/Items/{id}", delete(delete_item))
        .route("/Items/{id}/Children", get(get_children))
        .route("/Items/{id}/Similar", get(get_similar))
        .route("/Items/Latest", get(get_latest))
        .route("/Items/Resume", get(get_resume))
        .route("/Items/Filters", get(get_filters))
        .route("/Users/{userId}/Items", get(get_user_items))
        .route("/Users/{userId}/Items/Latest", get(get_user_latest))
        .route("/Users/{userId}/Items/Resume", get(get_user_resume))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemsQuery {
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
    library_id: Option<Uuid>,
    #[serde(rename = "libraryId")]
    library_id_lower: Option<Uuid>,
    include_item_types: Option<String>,
    #[serde(rename = "includeItemTypes")]
    include_item_types_lower: Option<String>,
    genres: Option<String>,
    years: Option<String>,
    search_term: Option<String>,
    #[serde(rename = "searchTerm")]
    search_term_lower: Option<String>,
    sort_by: Option<String>,
    #[serde(rename = "sortBy")]
    sort_by_lower: Option<String>,
    sort_order: Option<String>,
    #[serde(rename = "sortOrder")]
    sort_order_lower: Option<String>,
    start_index: Option<i64>,
    #[serde(rename = "startIndex")]
    start_index_lower: Option<i64>,
    limit: Option<i64>,
}

impl ItemsQuery {
    fn parent_id(&self) -> Option<Uuid> {
        self.parent_id.or(self.parent_id_lower)
    }
    fn library_id(&self) -> Option<Uuid> {
        self.library_id.or(self.library_id_lower)
    }
    fn include_item_types(&self) -> Option<&str> {
        self.include_item_types
            .as_deref()
            .or(self.include_item_types_lower.as_deref())
    }
    fn search_term(&self) -> Option<&str> {
        self.search_term
            .as_deref()
            .or(self.search_term_lower.as_deref())
    }
    fn sort_by(&self) -> Option<&str> {
        self.sort_by.as_deref().or(self.sort_by_lower.as_deref())
    }
    fn sort_order(&self) -> Option<&str> {
        self.sort_order
            .as_deref()
            .or(self.sort_order_lower.as_deref())
    }
    fn start_index(&self) -> i64 {
        self.start_index.or(self.start_index_lower).unwrap_or(0)
    }
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(100)
    }
}

async fn get_items(
    State(state): State<AppState>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let media_types: Option<Vec<String>> = query.include_item_types().map(|s| {
        s.split(',')
            .map(|t| format!("\"{}\"", t.trim()))
            .collect()
    });
    let genres: Option<Vec<String>> = query
        .genres
        .as_ref()
        .map(|s| s.split(',').map(|g| g.trim().to_string()).collect());
    let years: Option<Vec<i32>> = query
        .years
        .as_ref()
        .map(|s| s.split(',').filter_map(|y| y.trim().parse().ok()).collect());

    let start_index = query.start_index();
    let limit = query.limit();

    let (items, total) = rustyfin_db::repo_media::get_items_filtered(
        &state.db,
        query.parent_id(),
        query.library_id(),
        media_types.as_deref(),
        genres.as_deref(),
        years.as_deref(),
        query.search_term(),
        query.sort_by(),
        query.sort_order(),
        start_index,
        limit,
    )
    .await?;

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index,
    }))
}

async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MediaItem>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    Ok(Json(item))
}

async fn delete_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let deleted = rustyfin_db::repo_media::delete_item(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound("item not found".into()));
    }
    Ok(Json(json!({ "status": "ok" })))
}

async fn get_children(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let children = rustyfin_db::repo_media::get_children(&state.db, id).await?;
    let total = children.len() as i64;
    Ok(Json(QueryResult {
        items: children,
        total_record_count: total,
        start_index: 0,
    }))
}

async fn get_similar(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<LimitQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let limit = query.limit.unwrap_or(12);
    let similar = rustyfin_db::repo_media::get_similar_items(&state.db, &item, limit).await?;
    let total = similar.len() as i64;
    Ok(Json(QueryResult {
        items: similar,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LatestQuery {
    limit: Option<i64>,
    include_item_types: Option<String>,
    #[serde(rename = "includeItemTypes")]
    include_item_types_lower: Option<String>,
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
}

async fn get_latest(
    State(state): State<AppState>,
    Query(query): Query<LatestQuery>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    let media_type = query
        .include_item_types
        .as_deref()
        .or(query.include_item_types_lower.as_deref())
        .map(|s| format!("\"{}\"", s.split(',').next().unwrap_or("").trim()));
    let library_id = query.parent_id.or(query.parent_id_lower);
    let items = rustyfin_db::repo_media::get_latest_items(
        &state.db,
        limit,
        media_type.as_deref(),
        library_id,
    )
    .await?;
    Ok(Json(items))
}

async fn get_resume(
    State(_state): State<AppState>,
) -> Result<Json<QueryResult>, ApiError> {
    // Resume requires playback position tracking (Phase 2)
    Ok(Json(QueryResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FiltersQuery {
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
}

async fn get_filters(
    State(state): State<AppState>,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<FiltersResult>, ApiError> {
    let library_id = query.parent_id.or(query.parent_id_lower);
    let (genres, years, official_ratings) =
        rustyfin_db::repo_media::get_filters(&state.db, library_id).await?;
    Ok(Json(FiltersResult {
        genres,
        years,
        official_ratings,
    }))
}

// User-specific item endpoints (same as global for now; user-specific status comes in Phase 2)
async fn get_user_items(
    State(state): State<AppState>,
    Path(_user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    get_items(State(state), Query(query)).await
}

async fn get_user_latest(
    State(state): State<AppState>,
    Path(_user_id): Path<Uuid>,
    Query(query): Query<LatestQuery>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    get_latest(State(state), Query(query)).await
}

async fn get_user_resume(
    State(_state): State<AppState>,
    Path(_user_id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(QueryResult {
        items: vec![],
        total_record_count: 0,
        start_index: 0,
    }))
}
