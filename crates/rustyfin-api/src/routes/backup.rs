//! Jellyfin 10.11's backup API (`BackupController`), administrators only. See `backup_archive`
//! for what a backup holds. Rustyfin adds `GET /Backup/Download?path=` and `DELETE /Backup/{name}`.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::backup_archive as archive;
use crate::error::ApiError;
use crate::query::Query;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Backup", get(get_backups).post(create_backup))
        .route("/Backup/Create", post(create_backup))
        .route("/Backup/Manifest", get(get_manifest))
        .route("/Backup/Download", get(download_backup))
        .route("/Backup/Restore", post(restore_backup))
        .route("/Backup/{name}", delete(delete_backup))
}

/// GET /Backup - every backup's `BackupManifestDto`, newest first
async fn get_backups(State(state): State<AppState>) -> Json<Vec<Value>> {
    Json(archive::list(&state.data_dir).await)
}

/// POST /Backup/Create - Jellyfin's `BackupOptionsDto` body (optional) is accepted; the database
/// and settings are always backed up, the images, trickplay tiles and subtitles never, and the
/// manifest says so. Answers the new backup's manifest.
async fn create_backup(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let manifest = archive::create(&state.db, &state.data_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("backup failed: {e}")))?;
    tracing::info!(
        "backup created: {}",
        manifest["Path"].as_str().unwrap_or("")
    );
    Ok(Json(manifest))
}

fn named_backup(
    state: &AppState,
    params: &[(String, String)],
) -> Result<std::path::PathBuf, ApiError> {
    let name = crate::query::first_value(params, "path")
        .ok_or_else(|| ApiError::BadRequest("path is required".to_string()))?;
    archive::backup_path(&state.data_dir, name)
        .ok_or_else(|| ApiError::NotFound("Backup not found".to_string()))
}

/// GET /Backup/Manifest?path= - one backup's manifest
async fn get_manifest(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let path = named_backup(&state, &params)?;
    archive::manifest(&path)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Backup not found".to_string()))
}

/// GET /Backup/Download?path= - the backup file, as an attachment
async fn download_backup(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let path = named_backup(&state, &params)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((
        [
            (header::CONTENT_TYPE, "application/vnd.sqlite3".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

/// DELETE /Backup/{name} - removes a backup file: 204, or 404
async fn delete_backup(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let path = archive::backup_path(&state.data_dir, &name)
        .ok_or_else(|| ApiError::NotFound("Backup not found".to_string()))?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Jellyfin's `BackupRestoreRequestDto`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RestoreRequest {
    #[serde(default)]
    archive_file_name: String,
}

/// POST /Backup/Restore - stages the backup for a restore at the next start (204). Refused (409)
/// when the database is not the data directory's (`RUSTYFIN_DB` is set).
async fn restore_backup(
    State(state): State<AppState>,
    Json(body): Json<RestoreRequest>,
) -> Result<StatusCode, ApiError> {
    if body.archive_file_name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "ArchiveFileName is required".to_string(),
        ));
    }
    let path = archive::backup_path(&state.data_dir, body.archive_file_name.trim())
        .ok_or_else(|| ApiError::NotFound("Backup not found".to_string()))?;
    if archive::manifest(&path).await.is_none() {
        return Err(ApiError::BadRequest("not a Rustyfin backup".to_string()));
    }
    if std::env::var_os("RUSTYFIN_DB").is_some() {
        return Err(ApiError::Conflict(
            "the database is set by RUSTYFIN_DB, outside the data directory; restore it by hand"
                .to_string(),
        ));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    archive::stage_restore(&state.data_dir, &name)
        .await
        .map_err(ApiError::Internal)?;
    tracing::info!("restore of {name} staged for the next start");
    Ok(StatusCode::NO_CONTENT)
}
