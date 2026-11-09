use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_user::OwnerClaim;
use rustyfin_model::auth::Claims;
use rustyfin_model::server_info::ServerConfiguration;
use rustyfin_model::setup::{
    StartupConfiguration, StartupConfigurationUpdate, StartupUser, StartupUserCreate,
};
use rustyfin_model::user::name_too_long;

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

async fn get_startup_user(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
) -> Result<Json<StartupUser>, ApiError> {
    let Some(owner) = rustyfin_db::repo_user::get_owner(&state.db_ro).await? else {
        // Without an owner the wizard starts empty.
        return Ok(Json(StartupUser {
            name: String::new(),
            password: String::new(),
        }));
    };
    // The guard admits only administrators once the server has users. This check covers a race.
    if !claims.is_some_and(|Extension(c)| c.is_admin) {
        return Err(ApiError::Unauthorized);
    }
    Ok(Json(StartupUser {
        name: owner.name,
        password: String::new(),
    }))
}

async fn create_startup_user(
    State(state): State<AppState>,
    Json(input): Json<StartupUserCreate>,
) -> Result<StatusCode, ApiError> {
    if rustyfin_db::repo_user::users_exist(&state.db).await? {
        return Err(ApiError::Forbidden("The server already has users.".into()));
    }
    // The name is stored exactly as sent. Only this check trims it.
    if input.name.trim().is_empty() {
        return Err(ApiError::BadRequest("Name is required".into()));
    }
    if name_too_long(&input.name) {
        return Err(ApiError::BadRequest("Name is too long".into()));
    }

    let hash = rustyfin_auth::hash_password(&input.password)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    match rustyfin_db::repo_user::claim_owner(&state.db, &input.name, &hash).await? {
        OwnerClaim::Claimed => Ok(StatusCode::NO_CONTENT),
        OwnerClaim::Refused => Err(ApiError::Forbidden("The server already has users.".into())),
    }
}

async fn complete_startup(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    if !rustyfin_db::repo_user::owner_exists(&state.db).await? {
        return Err(ApiError::BadRequest(
            "Create the server owner before completing the setup.".into(),
        ));
    }
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

/// What `settle_owner_at_startup` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOwner {
    /// The server has no users, so the setup wizard is open.
    NoUsers,
    /// The server already had an owner.
    Existing,
    /// The earliest enabled administrator, named here, became the owner.
    Promoted(String),
    /// Users exist, but none is an enabled administrator, so the server has no owner.
    NoEnabledAdmin,
}

/// Runs at startup, before the server listens. With users, it makes the earliest enabled
/// administrator the owner if there is none, and marks the setup wizard complete whatever
/// `config.json` says. Without users it changes nothing, so the setup wizard stays open.
pub async fn settle_owner_at_startup(
    db: &sqlx::SqlitePool,
    config: &mut ServerConfiguration,
    data_dir: &std::path::Path,
) -> Result<StartupOwner, sqlx::Error> {
    if !rustyfin_db::repo_user::users_exist(db).await? {
        tracing::info!("no server owner yet; finish the setup wizard in the web client");
        return Ok(StartupOwner::NoUsers);
    }

    let owner = match rustyfin_db::repo_user::promote_earliest_admin_to_owner(db).await? {
        Some(user) => {
            tracing::info!(
                "promoted earliest administrator '{}' to server owner",
                user.name
            );
            StartupOwner::Promoted(user.name)
        }
        None => {
            if rustyfin_db::repo_user::owner_exists(db).await? {
                StartupOwner::Existing
            } else {
                tracing::error!(
                    "users exist, but none is an enabled administrator, so the server has no owner \
                    and the setup wizard stays closed. To recover, start the server with \
                    RUSTYFIN_API_KEY set, list the users with GET /Users, and make one an \
                    enabled administrator with POST /Users/<Id>/Policy and the body \
                    {{\"IsAdministrator\": true, \"IsDisabled\": false}}, sending the key in the \
                    X-Emby-Token header. After a restart the earliest enabled administrator \
                    becomes the owner."
                );
                StartupOwner::NoEnabledAdmin
            }
        }
    };

    if !config.is_startup_wizard_completed {
        config.is_startup_wizard_completed = true;
        let path = data_dir.join("config.json");
        let saved = match serde_json::to_string_pretty(&*config) {
            Ok(json) => tokio::fs::write(&path, json).await,
            Err(e) => Err(std::io::Error::other(e)),
        };
        if let Err(e) = saved {
            tracing::warn!(
                "could not save the setup wizard state to {}: {}",
                path.display(),
                e
            );
        }
    }
    Ok(owner)
}
