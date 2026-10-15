use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rustyfin_model::media::QueryResult;
use rustyfin_model::person::{NameIdPair, PersonDto};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Persons", get(get_persons))
        .route("/Persons/{id}", get(get_person))
        .route("/Persons/{id}/Items", get(get_person_items))
        .route("/Genres", get(get_genres))
        .route("/Studios", get(get_studios))
        .route("/Artists", get(get_artists))
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

async fn get_genres(State(state): State<AppState>) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let genres = rustyfin_db::repo_person::get_genres(&state.db).await?;
    Ok(Json(genres))
}

async fn get_studios(State(state): State<AppState>) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let studios = rustyfin_db::repo_person::get_studios(&state.db).await?;
    Ok(Json(studios))
}

async fn get_artists(State(state): State<AppState>) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let artists = rustyfin_db::repo_person::get_artists(&state.db).await?;
    Ok(Json(artists))
}
