use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json};
use rustyfin_model::auth::Claims;
use serde::Serialize;
use uuid::Uuid;

use crate::parental::HiddenItems;
use crate::state::AppState;

/// Additional Jellyfin-compatible controllers: recommendations, featured, instant mix.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Movies/Recommendations", get(get_recommendations))
        .route("/Movies/Featured", get(get_featured))
        .route(
            "/Users/{userId}/Items/{itemId}/InstantMix",
            get(get_instant_mix),
        )
        // Audio/Music similar endpoints
        .route("/Albums/{id}/InstantMix", get(get_albums_instant_mix))
        .route("/Albums/{id}/Similar", get(get_albums_similar))
        .route("/Artists/{id}/InstantMix", get(get_artists_instant_mix))
        .route("/Artists/{id}/Similar", get(get_artists_similar))
        .route("/Artists/AlbumArtists", get(get_album_artists))
        // Artists by name
        .route("/Artists/{name}", get(get_artists_by_name))
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    // Get highly-rated movies (community_rating >= 7.0)
    let mut items = rustyfin_db::repo_media::get_recommendations(&state.db_ro, 25)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut items);

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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let mut items = rustyfin_db::repo_media::get_featured_movies(&state.db_ro, 10)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut items);

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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    // Get the source item to find similar items

    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Item not found".to_string()))?;

    // Get similar items based on the source
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 50)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);

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

/// GET /Albums/{itemId}/InstantMix
async fn get_albums_instant_mix(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Album not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 50)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Album not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Artist not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 50)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Artist not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
    Query(query): Query<crate::routes::persons::ArtistsQuery>,
) -> Result<(StatusCode, Json<rustyfin_model::media::QueryResult>), crate::error::ApiError> {
    let page = crate::routes::persons::music_artists_page(
        &state,
        &claims,
        &query,
        "AlbumArtist",
        "MusicAlbum",
    )
    .await?;
    Ok((StatusCode::OK, Json(page)))
}

/// GET /Artists/{name} — Search artists by name
async fn get_artists_by_name(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(name): Path<String>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let mut items = rustyfin_db::repo_person::search_persons(&state.db_ro, &name, 20)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    if let Some(visible) = crate::routes::persons::visible_people(&state, &claims).await? {
        items.retain(|i| visible.contains(&i.id.to_string()));
    }
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Movie not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Show not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 20)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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
    Extension(claims): Extension<Claims>,
) -> Result<
    (
        StatusCode,
        Json<QueryResult<rustyfin_model::media::MediaItem>>,
    ),
    crate::error::ApiError,
> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let source = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?
        .ok_or_else(|| crate::error::ApiError::NotFound("Playlist not found".to_string()))?;
    let mut items = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &source, 50)
        .await
        .map_err(|e| crate::error::ApiError::Internal(e.to_string()))?;
    hidden.retain(&mut items);
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

/// Jellyfin's default and largest `size` for the bitrate test.
const BITRATE_TEST_DEFAULT: u64 = 102_400;
const BITRATE_TEST_MAX: u64 = 100_000_000;

/// GET /Playback/BitrateTest?size=: `size` bytes of noise as `application/octet-stream`, as
/// Jellyfin answers. Clients (the Cast receiver, Jellyfin Web) time the download to choose a
/// streaming bitrate. `size` defaults to 102400 and must be 1 to 100000000.
async fn get_bitrate_test(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<axum::response::Response, crate::error::ApiError> {
    use axum::response::IntoResponse;
    let size = match q.iter().find(|(k, _)| k.eq_ignore_ascii_case("size")) {
        None => BITRATE_TEST_DEFAULT,
        Some((_, v)) => v
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=BITRATE_TEST_MAX).contains(n))
            .ok_or_else(|| {
                crate::error::ApiError::BadRequest(format!(
                    "The field size must be between 1 and {BITRATE_TEST_MAX}."
                ))
            })?,
    };
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bitrate_test_bytes(size as usize),
    )
        .into_response())
}

/// `len` bytes that don't compress, so a compressing proxy can't shorten the test (xorshift).
fn bitrate_test_bytes(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15 ^ len as u64;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}
