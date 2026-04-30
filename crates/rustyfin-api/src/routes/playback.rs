use axum::extract::{Path, State};
use axum::routing::{delete, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::playback::{PlaybackProgressInfo, PlaybackStartInfo, PlaybackStopInfo, UserItemDataDto};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Sessions/Playing", post(playing))
        .route("/Sessions/Playing/Progress", post(playing_progress))
        .route("/Sessions/Playing/Stopped", post(playing_stopped))
        .route("/Users/{userId}/PlayedItems/{itemId}", post(mark_played))
        .route("/Users/{userId}/PlayedItems/{itemId}", delete(mark_unplayed))
        .route("/Users/{userId}/FavoriteItems/{itemId}", post(set_favorite))
        .route("/Users/{userId}/FavoriteItems/{itemId}", delete(unset_favorite))
}

async fn playing(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackStartInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let position = req.position_ticks.unwrap_or(0);
    rustyfin_db::repo_playback::update_playback_position(&state.db, claims.sub, req.item_id, position).await?;
    Ok(Json(serde_json::json!({})))
}

async fn playing_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackProgressInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(position) = req.position_ticks {
        rustyfin_db::repo_playback::update_playback_position(&state.db, claims.sub, req.item_id, position).await?;
    }
    Ok(Json(serde_json::json!({})))
}

async fn playing_stopped(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackStopInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::on_playback_stopped(&state.db, claims.sub, req.item_id, req.position_ticks).await?;
    Ok(Json(serde_json::json!({})))
}

async fn mark_played(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::mark_played(&state.db, claims.sub, item_id).await?;
    Ok(Json(data))
}

async fn mark_unplayed(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::mark_unplayed(&state.db, claims.sub, item_id).await?;
    Ok(Json(data))
}

async fn set_favorite(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::set_favorite(&state.db, claims.sub, item_id, true).await?;
    Ok(Json(data))
}

async fn unset_favorite(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::set_favorite(&state.db, claims.sub, item_id, false).await?;
    Ok(Json(data))
}
