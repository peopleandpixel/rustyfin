use axum::extract::{Path, State};
use axum::routing::{delete, post, put};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_playback::StreamChoice;
use rustyfin_model::auth::Claims;
use rustyfin_model::media_source::MediaSourceResponse;
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
        .route("/Playstate/Pause", post(playstate_pause))
        .route("/Playstate/Unpause", post(playstate_unpause))
        .route("/Playstate/Stop", post(playstate_stop))
        .route("/Users/{userId}/Items/{itemId}/Rating", put(set_rating))
}

/// The remembered tracks a playback report names. Each index is stored with the key of the stream
/// of its kind at that index: in the source `media_source_id` names, else in the first source listed
/// that has one. A negative index names no stream and gets no key, so the sources are loaded only
/// for an index of 0 or more.
async fn reported_choices(
    state: &AppState,
    item_id: Uuid,
    media_source_id: Option<&str>,
    audio: Option<i32>,
    subtitle: Option<i32>,
) -> Result<(Option<StreamChoice>, Option<StreamChoice>), ApiError> {
    let named = media_source_id.and_then(|id| Uuid::parse_str(id).ok());
    let needs_streams =
        audio.is_some_and(|index| index >= 0) || subtitle.is_some_and(|index| index >= 0);
    let sources = if needs_streams {
        rustyfin_db::repo_media_source::get_media_sources(&state.db, &item_id).await?
    } else {
        Vec::new()
    };
    let choose = |kind: &str, index: Option<i32>| {
        index.map(|index| StreamChoice {
            index,
            key: if index < 0 {
                None
            } else {
                stream_key(&sources, named, kind, index)
            },
        })
    };
    Ok((choose("Audio", audio), choose("Subtitle", subtitle)))
}

/// The key of the stream of `kind` ("Audio" or "Subtitle") at `index`: looked up in the source
/// `named` is, when it is one of `sources`, else in the first source that has such a stream.
fn stream_key(
    sources: &[MediaSourceResponse],
    named: Option<Uuid>,
    kind: &str,
    index: i32,
) -> Option<String> {
    let key_at = |source: &MediaSourceResponse| {
        source
            .media_streams
            .iter()
            .find(|stream| stream.index == index && stream.codec_type.eq_ignore_ascii_case(kind))
            .map(|stream| stream.identity().key())
    };
    let named_source = named.and_then(|id| {
        sources
            .iter()
            .find(|source| Uuid::parse_str(&source.id).ok() == Some(id))
    });
    match named_source {
        Some(source) => key_at(source),
        None => sources.iter().find_map(key_at),
    }
}

async fn playing(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<PlaybackStartInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tracing::debug!(
        item = %req.item_id,
        media_source = ?req.media_source_id,
        position = ?req.position_ticks,
        audio = ?req.audio_stream_index,
        subtitle = ?req.subtitle_stream_index,
        "Playback started"
    );
    // Jellyfin writes user data only for an item it finds, and has no now-playing item for one it
    // does not (SessionManager.OnPlaybackStart), so a non-media id stores nothing and reports null.
    let is_media = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, req.item_id)
        .await?
        .is_some();
    if is_media {
        let position = req.position_ticks.unwrap_or(0);
        rustyfin_db::repo_playback::update_playback_position(
            &state.db,
            claims.sub,
            req.item_id,
            position,
        )
        .await?;

        let (audio, subtitle) = reported_choices(
            &state,
            req.item_id,
            req.media_source_id.as_deref(),
            req.audio_stream_index,
            req.subtitle_stream_index,
        )
        .await?;
        rustyfin_db::repo_playback::save_stream_choice(
            &state.db,
            claims.sub,
            req.item_id,
            audio.as_ref(),
            subtitle.as_ref(),
        )
        .await?;
    }

    let now_playing = if is_media {
        serde_json::json!({
            "Id": req.item_id.to_string(),
            "Name": req.item_name.as_deref().unwrap_or(""),
        })
    } else {
        serde_json::Value::Null
    };
    // Broadcast SessionChanged event so clients see the now-playing item update
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "NowPlayingItem": now_playing,
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
    tracing::debug!(
        item = %req.item_id,
        media_source = ?req.media_source_id,
        position = ?req.position_ticks,
        audio = ?req.audio_stream_index,
        subtitle = ?req.subtitle_stream_index,
        "Playback progress"
    );
    // Same as `playing`: a non-media id stores nothing and reports a null now-playing item.
    let is_media = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, req.item_id)
        .await?
        .is_some();
    if is_media {
        if let Some(position) = req.position_ticks {
            rustyfin_db::repo_playback::update_playback_position(
                &state.db,
                claims.sub,
                req.item_id,
                position,
            )
            .await?;
        }

        let (audio, subtitle) = reported_choices(
            &state,
            req.item_id,
            req.media_source_id.as_deref(),
            req.audio_stream_index,
            req.subtitle_stream_index,
        )
        .await?;
        rustyfin_db::repo_playback::save_stream_choice(
            &state.db,
            claims.sub,
            req.item_id,
            audio.as_ref(),
            subtitle.as_ref(),
        )
        .await?;
    }

    let now_playing = if is_media {
        serde_json::json!({
            "Id": req.item_id.to_string(),
            "Name": req.item_name.as_deref().unwrap_or(""),
            "PositionTicks": req.position_ticks,
        })
    } else {
        serde_json::Value::Null
    };
    // Broadcast SessionChanged event with updated playback position
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "NowPlayingItem": now_playing,
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
    tracing::debug!(
        item = %req.item_id,
        media_source = ?req.media_source_id,
        position = ?req.position_ticks,
        audio = ?req.audio_stream_index,
        subtitle = ?req.subtitle_stream_index,
        "Playback stopped"
    );
    // Same as `playing`: a non-media id stores nothing.
    let is_media = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, req.item_id)
        .await?
        .is_some();
    if is_media {
        rustyfin_db::repo_playback::on_playback_stopped(
            &state.db,
            claims.sub,
            req.item_id,
            req.position_ticks,
        )
        .await?;

        let (audio, subtitle) = reported_choices(
            &state,
            req.item_id,
            req.media_source_id.as_deref(),
            req.audio_stream_index,
            req.subtitle_stream_index,
        )
        .await?;
        rustyfin_db::repo_playback::save_stream_choice(
            &state.db,
            claims.sub,
            req.item_id,
            audio.as_ref(),
            subtitle.as_ref(),
        )
        .await?;
    }

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

/// The path's user, when the caller may act for them: that user or an administrator (Jellyfin's
/// `RequestHelpers.GetUserId`). An unknown user is 404.
pub(crate) async fn path_user(
    state: &AppState,
    claims: &Claims,
    user_id: Uuid,
) -> Result<Uuid, ApiError> {
    if user_id != claims.sub && !claims.is_admin {
        return Err(ApiError::Forbidden("User update not allowed.".to_string()));
    }
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db, user_id).await?;
    if user.is_none() {
        return Err(ApiError::NotFound("user not found".into()));
    }
    Ok(user_id)
}

/// The user a query's optional `userId` names, under `path_user`'s rules. None or the nil id means
/// the caller; for the API key (nil `sub`), which has no user of its own, that is `None`.
pub(crate) async fn query_user(
    state: &AppState,
    claims: &Claims,
    user_id: Option<Uuid>,
) -> Result<Option<Uuid>, ApiError> {
    match user_id.filter(|id| !id.is_nil()) {
        Some(id) => path_user(state, claims, id).await.map(Some),
        None if claims.sub.is_nil() => Ok(None),
        None => Ok(Some(claims.sub)),
    }
}

/// A 404 unless `item_id` is a media item, as Jellyfin answers user data on an unknown item.
/// The ids of the genre, studio and year lists name no media item.
pub(crate) async fn require_item(state: &AppState, item_id: Uuid) -> Result<(), ApiError> {
    rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await?
        .map(|_| ())
        .ok_or_else(|| ApiError::NotFound("item not found".to_string()))
}

async fn mark_played(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    let data = rustyfin_db::repo_playback::mark_played(&state.db, user_id, item_id).await?;
    Ok(Json(data))
}

async fn mark_unplayed(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    let data = rustyfin_db::repo_playback::mark_unplayed(&state.db, user_id, item_id).await?;
    Ok(Json(data))
}

async fn set_favorite(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    let data = rustyfin_db::repo_playback::set_favorite(&state.db, user_id, item_id, true).await?;
    Ok(Json(data))
}

async fn unset_favorite(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    let data = rustyfin_db::repo_playback::set_favorite(&state.db, user_id, item_id, false).await?;
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
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<SetRatingRequest>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    let data = rustyfin_db::repo_playback::set_user_rating(
        &state.db,
        user_id,
        item_id,
        req.likes,
        req.user_rating,
    )
    .await?;
    tracing::info!(
        "set rating for item {} by user {}: likes={:?}, rating={:?}",
        item_id,
        user_id,
        req.likes,
        req.user_rating
    );
    Ok(Json(data))
}

/// POST /Playstate/Pause – Report that playback has been paused
async fn playstate_pause(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(_req): Json<PlaybackProgressInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "IsPaused": true,
            }),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// POST /Playstate/Unpause – Report that playback has been unpaused
async fn playstate_unpause(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(_req): Json<PlaybackProgressInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
    broadcast_event(
        &state,
        WsEvent::SessionChanged {
            data: serde_json::json!({
                "UserId": claims.sub.to_string(),
                "IsPaused": false,
            }),
        },
    )
    .await;
    Ok(Json(serde_json::json!({})))
}

/// POST /Playstate/Stop – Report that playback has been stopped
async fn playstate_stop(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(_req): Json<PlaybackStopInfo>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
