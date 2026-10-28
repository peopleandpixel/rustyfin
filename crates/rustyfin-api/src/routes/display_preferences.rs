use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
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

#[derive(Debug, Serialize, Deserialize)]
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
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(q): Query<DisplayPrefsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<DisplayPreferencesDto>, ApiError> {
    let client = q.client.unwrap_or_else(|| "emby".to_string());
    let user_id_str = q.user_id.unwrap_or_else(|| claims.sub.to_string());
    let id = format!("{}-{}", user_id_str, key);

    // Try to load from database
    let stored = rustyfin_db::repo_user::get_display_preference(&state.db_ro, claims.sub, &key)
        .await
        .ok()
        .flatten();

    if let Some(stored_value) = &stored {
        // Parse stored JSON and merge with defaults
        if let Ok(mut prefs) = serde_json::from_str::<DisplayPreferencesDto>(stored_value) {
            prefs.client = client;
            prefs.id = id;
            return Ok(Json(prefs));
        }
    }

    Ok(Json(DisplayPreferencesDto::default_for(id, client)))
}

async fn update_display_preferences(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Query(_q): Query<DisplayPrefsQuery>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::http::StatusCode, ApiError> {
    // Persist to database
    let json_str = serde_json::to_string(&body).map_err(|e| ApiError::Internal(e.to_string()))?;

    rustyfin_db::repo_user::update_display_preference(&state.db, claims.sub, &key, &json_str)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}
