use axum::extract::{Path, State};
use axum::routing::{delete, post, put};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::playback::{
    LastActivityDateRequest, PinEpisodeRequest, PlaybackProgressInfo, PlaybackStartInfo,
    PlaybackStopInfo, SetRatingRequest, UserItemDataDto,
};
use rustyfin_model::ws_event::WsEvent;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::websocket::broadcast_event;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Sessions/Playing", post(playing))
        .route("/Sessions/Playing/Progress", post(playing_progress))
        .route("/Sessions/Playing/Stopped", post(playing_stopped))
        .route("/Users/{userId}/PlayedItems/{itemId}", post(mark_played))
        .route(
            "/Users/{userId}/PlayedItems/{itemId}",
            delete(mark_unplayed),
        )
        .route("/Users/{userId}/FavoriteItems/{itemId}", post(set_favorite))
        .route(
            "/Users/{userId}/FavoriteItems/{itemId}",
            delete(unset_favorite),
        )
        // Playstate Controller endpoints
        .route("/Playstate/PinEpisode", post(pin_episode))
        .route("/Playstate/LastActivityDate", post(last_activity_date))
        .route("/Users/{userId}/Items/{itemId}/Rating", put(set_rating))
}

async fn playing(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackStartInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let position = req.position_ticks.unwrap_or(0);
    rustyfin_db::repo_playback::update_playback_position(
        &state.db,
        claims.sub,
        req.item_id,
        position,
    )
    .await?;

    // Broadcast SessionChanged event so clients see the now-playing item update
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "NowPlayingItem": {
                    "Id": req.item_id.to_string(),
                    "Name": req.item_name.as_deref().unwrap_or(""),
                }
            }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({})))
}

async fn playing_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackProgressInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(position) = req.position_ticks {
        rustyfin_db::repo_playback::update_playback_position(
            &state.db,
            claims.sub,
            req.item_id,
            position,
        )
        .await?;
    }

    // Broadcast SessionChanged event with updated playback position
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "NowPlayingItem": {
                    "Id": req.item_id.to_string(),
                    "Name": req.item_name.as_deref().unwrap_or(""),
                    "PositionTicks": req.position_ticks,
                },
                "IsPaused": req.is_paused.unwrap_or(false),
            }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({})))
}

async fn playing_stopped(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackStopInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::on_playback_stopped(
        &state.db,
        claims.sub,
        req.item_id,
        req.position_ticks,
    )
    .await?;

    // Broadcast SessionChanged event to clear now-playing item
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "NowPlayingItem": null,
            }),
        },
    )
    .await;

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
    let data =
        rustyfin_db::repo_playback::set_favorite(&state.db, claims.sub, item_id, true).await?;
    Ok(Json(data))
}

async fn unset_favorite(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data =
        rustyfin_db::repo_playback::set_favorite(&state.db, claims.sub, item_id, false).await?;
    Ok(Json(data))
}

/// POST /Playstate/PinEpisode – Pin current episode for a series
async fn pin_episode(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PinEpisodeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::pin_episode(&state.db, claims.sub, req.item_id).await?;
    tracing::info!("pinned episode {} for user {}", req.item_id, claims.sub);
    Ok(Json(serde_json::json!({})))
}

/// POST /Playstate/LastActivityDate – Update last activity date for items
async fn last_activity_date(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<LastActivityDateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    rustyfin_db::repo_playback::update_last_activity_date(&state.db, claims.sub, &req.item_ids)
        .await?;
    tracing::info!(
        "updated last activity date for {} items for user {}",
        req.item_ids.len(),
        claims.sub
    );
    Ok(Json(serde_json::json!({})))
}

/// PUT /Users/{userId}/Items/{itemId}/Rating – Set user rating for an item
async fn set_rating(
    State(state): State<AppState>,
    Path((_user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SetRatingRequest>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::set_user_rating(
        &state.db,
        claims.sub,
        item_id,
        req.likes,
        req.user_rating,
    )
    .await?;
    tracing::info!(
        "set rating for item {} by user {}: likes={:?}, rating={:?}",
        item_id,
        claims.sub,
        req.likes,
        req.user_rating
    );
    Ok(Json(data))
}
