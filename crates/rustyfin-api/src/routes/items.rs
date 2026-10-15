use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use rustyfin_model::media::{FiltersResult, MediaItem, QueryResult};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Populate `media_sources` on each item using a single batched query.
async fn populate_media_sources(
    pool: &SqlitePool,
    items: &mut [MediaItem],
) -> Result<(), ApiError> {
    if items.is_empty() {
        return Ok(());
    }

    // Collect all item IDs for a single batched query
    let item_ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();

    // Fetch all sources and streams in one round-trip
    let batch_map =
        rustyfin_db::repo_media_source::get_media_sources_batch(pool, &item_ids).await?;

    // Populate each item from the batch results
    for item in items.iter_mut() {
        let sources = batch_map
            .get(&item.id.to_string())
            .cloned()
            .unwrap_or_default();
        item.media_sources = sources;
    }
    Ok(())
}

/// Populate `media_sources` on a single item.
async fn populate_media_source_single(
    pool: &SqlitePool,
    item: &mut MediaItem,
) -> Result<(), ApiError> {
    let sources = rustyfin_db::repo_media_source::get_media_sources(pool, &item.id).await?;
    item.media_sources = sources;
    Ok(())
}

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
        .route(
            "/Items/{id}/Trickplay/Preview/{width}/{seekTicks}",
            get(get_trickplay_preview),
        )
        .route("/Users/{userId}/Items", get(get_user_items))
        .route("/Users/{userId}/Items/Latest", get(get_user_latest))
        .route("/Users/{userId}/Items/Resume", get(get_user_resume))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
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
    exclude_item_types: Option<String>,
    #[serde(rename = "excludeItemTypes")]
    exclude_item_types_lower: Option<String>,
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
    // --- Items Query Parameters ---
    recursive: Option<bool>,
    fields: Option<String>,
    image_types_limit: Option<i32>,
    #[serde(rename = "imageTypesLimit")]
    image_types_limit_lower: Option<i32>,
    is_favorite: Option<bool>,
    #[serde(rename = "isFavorite")]
    is_favorite_lower: Option<bool>,
    is_unplayed: Option<bool>,
    #[serde(rename = "isUnplayed")]
    is_unplayed_lower: Option<bool>,
    is_played: Option<bool>,
    #[serde(rename = "isPlayed")]
    is_played_lower: Option<bool>,
    is_movie: Option<bool>,
    #[serde(rename = "isMovie")]
    is_movie_lower: Option<bool>,
    is_series: Option<bool>,
    #[serde(rename = "isSeries")]
    is_series_lower: Option<bool>,
    is_people: Option<bool>,
    #[serde(rename = "isPeople")]
    is_people_lower: Option<bool>,
    has_subtitles: Option<bool>,
    #[serde(rename = "hasSubtitles")]
    has_subtitles_lower: Option<bool>,
    media_types: Option<String>,
    #[serde(rename = "mediaTypes")]
    media_types_lower: Option<String>,
    genre_ids: Option<String>,
    #[serde(rename = "genreIds")]
    genre_ids_lower: Option<String>,
    studio_ids: Option<String>,
    #[serde(rename = "studioIds")]
    studio_ids_lower: Option<String>,
    artist_ids: Option<String>,
    #[serde(rename = "artistIds")]
    artist_ids_lower: Option<String>,
    album_ids: Option<String>,
    #[serde(rename = "albumIds")]
    album_ids_lower: Option<String>,
    tags: Option<String>,
    features: Option<String>,
    official_ratings: Option<String>,
    #[serde(rename = "officialRatings")]
    official_ratings_lower: Option<String>,
    start_date: Option<String>,
    #[serde(rename = "startDate")]
    start_date_lower: Option<String>,
    end_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date_lower: Option<String>,
    premiere_date: Option<String>,
    #[serde(rename = "premiereDate")]
    premiere_date_lower: Option<String>,
    date_created: Option<String>,
    #[serde(rename = "dateCreated")]
    date_created_lower: Option<String>,
    runtime: Option<String>,
    video_codecs: Option<String>,
    #[serde(rename = "videoCodecs")]
    video_codecs_lower: Option<String>,
    audio_codecs: Option<String>,
    #[serde(rename = "audioCodecs")]
    audio_codecs_lower: Option<String>,
    audio_bitrate: Option<String>,
    #[serde(rename = "audioBitrate")]
    audio_bitrate_lower: Option<String>,
    audio_bitrates: Option<String>,
    #[serde(rename = "audioBitrates")]
    audio_bitrates_lower: Option<String>,
    audio_channels: Option<String>,
    #[serde(rename = "audioChannels")]
    audio_channels_lower: Option<String>,
    has_theme_song: Option<bool>,
    #[serde(rename = "hasThemeSong")]
    has_theme_song_lower: Option<bool>,
    has_special_feature: Option<bool>,
    #[serde(rename = "hasSpecialFeature")]
    has_special_feature_lower: Option<bool>,
    subtitle_language: Option<String>,
    #[serde(rename = "subtitleLanguage")]
    subtitle_language_lower: Option<String>,
    enable_auto_hide: Option<bool>,
    #[serde(rename = "enableAutoHide")]
    enable_auto_hide_lower: Option<bool>,
    collapse_box_set_items: Option<bool>,
    #[serde(rename = "collapseBoxSetItems")]
    collapse_box_set_items_lower: Option<bool>,
    group_projects: Option<bool>,
    #[serde(rename = "groupProjects")]
    group_projects_lower: Option<bool>,
}

#[allow(dead_code)]
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
    fn exclude_item_types(&self) -> Option<&str> {
        self.exclude_item_types
            .as_deref()
            .or(self.exclude_item_types_lower.as_deref())
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
    fn recursive(&self) -> bool {
        self.recursive.unwrap_or(false)
    }
    fn image_types_limit(&self) -> Option<i32> {
        self.image_types_limit.or(self.image_types_limit_lower)
    }
    fn is_favorite(&self) -> Option<bool> {
        self.is_favorite.or(self.is_favorite_lower)
    }
    fn is_unplayed(&self) -> Option<bool> {
        self.is_unplayed.or(self.is_unplayed_lower)
    }
    fn is_played(&self) -> Option<bool> {
        self.is_played.or(self.is_played_lower)
    }
    fn is_movie(&self) -> Option<bool> {
        self.is_movie.or(self.is_movie_lower)
    }
    fn is_series(&self) -> Option<bool> {
        self.is_series.or(self.is_series_lower)
    }
    fn is_people(&self) -> Option<bool> {
        self.is_people.or(self.is_people_lower)
    }
    fn has_subtitles(&self) -> Option<bool> {
        self.has_subtitles.or(self.has_subtitles_lower)
    }
    fn media_types(&self) -> Option<&str> {
        self.media_types
            .as_deref()
            .or(self.media_types_lower.as_deref())
    }
    fn genre_ids(&self) -> Option<&str> {
        self.genre_ids
            .as_deref()
            .or(self.genre_ids_lower.as_deref())
    }
    fn studio_ids(&self) -> Option<&str> {
        self.studio_ids
            .as_deref()
            .or(self.studio_ids_lower.as_deref())
    }
    fn artist_ids(&self) -> Option<&str> {
        self.artist_ids
            .as_deref()
            .or(self.artist_ids_lower.as_deref())
    }
    fn album_ids(&self) -> Option<&str> {
        self.album_ids
            .as_deref()
            .or(self.album_ids_lower.as_deref())
    }
    fn tags(&self) -> Option<&str> {
        self.tags.as_deref()
    }
    fn features(&self) -> Option<&str> {
        self.features.as_deref()
    }
    fn official_ratings(&self) -> Option<&str> {
        self.official_ratings
            .as_deref()
            .or(self.official_ratings_lower.as_deref())
    }
    fn start_date(&self) -> Option<&str> {
        self.start_date
            .as_deref()
            .or(self.start_date_lower.as_deref())
    }
    fn end_date(&self) -> Option<&str> {
        self.end_date.as_deref().or(self.end_date_lower.as_deref())
    }
    fn premiere_date(&self) -> Option<&str> {
        self.premiere_date
            .as_deref()
            .or(self.premiere_date_lower.as_deref())
    }
    fn date_created(&self) -> Option<&str> {
        self.date_created
            .as_deref()
            .or(self.date_created_lower.as_deref())
    }
    fn runtime(&self) -> Option<&str> {
        self.runtime.as_deref()
    }
    fn video_codecs(&self) -> Option<&str> {
        self.video_codecs
            .as_deref()
            .or(self.video_codecs_lower.as_deref())
    }
    fn audio_codecs(&self) -> Option<&str> {
        self.audio_codecs
            .as_deref()
            .or(self.audio_codecs_lower.as_deref())
    }
    fn audio_bitrate(&self) -> Option<&str> {
        self.audio_bitrate
            .as_deref()
            .or(self.audio_bitrate_lower.as_deref())
    }
    fn audio_bitrates(&self) -> Option<&str> {
        self.audio_bitrates
            .as_deref()
            .or(self.audio_bitrates_lower.as_deref())
    }
    fn audio_channels(&self) -> Option<&str> {
        self.audio_channels
            .as_deref()
            .or(self.audio_channels_lower.as_deref())
    }
    fn has_theme_song(&self) -> Option<bool> {
        self.has_theme_song.or(self.has_theme_song_lower)
    }
    fn has_special_feature(&self) -> Option<bool> {
        self.has_special_feature.or(self.has_special_feature_lower)
    }
    fn subtitle_language(&self) -> Option<&str> {
        self.subtitle_language
            .as_deref()
            .or(self.subtitle_language_lower.as_deref())
    }
    fn enable_auto_hide(&self) -> bool {
        self.enable_auto_hide
            .or(self.enable_auto_hide_lower)
            .unwrap_or(false)
    }
    fn collapse_box_set_items(&self) -> bool {
        self.collapse_box_set_items
            .or(self.collapse_box_set_items_lower)
            .unwrap_or(false)
    }
    fn group_projects(&self) -> bool {
        self.group_projects
            .or(self.group_projects_lower)
            .unwrap_or(false)
    }
}

async fn get_items(
    State(state): State<AppState>,
    Query(query): Query<ItemsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let media_types: Option<Vec<String>> = query
        .include_item_types()
        .map(|s| s.split(',').map(|t| format!("\"{}\"", t.trim())).collect());
    let exclude_media_types: Option<Vec<String>> = query
        .exclude_item_types()
        .map(|s| s.split(',').map(|t| format!("\"{}\"", t.trim())).collect());
    let genres: Option<Vec<String>> = query
        .genres
        .as_ref()
        .map(|s| s.split(',').map(|g| g.trim().to_string()).collect());
    let years: Option<Vec<i32>> = query
        .years
        .as_ref()
        .map(|s| s.split(',').filter_map(|y| y.trim().parse().ok()).collect());
    let official_ratings: Option<Vec<String>> = query
        .official_ratings()
        .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
    let tags: Option<Vec<String>> = query
        .tags()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let media_types_filter: Option<Vec<String>> = query
        .media_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let runtime_parts: Option<(i32, i32)> = query.runtime().map(|s| {
        let parts: Vec<&str> = s.split('-').collect();
        let min = parts
            .first()
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(0);
        let max = parts
            .get(1)
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(i32::MAX);
        (min, max)
    });

    let start_index = query.start_index();
    let limit = query.limit();

    // If IsMovie/IsSeries is set, constrain media_type
    let constrained_media_types: Option<Vec<String>> = if let Some(true) = query.is_movie() {
        Some(vec!["\"Movie\"".to_string()])
    } else if let Some(true) = query.is_series() {
        Some(vec!["\"TvShow\"".to_string()])
    } else {
        media_types
    };

    let (mut items, total) = rustyfin_db::repo_media::get_items_filtered(
        &state.db_ro,
        query.parent_id(),
        query.library_id(),
        constrained_media_types.as_deref(),
        exclude_media_types.as_deref(),
        genres.as_deref(),
        years.as_deref(),
        query.search_term(),
        query.sort_by(),
        query.sort_order(),
        start_index,
        limit,
        query.recursive(),
        query.is_favorite(),
        query.is_unplayed(),
        query.is_played(),
        official_ratings.as_deref(),
        tags.as_deref(),
        media_types_filter.as_deref(),
        query.start_date(),
        query.end_date(),
        query.premiere_date(),
        query.date_created(),
        runtime_parts,
    )
    .await?;

    let _ = populate_media_sources(&state.db_ro, &mut items).await;

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
    let id_str = id.to_string();
    // Check cache first
    if let Some(cached) = state.cache.item_details.get(&id_str)
        && let Ok(item) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(item));
    }
    let mut item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let _ = populate_media_source_single(&state.db_ro, &mut item).await;
    // Cache the result
    if let Ok(json) = serde_json::to_value(&item) {
        state.cache.item_details.insert(id_str, json);
    }
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
    let mut children = rustyfin_db::repo_media::get_children(&state.db_ro, id).await?;
    let total = children.len() as i64;
    let _ = populate_media_sources(&state.db_ro, &mut children).await;
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
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let limit = query.limit.unwrap_or(12);
    let similar = rustyfin_db::repo_media::get_similar_items(&state.db_ro, &item, limit).await?;
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
    let mut items = rustyfin_db::repo_media::get_latest_items(
        &state.db_ro,
        limit,
        media_type.as_deref(),
        library_id,
    )
    .await?;
    let _ = populate_media_sources(&state.db_ro, &mut items).await;
    Ok(Json(items))
}

async fn get_resume(
    State(state): State<AppState>,
    Query(query): Query<ResumeQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let limit = query.limit.unwrap_or(10);
    let mut items = rustyfin_db::repo_media::get_resume_items(&state.db_ro, limit).await?;
    let total = items.len() as i64;
    let _ = populate_media_sources(&state.db_ro, &mut items).await;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ResumeQuery {
    limit: Option<i64>,
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
    let cache_key = library_id.map_or("all".to_string(), |id| id.to_string());
    // Check cache first
    if let Some(cached) = state.cache.filter_lists.get(&cache_key)
        && let Ok(filters) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(filters));
    }
    let (genres, years, official_ratings) =
        rustyfin_db::repo_media::get_filters(&state.db_ro, library_id).await?;
    let result = FiltersResult {
        genres,
        years,
        official_ratings,
    };
    // Cache the result
    if let Ok(json) = serde_json::to_value(&result) {
        state.cache.filter_lists.insert(cache_key, json);
    }
    Ok(Json(result))
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
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ResumeQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    let limit = query.limit.unwrap_or(10);
    let mut items =
        rustyfin_db::repo_media::get_user_resume_items(&state.db_ro, user_id, limit).await?;
    let total = items.len() as i64;
    let _ = populate_media_sources(&state.db_ro, &mut items).await;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

/// `GET /Items/{id}/Trickplay/Preview/{width}/{seekTicks}` — Returns a trickplay preview thumbnail.
async fn get_trickplay_preview(
    State(state): State<AppState>,
    Path((id, width, seek_ticks)): Path<(Uuid, u32, i64)>,
) -> Result<Response, ApiError> {
    let result =
        rustyfin_core::trickplay::get_trickplay_thumbnail(&state.db_ro, id, width, seek_ticks)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    match result {
        Some((file_path, content_type)) => {
            let bytes = tokio::fs::read(&file_path)
                .await
                .map_err(|e| ApiError::NotFound(format!("Thumbnail not found: {}", e)))?;

            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type)],
                bytes,
            )
                .into_response())
        }
        None => Err(ApiError::NotFound("Trickplay not available".to_string())),
    }
}
