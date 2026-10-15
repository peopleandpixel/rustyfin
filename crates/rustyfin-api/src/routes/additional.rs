use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use serde::Serialize;
use uuid::Uuid;

use crate::state::AppState;

/// Additional Jellyfin-compatible controllers: recommendations, featured, instant mix, music genres.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Movies/Recommendations", get(get_recommendations))
        .route("/Movies/Featured", get(get_featured))
        .route(
            "/Users/{userId}/Items/{itemId}/InstantMix",
            get(get_instant_mix),
        )
        .route("/Genres/Music", get(get_music_genres))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T: Serialize> {
    items: Vec<T>,
    #[serde(rename = "TotalRecordCount")]
    total_record_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "StartIndex")]
    start_index: Option<i64>,
}

/// GET /Movies/Recommendations — Returns recommended movies based on user preferences.
/// Uses highly-rated and recently added movies as recommendations.
async fn get_recommendations(
    State(state): State<AppState>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    // Get highly-rated movies (community_rating >= 7.0)
    let items = rustyfin_db::repo_media::get_recommendations(&state.db_ro, 25)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let total = items.len() as i64;
    Ok((
        StatusCode::OK,
        Json(QueryResult {
            items,
            total_record_count: total,
            start_index: None,
        }),
    ))
}

/// GET /Movies/Featured — Returns featured movies (recently added or highly rated).
async fn get_featured(
    State(state): State<AppState>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let items = rustyfin_db::repo_media::get_featured_movies(&state.db_ro, 10)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let total = items.len() as i64;
    Ok((
        StatusCode::OK,
        Json(QueryResult {
            items,
            total_record_count: total,
            start_index: None,
        }),
    ))
}

/// GET /Users/{userId}/Items/{itemId}/InstantMix — Returns an instant mix based on an item.
/// Generates a playlist of similar items (same genre or similar rating).
async fn get_instant_mix(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    // Get the source item to find similar items
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Item not found".to_string()))?;

    // Get similar items based on the source
    let items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 50)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let total = items.len() as i64;
    Ok((
        StatusCode::OK,
        Json(QueryResult {
            items,
            total_record_count: total,
            start_index: None,
        }),
    ))
}

/// GET /Genres/Music — Returns all music genres from the library.
async fn get_music_genres(
    State(state): State<AppState>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let genres = rustyfin_db::repo_media::get_music_genres(&state.db_ro)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;

    let total = genres.len() as i64;
    Ok((
        StatusCode::OK,
        Json(QueryResult {
            items: genres,
            total_record_count: total,
            start_index: None,
        }),
    ))
}
