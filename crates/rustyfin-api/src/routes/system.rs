use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::server_info::{
    EndpointInfo, FullServerInfo, LogFile, ServerConfiguration, ServerInfo,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::ApiError;
use crate::state::AppState;

/// Public (unauthenticated) system routes
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info/Public", get(get_public_info))
        .route("/System/Ping", get(ping).post(ping))
        .route("/System/keys", get(get_keys))
        .route("/health", get(health))
}

/// Setup-or-admin routes. `routes::build_router` wraps them in `require_setup_or_admin`.
pub fn setup_routes() -> Router<AppState> {
    Router::new()
        .route("/Environment/Drives", get(get_drives))
        .route("/Environment/NetworkShares", get(get_network_shares))
        .route(
            "/Environment/DirectoryContents",
            get(get_directory_contents),
        )
        .route("/Environment/ParentPath", get(get_parent_path))
}

/// Protected (authenticated) system routes
pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Info", get(get_full_info))
        .route("/System/Configuration", get(get_configuration))
        .route("/System/Configuration/{key}", get(get_configuration_by_key))
        .route("/System/Endpoint", get(get_endpoint))
}

/// Administrator-only system routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/System/Configuration", post(update_configuration))
        .route(
            "/System/Configuration/{key}",
            post(update_configuration_by_key),
        )
        .route(
            "/System/Configuration/Branding",
            post(update_branding_config),
        )
        .route("/System/Info/Storage", get(get_storage_info))
        .route("/System/Logs/Log", get(get_logs_log))
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

async fn get_public_info(State(state): State<AppState>) -> Result<Json<ServerInfo>, ApiError> {
    let startup_wizard_completed = rustyfin_db::repo_user::users_exist(&state.db_ro).await?;
    let config = state.config.read().await;
    Ok(Json(ServerInfo {
        server_name: config.server_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        id: state.server_id.to_string(),
        startup_wizard_completed,
    }))
}

async fn get_full_info(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<FullServerInfo>, ApiError> {
    let startup_wizard_completed = rustyfin_db::repo_user::users_exist(&state.db_ro).await?;
    let config = state.config.read().await;
    let log_path = state.data_dir.join("logs").to_string_lossy().to_string();
    let cache_path = state.data_dir.join("cache").to_string_lossy().to_string();
    let program_data_path = state.data_dir.to_string_lossy().to_string();

    let os = std::env::consts::OS.to_string();

    Ok(Json(FullServerInfo {
        server_name: config.server_name.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        id: state.server_id.to_string(),
        startup_wizard_completed,
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
    }))
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

/// Clears the stored secrets for callers who are not administrators.
fn hide_secrets(config: &mut ServerConfiguration) {
    config.tmdb_api_key = None;
    config.omdb_api_key = None;
    config.fanart_api_key = None;
    config.smtp_password = None;
}

async fn get_configuration(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<ServerConfiguration> {
    let mut config = state.config.read().await.clone();
    if !claims.is_admin {
        hide_secrets(&mut config);
    }
    Json(config)
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
    state
        .task_scheduler
        .changes()
        .set_delay(std::time::Duration::from_secs(u64::from(
            config.library_monitor_delay,
        )));
    state
        .task_scheduler
        .watcher()
        .set_poll_interval(std::time::Duration::from_secs(u64::from(
            config.library_monitor_poll_interval.max(1),
        )));

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
            && let Ok(name) = entry.file_name().into_string()
            && listed_log_path(&log_dir, &name).await.is_some()
        {
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

/// Returns the canonical path of `<log_dir>/<name>` when `GET /System/Logs` lists `name`:
/// a bare `.log` file that is a regular file (not a symlink) whose canonical parent is the
/// canonical log directory. Otherwise `None`.
async fn listed_log_path(log_dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    let bare = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
    let file = log_dir.join(name);
    if !bare || file.extension().and_then(|e| e.to_str()) != Some("log") {
        return None;
    }
    let meta = tokio::fs::symlink_metadata(&file).await.ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let dir = tokio::fs::canonicalize(log_dir).await.ok()?;
    let path = tokio::fs::canonicalize(&file).await.ok()?;
    (path.parent() == Some(dir.as_path())).then_some(path)
}

async fn get_log_content(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<String, ApiError> {
    let log_dir = state.data_dir.join("logs");
    let path = listed_log_path(&log_dir, &name)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Log file not found: {name}")))?;
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

/// Query for GET /Environment/DirectoryContents.
#[derive(Deserialize)]
struct DirectoryContentsQuery {
    path: String,
    #[serde(rename = "includeFiles")]
    include_files: Option<bool>,
    #[serde(rename = "includeDirectories")]
    include_directories: Option<bool>,
    prefix: Option<String>,
}

/// One entry in a DirectoryContents answer.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct FileSystemEntryInfo {
    name: String,
    path: String,
    #[serde(rename = "Type")]
    entry_type: &'static str,
}

/// Cap on the entries one DirectoryContents answer returns, after sorting and filtering.
const MAX_DIRECTORY_ENTRIES: usize = 1000;

/// Query for GET /Environment/ParentPath.
#[derive(Deserialize)]
struct ParentPathQuery {
    path: String,
}

/// How long a folder listing may run before the answer is 504. The read keeps its thread and its
/// permit, so a hung mount can't hold more than `DirectoryListings::MAX` threads.
const DIRECTORY_LISTING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Normalises an absolute path lexically: drops `.` and empty components (`//`) and trailing
/// slashes, and lets `..` pop the previous component (never above `/`). Other paths are returned as given.
fn normalise_absolute_path(p: &str) -> String {
    if !p.starts_with('/') {
        return p.to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

/// GET /Environment/DirectoryContents - lists the folders (and optionally files) directly inside a server path.
/// Defaults match Jellyfin: folders are off unless `includeDirectories=true`, files off unless
/// `includeFiles=true`. Hidden names need a `prefix` starting with '.', and the prefix match ignores case.
/// Names that aren't UTF-8 are skipped. Paths are normalised first (`.`, `..`, `//`, trailing `/`).
/// At most 1,000 entries are returned, in name order (case-insensitive, then exact).
/// Missing or non-folder paths answer 404; relative or unreadable paths answer 400.
/// When `DirectoryListings::MAX` listings already run, the answer is 503 at once. A listing that
/// takes longer than 10 s answers 504.
async fn get_directory_contents(
    State(state): State<AppState>,
    Query(q): Query<DirectoryContentsQuery>,
) -> Result<Json<Vec<FileSystemEntryInfo>>, ApiError> {
    let dir = normalise_absolute_path(&q.path);
    let permit = state
        .directory_listings
        .0
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::ServiceUnavailable(
                "The server is busy reading other folders; try again.".into(),
            )
        })?;
    let include_files = q.include_files.unwrap_or(false);
    let include_directories = q.include_directories.unwrap_or(false);
    let prefix = q.prefix.unwrap_or_default();
    let show_hidden = prefix.starts_with('.');
    let prefix_lower = prefix.to_lowercase();
    let work_dir = dir.clone();
    let work = tokio::task::spawn_blocking(move || {
        // The permit lives until this thread finishes, even if the caller has timed out.
        let _permit = permit;
        match crate::routes::library::path_problem_blocking(&work_dir) {
            None => {}
            Some((true, sentence)) => return Err(ApiError::NotFound(sentence)),
            Some((false, sentence)) => return Err(ApiError::BadRequest(sentence)),
        }
        let Ok(read) = std::fs::read_dir(&work_dir) else {
            return Ok(Vec::new());
        };
        // The largest kept key sits on top, so each new smaller key can replace it.
        let mut kept: std::collections::BinaryHeap<((String, String), bool)> =
            std::collections::BinaryHeap::new();
        for entry in read.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let is_dir = if file_type.is_symlink() {
                match std::fs::metadata(entry.path()) {
                    Ok(meta) => meta.is_dir(),
                    Err(_) => continue,
                }
            } else {
                file_type.is_dir()
            };
            let wanted = if is_dir {
                include_directories
            } else {
                include_files
            };
            if !wanted {
                continue;
            }
            if name.starts_with('.') && !show_hidden {
                continue;
            }
            if !name.to_lowercase().starts_with(&prefix_lower) {
                continue;
            }
            let key = (name.to_lowercase(), name);
            if kept.len() < MAX_DIRECTORY_ENTRIES {
                kept.push((key, is_dir));
            } else if kept.peek().is_some_and(|(max, _)| key < *max) {
                kept.pop();
                kept.push((key, is_dir));
            }
        }
        Ok(kept
            .into_sorted_vec()
            .into_iter()
            .map(|((_, name), is_dir)| FileSystemEntryInfo {
                path: std::path::Path::new(&work_dir)
                    .join(&name)
                    .to_string_lossy()
                    .into_owned(),
                name,
                entry_type: if is_dir { "Directory" } else { "File" },
            })
            .collect())
    });
    let entries = match tokio::time::timeout(DIRECTORY_LISTING_TIMEOUT, work).await {
        Err(_elapsed) => {
            return Err(ApiError::GatewayTimeout(format!(
                "The server took too long to read the folder \"{dir}\"."
            )));
        }
        Ok(joined) => joined.map_err(|e| ApiError::Internal(e.to_string()))??,
    };
    Ok(Json(entries))
}

/// GET /Environment/ParentPath - the parent of a server path, from the text alone (no disk access).
/// Relative paths answer 400; the root answers 204 with no parent.
async fn get_parent_path(Query(q): Query<ParentPathQuery>) -> Result<Response, ApiError> {
    if !q.path.starts_with('/') {
        return Err(ApiError::BadRequest(format!(
            "The folder \"{}\" is not a full path on the server.",
            q.path
        )));
    }
    let trimmed = q.path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // `trimmed` starts with '/', so the last '/' always exists.
    let cut = trimmed.rfind('/').unwrap_or(0);
    let parent = if cut == 0 { "/" } else { &trimmed[..cut] };
    Ok((
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        parent.to_string(),
    )
        .into_response())
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
            drive_type: "Directory".to_string(),
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
    Extension(claims): Extension<Claims>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut config = state.config.read().await.clone();
    if !claims.is_admin {
        hide_secrets(&mut config);
    }
    let json = serde_json::to_value(&config).map_err(|e| ApiError::Internal(e.to_string()))?;
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
        serde_json::from_value(merged).map_err(|e| ApiError::BadRequest(format!("{key}: {e}")))?;

    *config = new_config;
    state
        .task_scheduler
        .changes()
        .set_delay(std::time::Duration::from_secs(u64::from(
            config.library_monitor_delay,
        )));
    state
        .task_scheduler
        .watcher()
        .set_poll_interval(std::time::Duration::from_secs(u64::from(
            config.library_monitor_poll_interval.max(1),
        )));
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
