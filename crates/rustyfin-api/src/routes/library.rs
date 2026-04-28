use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rustyfin_model::library::{CreateLibraryRequest, Library};
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
}

async fn get_libraries(State(state): State<AppState>) -> Result<Json<Vec<Library>>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db).await?;
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
    let lib = rustyfin_db::repo_library::get_library_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound("library not found".into()))?;
    let count = rustyfin_core::scanner::scan_library(&state.db, &lib)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({ "items_scanned": count })))
}

async fn refresh_all(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db).await?;
    let mut total = 0u32;
    for lib in &libs {
        match rustyfin_core::scanner::scan_library(&state.db, lib).await {
            Ok(count) => total += count,
            Err(e) => tracing::error!("failed to scan library '{}': {}", lib.name, e),
        }
    }
    Ok(Json(json!({ "items_scanned": total })))
}
