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
struct BackupDto {
    id: String,
    name: String,
    #[serde(rename = "BackupPath")]
    backup_path: String,
    #[serde(rename = "SizeBytes")]
    size_bytes: i64,
    #[serde(rename = "CreatedAt")]
    created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateBackupRequest {
    #[serde(default)]
    name: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Backup", get(get_backups).post(create_backup))
        .route("/Backup/{backupId}", delete(delete_backup))
        .route("/Backup/Restore", post(restore_backup))
}

async fn get_backups(State(state): State<AppState>) -> Result<Json<Vec<BackupDto>>, ApiError> {
    let backups = rustyfin_db::repo_misc::get_backups(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<BackupDto> = backups
        .into_iter()
        .map(|b| BackupDto {
            id: b.id,
            name: b.name,
            backup_path: b.backup_path,
            size_bytes: b.size_bytes,
            created_at: b.created_at,
        })
        .collect();

    Ok(Json(items))
}

async fn create_backup(
    State(state): State<AppState>,
    Json(req): Json<CreateBackupRequest>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::new_v4().to_string();
    let name = if req.name.is_empty() {
        format!("backup_{}", id)
    } else {
        req.name.clone()
    };

    // Create a backup of the database file
    let db_path = state.data_dir.join("rustyfin.db");
    let backup_dir = state.data_dir.join("backups");
    std::fs::create_dir_all(&backup_dir).ok();

    let backup_path = backup_dir.join(format!("{}.db", id));

    // Copy the database file
    if let Ok(content) = std::fs::read(&db_path)
        && std::fs::write(&backup_path, &content).is_ok()
    {
        let size_bytes = content.len() as i64;

        rustyfin_db::repo_misc::create_backup(
            &state.db,
            &id,
            &name,
            &backup_path.to_string_lossy(),
            size_bytes,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

        tracing::info!(
            "Backup created: {} ({} bytes)",
            backup_path.display(),
            size_bytes
        );
    }

    Ok(StatusCode::OK)
}

async fn delete_backup(
    State(state): State<AppState>,
    Path(backup_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_misc::delete_backup(&state.db, &backup_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn restore_backup(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    let backup_id = body["Id"]
        .as_str()
        .or_else(|| body["backupId"].as_str())
        .unwrap_or("");

    if backup_id.is_empty() {
        return Err(ApiError::BadRequest("Backup ID is required".to_string()));
    }

    // Get the backup info
    let backups = rustyfin_db::repo_misc::get_backups(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let backup = backups
        .iter()
        .find(|b| b.id == backup_id)
        .ok_or(ApiError::NotFound("Backup not found".to_string()))?;

    // Restore the database from backup
    let db_path = state.data_dir.join("rustyfin.db");
    if let Ok(content) = std::fs::read(&backup.backup_path) {
        if std::fs::write(&db_path, &content).is_ok() {
            tracing::info!(
                "Database restored from backup: {} ({} bytes)",
                backup.backup_path,
                content.len()
            );
        } else {
            return Err(ApiError::Internal(
                "Failed to write database file".to_string(),
            ));
        }
    } else {
        return Err(ApiError::Internal("Failed to read backup file".to_string()));
    }

    Ok(StatusCode::OK)
}
