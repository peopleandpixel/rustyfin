use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Branding/Configuration", get(get_branding))
        .route("/Branding/Css", get(get_branding_css))
        .route("/Branding/Css.css", get(get_branding_css))
        .route("/Branding/Splashscreen", get(get_splashscreen).post(set_splashscreen).delete(delete_splashscreen))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct BrandingOptions {
    login_disclaimer: String,
    custom_css: String,
    splashscreen_enabled: bool,
}

async fn get_branding(State(state): State<AppState>) -> Json<BrandingOptions> {
    // Check for splashscreen in config
    let splashscreen_enabled = state
        .cache
        .branding
        .get("splashscreen_enabled")
        .map(|v| v == "true")
        .unwrap_or(false);

    Json(BrandingOptions {
        login_disclaimer: String::new(),
        custom_css: String::new(),
        splashscreen_enabled,
    })
}

async fn get_branding_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], String::new())
}

/// GET /Branding/Splashscreen
async fn get_splashscreen(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let enabled = state.cache.branding.get("splashscreen_enabled").map(|v| v == "true").unwrap_or(false);
    let image = state.cache.branding.get("splashscreen_image").cloned();

    Ok(Json(serde_json::json!({
        "Enabled": enabled,
        "Image": image
    })))
}

/// POST /Branding/Splashscreen
async fn set_splashscreen(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::http::StatusCode, ApiError> {
    let enabled = body.get("Enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let image = body.get("Image").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Store in cache and persist to config file
    state.cache.branding.insert("splashscreen_enabled".to_string(), enabled.to_string());
    if let Some(img) = image {
        state.cache.branding.insert("splashscreen_image".to_string(), img);
    }

    // Persist to config
    let config_path = state.data_dir.join("config.json");
    let config = state.config.read().await;
    let json = serde_json::to_string_pretty(&*config)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    drop(config);
    tokio::fs::write(&config_path, &json)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// DELETE /Branding/Splashscreen
async fn delete_splashscreen(State(state): State<AppState>) -> Result<axum::http::StatusCode, ApiError> {
    state.cache.branding.remove("splashscreen_enabled");
    state.cache.branding.remove("splashscreen_image");
    Ok(axum::http::StatusCode::NO_CONTENT)
}
