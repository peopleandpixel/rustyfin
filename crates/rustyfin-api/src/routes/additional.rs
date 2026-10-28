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
        // Audio/Music similar endpoints
        .route("/Albums/{id}/InstantMix", get(get_albums_instant_mix))
        .route("/Albums/{id}/Similar", get(get_albums_similar))
        .route("/Artists/{id}/InstantMix", get(get_artists_instant_mix))
        .route("/Artists/{id}/Similar", get(get_artists_similar))
        .route("/Artists/AlbumArtists", get(get_album_artists))
        // Artists by name
        .route("/Artists/{name}", get(get_artists_by_name))
        // Genres by name
        .route("/Genres/{genreName}", get(get_genres_by_name))
        // Studios by name
        .route("/Studios/{name}", get(get_studios_by_name))
        // Movies similar
        .route("/Movies/{id}/Similar", get(get_movies_similar))
        // Shows similar
        .route("/Shows/{id}/Similar", get(get_shows_similar))
        // Playlists instant mix
        .route("/Playlists/{id}/InstantMix", get(get_playlists_instant_mix))
        // Playback bitrate test
        .route("/Playback/BitrateTest", get(get_bitrate_test))
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

/// GET /Albums/{itemId}/InstantMix
async fn get_albums_instant_mix(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Album not found".to_string()))?;
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

/// GET /Albums/{itemId}/Similar
async fn get_albums_similar(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Album not found".to_string()))?;
    let items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
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

/// GET /Artists/{itemId}/InstantMix
async fn get_artists_instant_mix(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Artist not found".to_string()))?;
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

/// GET /Artists/{itemId}/Similar
async fn get_artists_similar(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Artist not found".to_string()))?;
    let items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
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

/// GET /Artists/AlbumArtists — Returns album artists
async fn get_album_artists(
    State(state): State<AppState>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let items = rustyfin_db::repo_media::get_album_artists(&state.db_ro)
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

/// GET /Artists/{name} — Search artists by name
async fn get_artists_by_name(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let items = rustyfin_db::repo_person::search_persons(&state.db_ro, &name, 20)
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

/// GET /Genres/{genreName} — Search genres by name
async fn get_genres_by_name(
    State(state): State<AppState>,
    Path(genre_name): Path<String>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let items = rustyfin_db::repo_media::get_genres_by_name(&state.db_ro, &genre_name)
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

/// GET /Studios/{name} — Search studios by name
async fn get_studios_by_name(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let items = rustyfin_db::repo_media::get_studios_by_name(&state.db_ro, &name)
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

/// GET /Movies/{itemId}/Similar
async fn get_movies_similar(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Movie not found".to_string()))?;
    let items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
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

/// GET /Shows/{itemId}/Similar
async fn get_shows_similar(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Show not found".to_string()))?;
    let items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
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

/// GET /Playlists/{itemId}/InstantMix
async fn get_playlists_instant_mix(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Playlist not found".to_string()))?;
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

/// GET /Playback/BitrateTest — Returns a URL for bitrate testing
async fn get_bitrate_test(
    State(_state): State<AppState>,
) -> Result<(StatusCode, Json<serde_json::Value>), crate::error::ApiError> {
    // Return a small test file URL for bitrate testing
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "Url": "/Playback/BitrateTestStream",
            "MaxBitrate": null
        })),
    ))
}
