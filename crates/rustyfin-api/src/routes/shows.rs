use crate::query::Query;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::{LocationType, MediaItem, MediaType, QueryResult};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::query::{bool_param, first_value, parent_param, uuid_param};
use crate::routes::activity::parse_min_date;
use crate::routes::items::enrich_items;
use crate::routes::playback::query_user;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Shows/NextUp", get(next_up))
        .route("/Shows/Upcoming", get(upcoming))
        .route("/Shows/{seriesId}/Seasons", get(get_seasons))
        .route("/Shows/{seriesId}/Episodes", get(get_episodes))
}

/// Keeps the episodes of the library `parent_id` names (all when `None`), as a library's own
/// Next Up and Upcoming rows ask with `ParentId`
fn in_library(episodes: &mut Vec<MediaItem>, parent_id: Option<Uuid>) {
    if let Some(library) = parent_id {
        episodes.retain(|e| e.library_id == library);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct NextUpQuery {
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "SeriesId")]
    series_id: Option<Uuid>,
    #[serde(default, alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default, alias = "StartIndex")]
    start_index: Option<i64>,
    #[serde(default, alias = "Limit")]
    limit: Option<i64>,
}

/// `GET /Shows/NextUp`, with Jellyfin's `enableResumable` (default true), `enableRewatching`
/// (default false), `nextUpDateCutoff` and `enableTotalRecordCount` (default true).
/// Jellyfin 12.4 has no `disableFirstEpisode`; it is ignored.
async fn next_up(
    State(state): State<AppState>,
    Query(q): Query<NextUpQuery>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = query_user(&state, &claims, q.user_id).await?;
    let series_id = q.series_id;
    let start_index = q.start_index.unwrap_or(0) as usize;
    let limit = q.limit.unwrap_or(20) as usize;
    let date_cutoff = first_value(&params, "nextUpDateCutoff")
        .map(|d| {
            parse_min_date(d)
                .map(|d| d.and_utc())
                .ok_or_else(|| ApiError::BadRequest("nextUpDateCutoff must be a date".into()))
        })
        .transpose()?;
    let options = rustyfin_db::repo_media::NextUpOptions {
        date_cutoff,
        enable_resumable: bool_param(&params, "enableResumable")?.unwrap_or(true),
        enable_rewatching: bool_param(&params, "enableRewatching")?.unwrap_or(false),
    };
    let count_total = bool_param(&params, "enableTotalRecordCount")?.unwrap_or(true);

    // One episode per series: its next unplayed episode (see get_next_up)
    let mut episodes =
        rustyfin_db::repo_media::get_next_up(&state.db_ro, user_id, series_id, &options).await?;
    hidden_for(&state, &claims, user_id)
        .await?
        .retain(&mut episodes);
    in_library(&mut episodes, q.parent_id);

    let total = if count_total {
        episodes.len() as i64
    } else {
        0
    };
    let end = start_index.saturating_add(limit).min(episodes.len());
    let mut items: Vec<MediaItem> = if start_index < episodes.len() {
        episodes[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    // Enrich the page for the caller, as Resume items are (skip blurhash for list views)
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        user_id,
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
    )
    .await;

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: start_index as i64,
    }))
}

async fn upcoming(
    State(state): State<AppState>,
    Query(q): Query<NextUpQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = query_user(&state, &claims, q.user_id).await?;
    let series_id = q.series_id;
    let start_index = q.start_index.unwrap_or(0) as usize;
    let limit = q.limit.unwrap_or(20) as usize;

    let mut episodes = if let Some(sid) = series_id {
        rustyfin_db::repo_media::get_upcoming_for_series(&state.db_ro, user_id, sid).await?
    } else {
        rustyfin_db::repo_media::get_upcoming(&state.db_ro, user_id).await?
    };
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut episodes);
    in_library(&mut episodes, q.parent_id);

    let total = episodes.len() as i64;
    let end = start_index.saturating_add(limit).min(episodes.len());
    let mut items: Vec<MediaItem> = if start_index < episodes.len() {
        episodes[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    // Enrich the page as Next Up does, so its cards get their series and pictures
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        user_id,
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
    )
    .await;

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: start_index as i64,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeasonsQuery {
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
}

async fn get_seasons(
    State(state): State<AppState>,
    Path(series_id): Path<Uuid>,
    Query(q): Query<SeasonsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = query_user(&state, &claims, q.user_id).await?;
    let children = rustyfin_db::repo_media::get_children(&state.db_ro, series_id).await?;
    let mut seasons: Vec<MediaItem> = children
        .into_iter()
        .filter(|c| c.media_type == MediaType::Season)
        .collect();
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut seasons);
    let total = seasons.len() as i64;

    let tmdb_key = state
        .plugin_manager
        .get_configuration("tmdb")
        .await
        .and_then(|c| {
            c.values
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    let tmdb = tmdb_key.as_deref();
    enrich_items(
        &state.db_ro,
        &state.server_id,
        user_id,
        &mut seasons,
        &state.http_client,
        state.tmdb.api(tmdb).as_ref(),
        &state.data_dir,
        true,
    )
    .await;

    Ok(Json(QueryResult {
        items: seasons,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodesQuery {
    #[serde(default, alias = "SeasonId")]
    season_id: Option<Uuid>,
    #[serde(default, alias = "Season")]
    season: Option<i32>,
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "StartIndex")]
    start_index: Option<i64>,
    #[serde(default, alias = "Limit")]
    limit: Option<i64>,
}

/// `GET /Shows/{seriesId}/Episodes`, filtered as Jellyfin 12.4's `TvShowsController.GetEpisodes`
/// does: `isMissing`, then `startItemId` (drop the episodes before it), then `adjacentTo` (it and
/// its neighbours), then `sortBy=Random`, then paging. The count is of the filtered list.
/// Jellyfin maps an alternate version's `startItemId` to its primary version; Rustyfin stores no
/// alternate versions, so every id is already a primary one.
async fn get_episodes(
    State(state): State<AppState>,
    Path(series_id): Path<Uuid>,
    Query(q): Query<EpisodesQuery>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = query_user(&state, &claims, q.user_id).await?;
    let is_missing = bool_param(&params, "isMissing")?;
    let start_item = uuid_param(&params, "startItemId")?;
    let adjacent_to = parent_param(&params, "adjacentTo")?;
    let random = first_value(&params, "sortBy").is_some_and(|s| s.eq_ignore_ascii_case("Random"));

    // Jellyfin answers 404 for a seasonId that is not a season, or (without one) a series id
    // that is not a series.
    let (wanted, kind) = match q.season_id {
        Some(season_id) => (season_id, MediaType::Season),
        None => (series_id, MediaType::Series),
    };
    let found = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, wanted).await?;
    if !found.is_some_and(|item| item.media_type == kind) {
        return Err(ApiError::NotFound(format!(
            "No {kind:?} exists with Id {wanted}"
        )));
    }

    // Single-query fetch: get all episodes for the series using a subquery
    let mut episodes: Vec<MediaItem> = if let Some(season_id) = q.season_id {
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
    hidden_for(&state, &claims, user_id)
        .await?
        .retain(&mut episodes);

    if let Some(missing) = is_missing {
        // Jellyfin's IsMissingEpisode: a Virtual episode.
        episodes.retain(|e| (e.location_type == LocationType::Virtual) == missing);
    }
    if let Some(start_item) = start_item {
        let from = episodes
            .iter()
            .position(|e| e.id == start_item)
            .unwrap_or(episodes.len());
        episodes.drain(..from);
    }
    if let Some(adjacent_to) = adjacent_to {
        episodes = adjacent(episodes, adjacent_to);
    }
    if random {
        rustyfin_db::repo_media::shuffle(&mut episodes);
    }

    let total = episodes.len() as i64;
    let start = q.start_index.unwrap_or(0).max(0) as usize;
    let limit = q.limit.unwrap_or(i64::MAX).max(0) as usize;
    let end = start.saturating_add(limit).min(episodes.len());
    let mut page: Vec<MediaItem> = if start < episodes.len() {
        episodes[start..end].to_vec()
    } else {
        Vec::new()
    };

    let tmdb_key = state
        .plugin_manager
        .get_configuration("tmdb")
        .await
        .and_then(|c| {
            c.values
                .get("api_key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    let tmdb = tmdb_key.as_deref();
    enrich_items(
        &state.db_ro,
        &state.server_id,
        user_id,
        &mut page,
        &state.http_client,
        state.tmdb.api(tmdb).as_ref(),
        &state.data_dir,
        true,
    )
    .await;

    Ok(Json(QueryResult {
        items: page,
        total_record_count: total,
        start_index: start as i64,
    }))
}

/// The items hidden from the user a route answers for: `user` (an administrator's `userId`), else
/// the caller.
async fn hidden_for(
    state: &AppState,
    claims: &Claims,
    user: Option<Uuid>,
) -> Result<HiddenItems, ApiError> {
    match user {
        Some(user) => HiddenItems::for_user(state, user).await,
        None => HiddenItems::for_caller(state, claims).await,
    }
}

/// Jellyfin 12.4's `UserViewBuilder.FilterForAdjacency`: the episode `id` and the ones just
/// before and after it, or nothing when `id` is not in the list.
fn adjacent(episodes: Vec<MediaItem>, id: Uuid) -> Vec<MediaItem> {
    let Some(at) = episodes.iter().position(|e| e.id == id) else {
        return Vec::new();
    };
    let keep = |i: usize| i + 1 >= at && i <= at + 1;
    episodes
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep(*i))
        .map(|(_, e)| e)
        .collect()
}
