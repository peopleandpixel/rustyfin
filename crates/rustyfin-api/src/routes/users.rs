use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use chrono::Utc;
use rustyfin_db::repo_user::PolicyChange;
use rustyfin_model::auth::Claims;
use rustyfin_model::parental::{self, ParentalPolicy};
use rustyfin_model::user::{
    ChangePasswordRequest, CreateUserRequest, EasyPasswordRequest, ForgotPasswordPinRequest,
    ForgotPasswordRequest, ForgotPasswordResult, PublicUserDto, ResetPasswordRequest,
    UpdatePolicyRequest, UpdateUserNameRequest, UserAccessSchedule, UserDto, UserFlags,
    UserImageInfo, UserPreferences, UserSecurityInfo, name_too_long,
};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::path::PathBuf;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::resolve_client;
use crate::oidc::{PASSWORD_RESET_OFF, password_allowed};
use crate::query::Query;
use crate::state::AppState;

pub fn public_routes() -> Router<AppState> {
    // As in Jellyfin, a profile picture is served without sign-in, for the login page's user list.
    Router::new()
        .route("/Users/Public", get(get_public_users))
        .route("/UserImage", get(get_user_image_file))
        .route(
            "/Users/{userId}/Images/{image_type}",
            get(get_user_image_legacy),
        )
}

/// Endpoints that check a password or reset PIN. Only these are rate limited.
pub fn login_routes() -> Router<AppState> {
    Router::new()
        .route("/Users/ForgotPassword", post(forgot_password))
        .route("/Users/ForgotPassword/Pin", post(reset_password_pin))
        .route("/Users/ResetPassword", post(reset_password))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users", get(get_users))
        .route("/Users/Me", get(get_me))
        .route("/Users/{userId}", get(get_user))
        // Jellyfin's UpdateUser is a POST of the whole UserDto; only its Name is read.
        .route(
            "/Users/{userId}",
            put(update_user_name).post(update_user_name),
        )
        .route("/Users/{userId}/Password", post(change_password))
        .route("/Users/{userId}/Configuration", post(update_configuration))
        .route("/Users/{userId}/Images", get(get_user_images))
        .route(
            "/Users/{userId}/Images/{image_type}",
            post(post_user_image_legacy)
                .delete(delete_user_image_legacy)
                .layer(DefaultBodyLimit::max(USER_IMAGE_BODY_LIMIT)),
        )
        .route("/Users/{userId}/SecurityInfo", get(get_security_info))
        .route("/Users/{userId}/EasyPassword", post(set_easy_password))
        .route("/Users/{userId}/Preferences", get(get_preferences))
        .route("/Users/{userId}/Periods", get(get_periods))
        .route(
            "/UserImage",
            post(post_user_image)
                .delete(delete_user_image)
                .layer(DefaultBodyLimit::max(USER_IMAGE_BODY_LIMIT)),
        )
}

/// Administrator-only user and auth routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Users/New", post(create_user))
        .route("/Users/{userId}", delete(delete_user_by_id))
        .route("/Users/{userId}/Delete", post(delete_user))
        .route("/Users/{userId}/Policy", post(update_policy))
        .route("/Auth/Keys", get(get_auth_keys))
        .route("/Auth/Keys/{key}", delete(delete_auth_key))
        .route("/Auth/Providers", get(get_auth_providers))
        .route(
            "/Auth/PasswordResetProviders",
            get(get_password_reset_providers),
        )
}

async fn get_users(State(state): State<AppState>) -> Result<Json<Vec<UserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db_ro).await?;
    let flags = rustyfin_db::repo_user::all_user_flags(&state.db_ro).await?;
    let parental = rustyfin_db::repo_parental::all_policies(&state.db_ro).await?;
    let tags = rustyfin_db::repo_user::primary_image_tags(&state.db_ro).await?;
    Ok(Json(
        users
            .into_iter()
            .map(|u| {
                let f = flags.get(&u.id).copied().unwrap_or_default();
                let p = parental.get(&u.id).cloned().unwrap_or_default();
                let tag = tags.get(&u.id).cloned();
                UserDto {
                    primary_image_tag: tag,
                    ..UserDto::new(u, f, &state.server_id, &p)
                }
            })
            .collect(),
    ))
}

/// The user's profile picture tag, if they have one.
pub(crate) async fn primary_image_tag(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    Ok(rustyfin_db::repo_user::user_image(pool, id, "Primary")
        .await?
        .map(|(_, tag)| tag))
}

async fn get_me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, claims.sub)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    let flags = rustyfin_db::repo_user::user_flags(&state.db_ro, user.id).await?;
    let parental = rustyfin_db::repo_parental::get_policy(&state.db_ro, user.id).await?;
    let primary_image_tag = primary_image_tag(&state.db_ro, user.id).await?;
    Ok(Json(UserDto {
        primary_image_tag,
        ..UserDto::new(user, flags, &state.server_id, &parental)
    }))
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    let flags = rustyfin_db::repo_user::user_flags(&state.db_ro, user.id).await?;
    let parental = rustyfin_db::repo_parental::get_policy(&state.db_ro, user.id).await?;
    let primary_image_tag = primary_image_tag(&state.db_ro, user.id).await?;
    Ok(Json(UserDto {
        primary_image_tag,
        ..UserDto::new(user, flags, &state.server_id, &parental)
    }))
}

async fn get_public_users(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicUserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_public_users(&state.db_ro).await?;
    let tags = rustyfin_db::repo_user::primary_image_tags(&state.db_ro).await?;
    // A hidden user (IsHidden) signs in by typing the name, as in Jellyfin.
    let policies = rustyfin_db::repo_parental::all_policies(&state.db_ro).await?;
    Ok(Json(
        users
            .into_iter()
            .filter(|u| !policies.get(&u.id).is_some_and(|p| p.access.is_hidden))
            .map(|u| {
                let primary_image_tag = tags.get(&u.id).cloned();
                PublicUserDto {
                    primary_image_tag,
                    ..PublicUserDto::from(u)
                }
            })
            .collect(),
    ))
}

async fn create_user(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserDto>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if name_too_long(&req.name) {
        return Err(ApiError::BadRequest("name is too long".into()));
    }

    let hash = rustyfin_auth::hash_password(&req.password)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let user = rustyfin_model::user::User {
        id: Uuid::new_v4(),
        name: req.name,
        password_hash: hash,
        // As in Jellyfin, a new user is never an administrator; POST /Users/{id}/Policy grants that.
        is_administrator: false,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    };

    if !rustyfin_db::repo_user::create_user_unless_taken(&state.db, &user).await? {
        return Err(ApiError::BadRequest("user already exists".into()));
    }
    super::activity::record(
        &state,
        &format!("User {} has been created", user.name),
        "UserCreated",
        Some(user.id),
        "Information",
    )
    .await;
    Ok(Json(UserDto::new(
        user,
        UserFlags::default(),
        &state.server_id,
        &ParentalPolicy::default(),
    )))
}

/// Checks the caller may delete user `id`, then deletes the user and their data.
async fn remove_user(state: &AppState, claims: &Claims, id: Uuid) -> Result<(), ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }

    if rustyfin_db::repo_user::is_owner(&state.db, id).await? {
        return Err(ApiError::Forbidden(
            "The server owner cannot be deleted.".into(),
        ));
    }

    if claims.sub == id {
        return Err(ApiError::BadRequest("cannot delete yourself".into()));
    }

    let name = rustyfin_db::repo_user::get_user_by_id(&state.db, id)
        .await?
        .map(|u| u.name);
    let ended = rustyfin_db::repo_session::session_ids(&state.db, Some(id), None).await?;
    let deleted = rustyfin_db::repo_user::delete_user(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound("user not found".into()));
    }
    crate::sessions::forget_all(&ended);
    crate::sessions::changed(state);
    // The profile picture's folder; its rows went with the user
    let _ =
        tokio::fs::remove_dir_all(state.data_dir.join("users").join(id.simple().to_string())).await;
    super::activity::record(
        state,
        &format!("User {} has been deleted", name.unwrap_or_default()),
        "UserDeleted",
        Some(id),
        "Information",
    )
    .await;

    Ok(())
}

/// POST /Users/{id}/Delete - Delete a user (the web client's contract)
async fn delete_user(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    remove_user(&state, &claims, id).await?;
    Ok(Json(serde_json::json!({})))
}

async fn change_password(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // users can change their own password, admins can change any
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }

    // The current password is required, except when an admin changes someone else's. A user with
    // no password yet (empty stored hash) may set the first one without it.
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    let admin_for_other = claims.is_admin && claims.sub != id;
    if !admin_for_other && !user.password_hash.is_empty() {
        let current = req.current_pw.as_deref().unwrap_or("");
        let valid = rustyfin_auth::verify_password(current, &user.password_hash)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if !valid {
            return Err(ApiError::Unauthorized);
        }
    }

    let hash = rustyfin_auth::hash_password(&req.new_pw)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let updated = rustyfin_db::repo_user::update_password(&state.db, id, &hash).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    // Revoke the user's other tokens. A self-change keeps the caller's own session; an admin
    // changing another user's password revokes all of that user's tokens.
    let keep: Option<String> = if claims.sub == id {
        token.map(|Extension(rt)| rt.0)
    } else {
        None
    };
    rustyfin_auth::revoke_user_tokens(&state.db, id, keep.as_deref())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    super::activity::record(
        &state,
        &format!("Password has been changed for user {}", user.name),
        "UserPasswordChanged",
        Some(id),
        "Information",
    )
    .await;

    Ok(Json(serde_json::json!({})))
}

/// Sets an account's administrator and disabled flags and its parental and access policy.
/// Administrators only. Refuses, with 403, to change the owner's role or state, to remove or disable
/// one's own administrator rights or account, to take away the owner's or one's own remote access or
/// add an access schedule to them, to disable an administrator, to leave no enabled administrator,
/// and to leave no enabled administrator with remote access. Disabling an account deletes its tokens,
/// sessions and devices.
async fn update_policy(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePolicyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }

    if rustyfin_db::repo_user::get_user_by_id(&state.db, id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound("user not found".into()));
    }

    // Checked before anything is stored, so a policy that can't be kept changes nothing.
    let stored = rustyfin_db::repo_parental::get_policy(&state.db, id).await?;
    let parental = parental_policy(&req, stored.clone())?;
    // A change that would lock the account out: remote access taken away, or a new schedule
    let locks_out = (stored.access.enable_remote_access && !parental.access.enable_remote_access)
        || (!parental.access.access_schedules.is_empty()
            && parental.access.access_schedules != stored.access.access_schedules);
    let is_owner = rustyfin_db::repo_user::is_owner(&state.db, id).await?;
    if locks_out && (is_owner || claims.sub == id) {
        return Err(ApiError::Forbidden(if is_owner {
            "The server owner's remote access and access times cannot be limited.".into()
        } else {
            "You cannot limit your own remote access or access times.".into()
        }));
    }

    if is_owner {
        if req.is_administrator == Some(false) {
            return Err(ApiError::Forbidden(
                "The server owner must stay an administrator.".into(),
            ));
        }
        if req.is_disabled == Some(true) {
            return Err(ApiError::Forbidden(
                "The server owner cannot be disabled.".into(),
            ));
        }
    }

    if claims.sub == id {
        if req.is_administrator == Some(false) {
            return Err(ApiError::Forbidden(
                "You cannot remove your own administrator rights.".into(),
            ));
        }
        if req.is_disabled == Some(true) {
            return Err(ApiError::Forbidden(
                "You cannot disable your own account.".into(),
            ));
        }
    }

    // The flags and the parental and access policy are stored in one transaction.
    let change = rustyfin_db::repo_user::set_policy_with(
        &state.db,
        id,
        req.is_administrator,
        req.is_disabled,
        Some(&parental),
    )
    .await?;
    match change {
        PolicyChange::Done => {}
        PolicyChange::NotFound => return Err(ApiError::NotFound("user not found".into())),
        PolicyChange::AdminDisabled => {
            return Err(ApiError::Forbidden(
                "Administrators cannot be disabled. Remove the administrator rights first.".into(),
            ));
        }
        PolicyChange::PromoteDisabled => {
            return Err(ApiError::Forbidden(
                "A disabled account cannot be made an administrator. Enable it first.".into(),
            ));
        }
        PolicyChange::LastAdmin => {
            return Err(ApiError::Forbidden(
                "There must be at least one enabled administrator.".into(),
            ));
        }
        PolicyChange::NoRemoteAdmin => {
            return Err(ApiError::Forbidden(
                "There must be at least one enabled administrator who may sign in remotely.".into(),
            ));
        }
    }

    Ok(Json(serde_json::json!({})))
}

/// The stored parental and access policy with the request's fields over it. An unknown kind of
/// unrated item, a schedule with an unknown day or hours out of order, and a negative limit are
/// refused, so an administrator is told a limit wasn't kept instead of believing it was. Library
/// ids are stored in one spelling; one that isn't an id is dropped, which shows less, not more.
fn parental_policy(
    req: &UpdatePolicyRequest,
    mut policy: ParentalPolicy,
) -> Result<ParentalPolicy, ApiError> {
    let access = &mut policy.access;
    if let Some(schedules) = &req.access_schedules {
        access.access_schedules = schedules
            .iter()
            .map(|s| {
                s.validated().ok_or_else(|| {
                    ApiError::BadRequest(format!(
                        "Invalid access schedule: {} {} to {}",
                        s.day_of_week, s.start_hour, s.end_hour
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
    }
    if let Some(folders) = &req.enabled_folders {
        let mut ids: Vec<String> = Vec::new();
        for id in folders
            .iter()
            .filter_map(|f| Uuid::parse_str(f.trim()).ok())
        {
            if !ids.contains(&id.to_string()) {
                ids.push(id.to_string());
            }
        }
        access.enabled_folders = ids;
    }
    if req.max_active_sessions.is_some_and(|n| n < 0)
        || req.remote_client_bitrate_limit.is_some_and(|n| n < 0)
    {
        return Err(ApiError::BadRequest("Limits cannot be negative.".into()));
    }
    let set = |field: &mut bool, value: Option<bool>| {
        if let Some(value) = value {
            *field = value;
        }
    };
    set(&mut access.enable_all_folders, req.enable_all_folders);
    set(&mut access.is_hidden, req.is_hidden);
    set(&mut access.enable_remote_access, req.enable_remote_access);
    set(&mut access.enable_media_playback, req.enable_media_playback);
    set(
        &mut access.enable_audio_playback_transcoding,
        req.enable_audio_playback_transcoding,
    );
    set(
        &mut access.enable_video_playback_transcoding,
        req.enable_video_playback_transcoding,
    );
    set(
        &mut access.enable_playback_remuxing,
        req.enable_playback_remuxing,
    );
    set(
        &mut access.force_remote_source_transcoding,
        req.force_remote_source_transcoding,
    );
    if req.enable_content_deletion.is_some() {
        access.enable_content_deletion = req.enable_content_deletion;
    }
    if let Some(n) = req.max_active_sessions {
        access.max_active_sessions = n;
    }
    if let Some(n) = req.remote_client_bitrate_limit {
        access.remote_client_bitrate_limit = n;
    }
    if let Some(max) = req.max_parental_rating {
        policy.max_parental_rating = max;
    }
    if let Some(sub) = req.max_parental_sub_rating {
        policy.max_parental_sub_rating = sub;
    }
    if let Some(kinds) = &req.block_unrated_items {
        let mut out = Vec::new();
        for kind in kinds.iter().flatten() {
            let kind = parental::unrated_item_kind(kind).ok_or_else(|| {
                ApiError::BadRequest(format!("Unknown kind of unrated item: {kind}"))
            })?;
            if !out.iter().any(|k| k == kind) {
                out.push(kind.to_string());
            }
        }
        policy.block_unrated_items = out;
    }
    let tags = |tags: &Option<Vec<String>>| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tag in tags.iter().flatten().map(|t| t.trim()) {
            if !tag.is_empty()
                && !out
                    .iter()
                    .any(|t| parental::tag_key(t) == parental::tag_key(tag))
            {
                out.push(tag.to_string());
            }
        }
        out
    };
    if let Some(blocked) = &req.blocked_tags {
        policy.blocked_tags = tags(blocked);
    }
    if let Some(allowed) = &req.allowed_tags {
        policy.allowed_tags = tags(allowed);
    }
    Ok(policy)
}

async fn update_configuration(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(config): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // users can update their own config, admins can update any
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }

    let updated = rustyfin_db::repo_user::update_configuration(&state.db, id, &config).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

/// Every ForgotPassword answer waits until this long after the request reached the handler, so its
/// timing reveals neither whether the name exists nor whether the reset work was slow.
const FORGOT_PASSWORD_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Start a password reset. The response is uniform whether or not the user exists, so it never
/// reveals account existence, and it never contains the PIN. The PIN is written (mode 0600 on Unix)
/// to a file under the data dir ONLY when the user exists AND the resolved client (see
/// `resolve_client`) is on the local network AND the user has no pending or voided reset, since a
/// new request never replaces an open reset. Every other request gets the same 200 with a file path
/// that is not created. Every answer comes `FORGOT_PASSWORD_DELAY` (250 ms) after the request
/// reached the handler. The reset work runs in its own task: it finishes before the answer when it
/// is quick, and in the background when it takes longer.
async fn forgot_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<ForgotPasswordRequest>,
) -> Result<Json<ForgotPasswordResult>, ApiError> {
    let deadline = tokio::time::Instant::now() + FORGOT_PASSWORD_DELAY;
    let pin_path = state
        .data_dir
        .join(rustyfin_auth::reset_pin_filename(&req.entered_username));
    let expires_at = (Utc::now() + chrono::Duration::minutes(30)).to_rfc3339();

    if resolve_client(peer.ip(), &headers, &state.known_proxies).is_local() {
        let handle = tokio::spawn(issue_reset_pin(
            state.db.clone(),
            req.entered_username.clone(),
            pin_path.clone(),
            expires_at.clone(),
        ));
        // Waited for only until the deadline; a slower task is detached and finishes alone.
        let _ = tokio::time::timeout_at(deadline, handle).await;
    }
    tokio::time::sleep_until(deadline).await;

    Ok(Json(ForgotPasswordResult {
        action: "PinCode".to_string(),
        pin_file: pin_path.to_string_lossy().into_owned(),
        pin_expiration_date: expires_at,
    }))
}

/// The reset work for a local ForgotPassword, run in its own task (see `forgot_password`). Nothing
/// is returned to the client; a failure is logged without the PIN.
async fn issue_reset_pin(db: SqlitePool, name: String, pin_path: PathBuf, expires_at: String) {
    let user = match rustyfin_db::repo_user::find_user_for_sign_in(&db, &name).await {
        Ok(Some(user)) => user,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, "reset PIN: user lookup failed");
            return;
        }
    };
    // While password sign-in is off, only a user the switch allows gets a reset PIN (AC22, AC41).
    let allowed = match rustyfin_db::repo_oidc::get_settings(&db).await {
        Ok(config) => password_allowed(&db, &config, &user).await,
        Err(e) => Err(e),
    };
    match allowed {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(error = %e, "reset PIN: could not read the sign-in settings");
            return;
        }
    }
    let pin = match rustyfin_auth::create_reset_pin(&db, user.id, &expires_at).await {
        Ok(Some(pin)) => pin,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, "reset PIN: could not store the PIN");
            return;
        }
    };
    // The PIN lives only in this file. Never log it.
    let contents = serde_json::json!({
        "Pin": pin,
        "ExpirationDate": expires_at,
        "UserName": user.name,
    })
    .to_string();
    if let Err(e) = write_reset_file(&pin_path, &contents) {
        tracing::warn!(error = %e, "reset PIN: could not write the reset file");
    }
}

/// Write the reset file, truncating any prior file. On Unix it is created owner-only (mode 0600).
fn write_reset_file(path: &std::path::Path, contents: &str) -> Result<(), ApiError> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(())
}

/// Redeem a reset PIN (no username). Success: set the user's password to the PIN itself, revoke all
/// their tokens, delete the reset file, return `{Success:true, UsersReset:[name]}`. No match → 404.
async fn reset_password_pin(
    State(state): State<AppState>,
    Json(req): Json<ForgotPasswordPinRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let uid = rustyfin_auth::redeem_reset_pin(&state.db, &req.pin)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::NotFound("invalid or expired PIN".to_string()))?;

    let user = rustyfin_db::repo_user::get_user_by_id(&state.db, uid)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".to_string()))?;

    // While password sign-in is off, only a user the switch allows may reset a password by PIN
    // (AC22, AC41). The PIN is already spent here, so its file goes too.
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    if !password_allowed(&state.db, &config, &user).await? {
        remove_reset_pin_files(&state.data_dir, &user.name);
        return Err(ApiError::Forbidden(PASSWORD_RESET_OFF.to_string()));
    }

    let norm = rustyfin_auth::normalize_reset_pin(&req.pin);
    let hash = rustyfin_auth::hash_password(&norm)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    rustyfin_db::repo_user::update_password(&state.db, uid, &hash).await?;
    rustyfin_auth::revoke_user_tokens(&state.db, uid, None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    remove_reset_pin_files(&state.data_dir, &user.name);

    Ok(Json(serde_json::json!({
        "Success": true,
        "UsersReset": [user.name],
    })))
}

/// Removes the reset PIN files of `name`: the one named from its name key, and the one an older
/// server named from the uppercased name (AC65). The redeem calls it on both of its paths.
fn remove_reset_pin_files(data_dir: &std::path::Path, name: &str) {
    let _ = std::fs::remove_file(data_dir.join(rustyfin_auth::reset_pin_filename(name)));
    let _ = std::fs::remove_file(data_dir.join(rustyfin_auth::legacy_reset_pin_filename(name)));
}

async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the user by username. An unknown user gets the wrong-PIN error, so the response does
    // not reveal whether the account exists.
    let user = rustyfin_db::repo_user::find_user_for_sign_in(&state.db, &req.username)
        .await?
        .ok_or_else(|| ApiError::BadRequest("invalid or expired PIN".to_string()))?;

    // Validate the PIN
    let validated = rustyfin_auth::validate_reset_pin(&state.db, user.id, &req.pin)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("invalid or expired PIN".to_string()))?;

    // While password sign-in is off, only a user the switch allows may reset a password
    // (AC22, AC41).
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    if !password_allowed(&state.db, &config, &user).await? {
        return Err(ApiError::Forbidden(PASSWORD_RESET_OFF.to_string()));
    }

    // Hash the new password
    let hash = rustyfin_auth::hash_password(&req.temp_password)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Update the user's password
    let updated = rustyfin_db::repo_user::update_password(&state.db, validated, &hash)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    rustyfin_auth::revoke_user_tokens(&state.db, validated, None)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({})))
}

async fn update_user_name(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<UpdateUserNameRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if name_too_long(&req.name) {
        return Err(ApiError::BadRequest("name is too long".into()));
    }
    match rustyfin_db::repo_user::rename_user_unless_taken(&state.db, id, &req.name).await? {
        rustyfin_db::repo_user::Renamed::Done => Ok(Json(serde_json::json!({}))),
        rustyfin_db::repo_user::Renamed::Taken => {
            Err(ApiError::BadRequest("user already exists".into()))
        }
        rustyfin_db::repo_user::Renamed::NotFound => {
            Err(ApiError::NotFound("user not found".into()))
        }
    }
}

async fn get_user_images(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<UserImageInfo>>, ApiError> {
    let images = rustyfin_db::repo_user::get_user_images(&state.db_ro, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let infos: Vec<UserImageInfo> = images
        .into_iter()
        .map(|(image_id, image_type, _width, _height)| UserImageInfo {
            user_id: id,
            image_type,
            id: image_id,
        })
        .collect();

    // If no images exist, return a default Primary placeholder
    if infos.is_empty() {
        return Ok(Json(vec![UserImageInfo {
            user_id: id,
            image_type: "Primary".to_string(),
            id: String::new(),
        }]));
    }

    Ok(Json(infos))
}

async fn get_security_info(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserSecurityInfo>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".to_string()))?;
    Ok(Json(UserSecurityInfo {
        user_id: user.id,
        name: user.name.clone(),
        is_administrator: user.is_administrator,
        authentication_provider_id: "Jellyfin.Authentication.DefaultProvider, Jellyfin.Server"
            .to_string(),
        authentication_provider_description: "Jellyfin".to_string(),
        password_reset_provider_id:
            "Jellyfin.Authentication.DefaultPasswordResetProvider, Jellyfin.Server".to_string(),
        password_reset_provider_description: "Default".to_string(),
        supports_two_step: false,
    }))
}

async fn set_easy_password(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<EasyPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }
    // Only an argon2 hash is stored, apart from the configuration, so no response returns the PIN;
    // an empty PIN clears it.
    let hash = if req.easy_password.is_empty() {
        None
    } else {
        Some(
            rustyfin_auth::hash_password(&req.easy_password)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?,
        )
    };
    let updated =
        rustyfin_db::repo_user::set_easy_password_hash(&state.db, id, hash.as_deref()).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }
    Ok(Json(serde_json::json!({})))
}

async fn get_preferences(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserPreferences>, ApiError> {
    let prefs = rustyfin_db::repo_user::get_user_preferences(&state.db_ro, id).await?;
    Ok(Json(prefs))
}

async fn get_periods(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<UserAccessSchedule>>, ApiError> {
    let schedules = rustyfin_db::repo_user::get_user_access_schedules(&state.db_ro, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let periods: Vec<UserAccessSchedule> = schedules
        .into_iter()
        .map(|(day_of_week, start_time, end_time)| {
            // Parse HH:MM format to hour
            let start_hour = start_time
                .split(':')
                .next()
                .and_then(|h| h.parse::<u32>().ok())
                .unwrap_or(0);
            let end_hour = end_time
                .split(':')
                .next()
                .and_then(|h| h.parse::<u32>().ok())
                .unwrap_or(0);
            UserAccessSchedule {
                day_of_week,
                start_hour,
                end_hour,
            }
        })
        .collect();

    Ok(Json(periods))
}

/// DELETE /Users/{id} - Delete user by ID (204, as Jellyfin's UserController.DeleteUser)
async fn delete_user_by_id(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    remove_user(&state, &claims, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /Auth/Keys - List API keys
async fn get_auth_keys(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!([])))
}

/// DELETE /Auth/Keys/{key} - Revoke API key
async fn delete_auth_key(
    State(_state): State<AppState>,
    Path(_key): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}

/// GET /Auth/Providers - List authentication providers
async fn get_auth_providers(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!([{
        "Name": "Jellyfin",
        "Description": "Jellyfin Authentication",
        "Id": "Jellyfin.Authentication.DefaultProvider, Jellyfin.Server"
    }])))
}

/// GET /Auth/PasswordResetProviders - List password reset providers
async fn get_password_reset_providers(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!([{
        "Name": "Default",
        "Description": "Default Password Reset Provider",
        "Id": "Jellyfin.Authentication.DefaultPasswordResetProvider, Jellyfin.Server"
    }])))
}

/// The largest profile picture upload, base64 included.
const USER_IMAGE_BODY_LIMIT: usize = 20 * 1024 * 1024;

#[derive(Debug, serde::Deserialize)]
struct UserImageQuery {
    #[serde(rename = "userId")]
    user_id: Option<Uuid>,
    /// The picture's tag; a URL with one names that version, so it may be cached for long
    tag: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ImageTagQuery {
    tag: Option<String>,
}

/// Users have only a Primary image, as in Jellyfin; another type is refused.
fn require_primary(image_type: &str) -> Result<(), ApiError> {
    if image_type.eq_ignore_ascii_case("Primary") {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!(
            "users have no {image_type} image"
        )))
    }
}

/// User `id`'s profile picture: long cached when the URL names its tag, else checked each time.
async fn serve_user_image(
    state: &AppState,
    id: Uuid,
    tagged: bool,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    let (path, _) = rustyfin_db::repo_user::user_image(&state.db_ro, id, "Primary")
        .await?
        .ok_or_else(|| ApiError::NotFound("user has no image".into()))?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound("user has no image".into()))?;
    let content_type = image_format(&bytes).map_or("application/octet-stream", |(_, ct)| ct);
    let cache = if tagged {
        "public, max-age=31536000"
    } else {
        "no-cache"
    };
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, content_type),
            (axum::http::header::CACHE_CONTROL, cache),
        ],
        bytes,
    )
        .into_response())
}

/// The image format of `bytes` by its signature: its file extension and content type.
fn image_format(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(("png", "image/png"))
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"GIF8") {
        Some(("gif", "image/gif"))
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else {
        None
    }
}

/// The picture an upload carries. Jellyfin's clients send it base64-encoded, as Jellyfin reads it;
/// the raw file, or a data URL, is taken too.
fn decode_user_image(body: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine as _;
    if image_format(body).is_some() {
        return Some(body.to_vec());
    }
    let text = std::str::from_utf8(body).ok()?.trim();
    let text = text.split_once(";base64,").map_or(text, |(_, data)| data);
    let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()?;
    image_format(&bytes).map(|_| bytes)
}

/// The user a profile picture request names (`userId`, else the caller), once the caller may
/// change that user's picture: their own, or anyone's for an administrator.
async fn image_owner(
    state: &AppState,
    claims: &Claims,
    user_id: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let id = user_id.unwrap_or(claims.sub);
    if id != claims.sub && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }
    rustyfin_db::repo_user::get_user_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    Ok(id)
}

/// Removes picture files that no row names any more; a file already gone is fine.
async fn remove_image_files(paths: Vec<String>) {
    for path in paths {
        if let Err(e) = tokio::fs::remove_file(&path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, path, "could not remove an old profile picture");
        }
    }
}

/// Saves `body` as user `id`'s profile picture under the data dir, replacing the old one.
async fn save_user_image(state: &AppState, id: Uuid, body: &[u8]) -> Result<StatusCode, ApiError> {
    let bytes = decode_user_image(body).ok_or_else(|| {
        ApiError::BadRequest("the body is not a PNG, JPEG, GIF or WebP image".into())
    })?;
    let (ext, _) = image_format(&bytes).unwrap_or(("jpg", "image/jpeg"));
    let dir = state.data_dir.join("users").join(id.simple().to_string());
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let path = dir.join(format!("primary-{}.{ext}", Uuid::new_v4().simple()));
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let saved = rustyfin_db::repo_user::save_user_image(
        &state.db,
        id,
        "Primary",
        &path.to_string_lossy(),
        None,
        None,
    )
    .await;
    match saved {
        Ok((_, old)) => {
            remove_image_files(old).await;
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => {
            remove_image_files(vec![path.to_string_lossy().into_owned()]).await;
            Err(e.into())
        }
    }
}

/// Deletes user `id`'s profile picture; deleting one that isn't there succeeds too.
async fn remove_user_image(state: &AppState, id: Uuid) -> Result<StatusCode, ApiError> {
    let old = rustyfin_db::repo_user::delete_user_image(&state.db, id, "Primary").await?;
    remove_image_files(old).await;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /UserImage?userId= - the user's profile picture, 404 when they have none
async fn get_user_image_file(
    State(state): State<AppState>,
    Query(q): Query<UserImageQuery>,
) -> Result<axum::response::Response, ApiError> {
    let id = q
        .user_id
        .ok_or_else(|| ApiError::BadRequest("userId is required".into()))?;
    serve_user_image(&state, id, q.tag.is_some_and(|t| !t.is_empty())).await
}

/// GET /Users/{userId}/Images/{imageType} - Jellyfin's older form of GET /UserImage, also without sign-in
async fn get_user_image_legacy(
    State(state): State<AppState>,
    Path((id, image_type)): Path<(Uuid, String)>,
    Query(q): Query<ImageTagQuery>,
) -> Result<axum::response::Response, ApiError> {
    if !image_type.eq_ignore_ascii_case("Primary") {
        return Err(ApiError::NotFound("user has no image".into()));
    }
    serve_user_image(&state, id, q.tag.is_some_and(|t| !t.is_empty())).await
}

/// POST /UserImage?userId= - set the profile picture (base64 body, as Jellyfin takes it)
async fn post_user_image(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<UserImageQuery>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let id = image_owner(&state, &claims, q.user_id).await?;
    save_user_image(&state, id, &body).await
}

/// DELETE /UserImage?userId= - delete the profile picture
async fn delete_user_image(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<UserImageQuery>,
) -> Result<StatusCode, ApiError> {
    let id = image_owner(&state, &claims, q.user_id).await?;
    remove_user_image(&state, id).await
}

/// POST /Users/{userId}/Images/{imageType} - Jellyfin's older form; users have only a Primary image
async fn post_user_image_legacy(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path((id, image_type)): Path<(Uuid, String)>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    require_primary(&image_type)?;
    let id = image_owner(&state, &claims, Some(id)).await?;
    save_user_image(&state, id, &body).await
}

/// DELETE /Users/{userId}/Images/{imageType} - Jellyfin's older form
async fn delete_user_image_legacy(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path((id, image_type)): Path<(Uuid, String)>,
) -> Result<StatusCode, ApiError> {
    require_primary(&image_type)?;
    let id = image_owner(&state, &claims, Some(id)).await?;
    remove_user_image(&state, id).await
}
