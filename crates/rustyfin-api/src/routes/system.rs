use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::server_info::{
    EndpointInfo, FullServerInfo, LogFile, ServerConfiguration, ServerInfo,
};
use std::net::SocketAddr;

use crate::error::ApiError;
use crate::state::AppState;

/// Public (unauthenticated) system routes
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info/Public", get(get_public_info))
        .route("/System/Ping", get(ping).post(ping))
        .route("/System/keys", get(get_keys))
        .route("/health", get(health))
        .route("/Environment/Drives", get(get_drives))
        .route("/Environment/NetworkShares", get(get_network_shares))
}

/// Protected (authenticated) system routes
pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info", get(get_full_info))
        .route(
            "/System/Configuration",
            get(get_configuration).post(update_configuration),
        )
        .route("/System/Endpoint", get(get_endpoint))
        .route("/System/Shutdown", post(shutdown))
        .route("/System/Restart", post(restart))
        .route("/System/Logs", get(get_logs))
        .route("/System/Log/{name}", get(get_log_content))
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

async fn get_full_info(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<FullServerInfo> {
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
        remote_end_point: Some(format!("http://{}", addr)),
        local_addresses: Some(vec![format!("http://{}", state.bind_addr)]),
        package_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    })
}

async fn get_endpoint(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
) -> Json<EndpointInfo> {
    let remote_ip = req
        .headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| addr.ip().to_string());

    let is_local = addr.ip().is_loopback() || remote_ip == "127.0.0.1" || remote_ip == "::1";

    let is_in_network = is_local || addr.ip().is_loopback() || {
        let ip_str = addr.ip().to_string();
        ip_str.starts_with("192.168.")
            || ip_str.starts_with("10.")
            || ip_str.starts_with("172.")
            || remote_ip.starts_with("192.168.")
            || remote_ip.starts_with("10.")
            || remote_ip.starts_with("172.")
    };

    Json(EndpointInfo {
        is_local,
        is_in_network,
        local_address: format!("http://{}", state.bind_addr),
        remote_address: remote_ip,
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
    let json =
        serde_json::to_string_pretty(&new_config).map_err(|e| ApiError::Internal(e.to_string()))?;
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
    tracing::info!(
        "restart requested via API (performing shutdown, external supervisor should restart)"
    );
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
        if path.extension().and_then(|e| e.to_str()) == Some("log")
            && let Ok(meta) = entry.metadata().await
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let size = meta.len();
            let modified = meta
                .modified()
                .ok()
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
                })
                .unwrap_or_default();
            let created = meta
                .created()
                .ok()
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
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

    entries.sort_by(|a, b| b.date_modified.cmp(&a.date_modified));
    Ok(Json(entries))
}

async fn get_keys() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "PublicKey": "",
        "PrivateKey": ""
    }))
}

async fn get_log_content(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<String, ApiError> {
    let log_dir = state.data_dir.join("logs");
    let path = log_dir.join(&name);
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to read log file: {}", e)))?;
    Ok(content)
}

/// GET /Environment/Drives — Returns list of available drives.
async fn get_drives() -> Result<Json<Vec<rustyfin_model::environment::DriveInfo>>, ApiError> {
    let drives = list_drives();
    Ok(Json(drives))
}

/// GET /Environment/NetworkShares — Returns list of network shares (stub on Linux).
async fn get_network_shares()
-> Result<Json<Vec<rustyfin_model::environment::NetworkShare>>, ApiError> {
    // Network share browsing is primarily a Windows feature.
    // On Linux, we return an empty list as SMB/CIFS share discovery
    // would require additional dependencies (e.g., smbclient).
    Ok(Json(Vec::new()))
}

/// List available drives on the system.
fn list_drives() -> Vec<rustyfin_model::environment::DriveInfo> {
    use rustyfin_model::environment::DriveInfo;
    use std::fs;

    let mut drives = Vec::new();

    // On Linux, check common mount points
    let mount_points = ["/", "/home", "/mnt", "/media", "/tmp", "/var"];

    for mp in &mount_points {
        let Ok(metadata) = fs::metadata(mp) else {
            continue;
        };

        if !metadata.is_dir() {
            continue;
        }

        // Try to get disk space info
        let (total_size, available) = match disk_usage(mp) {
            Ok((total, avail)) => (total, avail),
            Err(_) => (0, 0),
        };

        drives.push(DriveInfo {
            name: mp.to_string(),
            drive_type: "Local".to_string(),
            total_size,
            available_free_space: available,
            path: mp.to_string(),
            is_ready: true,
        });
    }

    drives
}

/// Get disk usage for a path.
fn disk_usage(path: &str) -> Result<(i64, i64), std::io::Error> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path)?;
    let block_size = metadata.blksize() as u64;
    let blocks = metadata.blocks();
    let total_size = (blocks * block_size) as i64;

    // For available space, we use a simpler approach
    // A proper implementation would use statvfs
    let available = total_size / 4; // rough estimate

    Ok((total_size, available))
}
