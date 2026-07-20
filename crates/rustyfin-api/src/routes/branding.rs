use axum::http::header;
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct BrandingOptions {
    login_disclaimer: String,
    custom_css: String,
    splashscreen_enabled: bool,
}

async fn get_branding() -> Json<BrandingOptions> {
    Json(BrandingOptions {
        login_disclaimer: String::new(),
        custom_css: String::new(),
        splashscreen_enabled: false,
    })
}

async fn get_branding_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], String::new())
}
