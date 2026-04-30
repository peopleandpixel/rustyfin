use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use axum::extract::Path;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Plugins", get(get_plugins))
        .route("/Plugins/{id}/Configuration", get(get_plugin_config).post(set_plugin_config))
}

async fn get_plugins(
    State(state): State<AppState>,
) -> Json<Vec<rustyfin_model::plugin::PluginInfo>> {
    let plugins = state.plugin_manager.list_plugins().await;
    Json(plugins)
}

async fn get_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rustyfin_model::plugin::PluginConfiguration>, crate::error::ApiError> {
    match state.plugin_manager.get_configuration(&id).await {
        Some(config) => Ok(Json(config)),
        None => Err(crate::error::ApiError::NotFound("plugin not found".into())),
    }
}

async fn set_plugin_config(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(config): Json<serde_json::Value>,
) -> Result<(), crate::error::ApiError> {
    state
        .plugin_manager
        .set_configuration(&id, config)
        .await
        .map_err(|e| crate::error::ApiError::NotFound(e))
}
