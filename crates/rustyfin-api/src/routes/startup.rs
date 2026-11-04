use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_user::OwnerClaim;
use rustyfin_model::auth::Claims;
use rustyfin_model::setup::{
    StartupConfiguration, StartupConfigurationUpdate, StartupUser, StartupUserCreate,
};
use rustyfin_model::user::User;

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
        // Before an owner exists, the wizard starts empty, even if other administrators exist.
        return Ok(Json(StartupUser {
            name: String::new(),
            password: String::new(),
        }));
    };
    // The guard already admits only administrators once an owner exists. This check covers a race.
    if !claims.is_some_and(|Extension(c)| c.is_admin) {
        return Err(ApiError::Unauthorized);
    }
    Ok(Json(StartupUser {
        name: owner.name,
        password: String::new(),
    }))
}

/// Returns the account named `admin` if its password is still the default `admin`. The owner claim
/// disables such an account. The name `admin` itself is not a leftover, because the claim takes over
/// that account. Any error counts as no leftover.
async fn default_admin_leftover(state: &AppState, name: &str) -> Option<User> {
    if name == "admin" {
        return None;
    }
    let admin = rustyfin_db::repo_user::get_user_by_name(&state.db, "admin")
        .await
        .ok()
        .flatten()?;
    let has_default_password = matches!(
        rustyfin_auth::verify_password("admin", &admin.password_hash),
        Ok(true)
    );
    has_default_password.then_some(admin)
}

async fn create_startup_user(
    State(state): State<AppState>,
    Json(input): Json<StartupUserCreate>,
) -> Result<StatusCode, ApiError> {
    if rustyfin_db::repo_user::owner_exists(&state.db).await? {
        return Err(ApiError::Forbidden("A server owner already exists.".into()));
    }
    // The name is stored exactly as sent. Only this check trims it.
    if input.name.trim().is_empty() {
        return Err(ApiError::BadRequest("Name is required".into()));
    }

    let hash = rustyfin_auth::hash_password(&input.password)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let leftover = default_admin_leftover(&state, &input.name).await;

    let claim =
        rustyfin_db::repo_user::claim_owner(&state.db, &input.name, &hash, leftover.as_ref())
            .await?;
    match claim {
        OwnerClaim::AlreadyOwned => {
            Err(ApiError::Forbidden("A server owner already exists.".into()))
        }
        OwnerClaim::Claimed { leftover_disabled } => {
            if leftover_disabled {
                tracing::warn!(
                    "disabled the leftover account 'admin' whose password was still the default 'admin'; the server owner is now '{}'",
                    input.name
                );
            }
            Ok(StatusCode::NO_CONTENT)
        }
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
