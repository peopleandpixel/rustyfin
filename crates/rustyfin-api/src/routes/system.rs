use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::server_info::{
    EndpointInfo, FullServerInfo, LogFile, ServerConfiguration, ServerInfo,
};
use serde::Deserialize;
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
        .route(
            "/System/Configuration/{key}",
            get(get_configuration_by_key).post(update_configuration_by_key),
        )
        .route(
            "/System/Configuration/Branding",
            post(update_branding_config),
        )
        .route("/System/Info/Storage", get(get_storage_info))
        .route("/System/Logs/Log", get(get_logs_log))
        .route("/System/Endpoint", get(get_endpoint))
        .route("/System/Shutdown", post(shutdown))
        .route("/System/Restart", post(restart))
        .route("/System/Logs", get(get_logs))
        .route("/System/Log/{name}", get(get_log_content))
        .route("/System/LogLevel", get(get_log_level).post(set_log_level))
        .route("/System/UpdatePackages", get(get_update_packages))
        .route(
            "/System/RestartScheduledTasks",
            post(restart_scheduled_tasks),
        )
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

/// GET /System/LogLevel — Returns the current live log verbosity level.
async fn get_log_level(State(state): State<AppState>) -> Json<serde_json::Value> {
    let level = state.log_level_tx.borrow().clone();
    Json(serde_json::json!({ "LogLevel": level }))
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct LogLevelRequest {
    #[serde(alias = "level")]
    LogLevel: String,
}

/// POST /System/LogLevel — Updates the live log verbosity level (trace, debug, info, warn, error).
async fn set_log_level(
    State(state): State<AppState>,
    Json(req): Json<LogLevelRequest>,
) -> Result<StatusCode, ApiError> {
    let level = req.LogLevel.to_lowercase();
    match level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => {
            let level_clone = level.clone();
            state
                .log_level_tx
                .send(level)
                .map_err(|e| ApiError::Internal(format!("Failed to update log level: {}", e)))?;
            tracing::info!(level = ?level_clone, "log level updated");
            Ok(StatusCode::NO_CONTENT)
        }
        _ => Err(ApiError::BadRequest(format!(
            "Invalid log level: {}. Must be one of: trace, debug, info, warn, error",
            level
        ))),
    }
}

/// GET /Environment/Drives — Returns list of available drives.
async fn get_drives() -> Result<Json<Vec<rustyfin_model::environment::DriveInfo>>, ApiError> {
    let drives = list_drives();
    Ok(Json(drives))
}

/// GET /Environment/NetworkShares — Returns list of network shares detected on Linux.
async fn get_network_shares()
-> Result<Json<Vec<rustyfin_model::environment::NetworkShare>>, ApiError> {
    let shares = detect_network_shares();
    Ok(Json(shares))
}

/// Detect network shares by parsing /etc/fstab and current mounts.
fn detect_network_shares() -> Vec<rustyfin_model::environment::NetworkShare> {
    use rustyfin_model::environment::NetworkShare;
    use std::collections::HashMap;

    let mut shares = Vec::new();

    // Parse /etc/fstab for network share entries
    // Format: device mount_point fs_type options dump pass
    let mut fstab_shares: HashMap<String, (String, String, String)> = HashMap::new();
    if let Ok(content) = std::fs::read_to_string("/etc/fstab") {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }

            // fstab format: device(0) mount_point(1) fs_type(2) options(3) dump(4) pass(5)
            let fs_type = parts[2].to_lowercase();
            let mount_point = parts[1].to_string();

            // Detect network filesystem types
            if fs_type == "cifs" || fs_type == "smbfs" || fs_type == "nfs" || fs_type == "nfs4" {
                let (server, share_name) = parse_server_share(parts[0]);
                let share_type = if fs_type == "cifs" || fs_type == "smbfs" {
                    "CIFS".to_string()
                } else {
                    "NFS".to_string()
                };

                fstab_shares.insert(mount_point.clone(), (server, share_name, share_type));
            }
        }
    }

    // Parse current mounts for active network shares
    if let Ok(content) = std::fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }

            let fs_type = parts[2].to_lowercase();
            let mount_point = parts[1].to_string();

            if fs_type == "cifs" || fs_type == "smbfs" || fs_type == "nfs" || fs_type == "nfs4" {
                let (server, share_name) = parse_server_share(parts[0]);
                let share_type = if fs_type == "cifs" || fs_type == "smbfs" {
                    "CIFS".to_string()
                } else {
                    "NFS".to_string()
                };

                // Check if we already have this from fstab
                if let Some((server, share_name, share_type)) = fstab_shares.remove(&mount_point) {
                    shares.push(NetworkShare {
                        name: share_name.clone(),
                        path: mount_point.clone(),
                        server: server.clone(),
                        share_type: share_type.clone(),
                    });
                } else {
                    shares.push(NetworkShare {
                        name: share_name,
                        path: mount_point,
                        server,
                        share_type,
                    });
                }
            }
        }
    }

    // Add any fstab entries that aren't currently mounted
    for (mount_point, (server, share_name, share_type)) in fstab_shares {
        shares.push(NetworkShare {
            name: share_name,
            path: mount_point,
            server,
            share_type,
        });
    }

    shares
}

/// Parse a server/share string from fstab or mount output.
/// Formats: "//server/share", "/server/share", "server:/share", "server:/path/to/share"
fn parse_server_share(source: &str) -> (String, String) {
    // CIFS format: //server/share
    if let Some(rest) = source.strip_prefix("//") {
        if let Some(pos) = rest.find('/') {
            let server = &rest[..pos];
            let share = &rest[pos + 1..];
            return (server.to_string(), share.to_string());
        }
        return (rest.to_string(), "/".to_string());
    }

    // NFS format: server:/share
    if let Some(pos) = source.find(':') {
        let server = &source[..pos];
        let share = &source[pos + 1..];
        return (server.to_string(), share.to_string());
    }

    // Fallback
    (source.to_string(), "/".to_string())
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
#[cfg(target_family = "unix")]
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

/// Fallback for non-Unix platforms (Windows, etc.)
#[cfg(not(target_family = "unix"))]
fn disk_usage(_path: &str) -> Result<(i64, i64), std::io::Error> {
    // Return zero values as a fallback; a proper implementation would use
    // platform-specific APIs (e.g., GetDiskFreeSpaceEx on Windows)
    Ok((0, 0))
}

/// GET /System/Configuration/{key} - Get configuration by key
async fn get_configuration_by_key(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let config = state.config.read().await;
    let json = serde_json::to_value(&*config).map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(
        json.get(&key).cloned().unwrap_or(serde_json::json!({})),
    ))
}

/// POST /System/Configuration/{key} - Update configuration by key
async fn update_configuration_by_key(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    // Update the in-memory config by merging the body into the specified key
    let mut config = state.config.write().await;
    let json = serde_json::to_value(&*config).map_err(|e| ApiError::Internal(e.to_string()))?;

    // Merge the body into the config at the specified key path
    let merged = merge_json_values(json, &key, body);

    // Parse back into ServerConfiguration
    let new_config: ServerConfiguration =
        serde_json::from_value(merged).map_err(|e| ApiError::Internal(e.to_string()))?;

    *config = new_config;
    drop(config);

    // Persist to disk
    let config_path = state.data_dir.join("config.json");
    let config = state.config.read().await;
    let json =
        serde_json::to_string_pretty(&*config).map_err(|e| ApiError::Internal(e.to_string()))?;
    drop(config);
    tokio::fs::write(&config_path, &json)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    tracing::info!("configuration key updated: {}", key);
    Ok(StatusCode::NO_CONTENT)
}

/// Merge a JSON value into a parent at the specified key path
fn merge_json_values(
    parent: serde_json::Value,
    key: &str,
    child: serde_json::Value,
) -> serde_json::Value {
    match parent {
        serde_json::Value::Object(mut map) => {
            if let Some(val) = map.get_mut(key) {
                *val = child;
            } else {
                map.insert(key.to_string(), child);
            }
            serde_json::Value::Object(map)
        }
        other => other,
    }
}

/// POST /System/Configuration/Branding - Update branding configuration
async fn update_branding_config(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    // Extract branding settings and store in cache
    if let Some(enabled) = body.get("SplashscreenEnabled").and_then(|v| v.as_bool()) {
        state
            .cache
            .branding
            .insert("splashscreen_enabled".to_string(), enabled.to_string());
    }
    if let Some(css) = body.get("CustomCss").and_then(|v| v.as_str()) {
        state
            .cache
            .branding
            .insert("custom_css".to_string(), css.to_string());
    }
    if let Some(disclaimer) = body.get("LoginDisclaimer").and_then(|v| v.as_str()) {
        state
            .cache
            .branding
            .insert("login_disclaimer".to_string(), disclaimer.to_string());
    }

    tracing::info!("branding configuration updated");
    Ok(StatusCode::NO_CONTENT)
}

/// GET /System/Info/Storage - Get storage information
async fn get_storage_info(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let log_path = state.data_dir.join("logs").to_string_lossy().to_string();
    let cache_path = state.data_dir.join("cache").to_string_lossy().to_string();
    let program_data_path = state.data_dir.to_string_lossy().to_string();
    Ok(Json(serde_json::json!({
        "LogPath": log_path,
        "CachePath": cache_path,
        "ProgramDataPath": program_data_path,
        "InternalMemoryPaths": [],
        "InternalUsbPaths": []
    })))
}

/// GET /System/Logs/Log - Download a log file
async fn get_logs_log(State(state): State<AppState>) -> Result<Json<Vec<LogFile>>, ApiError> {
    get_logs(State(state)).await
}

/// GET /System/UpdatePackages - Get available updates
async fn get_update_packages(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!([])))
}

/// POST /System/RestartScheduledTasks - Restart scheduled tasks
async fn restart_scheduled_tasks(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    tracing::info!("scheduled tasks restart requested");
    // Restart the task scheduler
    state.task_scheduler.restart().await;
    Ok(StatusCode::NO_CONTENT)
}
