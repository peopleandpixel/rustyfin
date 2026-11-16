//! Jellyfin's anonymous branding endpoints, which Jellyfin Web loads before sign-in. Rustyfin has
//! no branding settings, so they answer Jellyfin's defaults.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Branding/Configuration", get(get_branding))
        .route("/Branding/Css", get(get_branding_css))
        .route("/Branding/Css.css", get(get_branding_css))
        .route("/Branding/Splashscreen", get(get_splashscreen))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct BrandingOptions {
    login_disclaimer: String,
    custom_css: String,
    splashscreen_enabled: bool,
}

/// GET /Branding/Configuration
async fn get_branding() -> Json<BrandingOptions> {
    Json(BrandingOptions {
        login_disclaimer: String::new(),
        custom_css: String::new(),
        splashscreen_enabled: false,
    })
}

/// GET /Branding/Css and /Branding/Css.css
async fn get_branding_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        String::new(),
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
