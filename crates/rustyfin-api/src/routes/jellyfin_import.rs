//! The Jellyfin import (B20 stages 2a, 2b and 3, spec 2.17-2.19, 2.27, 3.5, 3.10): admin only.
//! Preview writes nothing; a run goes through the server's one import job, and its status is read
//! from there. Stage 3 lists the pending users and reissues their one-time setup codes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use rustyfin_core::jellyfin_import::paths::{PathMapping, suggest};
use rustyfin_core::jellyfin_import::setup::{SetupPreview, setup_preview};
use rustyfin_core::jellyfin_import::{
    ImportError, ImportReport, ImportRequest, ImportStatus, JobHandle, LayoutChoice, MetadataMode,
    prepare, preview, run,
};
use rustyfin_db::{repo_library, repo_setup_code, repo_user};
use rustyfin_model::auth::Claims;
use rustyfin_model::library::{Library, LibraryContentType};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::library::library_path_problems;
use crate::error::ApiError;
use crate::state::AppState;
use crate::wizard_import::{
    QueueFile, QueueState, queue_path, remove_queue, run_queued, write_queue, written_cache_clear,
};

/// The setup router's routes: open until the owner exists (B119).
pub fn setup_routes() -> Router<AppState> {
    Router::new().route("/Startup/JellyfinImport/Preview", post(setup_preview_route))
}

/// The body of `POST /Startup/JellyfinImport/Preview`.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SetupPreviewRequest {
    source_path: String,
    #[serde(default)]
    layout: LayoutChoice,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/JellyfinImport/Preview", post(preview_route))
        .route("/JellyfinImport/Run", post(run_route))
        .route("/JellyfinImport/Queue", post(queue_route))
        .route("/JellyfinImport/Status", get(status_route))
        .route("/JellyfinImport/PendingUsers", get(pending_users_route))
        .route(
            "/JellyfinImport/Users/{user_id}/SetupCode",
            post(new_code_route),
        )
}

/// A bad source or a request the source can't satisfy is a 400 naming the field; a failed write or
/// hash is a 500.
fn import_error(error: ImportError) -> ApiError {
    match error {
        ImportError::BadSource(msg) | ImportError::BadRequest(msg) => ApiError::BadRequest(msg),
        ImportError::Db(e) => ApiError::from(e),
        ImportError::Hash => ApiError::Internal("An internal error occurred.".to_string()),
    }
}

/// The 403 text once the server has a user (B119).
const OWNER_EXISTS: &str = "The server already has users. Use /JellyfinImport/Preview.";

/// The setup wizard's preview (B119): names and counts of a Jellyfin data folder. Closed for good
/// once any user exists, admin or not (403); a bad source is a 400. Writes nothing. One preview runs
/// at a time (409); a user created during the read also closes it (403).
async fn setup_preview_route(
    State(state): State<AppState>,
    Json(req): Json<SetupPreviewRequest>,
) -> Result<Json<SetupPreview>, ApiError> {
    if repo_user::users_exist(&state.db).await? {
        return Err(ApiError::Forbidden(OWNER_EXISTS.to_string()));
    }
    let _gate = state
        .jellyfin_import
        .try_preview()
        .ok_or_else(|| ApiError::Conflict("A preview is already running.".to_string()))?;
    let answer = setup_preview(&req.source_path, req.layout, &state.data_dir)
        .await
        .map_err(import_error)?;
    if repo_user::users_exist(&state.db).await? {
        return Err(ApiError::Forbidden(OWNER_EXISTS.to_string()));
    }
    Ok(Json(answer))
}

async fn preview_route(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ImportReport>, ApiError> {
    let report = preview(&state.db_ro, &req, claims.sub, &state.data_dir)
        .await
        .map_err(import_error)?;
    Ok(Json(report))
}

/// Starts the import in a spawned task: 202 `{"State": "Running"}`, or 409 while one runs. The
/// source is read first, so a bad source is refused before any run starts.
async fn run_route(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let prepared = prepare(&req, claims.sub, &state.data_dir)
        .await
        .map_err(import_error)?;
    let handle = state
        .jellyfin_import
        .try_start()
        .ok_or_else(|| ApiError::Conflict("An import is already running.".to_string()))?;
    tokio::spawn(run(
        state.db.clone(),
        req,
        prepared,
        handle,
        (*state.data_dir).clone(),
        written_cache_clear(&state),
    ));
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "State": "Running" })),
    ))
}

/// The body of `POST /JellyfinImport/Queue` (B119): the import's request, and the libraries to
/// create for it. `Libraries` may be empty.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QueueRequest {
    source_path: String,
    #[serde(default)]
    layout: LayoutChoice,
    user_ids: Option<Vec<Uuid>>,
    into_my_account: Option<Uuid>,
    #[serde(default)]
    metadata_mode: MetadataMode,
    #[serde(default)]
    libraries: Vec<QueueLibrary>,
}

/// A library to create for the import, with the Jellyfin folder each path came from.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QueueLibrary {
    name: String,
    content_type: LibraryContentType,
    paths: Vec<QueuePath>,
}

/// A folder of a new library: the Jellyfin path it is read from (`From`, may be empty) and the
/// folder on this server (`To`).
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QueuePath {
    from: String,
    to: String,
}

/// The 409 text while a queued or running import holds the server (B119).
const QUEUE_BUSY: &str = "An import is already queued or running.";

/// Queues the setup wizard's Jellyfin import (B119). Checks the request, then creates its libraries
/// and starts the job, which scans them and then imports: 202 with the new library ids. Nothing is
/// written when a check fails.
async fn queue_route(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<QueueRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    if claims.sub.is_nil() {
        return Err(ApiError::BadRequest(
            "Queue needs a signed-in administrator.".to_string(),
        ));
    }
    if queue_path(&state.data_dir).exists() {
        return Err(ApiError::Conflict(QUEUE_BUSY.to_string()));
    }
    let existing = repo_library::get_all_libraries(&state.db_ro).await?;
    let mut names: Vec<String> = existing.iter().map(|lib| lib.name.to_lowercase()).collect();
    let mut libraries = Vec::with_capacity(req.libraries.len());
    let mut mappings = Vec::new();
    for lib in &req.libraries {
        if lib.name.trim().is_empty() {
            return Err(ApiError::BadRequest("A library needs a name.".to_string()));
        }
        if names.contains(&lib.name.to_lowercase()) {
            return Err(ApiError::BadRequest(format!(
                "A library named \"{}\" already exists.",
                lib.name
            )));
        }
        names.push(lib.name.to_lowercase());
        if lib.paths.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "The library \"{}\" needs a folder.",
                lib.name
            )));
        }
        let to: Vec<String> = lib.paths.iter().map(|p| p.to.clone()).collect();
        let problems = library_path_problems(&to).await;
        if !problems.is_empty() {
            return Err(ApiError::BadRequest(problems.join(" ")));
        }
        mappings.extend(
            lib.paths
                .iter()
                .filter(|p| !p.from.is_empty())
                .map(|p| PathMapping {
                    from: p.from.clone(),
                    to: p.to.clone(),
                }),
        );
        libraries.push(Library {
            id: Uuid::new_v4(),
            name: lib.name.clone(),
            content_type: lib.content_type,
            paths: to,
        });
    }

    let mut user_ids = req.user_ids;
    if let (Some(ids), Some(me)) = (user_ids.as_mut(), req.into_my_account)
        && !ids.contains(&me)
    {
        ids.push(me);
    }
    let mut request = ImportRequest {
        source_path: req.source_path,
        layout: req.layout,
        path_mappings: Some(mappings),
        user_ids,
        confirmed_matches: Vec::new(),
        into_my_account: req.into_my_account,
        metadata_mode: req.metadata_mode,
    };
    // The source is read once here, with the request's own pairs, for the folders the suggested
    // %MetadataPath% and %AppDataPath% pairs come from. The job reads it again when it imports.
    let prepared = prepare(&request, claims.sub, &state.data_dir)
        .await
        .map_err(import_error)?;
    let suggested = suggest(
        &prepared.folders,
        &[],
        &prepared.location.program_data_path,
        &prepared.location.data_path,
    );
    if let Some(pairs) = request.path_mappings.as_mut() {
        pairs.extend(suggested.pairs);
    }

    let handle = state
        .jellyfin_import
        .try_start()
        .ok_or_else(|| ApiError::Conflict(QUEUE_BUSY.to_string()))?;
    let data_dir = (*state.data_dir).clone();
    let library_ids: Vec<Uuid> = libraries.iter().map(|lib| lib.id).collect();
    let file = QueueFile {
        state: QueueState::WaitingForScans,
        caller: claims.sub,
        library_ids: library_ids.clone(),
        request,
    };
    if let Err(error) = write_queue(&data_dir, &file).await {
        return Err(queue_failed(handle, &data_dir, error.to_string()).await);
    }
    if let Err(error) = repo_library::create_libraries(&state.db, &libraries).await {
        return Err(queue_failed(handle, &data_dir, error.to_string()).await);
    }
    state.cache.invalidate_library_lists();
    state.task_scheduler.watcher().libraries_changed();
    tracing::info!("jellyfin import queued: {} libraries", library_ids.len());
    tokio::spawn(run_queued(state.clone(), file, handle));
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "State": "Running", "LibraryIds": library_ids })),
    ))
}

/// Ends a queue that could not be written or stored: the job fails with `error`, the queue file is
/// removed, and the answer is a 500 with the same text.
async fn queue_failed(handle: JobHandle, data_dir: &std::path::Path, error: String) -> ApiError {
    handle.fail(error.clone());
    remove_queue(data_dir).await;
    ApiError::Internal(error)
}

async fn status_route(State(state): State<AppState>) -> Json<ImportStatus> {
    Json(state.jellyfin_import.status())
}

/// A user who has a setup code. `CreatedAt` is when the current code was made.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PendingUserDto {
    user_id: Uuid,
    name: String,
    created_at: DateTime<Utc>,
}

/// A new setup code, shown in this answer only.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SetupCodeDto {
    user_id: Uuid,
    name: String,
    code: String,
}

async fn pending_users_route(
    State(state): State<AppState>,
) -> Result<Json<Vec<PendingUserDto>>, ApiError> {
    let users = repo_setup_code::pending_users(&state.db).await?;
    Ok(Json(
        users
            .into_iter()
            .map(|user| PendingUserDto {
                user_id: user.user_id,
                name: user.name,
                created_at: user.created_at,
            })
            .collect(),
    ))
}

/// Replaces a pending user's setup code: 200 with the new code, 404 for an unknown user, and 409
/// when the user has no pending code (the user has set a password since).
async fn new_code_route(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<SetupCodeDto>, ApiError> {
    let user = repo_user::get_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".to_string()))?;
    let code = rustyfin_auth::generate_setup_code();
    let hash = rustyfin_auth::hash_setup_code(&code)
        .await
        .map_err(|_| ApiError::Internal("An internal error occurred.".to_string()))?;
    if !repo_setup_code::replace_code(&state.db, user_id, &hash, Utc::now()).await? {
        return Err(ApiError::Conflict(
            "The user has no pending setup code.".to_string(),
        ));
    }
    tracing::info!(user_id = %user_id, "jellyfin import: new setup code");
    Ok(Json(SetupCodeDto {
        user_id,
        name: user.name,
        code,
    }))
}
