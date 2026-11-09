//! Genres, music genres, studios and years as lists, and one entry of each by its name.
//! Entries, ids and counts come from `rustyfin_db::repo_by_name`.

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_db::repo_by_name::{
    self, ByNameCounts, ByNameEntry, ByNameFilter, ByNameKind, ByNamePage,
};
use rustyfin_model::auth::Claims;
use rustyfin_model::library::LibraryContentType;
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::query::{
    bool_param, count_param, enum_values, first_value, item_kinds, list_values, media_types,
    parent_param, uuid_param,
};
use crate::routes::playback::query_user;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Genres", get(list_genres))
        .route("/Genres/{genreName}", get(get_genre))
        .route("/MusicGenres", get(list_music_genres))
        .route("/MusicGenres/{genreName}", get(get_music_genre))
        .route("/Studios", get(list_studios))
        .route("/Studios/{name}", get(get_studio))
        .route("/Years", get(list_years))
        .route("/Years/{year}", get(get_year))
}

async fn list_genres(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameList>, ApiError> {
    list_of(&state, &claims, &params, ByNameKind::Genre).await
}

async fn list_music_genres(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameList>, ApiError> {
    list_of(&state, &claims, &params, ByNameKind::MusicGenre).await
}

async fn list_studios(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameList>, ApiError> {
    list_of(&state, &claims, &params, ByNameKind::Studio).await
}

async fn list_years(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameList>, ApiError> {
    list_of(&state, &claims, &params, ByNameKind::Year).await
}

async fn get_genre(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameDto>, ApiError> {
    single_of(&state, &claims, &params, ByNameKind::Genre, &name).await
}

async fn get_music_genre(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameDto>, ApiError> {
    single_of(&state, &claims, &params, ByNameKind::MusicGenre, &name).await
}

async fn get_studio(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameDto>, ApiError> {
    single_of(&state, &claims, &params, ByNameKind::Studio, &name).await
}

async fn get_year(
    State(state): State<AppState>,
    Path(value): Path<String>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ByNameDto>, ApiError> {
    let year = parse_year(&value)?;
    single_of(
        &state,
        &claims,
        &params,
        ByNameKind::Year,
        &year.to_string(),
    )
    .await
}

/// The list of `kind` for a request. On the genre list, a music library as the parent makes it
/// the music-genre list, as Jellyfin's GenresController does.
async fn list_of(
    state: &AppState,
    claims: &Claims,
    params: &[(String, String)],
    kind: ByNameKind,
) -> Result<Json<ByNameList>, ApiError> {
    let query = ListQuery::parse(params)?;
    query_user(state, claims, query.user_id).await?;
    let kind =
        if kind == ByNameKind::Genre && is_music_library(state, query.filter.parent_id).await? {
            ByNameKind::MusicGenre
        } else {
            kind
        };
    let with_counts = query.item_counts || !query.filter.include_item_types.is_empty();
    let ByNamePage { items, total } = repo_by_name::list(&state.db_ro, kind, &query.filter).await?;
    Ok(Json(ByNameList {
        items: items
            .into_iter()
            .map(|entry| ByNameDto::new(entry, state.server_id.as_str(), with_counts))
            .collect(),
        total_record_count: if query.total_count { total } else { 0 },
        start_index: query.filter.start_index,
    }))
}

/// One entry of `kind` by name, with its counts over all its items. A name that no item has is a 404.
async fn single_of(
    state: &AppState,
    claims: &Claims,
    params: &[(String, String)],
    kind: ByNameKind,
    name: &str,
) -> Result<Json<ByNameDto>, ApiError> {
    query_user(
        state,
        claims,
        uuid_param(params, "UserId")?.filter(|id| !id.is_nil()),
    )
    .await?;
    let entry = repo_by_name::get_by_name(&state.db_ro, kind, name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("{} not found", kind.as_str())))?;
    Ok(Json(ByNameDto::new(entry, state.server_id.as_str(), true)))
}

/// Whether `parent` is a library of music content.
async fn is_music_library(state: &AppState, parent: Option<Uuid>) -> Result<bool, ApiError> {
    let Some(parent) = parent else {
        return Ok(false);
    };
    let library = rustyfin_db::repo_library::get_library_by_id(&state.db_ro, parent).await?;
    Ok(library.is_some_and(|library| library.content_type == LibraryContentType::Music))
}

/// The parameters of a list route, read as the spec's table gives them.
struct ListQuery {
    filter: ByNameFilter,
    user_id: Option<Uuid>,
    /// `EnableTotalRecordCount=false` makes the list report a total of 0.
    total_count: bool,
    /// `Fields=ItemCounts` turns the count fields on even without an item-type filter.
    item_counts: bool,
}

impl ListQuery {
    fn parse(params: &[(String, String)]) -> Result<Self, ApiError> {
        let total_count = bool_param(params, "EnableTotalRecordCount")?.unwrap_or(true);
        let filter = ByNameFilter {
            parent_id: parent_param(params, "ParentId")?,
            include_item_types: item_kinds(params, "IncludeItemTypes"),
            exclude_item_types: item_kinds(params, "ExcludeItemTypes"),
            media_types: media_types(params, "MediaTypes"),
            search_term: first_value(params, "SearchTerm").map(str::to_string),
            name_starts_with: first_value(params, "NameStartsWith").map(str::to_string),
            name_starts_with_or_greater: first_value(params, "NameStartsWithOrGreater")
                .map(str::to_string),
            name_less_than: first_value(params, "NameLessThan").map(str::to_string),
            descending: enum_values(params, "SortOrder", &["Ascending", "Descending"]).first()
                == Some(&"Descending"),
            start_index: count_param(params, "StartIndex")?.unwrap_or(0),
            limit: count_param(params, "Limit")?,
        };
        Ok(Self {
            filter,
            user_id: uuid_param(params, "UserId")?.filter(|id| !id.is_nil()),
            total_count,
            item_counts: list_values(params, "Fields", ',')
                .iter()
                .any(|field| field.eq_ignore_ascii_case("ItemCounts")),
        })
    }
}

/// The year in a path: a positive integer, or a 400.
fn parse_year(value: &str) -> Result<i64, ApiError> {
    match value.trim().parse::<i64>() {
        Ok(year) if year > 0 => Ok(year),
        _ => Err(ApiError::BadRequest(
            "Year must be a positive integer".to_string(),
        )),
    }
}

/// The list response: `Items`, `TotalRecordCount` and `StartIndex`, in that order.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ByNameList {
    items: Vec<ByNameDto>,
    total_record_count: usize,
    start_index: usize,
}

/// One genre, music genre, studio or year, with the keys Jellyfin sends for these types.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ByNameDto {
    name: String,
    server_id: String,
    id: Uuid,
    channel_id: Option<String>,
    #[serde(rename = "Type")]
    kind: &'static str,
    #[serde(flatten)]
    counts: Option<CountsDto>,
    image_tags: BTreeMap<String, String>,
    backdrop_image_tags: Vec<String>,
    image_blur_hashes: BTreeMap<String, String>,
    location_type: &'static str,
    media_type: &'static str,
}

/// The count fields. Rustyfin has no programs, trailers, artists or music videos, so those are 0.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct CountsDto {
    child_count: i64,
    movie_count: i64,
    series_count: i64,
    episode_count: i64,
    album_count: i64,
    song_count: i64,
    program_count: i64,
    trailer_count: i64,
    artist_count: i64,
    music_video_count: i64,
}

impl CountsDto {
    fn new(counts: &ByNameCounts) -> Self {
        Self {
            child_count: counts.items,
            movie_count: counts.movies,
            series_count: counts.series,
            episode_count: counts.episodes,
            album_count: counts.albums,
            song_count: counts.songs,
            program_count: 0,
            trailer_count: 0,
            artist_count: 0,
            music_video_count: 0,
        }
    }
}

impl ByNameDto {
    /// The DTO of `entry`. The count fields are included only when `with_counts` is set.
    fn new(entry: ByNameEntry, server_id: &str, with_counts: bool) -> Self {
        let counts = with_counts.then(|| CountsDto::new(&entry.counts));
        Self {
            name: entry.name,
            server_id: server_id.to_string(),
            id: entry.id,
            channel_id: None,
            kind: entry.kind.as_str(),
            counts,
            image_tags: BTreeMap::new(),
            backdrop_image_tags: Vec::new(),
            image_blur_hashes: BTreeMap::new(),
            location_type: "FileSystem",
            media_type: "Unknown",
        }
    }
}
