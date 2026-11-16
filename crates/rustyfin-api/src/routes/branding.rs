//! Jellyfin's anonymous branding endpoints, which Jellyfin Web loads before sign-in. They answer
//! the named configuration `branding` (see `named_config`), which administrators save through
//! `POST /System/Configuration/branding`.

use axum::extract::State;
use axum::http::{StatusCode, header};
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
        .route("/Branding/Splashscreen", get(get_splashscreen))
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

/// The splash screen files, in the order Jellyfin's GetSplashscreen looks for them: an uploaded
/// one (`splashscreen-upload.*`), then the generated `splashscreen.png`, both in the data dir.
const SPLASHSCREEN_FILES: [(&str, &str); 5] = [
    ("splashscreen-upload.png", "image/png"),
    ("splashscreen-upload.jpg", "image/jpeg"),
    ("splashscreen-upload.jpeg", "image/jpeg"),
    ("splashscreen-upload.webp", "image/webp"),
    ("splashscreen.png", "image/png"),
];

/// GET /Branding/Splashscreen: the server's splash screen image, or 404 when it has none, as
/// Jellyfin answers. Clients that find none show their own (Swiftfin, Android TV).
async fn get_splashscreen(State(state): State<AppState>) -> axum::response::Response {
    for (name, content_type) in SPLASHSCREEN_FILES {
        if let Ok(bytes) = tokio::fs::read(state.data_dir.join(name)).await {
            return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
        }
    }
    StatusCode::NOT_FOUND.into_response()
}
