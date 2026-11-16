use crate::query::Query;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_core::scan_progress::ScanTicket;
use rustyfin_model::auth::Claims;
use rustyfin_model::library::{
    CreateLibraryRequest, Library, LibraryOptions, LibraryOptionsUpdate,
};
use rustyfin_model::ws_event::WsEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, ErrorKind};
use std::path::Path as StdPath;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::websocket::broadcast_event;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users/{userId}/Views", get(get_user_views))
        .route("/UserViews", get(get_user_views_no_user))
        .route("/Library/ModifiedVolumes", get(get_modified_volumes))
}

/// Setup-or-admin library routes. `routes::build_router` wraps them in `require_setup_or_admin`.
pub fn setup_routes() -> Router<AppState> {
    Router::new()
        .route("/Library/BrowseDirectories", get(browse_directories))
        .route("/Library/VirtualFolders", get(get_libraries))
        .route("/Library/VirtualFolders", post(create_library))
        .route("/Environment/ValidatePath", post(validate_path))
        .route("/Library/VirtualFolders/{id}", delete(remove_library))
        .route(
            "/Library/VirtualFolders/Paths",
            post(add_media_path).delete(remove_library_paths),
        )
        .route(
            "/Library/VirtualFolders/Paths/Update",
            post(update_media_path),
        )
        .route("/Library/VirtualFolders/Name", post(rename_virtual_folder))
        .route(
            "/Library/{id}/Options",
            get(get_library_options).post(update_library_options),
        )
        .route(
            "/Library/VirtualFolders/LibraryOptions",
            post(update_virtual_folder_options),
        )
}

/// Administrator-only library refresh routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Library/VirtualFolders/{id}/Refresh", post(scan_library))
        .route("/Library/Refresh", post(refresh_all))
        .route("/Library/MediaFolders", get(get_media_folders))
        .route("/Library/PhysicalPaths", get(get_physical_paths))
}

/// GET /Library/MediaFolders - the libraries as Jellyfin's collection folders. `isHidden=true`
/// gives none: no library is hidden.
async fn get_media_folders(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let libs = if crate::query::bool_param(&params, "isHidden")? == Some(true) {
        Vec::new()
    } else {
        rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?
    };
    Ok(Json(libraries_to_views(&libs, &state.server_id)))
}

/// GET /Library/PhysicalPaths - every library folder path
async fn get_physical_paths(State(state): State<AppState>) -> Result<Json<Vec<String>>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    Ok(Json(libs.into_iter().flat_map(|l| l.paths).collect()))
}

/// A library as Jellyfin's `CollectionFolder` item: in `/UserViews`, `GET /Items/{library id}` and
/// an item's ancestors.
pub(crate) fn library_view(lib: &Library, server_id: &str) -> Value {
    json!({
        "Id": lib.id,
        "Name": lib.name,
        "ServerId": server_id,
        "Type": "CollectionFolder",
        "CollectionType": lib.content_type.jellyfin_collection_type(),
        "MediaType": "Unknown",
        "IsFolder": true,
        "ParentId": Value::Null,
        "Path": lib.paths.first().cloned().unwrap_or_default(),
        "LocationType": "FileSystem",
        "CanDelete": false,
        "CanDownload": false,
        "PlayAccess": "Full",
        "DisplayPreferencesId": lib.id.simple().to_string(),
        "ChildCount": 0,
        "ImageTags": {},
        "BackdropImageTags": [],
        "ImageBlurHashes": {},
        "PrimaryImageAspectRatio": 1.7777777777777777,
        "ProviderIds": {},
        "ExternalUrls": [],
        "Genres": [],
        "Tags": [],
        "LockedFields": [],
        "LockData": false,
        "UserData": {
            "PlaybackPositionTicks": 0,
            "PlayCount": 0,
            "IsFavorite": false,
            "Played": false,
            "Key": lib.id.to_string(),
            "ItemId": lib.id.simple().to_string(),
        },
    })
}

pub(crate) fn libraries_to_views(libs: &[Library], server_id: &str) -> Value {
    let items: Vec<Value> = libs
        .iter()
        .map(|lib| library_view(lib, server_id))
        .collect();
    let count = items.len() as i64;
    json!({
        "Items": items,
        "TotalRecordCount": count,
        "StartIndex": 0,
    })
}

/// Jellyfin's `includeHidden`: also list the libraries the user hid from "My Media".
#[derive(Deserialize, Default)]
struct UserViewsQuery {
    #[serde(default, rename = "includeHidden", alias = "IncludeHidden")]
    include_hidden: bool,
}

async fn get_user_views(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<Uuid>,
    Query(q): Query<UserViewsQuery>,
) -> Result<Json<Value>, ApiError> {
    // An administrator may look at another user's views; anyone else gets their own.
    let user = if claims.is_admin { user_id } else { claims.sub };
    Ok(Json(views_for(&state, user, q.include_hidden).await?))
}

async fn get_user_views_no_user(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<UserViewsQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(views_for(&state, claims.sub, q.include_hidden).await?))
}

/// Orders and hides `views` by the user's configuration, as Jellyfin's `UserViewManager`: those in
/// `MyMediaExcludes` are left out unless `include_hidden`, and the rest are listed in
/// `OrderedViews` order, the libraries it doesn't name after them in their own order.
fn arrange_views(views: &mut Value, config: &Value, include_hidden: bool) {
    let ids = |key: &str| -> Vec<String> {
        config[key]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| id.replace('-', "").to_lowercase())
                    .collect()
            })
            .unwrap_or_default()
    };
    let key = |view: &Value| {
        view["Id"]
            .as_str()
            .unwrap_or("")
            .replace('-', "")
            .to_lowercase()
    };
    let (hidden, order) = (ids("MyMediaExcludes"), ids("OrderedViews"));
    let Some(items) = views.get_mut("Items").and_then(Value::as_array_mut) else {
        return;
    };
    if !include_hidden {
        items.retain(|view| !hidden.contains(&key(view)));
    }
    // A stable sort keeps the libraries `OrderedViews` doesn't name in their own order.
    items.sort_by_key(|view| {
        order
            .iter()
            .position(|id| *id == key(view))
            .unwrap_or(usize::MAX)
    });
    let count = items.len();
    views["TotalRecordCount"] = count.into();
}

/// Every library as a view (cached for all users), less those `user`'s library access leaves out,
/// arranged by the user's `OrderedViews` and `MyMediaExcludes`.
async fn views_for(state: &AppState, user: Uuid, include_hidden: bool) -> Result<Value, ApiError> {
    let mut views = match state.cache.library_views.get("views") {
        Some(cached) => cached,
        None => {
            let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
            let views = libraries_to_views(&libs, &state.server_id);
            state
                .cache
                .library_views
                .insert("views".to_string(), views.clone());
            views
        }
    };
    let policy = crate::parental::user_policy(state, user).await?;
    if policy.enabled_folders().is_some()
        && let Some(items) = views.get_mut("Items").and_then(Value::as_array_mut)
    {
        items.retain(|v| v["Id"].as_str().is_some_and(|id| policy.allows_folder(id)));
        let count = items.len();
        views["TotalRecordCount"] = count.into();
    }
    let config = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user)
        .await?
        .map(|user| rustyfin_model::user::jellyfin_configuration(&user.configuration))
        .unwrap_or_default();
    arrange_views(&mut views, &config, include_hidden);
    Ok(views)
}

/// A library in `GET /Library/VirtualFolders`, with its saved options. Rustyfin's `Id`,
/// `ContentType` and `Paths` come with Jellyfin's `VirtualFolderInfo` names for them.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct LibraryWithOptions {
    #[serde(flatten)]
    library: Library,
    library_options: LibraryOptions,
    item_id: Uuid,
    collection_type: Option<&'static str>,
    locations: Vec<String>,
    refresh_status: &'static str,
}

impl LibraryWithOptions {
    fn new(library: Library, library_options: LibraryOptions) -> Self {
        Self {
            item_id: library.id,
            // Jellyfin gives a mixed library no collection type.
            collection_type: Some(library.content_type.jellyfin_collection_type())
                .filter(|t| *t != "mixed"),
            locations: library.paths.clone(),
            refresh_status: "Idle",
            library,
            library_options,
        }
    }
}

async fn get_libraries(
    State(state): State<AppState>,
) -> Result<Json<Vec<LibraryWithOptions>>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    let mut items = Vec::with_capacity(libs.len());
    for library in libs {
        let library_options =
            rustyfin_db::repo_library::get_library_options(&state.db_ro, library.id).await?;
        items.push(LibraryWithOptions::new(library, library_options));
    }
    Ok(Json(items))
}

/// Checks one library folder path. Returns the sentence for a path the server cannot use as a
/// folder, with `true` when the path is absolute but not an existing folder (not found, or a file).
pub(crate) async fn path_problem(p: &str) -> Option<(bool, String)> {
    let owned = p.to_string();
    match tokio::task::spawn_blocking(move || path_problem_blocking(&owned)).await {
        Ok(problem) => problem,
        Err(_) => Some((false, format!("The server cannot read the folder \"{p}\"."))),
    }
}

/// The blocking check behind `path_problem`; call it from a blocking thread.
pub(crate) fn path_problem_blocking(p: &str) -> Option<(bool, String)> {
    if !StdPath::new(p).is_absolute() {
        return Some((
            false,
            format!("The folder \"{p}\" is not a full path on the server."),
        ));
    }
    match std::fs::metadata(p) {
        Ok(meta) if !meta.is_dir() => Some((
            true,
            format!("The path \"{p}\" on the server is not a folder."),
        )),
        Ok(_) => match std::fs::read_dir(p) {
            Ok(_) => None,
            Err(e) => Some((false, unreadable_sentence(p, &e))),
        },
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Some((
            true,
            format!("The folder \"{p}\" was not found on the server."),
        )),
        Err(e) => Some((false, unreadable_sentence(p, &e))),
    }
}

/// Checks each library folder path, in request order. Returns one sentence for
/// each path the server cannot use as a folder; an empty list means all are usable.
pub(crate) async fn library_path_problems(paths: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    for p in paths {
        problems.extend(path_problem(p).await.map(|(_, sentence)| sentence));
    }
    problems
}

/// The sentence for a folder the server cannot read, given the error it hit.
fn unreadable_sentence(p: &str, e: &io::Error) -> String {
    if e.kind() == ErrorKind::PermissionDenied {
        format!("The server does not have permission to read the folder \"{p}\".")
    } else {
        format!("The server cannot read the folder \"{p}\": {e}.")
    }
}

async fn create_library(
    State(state): State<AppState>,
    Json(req): Json<CreateLibraryRequest>,
) -> Result<Json<Library>, ApiError> {
    let problems = library_path_problems(&req.paths).await;
    if !problems.is_empty() {
        return Err(ApiError::BadRequest(problems.join(" ")));
    }
    let lib = Library {
        id: Uuid::new_v4(),
        name: req.name,
        content_type: req.content_type,
        paths: req.paths,
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib).await?;
    state.cache.invalidate_library_lists();
    crate::routes::streaming::forget_library_roots(&state);
    if let Some(options) = &req.library_options {
        rustyfin_db::repo_library::set_library_options(&state.db, lib.id, options).await?;
    }
    state.task_scheduler.watcher().libraries_changed();
    Ok(Json(lib))
}

/// The body of POST /Environment/ValidatePath. Jellyfin's ValidateWritable and IsFile are ignored.
#[derive(Deserialize)]
struct ValidatePathRequest {
    #[serde(rename = "Path")]
    path: String,
}

/// POST /Environment/ValidatePath - checks one folder path with the same sentences as create; stores nothing.
/// An absolute path that is not an existing folder answers 404; other problems answer 400.
async fn validate_path(Json(req): Json<ValidatePathRequest>) -> Result<StatusCode, ApiError> {
    match path_problem(&req.path).await {
        None => Ok(StatusCode::NO_CONTENT),
        Some((true, sentence)) => Err(ApiError::NotFound(sentence)),
        Some((false, sentence)) => Err(ApiError::BadRequest(sentence)),
    }
}

async fn remove_library(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let covers = rustyfin_core::local_images::embedded_cover_paths(&state.db, id).await;
    rustyfin_db::repo_library::delete_library(&state.db, id).await?;
    rustyfin_core::local_images::remove_orphaned_covers(&state.db, &covers).await;
    state.cache.invalidate_library_lists();
    crate::routes::streaming::forget_library_roots(&state);
    state.task_scheduler.watcher().libraries_changed();
    Ok(Json(json!({ "status": "ok" })))
}

pub(crate) async fn scan_library(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let lib = rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("library not found".into()))?;

    // The registry lists the scan from here until it ends, its probes included.
    let ticket = state.task_scheduler.scans().queue(lib.id);
    // Claimed here, not when the scan starts, so that a Scan all reaching this library skips it.
    // A run another caller has claimed is scanned by that caller, so this request answers at once.
    if !ticket.claim_announced() {
        // The scheduler claims a run without sending its Started or Completed. The first request
        // that joins such a run sends them, and its Completed comes when the run ends.
        if ticket.announce() {
            broadcast_event(
                &state,
                WsEvent::ScheduledTaskStarted {
                    data: json!({
                        "Name": format!("Library scan: {}", lib.name),
                        "State": "Running",
                        "ItemId": lib.id.to_string(),
                    }),
                },
            )
            .await;
            let state = state.clone();
            let lib = lib.clone();
            tokio::spawn(ticket.wait_end_then(move || send_scan_completed(&state, &lib, 0)));
        }
        // Otherwise this request drops its ticket here, and does not wait for the run.
        return Ok(Json(json!({ "items_scanned": 0 })));
    }

    let count = run_claimed_scan(&state, &lib, &ticket).await?;

    // Completed goes out once the scan has ended, which is after its probes. The registry sends it
    // before the run's waiters wake, so a Scan all waiting on this run completes after it.
    let state_end = state.clone();
    let lib_end = lib.clone();
    tokio::spawn(ticket.wait_end_then(move || send_scan_completed(&state_end, &lib_end, count)));
    Ok(Json(json!({ "items_scanned": count })))
}

/// Sends the Completed event for a library scan that has ended, with its item count (0 when the
/// scan failed). The caller sends it once the run ends, after `ticket.wait_end`.
pub(crate) fn send_scan_completed(state: &AppState, lib: &Library, count: u32) {
    // A send fails only when no socket is listening, which is fine.
    let _ = state.ws_broadcast.send(WsEvent::ScheduledTaskCompleted {
        data: json!({
            "Name": format!("Library scan: {}", lib.name),
            "State": "Completed",
            "ItemId": lib.id.to_string(),
            "ItemsScanned": count,
        }),
    });
}

/// Runs a library scan that the caller has claimed on `ticket` (B119 shares it with the wizard's
/// import job): the Started event, the scan with the server's metadata keys, the cache clears and
/// LibraryChanged. The caller sends Completed once the run ends, as the Refresh route does. Returns
/// the scan's item count; a failed scan sends no LibraryChanged.
pub(crate) async fn run_claimed_scan(
    state: &AppState,
    lib: &Library,
    ticket: &ScanTicket,
) -> Result<u32, ApiError> {
    // Broadcast scan started event
    broadcast_event(
        state,
        WsEvent::ScheduledTaskStarted {
            data: json!({
                "Name": format!("Library scan: {}", lib.name),
                "State": "Running",
                "ItemId": lib.id.to_string(),
            }),
        },
    )
    .await;

    let count = rustyfin_core::scanner::scan_library_with_metadata(
        &state.db,
        lib,
        &state.http_client,
        &state.plugin_manager,
        ticket,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Invalidate caches after scan
    state.cache.invalidate_on_scan();
    state.image_cache.invalidate_all();
    broadcast_event(
        state,
        WsEvent::LibraryChanged {
            data: json!({
                "LibraryId": lib.id.to_string(),
                "LibraryName": lib.name,
            }),
        },
    )
    .await;
    Ok(count)
}

async fn refresh_all(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db_ro).await?;
    tracing::info!("refresh_all: found {} libraries to scan", libs.len());

    let plugin_manager = state.plugin_manager.clone();

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

    // Every library is listed as queued before the first one starts.
    let tickets: Vec<_> = libs
        .iter()
        .map(|lib| state.task_scheduler.scans().queue(lib.id))
        .collect();
    let mut total = 0u32;
    // Libraries another caller claimed. Their runs end on their own, and Scan all completes after them.
    let mut others = Vec::new();
    // The Completed events of the libraries this request scans.
    let mut ends = Vec::new();
    for (lib, ticket) in libs.iter().zip(tickets) {
        // Only the caller that claimed a library's run scans it, so a claimed library is skipped.
        // This request sends the library's Started and Completed, so the claim announces the run.
        if !ticket.claim_announced() {
            others.push(ticket);
            continue;
        }
        tracing::info!(
            "refresh_all: scanning library '{}' (id={})",
            lib.name,
            lib.id
        );

        // Broadcast individual library scan started
        broadcast_event(
            &state,
            WsEvent::ScheduledTaskStarted {
                data: json!({
                    "Name": format!("Library scan: {}", lib.name),
                    "State": "Running",
                    "ItemId": lib.id.to_string(),
                }),
            },
        )
        .await;

        let result = rustyfin_core::scanner::scan_library_with_metadata(
            &state.db,
            lib,
            &state.http_client,
            &plugin_manager,
            &ticket,
        )
        .await;
        // Caches are cleared after each library's scan, before its LibraryChanged, as scan_library does.
        state.cache.invalidate_on_scan();
        state.image_cache.invalidate_all();
        match result {
            Ok(count) => {
                tracing::info!(
                    "refresh_all: library '{}' scanned, {} items",
                    lib.name,
                    count
                );
                total += count;

                broadcast_event(
                    &state,
                    WsEvent::LibraryChanged {
                        data: json!({
                            "LibraryId": lib.id.to_string(),
                            "LibraryName": lib.name,
                        }),
                    },
                )
                .await;

                // Completed goes out once this library's scan has ended, after its probes. The registry
                // sends it before the run's waiters wake, so the overall Completed comes after it.
                let ws_broadcast = state.ws_broadcast.clone();
                let lib_name = lib.name.clone();
                let lib_id = lib.id;
                let scanned_count = count;
                ends.push(tokio::spawn(ticket.wait_end_then(move || {
                    let _ = ws_broadcast.send(WsEvent::ScheduledTaskCompleted {
                        data: json!({
                            "Name": format!("Library scan: {}", lib_name),
                            "State": "Completed",
                            "ItemId": lib_id.to_string(),
                            "ItemsScanned": scanned_count,
                        }),
                    });
                })));
            }
            Err(e) => {
                tracing::error!("failed to scan library '{}': {}", lib.name, e);
                // Failed goes out once this library's run has ended, as Completed does above.
                // The registry sends it before the run's waiters wake, so the overall Completed
                // comes after it.
                let ws_broadcast = state.ws_broadcast.clone();
                let lib_name = lib.name.clone();
                let lib_id = lib.id;
                let error_msg = e.to_string();
                ends.push(tokio::spawn(ticket.wait_end_then(move || {
                    let _ = ws_broadcast.send(WsEvent::ScheduledTaskCompleted {
                        data: json!({
                            "Name": format!("Library scan: {}", lib_name),
                            "State": "Failed",
                            "ItemId": lib_id.to_string(),
                            "Error": error_msg,
                        }),
                    });
                })));
            }
        }
    }

    // Broadcast all libraries scan completed, once every library's run has ended
    {
        let state_clone = state.clone();
        let total_scanned = total;
        tokio::spawn(async move {
            for end in ends {
                let _ = end.await;
            }
            for ticket in others {
                ticket.wait_end().await;
            }
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

    Ok(Json(json!({ "items_scanned": total })))
}

/// GET /Library/{id}/Options — Get library options.
async fn get_library_options(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<LibraryOptions>, ApiError> {
    // An unreadable row reads as the defaults (and is logged), so it can be saved over
    Ok(Json(stored_options(&state, id).await))
}

/// POST /Library/{id}/Options — Update library options.
async fn update_library_options(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(update): Json<LibraryOptionsUpdate>,
) -> Result<Json<Value>, ApiError> {
    let options = update.resolve(&stored_options(&state, id).await);
    rustyfin_db::repo_library::set_library_options(&state.db, id, &options).await?;
    state.task_scheduler.watcher().libraries_changed();
    Ok(Json(json!({ "status": "ok" })))
}

/// Jellyfin's `UpdateLibraryOptionsDto`: the library and its new options.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateLibraryOptionsBody {
    id: Uuid,
    #[serde(default)]
    library_options: Option<LibraryOptionsUpdate>,
}

/// The options stored for a library, against which a save that sends a setting under both its
/// names is read. A row that can't be read counts as the defaults, since the save replaces it.
async fn stored_options(state: &AppState, id: Uuid) -> LibraryOptions {
    match rustyfin_db::repo_library::get_library_options(&state.db, id).await {
        Ok(options) => options,
        Err(e) => {
            tracing::warn!(library = %id, error = %e, "stored library options unreadable; replacing them");
            LibraryOptions::default()
        }
    }
}

/// POST /Library/VirtualFolders/LibraryOptions — Jellyfin's route for saving a library's options:
/// `{Id, LibraryOptions}` replaces them and answers 204. An unknown library is 404.
async fn update_virtual_folder_options(
    State(state): State<AppState>,
    Json(body): Json<UpdateLibraryOptionsBody>,
) -> Result<StatusCode, ApiError> {
    if rustyfin_db::repo_library::get_library_by_id(&state.db, body.id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound(format!(
            "library '{}' not found",
            body.id
        )));
    }
    let options = match body.library_options {
        Some(update) => update.resolve(&stored_options(&state, body.id).await),
        None => LibraryOptions::default(),
    };
    rustyfin_db::repo_library::set_library_options(&state.db, body.id, &options).await?;
    state.task_scheduler.watcher().libraries_changed();
    Ok(StatusCode::NO_CONTENT)
}

/// Whether two library names are the same without case, in any script (not only ASCII).
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Whether two folder paths name the same folder: a trailing `/` makes no difference.
fn same_folder(a: &str, b: &str) -> bool {
    let trim = |p: &str| match p.trim_end_matches('/') {
        "" if p.starts_with('/') => "/".to_string(),
        t => t.to_string(),
    };
    trim(a) == trim(b)
}

/// The library named `name`, as Jellyfin finds a virtual folder by name: exact, else without case.
async fn library_named(state: &AppState, name: &str) -> Result<Library, ApiError> {
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db).await?;
    let exact = libs.iter().position(|l| l.name == name);
    let index = exact.or_else(|| libs.iter().position(|l| same_name(&l.name, name)));
    index
        .map(|i| libs[i].clone())
        .ok_or_else(|| ApiError::NotFound(format!("The library \"{name}\" does not exist.")))
}

/// Saves a library's new folders, and scans it in the background when `refresh` asks to.
async fn store_paths(
    state: &AppState,
    lib: &Library,
    paths: &[String],
    refresh: bool,
) -> Result<(), ApiError> {
    rustyfin_db::repo_library::update_library_paths(&state.db, lib.id, paths).await?;
    state.cache.invalidate_library_lists();
    crate::routes::streaming::forget_library_roots(state);
    state.task_scheduler.watcher().libraries_changed();
    if refresh {
        scan_in_background(state, lib.id);
    }
    Ok(())
}

/// Starts a scan of the library `id` that the request does not wait for (`refreshLibrary=true`).
fn scan_in_background(state: &AppState, id: Uuid) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = scan_library(State(state), Path(id)).await {
            tracing::warn!(library = %id, error = ?e, "library scan after a library change failed");
        }
    });
}

/// Jellyfin's `MediaPathInfo`.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct MediaPathInfo {
    #[serde(default)]
    path: Option<String>,
}

/// Jellyfin's `AddMediaPathDto` and `UpdateMediaPathRequestDto`: the library's name and a folder,
/// in `Path` or `PathInfo.Path`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MediaPathBody {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    path_info: Option<MediaPathInfo>,
}

impl MediaPathBody {
    fn folder(&self) -> Result<String, ApiError> {
        self.path
            .clone()
            .or_else(|| self.path_info.as_ref().and_then(|p| p.path.clone()))
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| ApiError::BadRequest("Path or PathInfo.Path is required".into()))
    }
}

#[derive(Deserialize, Default)]
struct RefreshQuery {
    #[serde(default, rename = "refreshLibrary", alias = "RefreshLibrary")]
    refresh_library: bool,
}

/// POST /Library/VirtualFolders/Paths - adds a folder to the library `Name`. Answers 204; an
/// unknown library is 404, a folder the server cannot use or one the library has already is 400.
async fn add_media_path(
    State(state): State<AppState>,
    Query(q): Query<RefreshQuery>,
    Json(body): Json<MediaPathBody>,
) -> Result<StatusCode, ApiError> {
    let folder = body.folder()?;
    let lib = library_named(&state, &body.name).await?;
    if let Some((_, sentence)) = path_problem(&folder).await {
        return Err(ApiError::BadRequest(sentence));
    }
    if lib.paths.iter().any(|p| same_folder(p, &folder)) {
        return Err(ApiError::BadRequest(format!(
            "The folder \"{folder}\" is already in the library."
        )));
    }
    let mut paths = lib.paths.clone();
    paths.push(folder);
    store_paths(&state, &lib, &paths, q.refresh_library).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Library/VirtualFolders/Paths/Update - Jellyfin's update of one folder's `PathInfo`, found
/// by its path. Rustyfin keeps no other folder settings, so it checks the folder and answers 204.
/// An unknown library is 404, a folder the library does not have is 400.
async fn update_media_path(
    State(state): State<AppState>,
    Json(body): Json<MediaPathBody>,
) -> Result<StatusCode, ApiError> {
    let folder = body.folder()?;
    let lib = library_named(&state, &body.name).await?;
    if !lib.paths.iter().any(|p| same_folder(p, &folder)) {
        return Err(ApiError::BadRequest(format!(
            "The library has no folder \"{folder}\"."
        )));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The query of DELETE /Library/VirtualFolders/Paths.
#[derive(Deserialize, Default)]
struct RemovePathQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(default, alias = "Path")]
    path: Option<String>,
    #[serde(default, rename = "refreshLibrary", alias = "RefreshLibrary")]
    refresh_library: bool,
}

/// Saves the library without the folders `removed`. A library keeps at least one folder: taking
/// its last is 400.
async fn remove_folders(
    state: &AppState,
    lib: &Library,
    removed: &[&str],
    refresh: bool,
) -> Result<(), ApiError> {
    let kept: Vec<String> = lib
        .paths
        .iter()
        .filter(|p| !removed.iter().any(|r| same_folder(p, r)))
        .cloned()
        .collect();
    if kept.is_empty() {
        return Err(ApiError::BadRequest(
            "A library needs at least one folder; delete the library instead.".into(),
        ));
    }
    store_paths(state, lib, &kept, refresh).await?;
    // Their items leave the library now: a scan prunes only under the folders it still has
    for folder in lib.paths.iter().filter(|p| !kept.contains(p)) {
        let gone =
            rustyfin_core::scan_sync::remove_items_under(&state.db, lib.id, folder, &kept).await?;
        tracing::info!(library = %lib.id, folder, gone, "removed the items of a folder taken out of the library");
    }
    state.cache.invalidate_library_lists();
    Ok(())
}

/// DELETE /Library/VirtualFolders/Paths?name=&path= - Jellyfin's removal of one folder from a
/// library: 204, 404 for an unknown library or a folder it does not have, 400 for its last
/// folder. Rustyfin's earlier body (`{LibraryId, Paths}`) still works and answers `{}`.
async fn remove_library_paths(
    State(state): State<AppState>,
    Query(q): Query<RemovePathQuery>,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    if let (Some(name), Some(path)) = (&q.name, &q.path) {
        let lib = library_named(&state, name).await?;
        if !lib.paths.iter().any(|p| same_folder(p, path)) {
            return Err(ApiError::NotFound(format!(
                "The library has no folder \"{path}\"."
            )));
        }
        remove_folders(&state, &lib, &[path.as_str()], q.refresh_library).await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let body: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("name and path are required".into()))?;
    let library_id = body
        .get("LibraryId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("name and path are required".into()))?;
    let lib_id = Uuid::parse_str(library_id)
        .map_err(|e| ApiError::BadRequest(format!("Invalid LibraryId: {}", e)))?;
    let paths = body
        .get("Paths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
        .unwrap_or_default();
    let lib = rustyfin_db::repo_library::get_library_by_id(&state.db, lib_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Library not found".to_string()))?;
    remove_folders(&state, &lib, &paths, false).await?;
    Ok(Json(json!({})).into_response())
}

/// The query of POST /Library/VirtualFolders/Name.
#[derive(Deserialize, Default)]
struct RenameQuery {
    #[serde(default, alias = "Name")]
    name: Option<String>,
    #[serde(default, rename = "newName", alias = "NewName")]
    new_name: Option<String>,
    #[serde(default, rename = "refreshLibrary", alias = "RefreshLibrary")]
    refresh_library: bool,
}

/// POST /Library/VirtualFolders/Name?name=&newName= - renames a library: 204. A missing or blank
/// name is 400, an unknown library 404, and a name another library has already 400, as Jellyfin.
async fn rename_virtual_folder(
    State(state): State<AppState>,
    Query(q): Query<RenameQuery>,
) -> Result<StatusCode, ApiError> {
    let name = q.name.as_deref().map(str::trim).unwrap_or_default();
    let new_name = q.new_name.as_deref().map(str::trim).unwrap_or_default();
    if name.is_empty() || new_name.is_empty() {
        return Err(ApiError::BadRequest("name and newName are required".into()));
    }
    let lib = library_named(&state, name).await?;
    let libs = rustyfin_db::repo_library::get_all_libraries(&state.db).await?;
    if libs
        .iter()
        .any(|l| l.id != lib.id && same_name(&l.name, new_name))
    {
        return Err(ApiError::BadRequest(format!(
            "A library named \"{new_name}\" already exists."
        )));
    }
    rustyfin_db::repo_library::rename_library(&state.db, lib.id, new_name).await?;
    state.cache.invalidate_library_lists();
    if q.refresh_library {
        scan_in_background(&state, lib.id);
    }
    Ok(StatusCode::NO_CONTENT)
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
            // Follow links as the library scan does, so linked folders appear in the picker.
            let is_dir = file_type.is_dir()
                || (file_type.is_symlink()
                    && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
            if is_dir {
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
    fn r3admin_folders_match_with_or_without_a_trailing_slash() {
        assert!(same_folder("/media/films", "/media/films/"));
        assert!(same_folder("/media/films//", "/media/films"));
        assert!(same_folder("/", "/"));
        assert!(!same_folder("/media/films", "/media/film"));
        assert!(!same_folder("/media/Films", "/media/films"));
    }

    #[test]
    fn r3admin_names_match_without_case_in_any_script() {
        assert!(same_name("Filme", "FILME"));
        assert!(same_name("Ärzte", "ärzte"));
        assert!(same_name("ΤΑΙΝΊΕΣ", "ταινίες"));
        assert!(!same_name("Films", "Shows"));
    }

    #[test]
    fn test_libraries_to_views_empty() {
        let result = libraries_to_views(&[], "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
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
        let result = libraries_to_views(&libs, "server");
        let user_data = &result["Items"][0]["UserData"];
        assert_eq!(user_data["PlaybackPositionTicks"], 0);
        assert_eq!(user_data["PlayCount"], 0);
        assert_eq!(user_data["IsFavorite"], false);
        assert_eq!(user_data["Played"], false);
    }
}
