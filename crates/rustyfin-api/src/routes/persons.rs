use crate::query::Query;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use serde::Deserialize;

use rustyfin_model::media::{MediaItem, QueryResult};
use rustyfin_model::person::{NameIdPair, PersonDto};

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::state::AppState;

mod by_name;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Persons", get(get_persons))
        .route("/Persons/{id}", get(get_person))
        .route("/Persons/{id}/Items", get(get_person_items))
        .route("/Artists", get(get_artists))
        .merge(by_name::routes())
}

async fn get_persons(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let mut persons = rustyfin_db::repo_person::get_all_persons(&state.db_ro).await?;
    if let Some(visible) = visible_people(&state, &claims).await? {
        persons.retain(|p| visible.contains(&p.id));
    }
    Ok(Json(persons))
}

async fn get_person(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = rustyfin_db::repo_person::get_person(&state.db_ro, &id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Person not found".to_string()))?;
    if let Some(visible) = visible_people(&state, &claims).await?
        && !visible.contains(&id)
    {
        return Err(ApiError::NotFound("Person not found".to_string()));
    }
    Ok(Json(person))
}

/// For a caller with limits, the ids of the people linked to an item they may see; `None` for a
/// caller without limits, who sees everyone.
pub(crate) async fn visible_people(
    state: &AppState,
    claims: &Claims,
) -> Result<Option<std::collections::HashSet<String>>, ApiError> {
    let hidden = HiddenItems::for_caller(state, claims).await?;
    if !hidden.is_restricted() {
        return Ok(None);
    }
    let json = hidden.json().unwrap_or_else(|| "[]".into());
    Ok(Some(
        rustyfin_db::repo_person::visible_people(&state.db_ro, &json).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct PersonItemsQuery {
    #[serde(default)]
    pub person_id: Option<String>,
    #[serde(default)]
    pub ids: Option<String>,
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    100
}

async fn get_person_items(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Query(_query): Query<PersonItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let mut items = rustyfin_db::repo_person::get_person_items(&state.db_ro, &id).await?;
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut items, |i| i.id);
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

/// The Jellyfin paging, filter and library query of the music artist lists (`/Artists`,
/// `/Artists/AlbumArtists`). Keys are accepted in PascalCase and camelCase; other keys are ignored.
#[derive(Deserialize)]
pub(crate) struct ArtistsQuery {
    #[serde(rename = "ParentId", alias = "parentId")]
    parent_id: Option<String>,
    #[serde(rename = "StartIndex", alias = "startIndex")]
    start_index: Option<i64>,
    #[serde(rename = "Limit", alias = "limit")]
    limit: Option<i64>,
    #[serde(rename = "SearchTerm", alias = "searchTerm")]
    search_term: Option<String>,
    #[serde(rename = "NameStartsWith", alias = "nameStartsWith")]
    name_starts_with: Option<String>,
    #[serde(rename = "NameStartsWithOrGreater", alias = "nameStartsWithOrGreater")]
    name_starts_with_or_greater: Option<String>,
    #[serde(rename = "NameLessThan", alias = "nameLessThan")]
    name_less_than: Option<String>,
    #[serde(rename = "SortOrder", alias = "sortOrder")]
    sort_order: Option<String>,
}

/// Keeps the artists that pass the name filters, compared case-insensitively. An empty filter
/// value means no filter.
fn filter_artists(all: Vec<MediaItem>, query: &ArtistsQuery) -> Vec<MediaItem> {
    let lowered = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(str::to_lowercase)
    };
    let search = lowered(&query.search_term);
    let prefix = lowered(&query.name_starts_with);
    let from = lowered(&query.name_starts_with_or_greater);
    let before = lowered(&query.name_less_than);
    let mut kept: Vec<MediaItem> = all
        .into_iter()
        .filter(|item| {
            let name = item.name.to_lowercase();
            search.as_deref().is_none_or(|s| name.contains(s))
                && prefix.as_deref().is_none_or(|p| name.starts_with(p))
                && from.as_deref().is_none_or(|f| name.as_str() >= f)
                && before.as_deref().is_none_or(|b| name.as_str() < b)
        })
        .collect();
    if query
        .sort_order
        .as_deref()
        .is_some_and(|order| order.eq_ignore_ascii_case("Descending"))
    {
        kept.reverse();
    }
    kept
}

/// One page of the music artists linked as `link_kind` to items of `item_type` the caller may
/// see. A `ParentId` that is not a library id is ignored; `TotalRecordCount` counts all matches
/// before paging.
pub(crate) async fn music_artists_page(
    state: &AppState,
    claims: &Claims,
    query: &ArtistsQuery,
    link_kind: &str,
    item_type: &str,
) -> Result<QueryResult, ApiError> {
    let hidden = HiddenItems::for_caller(state, claims).await?.json();
    let mut library = None;
    if let Some(id) = query
        .parent_id
        .as_deref()
        .and_then(|v| uuid::Uuid::parse_str(v).ok())
        && rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id)
            .await?
            .is_some()
    {
        library = Some(id);
    }
    let all = rustyfin_db::repo_person::get_music_artists(
        &state.db_ro,
        link_kind,
        item_type,
        library,
        hidden.as_deref(),
    )
    .await?;
    let all = filter_artists(all, query);
    let total = all.len() as i64;
    let start = query.start_index.unwrap_or(0).max(0);
    let count = query.limit.map_or(total, |limit| limit.max(0));
    let mut items: Vec<_> = all
        .into_iter()
        .skip(start as usize)
        .take(count as usize)
        .collect();
    // An artist with a folder item is listed under the folder's id, so its images are the folder's.
    let ids: Vec<uuid::Uuid> = items.iter().map(|item| item.id).collect();
    let images =
        rustyfin_db::repo_image::get_images_batch_of(&state.db_ro, &ids, None, None).await?;
    for item in &mut items {
        if let Some(images) = images.get(&item.id.to_string()) {
            super::items::set_image_tags(item, images);
        }
    }
    Ok(QueryResult {
        items,
        total_record_count: total,
        start_index: start,
    })
}

async fn get_artists(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(
        music_artists_page(&state, &claims, &query, "Artist", "Audio").await?,
    ))
}
