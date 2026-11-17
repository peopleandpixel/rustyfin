use crate::query::Query;
use axum::extract::{Path, State};
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

/// A field the client leaves out keeps its default, as Jellyfin's model binding does, so a partial
/// body is still read back with its `CustomPrefs`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct DisplayPreferencesDto {
    id: String,
    view_type: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    client: String,
    /// Custom prefs map (string -> string or null). Jellyfin web uses this for home sections. A
    /// null value is kept and given back as null, as Jellyfin 12.4 stores it.
    #[serde(deserialize_with = "custom_prefs_as_strings")]
    custom_prefs: HashMap<String, Option<String>>,
    remember_indexing: bool,
    remember_sorting: bool,
    primary_image_height: i32,
    primary_image_width: i32,
    show_backdrop: bool,
    show_sidebar: bool,
    scroll_direction: String,
}

/// Reads `CustomPrefs` leniently: null stays null, a number or boolean becomes its text, and a
/// missing or null map is empty.
fn custom_prefs_as_strings<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let prefs = Option::<serde_json::Map<String, serde_json::Value>>::deserialize(deserializer)?;
    Ok(prefs
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) => Some(s),
                other => Some(other.to_string()),
            };
            (key, value)
        })
        .collect())
}

impl Default for DisplayPreferencesDto {
    fn default() -> Self {
        Self::default_for(String::new(), "emby".to_string())
    }
}

impl DisplayPreferencesDto {
    fn default_for(id: String, client: String) -> Self {
        Self {
            id,
            view_type: None,
            sort_by: Some("SortName".to_string()),
            sort_order: Some("Ascending".to_string()),
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
    let stored =
        rustyfin_db::repo_user::get_display_preference(&state.db_ro, claims.sub, &key, &client)
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
    Query(q): Query<DisplayPrefsQuery>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::http::StatusCode, ApiError> {
    // Persist to database, per client as the GET reads it. As in Jellyfin 12.4
    // (DisplayPreferencesManager), the body replaces what was stored: its CustomPrefs are the
    // whole set (a key left out is gone, a null value is kept), and a field left out takes its
    // default. Clients post back the full set they read.
    let client = q.client.unwrap_or_else(|| "emby".to_string());
    let prefs: DisplayPreferencesDto =
        serde_json::from_value(body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let json_str = serde_json::to_string(&prefs).map_err(|e| ApiError::Internal(e.to_string()))?;

    rustyfin_db::repo_user::update_display_preference(
        &state.db, claims.sub, &key, &client, &json_str,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::DisplayPreferencesDto;
    use serde_json::json;

    /// What the POST stores and the GET then reads, as one round trip through the DTO
    fn stored(body: serde_json::Value) -> serde_json::Value {
        let prefs: DisplayPreferencesDto = serde_json::from_value(body).unwrap();
        serde_json::to_value(prefs).unwrap()
    }

    #[test]
    fn a_post_replaces_custom_prefs_as_jellyfin_12_4_does() {
        // The posted set is the whole set; a null value is kept, other values become text
        let got = stored(json!({"CustomPrefs": {"a": "9", "b": null, "d": true}}));
        assert_eq!(
            got["CustomPrefs"],
            json!({"a": "9", "b": null, "d": "true"})
        );
        // No CustomPrefs: an empty set; a field left out takes its default
        let got = stored(json!({"SortBy": "DateCreated"}));
        assert_eq!(got["CustomPrefs"], json!({}));
        assert_eq!(got["SortBy"], "DateCreated");
        assert_eq!(got["ScrollDirection"], "Horizontal");
    }
}
