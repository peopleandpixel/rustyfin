//! Older Emby-style configuration routes, not in Jellyfin 12.4. `/Configuration/MediaEncoding`
//! and `/Configuration/Networking` read and save the same stored settings as Jellyfin's named
//! configurations `encoding` and `network` (`/System/Configuration/{key}`), so there is one source
//! for each; the others answer fixed defaults.
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use serde_json::Value;

use rustyfin_model::configuration::{
    DisplayConfiguration, ParentalControlsConfiguration, ServerRecordingConfiguration,
};

use crate::error::ApiError;
use crate::named_config::NETWORK_KEY;
use crate::routes::system::{encoding_json, save_encoding_configuration, save_named};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Configuration/MediaEncoding", get(get_media_encoding))
        .route("/Configuration/Networking", get(get_networking))
        .route("/Configuration/ServerRecording", get(get_server_recording))
        .route("/Configuration/Display", get(get_display))
        .route(
            "/Configuration/ParentalControls",
            get(get_parental_controls),
        )
}

/// Administrator-only configuration routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Configuration/MediaEncoding", put(update_media_encoding))
        .route("/Configuration/Networking", put(update_networking))
}

/// GET /Configuration/MediaEncoding - the named configuration `encoding`
async fn get_media_encoding(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(encoding_json(&state, claims.is_admin).await?))
}

/// PUT /Configuration/MediaEncoding - saves the named configuration `encoding` (204)
async fn update_media_encoding(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    save_encoding_configuration(&state, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /Configuration/Networking - the named configuration `network`
async fn get_networking(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Json<Value> {
    let options = crate::named_config::load(&state.data_dir, NETWORK_KEY, claims.is_admin).await;
    Json(Value::Object(options))
}

/// PUT /Configuration/Networking - saves the named configuration `network` (204)
async fn update_networking(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    save_named(&state, NETWORK_KEY, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_server_recording() -> Json<ServerRecordingConfiguration> {
    Json(ServerRecordingConfiguration::default())
}

async fn get_display() -> Json<DisplayConfiguration> {
    Json(DisplayConfiguration::default())
}

async fn get_parental_controls() -> Json<ParentalControlsConfiguration> {
    Json(ParentalControlsConfiguration::default())
}
