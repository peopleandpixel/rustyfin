//! Jellyfin's anonymous branding endpoints, which Jellyfin Web loads before sign-in. They answer
//! the named configuration `branding` (see `named_config`), which administrators save through
//! `POST /System/Configuration/branding`.

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;

use crate::named_config::{BRANDING_KEY, load};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Branding/Configuration", get(get_branding))
        .route("/Branding/Css", get(get_branding_css))
        .route("/Branding/Css.css", get(get_branding_css))
}

/// GET /Branding/Configuration - Jellyfin's `BrandingOptionsDto`
async fn get_branding(State(state): State<AppState>) -> Json<Value> {
    let options = load(&state.data_dir, BRANDING_KEY, false).await;
    let text = |field: &str| options.get(field).and_then(Value::as_str).unwrap_or("");
    Json(serde_json::json!({
        "LoginDisclaimer": text("LoginDisclaimer"),
        "CustomCss": text("CustomCss"),
        "SplashscreenEnabled": options.get("SplashscreenEnabled").and_then(Value::as_bool).unwrap_or(false),
    }))
}

/// GET /Branding/Css and /Branding/Css.css - the custom CSS
async fn get_branding_css(State(state): State<AppState>) -> impl IntoResponse {
    let options = load(&state.data_dir, BRANDING_KEY, false).await;
    let css = options
        .get("CustomCss")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        css,
    )
}
