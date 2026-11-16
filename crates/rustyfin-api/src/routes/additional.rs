use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json};
use rustyfin_model::auth::Claims;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::parental::HiddenItems;
use crate::routes::playback::query_user;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationsQuery {
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default, alias = "CategoryLimit")]
    category_limit: Option<usize>,
    #[serde(default, alias = "ItemLimit")]
    item_limit: Option<usize>,
}

/// One row of movie recommendations, Jellyfin's RecommendationDto
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RecommendationDto {
    items: Vec<rustyfin_model::media::MediaItem>,
    recommendation_type: &'static str,
    baseline_item_name: String,
    category_id: Uuid,
}

/// GET /Movies/Recommendations — rows of unplayed movies like the ones the user played most
/// recently ("SimilarToRecentlyPlayed") or marked favourite or liked ("SimilarToLikedItem"), as
/// Jellyfin's MoviesController builds them: the two kinds take turns up to `CategoryLimit` rows
/// (default 5) of at most `ItemLimit` movies (default 8), then are ordered by kind. A user with no
/// history gets an empty list. Jellyfin's director and actor rows are not built.
async fn get_recommendations(
    State(state): State<AppState>,
    Query(q): Query<RecommendationsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<RecommendationDto>>, crate::error::ApiError> {
    let Some(user_id) = query_user(&state, &claims, q.user_id).await? else {
        return Ok(Json(Vec::new()));
    };
    let category_limit = q.category_limit.unwrap_or(5);
    let item_limit = q.item_limit.unwrap_or(8);
    if category_limit == 0 || item_limit == 0 {
        return Ok(Json(Vec::new()));
    }
    let db = |e: sqlx::Error| crate::error::ApiError::Internal(e.to_string());
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let played = rustyfin_db::repo_media::get_played_ids(&state.db_ro, user_id)
        .await
        .map_err(db)?;

    // A liked movie that is also a recently played seed heads only the recently played row, as
    // Jellyfin excludes those ids from the liked query
    let mut kinds = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for (liked, kind, count) in [
        (false, "SimilarToRecentlyPlayed", 6usize),
        (true, "SimilarToLikedItem", 10),
    ] {
        let mut seeds = rustyfin_db::repo_media::get_recommendation_seeds(
            &state.db_ro,
            user_id,
            q.parent_id,
            liked,
            (count + taken.len()) as i64,
        )
        .await
        .map_err(db)?;
        hidden.retain(&mut seeds);
        seeds.retain(|s| !taken.contains(&s.id));
        seeds.truncate(count);
        taken.extend(seeds.iter().map(|s| s.id));
        kinds.push((kind, seeds.into_iter()));
    }

    let mut rows = Vec::new();
    'fill: loop {
        let mut any = false;
        for (kind, seeds) in kinds.iter_mut() {
            // A seed with nothing unplayed like it gives no row; the next one takes its turn
            for seed in seeds.by_ref() {
                let mut items = rustyfin_db::repo_media::get_similar_items(
                    &state.db_ro,
                    &seed,
                    (item_limit * 3) as i64,
                )
                .await
                .map_err(db)?;
                hidden.retain(&mut items);
                items.retain(|i| !played.contains(&i.id));
                items.truncate(item_limit);
                if items.is_empty() {
                    continue;
                }
                rows.push(RecommendationDto {
                    items,
                    recommendation_type: kind,
                    baseline_item_name: seed.name,
                    category_id: seed.id,
                });
                any = true;
                break;
            }
            if rows.len() >= category_limit {
                break 'fill;
            }
        }
        if !any {
            break;
        }
    }
    // Jellyfin's RecommendationType order: recently played first
    rows.sort_by_key(|r| r.recommendation_type != "SimilarToRecentlyPlayed");

    let tmdb = state.tmdb.api(None);
    for row in rows.iter_mut() {
        crate::routes::items::enrich_items(
            &state.db_ro,
            &state.server_id,
            Some(user_id),
            &mut row.items,
            &state.http_client,
            tmdb.as_ref(),
            &state.data_dir,
            true,
        )
        .await;
    }
    Ok(Json(rows))
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
