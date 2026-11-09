use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_playlist::PlaylistCaller;
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
        .route("/Collections/{id}", get(get_collection))
        .route("/Collections/{id}/Items", get(get_collection_items))
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
}

/// Administrator-only collection routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Collections", post(create_collection))
        .route("/Collections/{id}", delete(delete_collection))
        .route(
            "/Collections/{id}/Items",
            post(add_items_to_collection).delete(remove_items_from_collection),
        )
        .route(
            "/Collections/{id}/Items/{item_id}",
            post(add_item_to_collection).delete(remove_item_from_collection),
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

/// Administrators reach any playlist; everyone else only their own.
fn playlist_caller(claims: &Claims) -> PlaylistCaller {
    if claims.is_admin {
        PlaylistCaller::Admin
    } else {
        PlaylistCaller::Owner(claims.sub)
    }
}

fn playlist_not_found() -> ApiError {
    ApiError::NotFound("Playlist not found".to_string())
}

async fn get_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PlaylistDto>, ApiError> {
    let caller = playlist_caller(&claims);
    let playlist = rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, caller)
        .await?
        .ok_or_else(playlist_not_found)?;
    Ok(Json(playlist))
}

async fn delete_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    let caller = playlist_caller(&claims);
    if !rustyfin_db::repo_playlist::delete_playlist(&state.db, id, caller).await? {
        return Err(playlist_not_found());
    }
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    let caller = playlist_caller(&claims);
    let items = rustyfin_db::repo_playlist::get_playlist_items(&state.db_ro, id, caller).await?;
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
    Extension(claims): Extension<Claims>,
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

    let found = rustyfin_db::repo_playlist::update_playlist(
        &state.db,
        id,
        name.as_deref(),
        media_type.as_deref(),
        playlist_caller(&claims),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !found {
        return Err(playlist_not_found());
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Playlists/{id}/Items - Add items to playlist
async fn add_items_to_playlist(
    State(state): State<AppState>,
    Path(playlist_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    ensure_playlist_visible(&state, playlist_id, &claims).await?;
    let caller = playlist_caller(&claims);
    if let Some(ids) = body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_playlist::add_item_to_playlist(
                    &state.db,
                    playlist_id,
                    item_id,
                    caller,
                )
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
    Extension(claims): Extension<Claims>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    ensure_playlist_visible(&state, playlist_id, &claims).await?;
    let caller = playlist_caller(&claims);
    if let Some(ids) = body.get("Ids").and_then(|v| v.as_array()) {
        for id_val in ids {
            if let Some(id_str) = id_val.as_str()
                && let Ok(item_id) = id_str.parse::<Uuid>()
            {
                rustyfin_db::repo_playlist::remove_item_from_playlist(
                    &state.db,
                    playlist_id,
                    item_id,
                    caller,
                )
                .await
                .ok();
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Shared visibility check for the playlist routes: 404 unless the caller can see the playlist.
async fn ensure_playlist_visible(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<(), ApiError> {
    rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims))
        .await?
        .map(|_| ())
        .ok_or_else(playlist_not_found)
}

/// GET /Playlists/{id}/Users - Get playlist users
async fn get_playlist_users(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    Ok(Json(serde_json::json!([])))
}

/// GET /Playlists/{id}/Users/{userId}
async fn get_playlist_user(
    State(state): State<AppState>,
    Path((id, _user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    Ok(Json(serde_json::json!({})))
}

/// DELETE /Playlists/{id}/Users/{userId}
async fn remove_playlist_user(
    State(state): State<AppState>,
    Path((id, _user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
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
