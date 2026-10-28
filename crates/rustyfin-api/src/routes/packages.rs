use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PackageDto {
    id: String,
    name: String,
    version: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "RepositoryId", skip_serializing_if = "Option::is_none")]
    repository_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct PackageRepositoryDto {
    id: String,
    name: String,
    url: String,
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AddRepositoryRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InstallPackageRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Packages", get(get_packages))
        .route(
            "/Packages/Repositories",
            get(get_repositories).post(add_repository),
        )
        .route(
            "/Packages/Repositories/{repositoryId}",
            delete(delete_repository),
        )
        .route("/Packages/{packageId}/Install", post(install_package))
        .route("/Packages/{packageId}/Uninstall", post(uninstall_package))
        .route("/Packages/{packageId}/Enable", post(enable_package))
        .route("/Packages/{packageId}/Disable", post(disable_package))
        .route("/Packages/Restore", post(restore_backup))
}

async fn get_packages(State(state): State<AppState>) -> Result<Json<Vec<PackageDto>>, ApiError> {
    let packages = rustyfin_db::repo_package::get_installed_packages(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<PackageDto> = packages
        .into_iter()
        .map(|p| PackageDto {
            id: p.id,
            name: p.name,
            version: p.version,
            enabled: p.enabled,
            description: if p.description.is_empty() {
                None
            } else {
                Some(p.description)
            },
            repository_id: p.repository_id,
        })
        .collect();

    Ok(Json(items))
}

async fn get_repositories(
    State(state): State<AppState>,
) -> Result<Json<Vec<PackageRepositoryDto>>, ApiError> {
    let repos = rustyfin_db::repo_package::get_repositories(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<PackageRepositoryDto> = repos
        .into_iter()
        .map(|r| PackageRepositoryDto {
            id: r.id,
            name: r.name,
            url: r.url,
            enabled: r.enabled,
        })
        .collect();

    Ok(Json(items))
}

async fn add_repository(
    State(state): State<AppState>,
    Json(req): Json<AddRepositoryRequest>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::new_v4().to_string();

    rustyfin_db::repo_package::add_repository(&state.db, &id, &req.name, &req.url)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn delete_repository(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_package::delete_repository(&state.db, &repo_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn install_package(
    State(state): State<AppState>,
    Path(package_id): Path<String>,
    Json(req): Json<InstallPackageRequest>,
) -> Result<StatusCode, ApiError> {
    let id = if package_id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        package_id
    };

    rustyfin_db::repo_package::install_package(
        &state.db,
        &id,
        &req.name,
        &req.version,
        None,
        &req.description,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn uninstall_package(
    State(state): State<AppState>,
    Path(package_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_package::uninstall_package(&state.db, &package_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn enable_package(
    State(state): State<AppState>,
    Path(package_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_package::toggle_package_enabled(&state.db, &package_id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn disable_package(
    State(state): State<AppState>,
    Path(package_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_package::toggle_package_enabled(&state.db, &package_id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn restore_backup(
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    // Backup restore would read from the backups table and restore the database
    // For now, just acknowledge
    tracing::info!("Backup restore requested");
    Ok(StatusCode::OK)
}
