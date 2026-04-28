use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::server_info::{
    FullServerInfo, LogFile, ServerConfiguration, ServerInfo,
};

use crate::error::ApiError;
use crate::state::AppState;

/// Public (unauthenticated) system routes
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info/Public", get(get_public_info))
        .route("/System/Ping", get(ping).post(ping))
        .route("/health", get(health))
}

/// Protected (authenticated) system routes
pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info", get(get_full_info))
        .route("/System/Configuration", get(get_configuration).post(update_configuration))
        .route("/System/Shutdown", post(shutdown))
        .route("/System/Restart", post(restart))
        .route("/System/Logs", get(get_logs))
}

async fn health() -> &'static str {
    "ok"
}

async fn ping() -> &'static str {
    "\"Rustyfin\""
}

async fn get_public_info(State(state): State<AppState>) -> Json<ServerInfo> {
    let config = state.config.read().await;
    Json(ServerInfo {
        server_name: config.server_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        id: state.server_id.to_string(),
        startup_wizard_completed: config.is_startup_wizard_completed,
    })
}

async fn get_full_info(State(state): State<AppState>) -> Json<FullServerInfo> {
    let config = state.config.read().await;
    let log_path = state.data_dir.join("logs").to_string_lossy().to_string();
    let cache_path = state.data_dir.join("cache").to_string_lossy().to_string();
    let program_data_path = state.data_dir.to_string_lossy().to_string();

    let os = std::env::consts::OS.to_string();

    Json(FullServerInfo {
        server_name: config.server_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        id: state.server_id.to_string(),
        startup_wizard_completed: config.is_startup_wizard_completed,
        operating_system: os,
        has_pending_restart: false,
        has_update_available: false,
        log_path,
        cache_path,
        can_self_restart: true,
        can_launch_web_browser: false,
        program_data_path,
        web_socket_port_number: config.public_port,
        local_address: format!("http://{}", state.bind_addr),
    })
}

async fn get_configuration(State(state): State<AppState>) -> Json<ServerConfiguration> {
    let config = state.config.read().await;
    Json(config.clone())
}

async fn update_configuration(
    State(state): State<AppState>,
    Json(new_config): Json<ServerConfiguration>,
) -> Result<StatusCode, ApiError> {
    // Persist to disk
    let config_path = state.data_dir.join("config.json");
    let json = serde_json::to_string_pretty(&new_config)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    tokio::fs::write(&config_path, &json)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Update in-memory
    let mut config = state.config.write().await;
    *config = new_config;

    Ok(StatusCode::NO_CONTENT)
}

async fn shutdown(State(state): State<AppState>) -> StatusCode {
    tracing::info!("shutdown requested via API");
    let _ = state.shutdown_tx.send(true);
    StatusCode::NO_CONTENT
}

async fn restart(State(state): State<AppState>) -> StatusCode {
    tracing::info!("restart requested via API (performing shutdown, external supervisor should restart)");
    let _ = state.shutdown_tx.send(true);
    StatusCode::NO_CONTENT
}

async fn get_logs(State(state): State<AppState>) -> Result<Json<Vec<LogFile>>, ApiError> {
    let log_dir = state.data_dir.join("logs");
    let mut entries = Vec::new();

    // If log directory doesn't exist, return empty list
    let read_dir = match tokio::fs::read_dir(&log_dir).await {
        Ok(rd) => rd,
        Err(_) => return Ok(Json(entries)),
    };

    let mut rd = read_dir;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("log") {
            if let Ok(meta) = entry.metadata().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = meta.len();
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        Some(dt.to_rfc3339())
                    })
                    .unwrap_or_default();
                let created = meta
                    .created()
                    .ok()
                    .and_then(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        Some(dt.to_rfc3339())
                    })
                    .unwrap_or_else(|| modified.clone());

                entries.push(LogFile {
                    date_created: created,
                    date_modified: modified,
                    name,
                    size,
                });
            }
        }
    }

    entries.sort_by(|a, b| b.date_modified.cmp(&a.date_modified));
    Ok(Json(entries))
}
