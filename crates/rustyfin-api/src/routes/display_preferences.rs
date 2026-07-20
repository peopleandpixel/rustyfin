use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/DisplayPreferences/{key}",
            get(get_display_preferences).post(update_display_preferences),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayPrefsQuery {
    #[serde(rename = "userId")]
    user_id: Option<String>,
    client: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DisplayPreferencesDto {
    id: String,
    view_type: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    client: String,
    /// Custom prefs map (string -> string). Jellyfin web uses this for home sections.
    custom_prefs: HashMap<String, String>,
    remember_indexing: bool,
    remember_sorting: bool,
    primary_image_height: i32,
    primary_image_width: i32,
    show_backdrop: bool,
    show_sidebar: bool,
    scroll_direction: String,
}

impl DisplayPreferencesDto {
    fn default_for(id: String, client: String) -> Self {
        Self {
            id,
            view_type: None,
            sort_by: None,
            sort_order: None,
            client,
            custom_prefs: HashMap::new(),
            remember_indexing: false,
            remember_sorting: false,
            primary_image_height: 0,
            primary_image_width: 0,
            show_backdrop: true,
            show_sidebar: false,
            scroll_direction: "Horizontal".to_string(),
        }
    }
}

async fn get_display_preferences(
    Path(key): Path<String>,
    Query(q): Query<DisplayPrefsQuery>,
) -> Result<Json<DisplayPreferencesDto>, ApiError> {
    let client = q.client.unwrap_or_else(|| "emby".to_string());
    let id = q.user_id.unwrap_or(key);
    Ok(Json(DisplayPreferencesDto::default_for(id, client)))
}

async fn update_display_preferences(
    Path(_key): Path<String>,
    Query(_q): Query<DisplayPrefsQuery>,
    Json(_body): Json<serde_json::Value>,
) -> Result<axum::http::StatusCode, ApiError> {
    // Persistenz noch nicht implementiert — wir akzeptieren Updates idempotent.
    Ok(axum::http::StatusCode::NO_CONTENT)
}
