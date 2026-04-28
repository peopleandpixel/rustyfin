use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use rustyfin_model::auth::Claims;
use rustyfin_model::user::{
    ChangePasswordRequest, CreateUserRequest, ForgotPasswordRequest, ForgotPasswordResult,
    PublicUserDto, UpdatePolicyRequest, UserDto,
};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/Users/Public", get(get_public_users))
        .route("/Users/ForgotPassword", post(forgot_password))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Users", get(get_users))
        .route("/Users/Me", get(get_me))
        .route("/Users/New", post(create_user))
        .route("/Users/{id}", get(get_user))
        .route("/Users/{id}/Delete", post(delete_user))
        .route("/Users/{id}/Password", post(change_password))
        .route("/Users/{id}/Policy", post(update_policy))
        .route("/Users/{id}/Configuration", post(update_configuration))
}

async fn get_users(State(state): State<AppState>) -> Result<Json<Vec<UserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db).await?;
    Ok(Json(users.into_iter().map(UserDto::from).collect()))
}

async fn get_me(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db, claims.sub)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    Ok(Json(UserDto::from(user)))
}

async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<UserDto>, ApiError> {
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound("user not found".into()))?;
    Ok(Json(UserDto::from(user)))
}

async fn get_public_users(
    State(state): State<AppState>,
) -> Result<Json<Vec<PublicUserDto>>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db).await?;
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

    if let Some(_existing) = rustyfin_db::repo_user::get_user_by_name(&state.db, &req.name).await?
    {
        return Err(ApiError::BadRequest("user already exists".into()));
    }

    let hash =
        rustyfin_auth::hash_password(&req.password).map_err(|e| ApiError::Internal(e.to_string()))?;

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
        let updated =
            rustyfin_db::repo_user::update_policy(&state.db, id, is_admin).await?;
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

    let updated =
        rustyfin_db::repo_user::update_configuration(&state.db, id, &config).await?;
    if !updated {
        return Err(ApiError::NotFound("user not found".into()));
    }

    Ok(Json(serde_json::json!({})))
}

async fn forgot_password(
    Json(_req): Json<ForgotPasswordRequest>,
) -> Result<Json<ForgotPasswordResult>, ApiError> {
    // Jellyfin-compatible stub: password reset via PIN is not yet implemented
    Ok(Json(ForgotPasswordResult {
        action: "PinCode".to_string(),
    }))
}
