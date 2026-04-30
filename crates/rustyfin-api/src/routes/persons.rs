use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use rustyfin_model::person::NameIdPair;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Persons", get(get_persons))
        .route("/Persons/{id}", get(get_person))
        .route("/Genres", get(get_genres))
        .route("/Studios", get(get_studios))
        .route("/Artists", get(get_artists))
}

async fn get_persons(
    State(state): State<AppState>,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    // Persons are not stored separately in the current schema;
    // return empty list (placeholder for future NFO person extraction)
    let _ = &state.db;
    Ok(Json(vec![]))
}

async fn get_person(
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Placeholder
    Ok(Json(serde_json::json!({
        "Name": "Unknown",
        "Id": _id,
        "Type": "Person"
    })))
}

async fn get_genres(
    State(state): State<AppState>,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let genres = rustyfin_db::repo_person::get_genres(&state.db).await?;
    Ok(Json(genres))
}

async fn get_studios(
    State(state): State<AppState>,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let studios = rustyfin_db::repo_person::get_studios(&state.db).await?;
    Ok(Json(studios))
}

async fn get_artists(
    State(state): State<AppState>,
) -> Result<Json<Vec<NameIdPair>>, ApiError> {
    let artists = rustyfin_db::repo_person::get_artists(&state.db).await?;
    Ok(Json(artists))
}
