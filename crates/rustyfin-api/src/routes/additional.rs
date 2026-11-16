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

/// Additional Jellyfin-compatible controllers: recommendations, featured, similar items.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Movies/Recommendations", get(get_recommendations))
        .route("/Movies/Featured", get(get_featured))
        // Audio/Music similar endpoints; the instant mixes are in `instant_mix`
        .route("/Albums/{id}/Similar", get(get_albums_similar))
        .route("/Artists/{id}/Similar", get(get_artists_similar))
        .route("/Artists/AlbumArtists", get(get_album_artists))
        // Artists by name
        .route("/Artists/{name}", get(get_artists_by_name))
        // Movies similar
        .route("/Movies/{id}/Similar", get(get_movies_similar))
        // Shows similar
        .route("/Shows/{id}/Similar", get(get_shows_similar))
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
    // One shared chunk, sent as often as `size` needs: a large test costs no memory per request
    let chunk = BITRATE_TEST_CHUNK.clone();
    let step = chunk.len() as u64;
    let chunks = (0..size.div_ceil(step)).map(move |i| {
        let len = step.min(size - i * step) as usize;
        Ok::<_, std::convert::Infallible>(chunk.slice(..len))
    });
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (axum::http::header::CONTENT_LENGTH, size.to_string()),
        ],
        axum::body::Body::from_stream(futures_util::stream::iter(chunks)),
    )
        .into_response())
}

/// 64 KiB that don't compress (xorshift), so a compressing proxy can't shorten the test.
static BITRATE_TEST_CHUNK: std::sync::LazyLock<axum::body::Bytes> =
    std::sync::LazyLock::new(|| {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut out = Vec::with_capacity(64 * 1024);
        while out.len() < 64 * 1024 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            out.extend_from_slice(&x.to_le_bytes());
        }
        axum::body::Bytes::from(out)
    });
