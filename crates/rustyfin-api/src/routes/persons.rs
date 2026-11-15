use crate::query::Query;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rustyfin_model::media::QueryResult;
use rustyfin_model::person::{NameIdPair, PersonDto};

use crate::error::ApiError;
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

async fn get_persons(State(state): State<AppState>) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let persons = rustyfin_db::repo_person::get_all_persons(&state.db_ro).await?;
    Ok(Json(persons))
}

async fn get_person(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<PersonDto>, ApiError> {
    let person = rustyfin_db::repo_person::get_person(&state.db_ro, &id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Person not found".to_string()))?;
    Ok(Json(person))
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
) -> Result<Json<QueryResult>, ApiError> {
    let items = rustyfin_db::repo_person::get_person_items(&state.db_ro, &id).await?;
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

/// The Jellyfin paging and library query of the music artist lists (`/Artists`, `/Artists/AlbumArtists`).
#[derive(Deserialize)]
pub(crate) struct ArtistsQuery {
    #[serde(rename = "ParentId")]
    parent_id: Option<String>,
    #[serde(rename = "StartIndex")]
    start_index: Option<i64>,
    #[serde(rename = "Limit")]
    limit: Option<i64>,
}

/// One page of the music artists linked as `link_kind` to items of `item_type`. A `ParentId`
/// that is not a library id is ignored; `TotalRecordCount` counts all matches before paging.
pub(crate) async fn music_artists_page(
    state: &AppState,
    query: &ArtistsQuery,
    link_kind: &str,
    item_type: &str,
) -> Result<QueryResult, ApiError> {
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
    let all =
        rustyfin_db::repo_person::get_music_artists(&state.db_ro, link_kind, item_type, library)
            .await?;
    let total = all.len() as i64;
    let start = query.start_index.unwrap_or(0).max(0);
    let count = query.limit.map_or(total, |limit| limit.max(0));
    let items = all
        .into_iter()
        .skip(start as usize)
        .take(count as usize)
        .collect();
    Ok(QueryResult {
        items,
        total_record_count: total,
        start_index: start,
    })
}

async fn get_artists(
    State(state): State<AppState>,
    Query(query): Query<ArtistsQuery>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(
        music_artists_page(&state, &query, "Artist", "Audio").await?,
    ))
}
