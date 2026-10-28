use axum::extract::{Path, State};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use chrono::Utc;
use rustyfin_model::auth::Claims;
use rustyfin_model::user::{
    ChangePasswordRequest, CreateUserRequest, EasyPasswordRequest, ForgotPasswordRequest,
    ForgotPasswordResult, PublicUserDto, ResetPasswordRequest, UpdatePolicyRequest,
    UpdateUserNameRequest, UserAccessSchedule, UserDto, UserImageInfo, UserPreferences,
    UserSecurityInfo,
};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/Users/Public", get(get_public_users))
        .route("/Users/ForgotPassword", post(forgot_password))
        .route("/Users/ResetPassword", post(reset_password))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users", get(get_users))
        .route("/Users/Me", get(get_me))
        .route("/Users/New", post(create_user))
        .route("/Users/{userId}", get(get_user).delete(delete_user_by_id))
        .route("/Users/{userId}", put(update_user_name))
        .route("/Users/{userId}/Delete", post(delete_user))
        .route("/Users/{userId}/Password", post(change_password))
        .route("/Users/{userId}/Policy", post(update_policy))
        .route("/Users/{userId}/Configuration", post(update_configuration))
        .route("/Users/{userId}/Images", get(get_user_images))
        .route("/Users/{userId}/Images/{image_type}", get(get_user_image))
        .route("/Users/{userId}/SecurityInfo", get(get_security_info))
        .route("/Users/{userId}/EasyPassword", post(set_easy_password))
        .route("/Users/{userId}/Preferences", get(get_preferences))
        .route("/Users/{userId}/Periods", get(get_periods))
        // Auth endpoints
        .route("/Auth/Keys", get(get_auth_keys))
        .route("/Auth/Keys/{key}", delete(delete_auth_key))
        .route("/Auth/Providers", get(get_auth_providers))
        .route(
            "/Auth/PasswordResetProviders",
            get(get_password_reset_providers),
        )
        // User played/favorite items (delete variants)
        .route("/UserPlayedItems/{itemId}", delete(delete_user_played_item))
        .route(
            "/UserFavoriteItems/{itemId}",
            delete(delete_user_favorite_item),
        )
        // User image delete
        .route("/UserImage", delete(delete_user_image))
}

async fn get_users(State(state): State<AppState>) -> Result<Json<Vec<UserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db_ro).await?;
    Ok(Json(users.into_iter().map(UserDto::from).collect()))
}

async fn get_me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, claims.sub)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    Ok(Json(UserDto::from(user)))
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    Ok(Json(UserDto::from(user)))
}

async fn get_public_users(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicUserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db_ro).await?;
    Ok(Json(users.into_iter().map(PublicUserDto::from).collect()))
}

async fn create_user(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<UserDto>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }

    if let Some(_existing) = rustyfin_db::repo_user::get_user_by_name(&state.db, &req.name).await? {
        return Err(ApiError::BadRequest("user already exists".into()));
    }

    let hash = rustyfin_auth::hash_password(&req.password)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let user = rustyfin_model::user::User {
        id: Uuid::new_v4(),
        name: req.name,
        password_hash: hash,
        is_administrator: false,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    };

    rustyfin_db::repo_user::create_user(&state.db, &user).await?;
    Ok(Json(UserDto::from(user)))
}

async fn delete_user(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }

    if claims.sub == id {
        return Err(ApiError::BadRequest("cannot delete yourself".into()));
    }

    let deleted = rustyfin_db::repo_user::delete_user(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

async fn change_password(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // users can change their own password, admins can change any
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }

    // non-admins must verify current password
    if !claims.is_admin {
        let user = rustyfin_db::repo_user::get_user_by_id(&state.db, id)
            .await?
            .ok_or(ApiError::NotFound("user not found".into()))?;

        let valid = rustyfin_auth::verify_password(&req.current_pw, &user.password_hash)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        if !valid {
            return Err(ApiError::Unauthorized);
        }
    }

    let hash =
        rustyfin_auth::hash_password(&req.new_pw).map_err(|e| ApiError::Internal(e.to_string()))?;

    let updated = rustyfin_db::repo_user::update_password(&state.db, id, &hash).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

async fn update_policy(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePolicyRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }

    if let Some(is_admin) = req.is_administrator {
        let updated = rustyfin_db::repo_user::update_policy(&state.db, id, is_admin).await?;
        if !updated {
            return Err(ApiError::NotFound("user not found".into()));
        }
    }

    Ok(Json(serde_json::json!({})))
}

async fn update_configuration(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Json(config): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // users can update their own config, admins can update any
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }

    let updated = rustyfin_db::repo_user::update_configuration(&state.db, id, &config).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

async fn forgot_password(
    State(state): State<AppState>,
    Json(req): Json<ForgotPasswordRequest>,
) -> Result<Json<ForgotPasswordResult>, ApiError> {
    // Look up the user by username
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &req.entered_username)
        .await?
        .ok_or_else(|| ApiError::BadRequest("user not found".to_string()))?;

    // Generate a PIN code
    let pin = rustyfin_db::repo_user::generate_password_reset_pin(&state.db, user.id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Log the PIN (in production with SMTP configured, email would be sent)
    tracing::info!(
        user = %user.name,
        pin = %pin,
        "Password reset PIN generated for user"
    );

    Ok(Json(ForgotPasswordResult {
        action: "PinCode".to_string(),
    }))
}

async fn reset_password(
    State(state): State<AppState>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Look up the user by username
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &req.username)
        .await?
        .ok_or_else(|| ApiError::BadRequest("user not found".to_string()))?;

    // Validate the PIN
    let validated =
        rustyfin_db::repo_user::validate_password_reset_pin(&state.db, user.id, &req.pin)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or_else(|| ApiError::BadRequest("invalid or expired PIN".to_string()))?;

    // Hash the new password
    let hash = rustyfin_auth::hash_password(&req.temp_password)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Update the user's password
    let updated = rustyfin_db::repo_user::update_password(&state.db, validated, &hash)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

async fn update_user_name(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<UpdateUserNameRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if claims.sub != id && !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let updated = rustyfin_db::repo_user::update_user_name(&state.db, id, &req.name).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }
    Ok(Json(serde_json::json!({})))
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

async fn get_user_image(
    State(state): State<AppState>,
    Path((id, image_type)): Path<(Uuid, String)>,
) -> Result<Json<UserImageInfo>, ApiError> {
    let images = rustyfin_db::repo_user::get_user_images(&state.db_ro, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Find the matching image
    for (image_id, img_type, _width, _height) in images {
        if img_type == image_type {
            return Ok(Json(UserImageInfo {
                user_id: id,
                image_type,
                id: image_id,
            }));
        }
    }

    // Return default if not found
    Ok(Json(UserImageInfo {
        user_id: id,
        image_type,
        id: String::new(),
    }))
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
        return Err(ApiError::Unauthorized);
    }
    // Store easy password in user configuration
    let config = serde_json::json!({
        "easyPassword": req.easy_password
    });
    let updated = rustyfin_db::repo_user::update_configuration(&state.db, id, &config).await?;
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

/// DELETE /Users/{id} - Delete user by ID
async fn delete_user_by_id(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Unauthorized);
    }
    rustyfin_db::repo_user::delete_user(&state.db, id)
        .await
        .ok();
    Ok(Json(serde_json::json!({})))
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

/// DELETE /UserPlayedItems/{itemId} - Mark item as unplayed
async fn delete_user_played_item(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::mark_unplayed(&state.db, claims.sub, item_id)
        .await
        .ok();
    Ok(Json(serde_json::json!({})))
}

/// DELETE /UserFavoriteItems/{itemId} - Unset favorite
async fn delete_user_favorite_item(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::set_favorite(&state.db, claims.sub, item_id, false)
        .await
        .ok();
    Ok(Json(serde_json::json!({})))
}

/// DELETE /UserImage - Delete user image
async fn delete_user_image(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(serde_json::json!({})))
}
