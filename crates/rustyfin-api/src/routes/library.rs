use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use rustyfin_model::library::{CreateLibraryRequest, Library, LibraryOptions};
use rustyfin_model::ws_event::WsEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path as StdPath;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::websocket::broadcast_event;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Library/VirtualFolders", get(get_libraries))
        .route("/Library/VirtualFolders", post(create_library))
        .route("/Library/VirtualFolders/{id}", delete(remove_library))
        .route("/Library/VirtualFolders/{id}/Refresh", post(scan_library))
        .route(
            "/Library/VirtualFolders/Paths",
            delete(remove_library_paths),
        )
        .route("/Library/Refresh", post(refresh_all))
        .route(
            "/Library/{id}/Options",
            get(get_library_options).post(update_library_options),
        )
        .route("/Users/{userId}/Views", get(get_user_views))
        .route("/UserViews", get(get_user_views_no_user))
        .route("/Library/ModifiedVolumes", get(get_modified_volumes))
        .route("/Library/BrowseDirectories", get(browse_directories))
}

pub(crate) fn libraries_to_views(libs: &[Library]) -> Value {
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

    // Broadcast scan started event
    {
        let state_clone = state.clone();
        let lib_name = lib.name.clone();
        let lib_id = lib.id;
        tokio::spawn(async move {
            broadcast_event(
                &state_clone,
                WsEvent::ScheduledTaskStarted {
                    data: json!({
                        "Name": format!("Library scan: {}", lib_name),
                        "State": "Running",
                        "ItemId": lib_id.to_string(),
                    }),
                },
            )
            .await;
        });
    }

    // Get API keys from config
    let config = state.config.read().await;
    let tmdb_key = config.tmdb_api_key.clone();
    let omdb_key = config.omdb_api_key.clone();
    let fanart_key = config.fanart_api_key.clone();
    drop(config);

    let count = rustyfin_core::scanner::scan_library_with_metadata(
        &state.db,
        &lib,
        &state.http_client,
        tmdb_key.as_deref(),
        omdb_key.as_deref(),
        fanart_key.as_deref(),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Broadcast scan completed event
    {
        let state_clone = state.clone();
        let lib_name = lib.name.clone();
        let lib_id = lib.id;
        let scanned_count = count;
        tokio::spawn(async move {
            broadcast_event(
                &state_clone,
                WsEvent::ScheduledTaskCompleted {
                    data: json!({
                        "Name": format!("Library scan: {}", lib_name),
                        "State": "Completed",
                        "ItemId": lib_id.to_string(),
                        "ItemsScanned": scanned_count,
                    }),
                },
            )
            .await;
            broadcast_event(
                &state_clone,
                WsEvent::LibraryChanged {
                    data: json!({
                        "LibraryId": lib_id.to_string(),
                        "LibraryName": lib_name,
                    }),
                },
            )
            .await;
        });
    }

    // Invalidate caches after scan
    state.cache.invalidate_on_scan();
    state.image_cache.invalidate_all();
    Ok(Json(json!({ "items_scanned": count })))
}

async fn refresh_all(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    tracing::info!("refresh_all: found {} libraries to scan", libs.len());

    // Get API keys from config
    let config = state.config.read().await;
    let tmdb_key = config.tmdb_api_key.clone();
    let omdb_key = config.omdb_api_key.clone();
    let fanart_key = config.fanart_api_key.clone();
    drop(config);

    // Broadcast scan started event
    {
        let state_clone = state.clone();
        tokio::spawn(async move {
            broadcast_event(
                &state_clone,
                WsEvent::ScheduledTaskStarted {
                    data: json!({
                        "Name": "Scan all libraries",
                        "State": "Running",
                    }),
                },
            )
            .await;
        });
    }

    let mut total = 0u32;
    for lib in &libs {
        tracing::info!(
            "refresh_all: scanning library '{}' (id={})",
            lib.name,
            lib.id
        );

        // Broadcast individual library scan started
        {
            let state_clone = state.clone();
            let lib_name = lib.name.clone();
            let lib_id = lib.id;
            tokio::spawn(async move {
                broadcast_event(
                    &state_clone,
                    WsEvent::ScheduledTaskStarted {
                        data: json!({
                            "Name": format!("Library scan: {}", lib_name),
                            "State": "Running",
                            "ItemId": lib_id.to_string(),
                        }),
                    },
                )
                .await;
            });
        }

        match rustyfin_core::scanner::scan_library_with_metadata(
            &state.db,
            lib,
            &state.http_client,
            tmdb_key.as_deref(),
            omdb_key.as_deref(),
            fanart_key.as_deref(),
        )
        .await
        {
            Ok(count) => {
                tracing::info!(
                    "refresh_all: library '{}' scanned, {} items",
                    lib.name,
                    count
                );
                total += count;

                // Broadcast individual library scan completed
                {
                    let state_clone = state.clone();
                    let lib_name = lib.name.clone();
                    let lib_id = lib.id;
                    let scanned_count = count;
                    tokio::spawn(async move {
                        broadcast_event(
                            &state_clone,
                            WsEvent::ScheduledTaskCompleted {
                                data: json!({
                                    "Name": format!("Library scan: {}", lib_name),
                                    "State": "Completed",
                                    "ItemId": lib_id.to_string(),
                                    "ItemsScanned": scanned_count,
                                }),
                            },
                        )
                        .await;
                        broadcast_event(
                            &state_clone,
                            WsEvent::LibraryChanged {
                                data: json!({
                                    "LibraryId": lib_id.to_string(),
                                    "LibraryName": lib_name,
                                }),
                            },
                        )
                        .await;
                    });
                }
            }
            Err(e) => {
                tracing::error!("failed to scan library '{}': {}", lib.name, e);
                let state_clone = state.clone();
                let lib_name = lib.name.clone();
                let lib_id = lib.id;
                let error_msg = e.to_string();
                tokio::spawn(async move {
                    broadcast_event(
                        &state_clone,
                        WsEvent::ScheduledTaskCompleted {
                            data: json!({
                                "Name": format!("Library scan: {}", lib_name),
                                "State": "Failed",
                                "ItemId": lib_id.to_string(),
                                "Error": error_msg,
                            }),
                        },
                    )
                    .await;
                });
            }
        }
    }

    // Broadcast all libraries scan completed
    {
        let state_clone = state.clone();
        let total_scanned = total;
        tokio::spawn(async move {
            broadcast_event(
                &state_clone,
                WsEvent::ScheduledTaskCompleted {
                    data: json!({
                        "Name": "Scan all libraries",
                        "State": "Completed",
                        "ItemsScanned": total_scanned,
                    }),
                },
            )
            .await;
        });
    }

    // Invalidate caches after scan
    state.cache.invalidate_on_scan();
    state.image_cache.invalidate_all();
    Ok(Json(json!({ "items_scanned": total })))
}

/// GET /Library/{id}/Options — Get library options.
async fn get_library_options(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<LibraryOptions>, ApiError> {
    let key = format!("library_options_{}", id);
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM configuration WHERE key = ?1")
            .bind(&key)
            .fetch_optional(&state.db_ro)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    match value {
        Some(json) => {
            let options: LibraryOptions =
                serde_json::from_str(&json).map_err(|e| ApiError::Internal(e.to_string()))?;
            Ok(Json(options))
        }
        None => Ok(Json(LibraryOptions::default())),
    }
}

/// POST /Library/{id}/Options — Update library options.
async fn update_library_options(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(options): Json<LibraryOptions>,
) -> Result<Json<Value>, ApiError> {
    let key = format!("library_options_{}", id);
    let json = serde_json::to_string(&options).map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query(
        "INSERT INTO configuration (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(&key)
    .bind(&json)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({ "status": "ok" })))
}

/// DELETE /Library/VirtualFolders/Paths - Remove library paths
async fn remove_library_paths(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Value>, ApiError> {
    // Extract libraryId and paths from the request body
    let library_id = body
        .get("LibraryId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("LibraryId is required".to_string()))?;
    let paths = body
        .get("Paths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
        .unwrap_or_default();

    let lib_id = Uuid::parse_str(library_id)
        .map_err(|e| ApiError::BadRequest(format!("Invalid LibraryId: {}", e)))?;

    // Get the current library
    let lib = rustyfin_db::repo_library::get_library_by_id(&state.db_ro, lib_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("Library not found".to_string()))?;

    // Remove the specified paths
    let new_paths: Vec<String> = lib
        .paths
        .into_iter()
        .filter(|p| !paths.contains(&p.as_str()))
        .collect();

    // Update the library
    rustyfin_db::repo_library::update_library_paths(&state.db, lib_id, &new_paths)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({})))
}

/// GET /Library/ModifiedVolumes - Get modified volumes
async fn get_modified_volumes(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    // Check all library paths for modified files
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut modified = Vec::new();
    for lib in &libs {
        for path in &lib.paths {
            if let Ok(metadata) = std::fs::metadata(path)
                && let Ok(modified_time) = metadata.modified()
            {
                let db_time = rustyfin_db::repo_media::get_last_scan_time(&state.db_ro, lib.id)
                    .await
                    .ok()
                    .flatten();

                // Convert DateTime<Utc> to SystemTime for comparison
                let db_system_time = db_time.as_ref().map(|dt| {
                    let ms: i64 = dt.timestamp_millis();
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64)
                });

                // If the directory was modified after the last scan, mark it as modified
                if let Some(last_scan) = db_system_time {
                    let is_modified = modified_time > last_scan;
                    modified.push(serde_json::json!({
                        "LibraryId": lib.id.to_string(),
                        "Path": path,
                        "IsModified": is_modified
                    }));
                } else {
                    // No scan history, assume modified
                    modified.push(serde_json::json!({
                        "LibraryId": lib.id.to_string(),
                        "Path": path,
                        "IsModified": true
                    }));
                }
            }
        }
    }

    Ok(Json(json!(modified)))
}

/// Directory browsing info returned to the client
#[derive(Serialize)]
struct DirectoryEntry {
    path: String,
    name: String,
    can_read: bool,
}

#[derive(Deserialize)]
struct BrowseQuery {
    /// Parent path to list; defaults to "/" if not provided
    parent: Option<String>,
}

/// GET /Library/BrowseDirectories?parent=/path
/// Lists subdirectories of the given parent path so the client can browse
/// the server's filesystem when adding a library.
async fn browse_directories(
    State(_state): State<AppState>,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<Vec<DirectoryEntry>>, ApiError> {
    let parent_path = query.parent.unwrap_or_else(|| "/".to_string());

    // Security: only allow paths under common media locations
    let allowed_roots = [
        "/", "/media", "/share", "/srv", "/mnt", "/home", "/data", "/videos",
    ];
    let is_allowed = allowed_roots
        .iter()
        .any(|root| parent_path == *root || parent_path.starts_with(&format!("{}/", root)));
    if !is_allowed {
        return Ok(Json(Vec::new()));
    }

    let path = StdPath::new(&parent_path);
    let mut entries = Vec::new();

    if let Ok(read_dir) = std::fs::read_dir(path) {
        for entry in read_dir.filter_map(|e| e.ok()) {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let full_path = entry.path();
                let name = entry.file_name().into_string().unwrap_or_default();
                let full_path_str = full_path.to_string_lossy().to_string();
                let can_read = std::fs::read_dir(&full_path).is_ok();
                entries.push(DirectoryEntry {
                    path: full_path_str,
                    name,
                    can_read,
                });
            }
        }
    }

    // Sort: name ascending
    entries.sort_by_key(|a| a.name.to_lowercase());

    Ok(Json(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_libraries_to_views_empty() {
        let result = libraries_to_views(&[]);
        assert_eq!(result["Items"].as_array().unwrap().len(), 0);
        assert_eq!(result["TotalRecordCount"], 0);
        assert_eq!(result["StartIndex"], 0);
    }

    #[test]
    fn test_libraries_to_views_single_movie_library() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "Movies".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Movies,
            paths: vec!["/media/movies".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["Name"], "Movies");
        assert_eq!(items[0]["Type"], "CollectionFolder");
        assert_eq!(items[0]["CollectionType"], "movies");
        assert_eq!(items[0]["IsFolder"], true);
        assert_eq!(result["TotalRecordCount"], 1);
    }

    #[test]
    fn test_libraries_to_views_tv_shows_library() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "TV Shows".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::TvShows,
            paths: vec!["/media/tv".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items[0]["CollectionType"], "tvshows");
    }

    #[test]
    fn test_libraries_to_views_music_library() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "Music".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Music,
            paths: vec!["/media/music".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items[0]["CollectionType"], "music");
    }

    #[test]
    fn test_libraries_to_views_photos_library() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "Photos".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Photos,
            paths: vec!["/media/photos".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items[0]["CollectionType"], "homevideos");
    }

    #[test]
    fn test_libraries_to_views_mixed_library() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "Mixed".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Mixed,
            paths: vec!["/media/mixed".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items[0]["CollectionType"], "mixed");
    }

    #[test]
    fn test_libraries_to_views_multiple_libraries() {
        let libs = vec![
            Library {
                id: Uuid::new_v4(),
                name: "Movies".to_string(),
                content_type: rustyfin_model::library::LibraryContentType::Movies,
                paths: vec!["/media/movies".to_string()],
            },
            Library {
                id: Uuid::new_v4(),
                name: "TV Shows".to_string(),
                content_type: rustyfin_model::library::LibraryContentType::TvShows,
                paths: vec!["/media/tv".to_string()],
            },
        ];
        let result = libraries_to_views(&libs);
        let items = result["Items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(result["TotalRecordCount"], 2);
    }

    #[test]
    fn test_libraries_to_views_user_data_defaults() {
        let libs = vec![Library {
            id: Uuid::new_v4(),
            name: "Test".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Movies,
            paths: vec!["/media/test".to_string()],
        }];
        let result = libraries_to_views(&libs);
        let user_data = &result["Items"][0]["UserData"];
        assert_eq!(user_data["PlaybackPositionTicks"], 0);
        assert_eq!(user_data["PlayCount"], 0);
        assert_eq!(user_data["IsFavorite"], false);
        assert_eq!(user_data["Played"], false);
    }

}
