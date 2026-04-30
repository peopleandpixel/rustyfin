use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::QueryResult;
use rustyfin_model::collection::{CollectionDto, CreateCollectionRequest};
use rustyfin_model::playlist::{CreatePlaylistRequest, PlaylistDto};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Collections", get(get_collections))
        .route("/Collections", post(create_collection))
        .route("/Playlists", post(create_playlist))
        .route("/Playlists/{id}/Items", get(get_playlist_items))
}

async fn get_collections(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<CollectionDto>>, ApiError> {
    let collections = rustyfin_db::repo_collection::get_collections(&state.db, claims.sub).await?;
    Ok(Json(collections))
}

async fn create_collection(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateCollectionRequest>,
) -> Result<Json<CollectionDto>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let id = Uuid::new_v4();
    let item_ids = req.ids.unwrap_or_default();
    rustyfin_db::repo_collection::create_collection(&state.db, id, &req.name, claims.sub, &item_ids)
        .await?;

    let collections = rustyfin_db::repo_collection::get_collections(&state.db, claims.sub).await?;
    let col = collections
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::Internal("collection not found after creation".into()))?;
    Ok(Json(col))
}

async fn create_playlist(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreatePlaylistRequest>,
) -> Result<Json<PlaylistDto>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let id = Uuid::new_v4();
    let item_ids = req.ids.unwrap_or_default();
    rustyfin_db::repo_playlist::create_playlist(
        &state.db,
        id,
        &req.name,
        claims.sub,
        req.media_type.as_deref(),
        &item_ids,
    )
    .await?;

    let playlists = rustyfin_db::repo_playlist::get_playlists(&state.db, claims.sub).await?;
    let pl = playlists
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::Internal("playlist not found after creation".into()))?;
    Ok(Json(pl))
}

async fn get_playlist_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let items = rustyfin_db::repo_playlist::get_playlist_items(&state.db, id).await?;
    let count = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: count,
        start_index: 0,
    }))
}
