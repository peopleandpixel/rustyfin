use axum::extract::Path;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Plugins", get(get_plugins))
        .route(
            "/Plugins/{id}/Configuration",
            get(get_plugin_config).post(set_plugin_config),
        )
        .route(
            "/Plugins/{id}/Priority",
            get(get_plugin_priority).put(set_plugin_priority),
        )
}

async fn get_plugins(
    State(state): State<AppState>,
) -> Json<Vec<rustyfin_model::plugin::PluginInfo>> {
    let plugins = state.plugin_manager.list_plugins().await;
    Json(plugins)
}

/// The provider plugins whose API key is write-only: GET shows only whether it is set.
fn is_provider(id: &str) -> bool {
    matches!(id, "tmdb" | "omdb" | "fanart")
}

async fn get_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rustyfin_model::plugin::PluginConfiguration>, crate::error::ApiError> {
    match state.plugin_manager.get_configuration(&id).await {
        Some(mut config) => {
            if is_provider(&id) {
                if !config.values.is_object() {
                    config.values = serde_json::json!({});
                }
                if let Some(obj) = config.values.as_object_mut() {
                    let set = obj
                        .remove("api_key")
                        .and_then(|k| k.as_str().map(|k| !k.trim().is_empty()))
                        .unwrap_or(false);
                    obj.insert("api_key_set".to_string(), serde_json::json!(set));
                }
            }
            Ok(Json(config))
        }
        None => Err(crate::error::ApiError::NotFound("plugin not found".into())),
    }
}

async fn set_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut config): Json<serde_json::Value>,
) -> Result<(), crate::error::ApiError> {
    use rustyfin_core::plugin_manager::SetConfigError;
    // api_key_set is reported by GET, never stored.
    if is_provider(&id)
        && let Some(obj) = config.as_object_mut()
    {
        obj.remove("api_key_set");
    }
    state
        .plugin_manager
        .set_configuration(&id, config)
        .await
        .map_err(|e| match e {
            SetConfigError::NotFound => crate::error::ApiError::NotFound("plugin not found".into()),
            SetConfigError::Rejected(msg) => crate::error::ApiError::BadRequest(msg),
            SetConfigError::Persist(msg) => crate::error::ApiError::Internal(msg),
        })
}

/// Get plugin priority
async fn get_plugin_priority(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let priority = state.plugin_manager.get_plugin_priority(&id).await;
    Json(serde_json::json!({ "priority": priority }))
}

/// Set plugin priority (lower number = higher priority)
async fn set_plugin_priority(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<(), crate::error::ApiError> {
    let priority: i32 = body
        .get("priority")
        .and_then(|v| v.as_i64())
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| {
            crate::error::ApiError::BadRequest("missing or invalid 'priority' field".into())
        })?;

    state
        .plugin_manager
        .set_plugin_priority(&id, priority)
        .await
        .map_err(crate::error::ApiError::NotFound)
}
