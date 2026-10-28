use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use rustyfin_model::setup::{
    StartupConfiguration, StartupConfigurationUpdate, StartupUser, StartupUserCreate,
};
use rustyfin_model::user::User;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Startup/Configuration",
            get(get_startup_config).post(update_startup_config),
        )
        .route(
            "/Startup/User",
            get(get_startup_user).post(create_startup_user),
        )
        .route("/Startup/Complete", post(complete_startup))
        .route("/Startup/FirstUser", get(get_startup_first_user))
        .route("/Startup/RemoteAccess", post(setup_remote_access))
}

async fn get_startup_config(State(state): State<AppState>) -> Json<StartupConfiguration> {
    let config = state.config.read().await;
    Json(StartupConfiguration {
        ui_culture: String::new(),
        metadata_country_code: config.metadata_country_code.clone(),
        preferred_metadata_language: config.preferred_metadata_language.clone(),
    })
}

async fn update_startup_config(
    State(state): State<AppState>,
    Json(update): Json<StartupConfigurationUpdate>,
) -> Result<StatusCode, ApiError> {
    {
        let mut config = state.config.write().await;
        if let Some(lang) = update.preferred_metadata_language {
            config.preferred_metadata_language = lang;
        }
        if let Some(country) = update.metadata_country_code {
            config.metadata_country_code = country;
        }
    }
    persist_config(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_startup_user(State(state): State<AppState>) -> Result<Json<StartupUser>, ApiError> {
    // Return the first admin user if one exists, otherwise empty
    let users = rustyfin_db::repo_user::get_all_users(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let admin = users.into_iter().find(|u| u.is_administrator);
    match admin {
        Some(u) => Ok(Json(StartupUser {
            name: u.name,
            password: String::new(),
        })),
        None => Ok(Json(StartupUser {
            name: String::new(),
            password: String::new(),
        })),
    }
}

async fn create_startup_user(
    State(state): State<AppState>,
    Json(input): Json<StartupUserCreate>,
) -> Result<StatusCode, ApiError> {
    if input.name.is_empty() {
        return Err(ApiError::BadRequest("Name is required".into()));
    }

    // Check if an admin already exists
    let users = rustyfin_db::repo_user::get_all_users(&state.db)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(existing) = users.iter().find(|u| u.is_administrator) {
        // Update existing admin user name/password
        let hash = if input.password.is_empty() {
            existing.password_hash.clone()
        } else {
            rustyfin_auth::hash_password(&input.password)
                .map_err(|e| ApiError::Internal(e.to_string()))?
        };
        rustyfin_db::repo_user::update_password(&state.db, existing.id, &hash)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        return Ok(StatusCode::NO_CONTENT);
    }

    // Create new admin user
    let hash = if input.password.is_empty() {
        rustyfin_auth::hash_password("").map_err(|e| ApiError::Internal(e.to_string()))?
    } else {
        rustyfin_auth::hash_password(&input.password)
            .map_err(|e| ApiError::Internal(e.to_string()))?
    };

    let user = User {
        id: Uuid::new_v4(),
        name: input.name,
        password_hash: hash,
        is_administrator: true,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(serde_json::Map::new()),
    };

    rustyfin_db::repo_user::create_user(&state.db, &user)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn complete_startup(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    {
        let mut config = state.config.write().await;
        config.is_startup_wizard_completed = true;
    }
    persist_config(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn persist_config(state: &AppState) -> Result<(), ApiError> {
    let config = state.config.read().await;
    let config_path = state.data_dir.join("config.json");
    let json =
        serde_json::to_string_pretty(&*config).map_err(|e| ApiError::Internal(e.to_string()))?;
    tokio::fs::write(&config_path, &json)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(())
}

/// GET /Startup/FirstUser - Check if first user exists
async fn get_startup_first_user(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let users = rustyfin_db::repo_user::get_all_users(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "HasExistingUsers": !users.is_empty(),
        "LocalLoginEnabled": true
    })))
}

/// POST /Startup/RemoteAccess - Configure remote access
async fn setup_remote_access(
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    Ok(StatusCode::NO_CONTENT)
}
