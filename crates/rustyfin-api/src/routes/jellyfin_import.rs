//! The Jellyfin import (B20 stages 2a, 2b and 3, spec 2.17-2.19, 2.27, 3.5, 3.10): admin only.
//! Preview writes nothing; a run goes through the server's one import job, and its status is read
//! from there. Stage 3 lists the pending users and reissues their one-time setup codes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use rustyfin_core::jellyfin_import::{
    ImportError, ImportReport, ImportRequest, ImportStatus, prepare, preview, run,
};
use rustyfin_db::{repo_setup_code, repo_user};
use rustyfin_model::auth::Claims;
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/JellyfinImport/Preview", post(preview_route))
        .route("/JellyfinImport/Run", post(run_route))
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
    // Written metadata changes what the item and image caches serve, so they're cleared then.
    let cache_state = state.clone();
    tokio::spawn(run(
        state.db.clone(),
        req,
        prepared,
        handle,
        (*state.data_dir).clone(),
        move || {
            cache_state.cache.invalidate_on_scan();
            cache_state.cache.item_details.invalidate_all();
            cache_state.cache.search_results.invalidate_all();
            cache_state.image_cache.invalidate_all();
        },
    ));
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "State": "Running" })),
    ))
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
