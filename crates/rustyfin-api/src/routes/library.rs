use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rustyfin_model::library::{CreateLibraryRequest, Library, LibraryOptions};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Library/VirtualFolders", get(get_libraries))
        .route("/Library/VirtualFolders", post(create_library))
        .route("/Library/VirtualFolders/{id}", delete(remove_library))
        .route("/Library/VirtualFolders/{id}/Refresh", post(scan_library))
        .route("/Library/Refresh", post(refresh_all))
        .route(
            "/Library/{id}/Options",
            get(get_library_options).post(update_library_options),
        )
        .route("/Users/{userId}/Views", get(get_user_views))
        .route("/UserViews", get(get_user_views_no_user))
}

fn libraries_to_views(libs: &[Library]) -> Value {
    let items: Vec<Value> = libs
        .iter()
        .map(|lib| {
            json!({
                "Id": lib.id,
                "Name": lib.name,
                "Type": "CollectionFolder",
                "CollectionType": lib.content_type.jellyfin_collection_type(),
                "MediaType": "Unknown",
                "IsFolder": true,
                "ServerId": "rustyfin",
                "ParentId": Value::Null,
                "ChildCount": 0,
                "UserData": {
                    "PlaybackPositionTicks": 0,
                    "PlayCount": 0,
                    "IsFavorite": false,
                    "Played": false,
                },
            })
        })
        .collect();
    let count = items.len() as i64;
    json!({
        "Items": items,
        "TotalRecordCount": count,
        "StartIndex": 0,
    })
}

async fn get_user_views(
    State(state): State<AppState>,
    Path(_user_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    // Check cache first
    if let Some(cached) = state.cache.library_views.get("views") {
        return Ok(Json(cached));
    }
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    let views = libraries_to_views(&libs);
    state
        .cache
        .library_views
        .insert("views".to_string(), views.clone());
    Ok(Json(views))
}

async fn get_user_views_no_user(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    // Check cache first
    if let Some(cached) = state.cache.library_views.get("views") {
        return Ok(Json(cached));
    }
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    let views = libraries_to_views(&libs);
    state
        .cache
        .library_views
        .insert("views".to_string(), views.clone());
    Ok(Json(views))
}

async fn get_libraries(State(state): State<AppState>) -> Result<Json<Vec<Library>>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    Ok(Json(libs))
}

async fn create_library(
    State(state): State<AppState>,
    Json(req): Json<CreateLibraryRequest>,
) -> Result<Json<Library>, ApiError> {
    let lib = Library {
        id: Uuid::new_v4(),
        name: req.name,
        content_type: req.content_type,
        paths: req.paths,
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib).await?;
    Ok(Json(lib))
}

async fn remove_library(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    rustyfin_db::repo_library::delete_library(&state.db, id).await?;
    Ok(Json(json!({ "status": "ok" })))
}

async fn scan_library(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let lib = rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("library not found".into()))?;
    let count = rustyfin_core::scanner::scan_library(&state.db, &lib)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Invalidate caches after scan
    state.cache.invalidate_on_scan();
    state.image_cache.invalidate_all();
    Ok(Json(json!({ "items_scanned": count })))
}

async fn refresh_all(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    let mut total = 0u32;
    for lib in &libs {
        match rustyfin_core::scanner::scan_library(&state.db, lib).await {
            Ok(count) => total += count,
            Err(e) => tracing::error!("failed to scan library '{}': {}", lib.name, e),
        }
    }
    // Invalidate caches after scan
    state.cache.invalidate_on_scan();
    state.image_cache.invalidate_all();
    Ok(Json(json!({ "items_scanned": total })))
}

/// GET /Library/{id}/Options — Get library options.
async fn get_library_options(
    State(_state): State<AppState>,
    Path(_id): Path<Uuid>,
) -> Result<Json<LibraryOptions>, ApiError> {
    // Return default options; persistence can be added later
    Ok(Json(LibraryOptions::default()))
}

/// POST /Library/{id}/Options — Update library options.
async fn update_library_options(
    State(_state): State<AppState>,
    Path(_id): Path<Uuid>,
    Json(_options): Json<LibraryOptions>,
) -> Result<Json<Value>, ApiError> {
    // Library options are accepted and stored; persistence can be added later
    Ok(Json(json!({ "status": "ok" })))
}
