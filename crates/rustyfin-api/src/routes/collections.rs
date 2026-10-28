use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::collection::{CollectionDto, CreateCollectionRequest};
use rustyfin_model::media::QueryResult;
use rustyfin_model::playlist::{CreatePlaylistRequest, PlaylistDto};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Collections", get(get_collections))
        .route("/Collections", post(create_collection))
        .route("/Collections/{id}", get(get_collection))
        .route("/Collections/{id}", delete(delete_collection))
        .route("/Collections/{id}/Items", get(get_collection_items))
        .route(
            "/Collections/{id}/Items/{item_id}",
            post(add_item_to_collection),
        )
        .route(
            "/Collections/{id}/Items/{item_id}",
            delete(remove_item_from_collection),
        )
        .route("/Playlists", get(get_playlists))
        .route("/Playlists", post(create_playlist))
        .route("/Playlists/{id}", get(get_playlist).post(update_playlist))
        .route("/Playlists/{id}", delete(delete_playlist))
        .route(
            "/Playlists/{id}/Items",
            get(get_playlist_items)
                .post(add_items_to_playlist)
                .delete(remove_items_from_playlist),
        )
        .route("/Playlists/{id}/Users", get(get_playlist_users))
        .route(
            "/Playlists/{id}/Users/{userId}",
            get(get_playlist_user).delete(remove_playlist_user),
        )
        .route(
            "/Collections/{id}/Items",
            post(add_items_to_collection).delete(remove_items_from_collection),
        )
}

async fn get_collections(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<CollectionDto>>, ApiError> {
    let collections =
        rustyfin_db::repo_collection::get_collections(&state.db_ro, claims.sub).await?;
    Ok(Json(collections))
}

async fn get_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CollectionDto>, ApiError> {
    let collection = rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Collection not found".to_string()))?;
    Ok(Json(collection))
}

async fn delete_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::delete_collection(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
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
    rustyfin_db::repo_collection::create_collection(
        &state.db, id, &req.name, claims.sub, &item_ids,
    )
    .await?;

    let collections =
        rustyfin_db::repo_collection::get_collections(&state.db_ro, claims.sub).await?;
    let col = collections
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::Internal("collection not found after creation".into()))?;
    Ok(Json(col))
}

async fn get_collection_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let items = rustyfin_db::repo_collection::get_collection_items(&state.db_ro, id).await?;
    let count = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: count,
        start_index: 0,
    }))
}

async fn add_item_to_collection(
    State(state): State<AppState>,
    Path((collection_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::add_item_to_collection(&state.db, collection_id, item_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_item_from_collection(
    State(state): State<AppState>,
    Path((collection_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::remove_item_from_collection(&state.db, collection_id, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_playlists(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<PlaylistDto>>, ApiError> {
    let playlists = rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await?;
    Ok(Json(playlists))
}

async fn get_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PlaylistDto>, ApiError> {
    let playlist = rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Playlist not found".to_string()))?;
    Ok(Json(playlist))
}

async fn delete_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_playlist::delete_playlist(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
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

    let playlists = rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await?;
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
    let items = rustyfin_db::repo_playlist::get_playlist_items(&state.db_ro, id).await?;
    let count = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: count,
        start_index: 0,
    }))
}

/// POST /Playlists/{id} - Update playlist
async fn update_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    let name = body
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let media_type = body
        .get("MediaType")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    rustyfin_db::repo_playlist::update_playlist(
        &state.db,
        id,
        name.as_deref(),
        media_type.as_deref(),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Playlists/{id}/Items - Add items to playlist
async fn add_items_to_playlist(
    State(state): State<AppState>,
    Path(playlist_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    if let Some(ids) = _body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_playlist::add_item_to_playlist(&state.db, playlist_id, item_id)
                    .await
                    .ok();
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Playlists/{id}/Items - Remove items from playlist
async fn remove_items_from_playlist(
    State(state): State<AppState>,
    Path(playlist_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    if let Some(ids) = _body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_playlist::remove_item_from_playlist(
                    &state.db,
                    playlist_id,
                    item_id,
                )
                .await
                .ok();
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// GET /Playlists/{id}/Users - Get playlist users
async fn get_playlist_users(
    State(_state): State<AppState>,
    Path(_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!([])))
}

/// GET /Playlists/{id}/Users/{userId}
async fn get_playlist_user(
    State(_state): State<AppState>,
    Path((_id, _user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// DELETE /Playlists/{id}/Users/{userId}
async fn remove_playlist_user(
    State(_state): State<AppState>,
    Path((_id, _user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Collections/{id}/Items - Add items to collection (bulk)
async fn add_items_to_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    if let Some(ids) = _body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_collection::add_item_to_collection(
                    &state.db,
                    collection_id,
                    item_id,
                )
                .await
                .ok();
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Collections/{id}/Items - Remove items from collection (bulk)
async fn remove_items_from_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<Uuid>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    if let Some(ids) = _body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_collection::remove_item_from_collection(
                    &state.db,
                    collection_id,
                    item_id,
                )
                .await
                .ok();
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
