use crate::query::Query;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_playback::StreamChoice;
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::media_source::MediaSourceResponse;
use rustyfin_model::playback::{
    LastActivityDateRequest, PinEpisodeRequest, PlaybackProgressInfo, PlaybackStartInfo,
    PlaybackStopInfo, SetRatingRequest, UpdateUserItemDataDto, UserItemDataDto,
};
use rustyfin_model::ws_event::WsEvent;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::websocket::broadcast_event;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Sessions/Playing", post(playing))
        .route("/Sessions/Playing/Progress", post(playing_progress))
        .route("/Sessions/Playing/Stopped", post(playing_stopped))
        // Playstate Controller's query-string forms; the legacy /Users/{userId} ones act for the caller.
        .route("/PlayingItems/{itemId}", post(playing_item_start))
        .route("/PlayingItems/{itemId}", delete(playing_item_stop))
        .route(
            "/PlayingItems/{itemId}/Progress",
            post(playing_item_progress),
        )
        .route(
            "/Users/{userId}/PlayingItems/{itemId}",
            post(legacy_playing_item_start),
        )
        .route(
            "/Users/{userId}/PlayingItems/{itemId}",
            delete(legacy_playing_item_stop),
        )
        .route(
            "/Users/{userId}/PlayingItems/{itemId}/Progress",
            post(legacy_playing_item_progress),
        )
        // Jellyfin 10.9+ forms, for the caller or the `userId` the query names.
        .route(
            "/UserPlayedItems/{itemId}",
            post(user_mark_played).delete(user_mark_unplayed),
        )
        .route(
            "/UserFavoriteItems/{itemId}",
            post(user_set_favorite).delete(user_unset_favorite),
        )
        .route(
            "/UserItems/{itemId}/UserData",
            get(user_get_user_data).post(user_update_user_data),
        )
        .route(
            "/UserItems/{itemId}/Rating",
            post(user_set_likes).delete(user_clear_rating),
        )
        .route(
            "/Users/{userId}/Items/{itemId}/UserData",
            get(legacy_get_user_data).post(legacy_update_user_data),
        )
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
        .route(
            "/Users/{userId}/Items/{itemId}/Rating",
            put(set_rating)
                .post(legacy_set_likes)
                .delete(legacy_clear_rating),
        )
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
    let mut sources = if needs_streams {
        rustyfin_db::repo_media_source::get_media_sources(&state.db, &item_id).await?
    } else {
        Vec::new()
    };
    // The client's indexes are Jellyfin's numbering, external subtitle files first
    for source in sources.iter_mut() {
        crate::routes::subtitle::with_external_streams(source).await;
    }
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

/// The runtime the play state rules use for a report: the `run_time_ticks` of the version
/// `media_source_id` names when it is a known version with a runtime above 0, else the item's own.
async fn reported_runtime(
    state: &AppState,
    item: &MediaItem,
    media_source_id: Option<&str>,
) -> Result<Option<i64>, ApiError> {
    let Some(wanted) = media_source_id.and_then(|id| Uuid::parse_str(id).ok()) else {
        return Ok(item.runtime_ticks);
    };
    let sources = rustyfin_db::repo_media_source::get_media_sources(&state.db, &item.id).await?;
    let version = sources
        .iter()
        .find(|source| Uuid::parse_str(&source.id).ok() == Some(wanted))
        .and_then(|source| source.run_time_ticks)
        .filter(|ticks| *ticks > 0);
    Ok(version.or(item.runtime_ticks))
}

/// The token of the request and the device its header names, for finding its session.
type Token = crate::sessions::Caller;

/// The item a playback report names, or `None` for an id that is no media item or one `user`'s
/// limits hide. Such a report stores nothing and sets no now-playing item, so a limited user can't
/// read a hidden item's name back from their session.
async fn reported_item(
    state: &AppState,
    user: Uuid,
    item_id: Uuid,
) -> Result<Option<rustyfin_model::media::MediaItem>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id).await?;
    if item.is_some() && !user.is_nil() {
        let policy = crate::parental::user_policy(state, user).await?;
        if rustyfin_db::repo_parental::is_item_hidden(&state.db_ro, &policy, item_id).await? {
            return Ok(None);
        }
    }
    Ok(item)
}

/// The caller's session, from the request's token and stated device.
async fn caller_session(state: &AppState, token: &Token) -> Result<Option<String>, ApiError> {
    token.session(state).await
}

async fn playing(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Token,
    Json(req): Json<PlaybackStartInfo>,
) -> Result<StatusCode, ApiError> {
    report_start(state, claims, token, req).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Records a playback start for the caller (`POST /Sessions/Playing` and `POST /PlayingItems/{id}`).
async fn report_start(
    state: AppState,
    claims: Claims,
    token: Token,
    req: PlaybackStartInfo,
) -> Result<(), ApiError> {
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
    let media_type = reported_item(&state, claims.sub, req.item_id)
        .await?
        .map(|item| item.media_type);
    let is_media = media_type.is_some();
    if let Some(media_type) = media_type {
        let position = req.position_ticks.unwrap_or(0);
        rustyfin_db::repo_playback::record_playback_start(
            &state.db,
            claims.sub,
            req.item_id,
            position,
            media_type,
        )
        .await?;
        crate::user_data_changes::changed(&state, claims.sub, req.item_id);

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

    // The session's now-playing item and play state
    if is_media && let Some(session) = caller_session(&state, &token).await? {
        let report = crate::sessions::Report {
            user_id: claims.sub,
            item_id: req.item_id,
            position_ticks: req.position_ticks,
            is_paused: Some(false),
            media_source_id: req.media_source_id.as_deref(),
            audio_stream_index: req.audio_stream_index,
            subtitle_stream_index: req.subtitle_stream_index,
            fields: &req.play_state,
        };
        crate::sessions::playing(&state, &session, report, true).await;
    }

    Ok(())
}

async fn playing_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Token,
    Json(req): Json<PlaybackProgressInfo>,
) -> Result<StatusCode, ApiError> {
    report_progress(state, claims, token, req).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Records a playback progress report for the caller (`/Sessions/Playing/Progress` and
/// `POST /PlayingItems/{id}/Progress`).
async fn report_progress(
    state: AppState,
    claims: Claims,
    token: Token,
    req: PlaybackProgressInfo,
) -> Result<(), ApiError> {
    tracing::debug!(
        item = %req.item_id,
        media_source = ?req.media_source_id,
        position = ?req.position_ticks,
        audio = ?req.audio_stream_index,
        subtitle = ?req.subtitle_stream_index,
        "Playback progress"
    );
    // Same as `playing`: a non-media id stores nothing and reports a null now-playing item.
    let item = reported_item(&state, claims.sub, req.item_id).await?;
    let is_media = item.is_some();
    let runtime = match &item {
        Some(item) => reported_runtime(&state, item, req.media_source_id.as_deref()).await?,
        None => None,
    };
    if let Some(item) = &item {
        if let Some(position) = req.position_ticks {
            let rules = state.config.read().await.resume_rules();
            rustyfin_db::repo_playback::record_playback_progress(
                &state.db,
                claims.sub,
                req.item_id,
                item.media_type,
                position,
                runtime,
                &rules,
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

    if is_media && let Some(session) = caller_session(&state, &token).await? {
        let report = crate::sessions::Report {
            user_id: claims.sub,
            item_id: req.item_id,
            position_ticks: req.position_ticks,
            is_paused: req.is_paused,
            media_source_id: req.media_source_id.as_deref(),
            audio_stream_index: req.audio_stream_index,
            subtitle_stream_index: req.subtitle_stream_index,
            fields: &req.play_state,
        };
        crate::sessions::playing(&state, &session, report, false).await;
    }

    Ok(())
}

async fn playing_stopped(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Token,
    Json(req): Json<PlaybackStopInfo>,
) -> Result<StatusCode, ApiError> {
    report_stop(state, claims, token, req).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Records a playback stop for the caller (`/Sessions/Playing/Stopped` and
/// `DELETE /PlayingItems/{id}`).
async fn report_stop(
    state: AppState,
    claims: Claims,
    token: Token,
    req: PlaybackStopInfo,
) -> Result<(), ApiError> {
    tracing::debug!(
        item = %req.item_id,
        media_source = ?req.media_source_id,
        position = ?req.position_ticks,
        audio = ?req.audio_stream_index,
        subtitle = ?req.subtitle_stream_index,
        "Playback stopped"
    );
    record_stop(&state, claims.sub, &req).await?;
    // The playback's transcodes end with it, as in Jellyfin, rather than at the stall sweep
    if let Some(psid) = req.play_state.play_session_id.as_deref() {
        let stopped = state.transcode_manager.stop_reported_playback(psid).await;
        if stopped > 0 {
            tracing::debug!(
                play_session = psid,
                stopped,
                "stopped the playback's transcodes"
            );
        }
    }
    if let Some(session) = caller_session(&state, &token).await? {
        crate::sessions::stopped(&state, &session).await;
    }

    Ok(())
}

/// The watch data of a stop: `user`'s position, played state and track choice for the item, by
/// Jellyfin's play state rules. Also stops a stalled playback (`sessions::expire`).
pub(crate) async fn record_stop(
    state: &AppState,
    user: Uuid,
    req: &PlaybackStopInfo,
) -> Result<(), ApiError> {
    // Same as `playing`: a non-media id stores nothing.
    let item = reported_item(state, user, req.item_id).await?;
    let Some(item) = item else {
        return Ok(());
    };
    let runtime = reported_runtime(state, &item, req.media_source_id.as_deref()).await?;
    // Jellyfin's OnPlaybackStopped returns before touching user data for a failed playback.
    if req.failed != Some(true) {
        let rules = state.config.read().await.resume_rules();
        rustyfin_db::repo_playback::record_playback_stop(
            &state.db,
            user,
            req.item_id,
            item.media_type,
            req.position_ticks,
            runtime,
            &rules,
        )
        .await?;
        crate::user_data_changes::changed(state, user, req.item_id);
    }

    let (audio, subtitle) = reported_choices(
        state,
        req.item_id,
        req.media_source_id.as_deref(),
        req.audio_stream_index,
        req.subtitle_stream_index,
    )
    .await?;
    rustyfin_db::repo_playback::save_stream_choice(
        &state.db,
        user,
        req.item_id,
        audio.as_ref(),
        subtitle.as_ref(),
    )
    .await?;
    Ok(())
}

/// Query form of the playback reports (`/PlayingItems/{itemId}` and its `/Progress` and legacy
/// `/Users/{userId}/...` forms). Each handler reads only the fields Jellyfin reads for it.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayingItemQuery {
    #[serde(default, alias = "PositionTicks")]
    position_ticks: Option<i64>,
    #[serde(default, alias = "IsPaused")]
    is_paused: Option<bool>,
    #[serde(default, alias = "MediaSourceId")]
    media_source_id: Option<String>,
    #[serde(default, alias = "AudioStreamIndex")]
    audio_stream_index: Option<i32>,
    #[serde(default, alias = "SubtitleStreamIndex")]
    subtitle_stream_index: Option<i32>,
}

/// POST /PlayingItems/{itemId} and POST /Users/{userId}/PlayingItems/{itemId}: report a start.
/// The start takes no position, as Jellyfin's does not.
async fn playing_item_start(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    report_start(
        state,
        claims,
        token,
        PlaybackStartInfo {
            item_id,
            item_name: None,
            position_ticks: None,
            media_source_id: query.media_source_id,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            play_state: Default::default(),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /PlayingItems/{itemId}/Progress and POST /Users/{userId}/PlayingItems/{itemId}/Progress.
async fn playing_item_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    report_progress(
        state,
        claims,
        token,
        PlaybackProgressInfo {
            item_id,
            item_name: None,
            position_ticks: query.position_ticks,
            is_paused: query.is_paused,
            media_source_id: query.media_source_id,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            play_state: Default::default(),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /PlayingItems/{itemId} and DELETE /Users/{userId}/PlayingItems/{itemId}: report a stop.
async fn playing_item_stop(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(item_id): Path<Uuid>,
    Query(query): Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    report_stop(
        state,
        claims,
        token,
        PlaybackStopInfo {
            item_id,
            item_name: None,
            position_ticks: query.position_ticks,
            media_source_id: query.media_source_id,
            audio_stream_index: query.audio_stream_index,
            subtitle_stream_index: query.subtitle_stream_index,
            failed: None,
            play_state: Default::default(),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The legacy forms name a user in the path; Jellyfin ignores it and reports for the caller.
async fn legacy_playing_item_start(
    state: State<AppState>,
    claims: Extension<Claims>,
    Path((_user_id, item_id)): Path<(String, Uuid)>,
    query: Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    playing_item_start(state, claims, Path(item_id), query, token).await
}

async fn legacy_playing_item_progress(
    state: State<AppState>,
    claims: Extension<Claims>,
    Path((_user_id, item_id)): Path<(String, Uuid)>,
    query: Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    playing_item_progress(state, claims, Path(item_id), query, token).await
}

async fn legacy_playing_item_stop(
    state: State<AppState>,
    claims: Extension<Claims>,
    Path((_user_id, item_id)): Path<(String, Uuid)>,
    query: Query<PlayingItemQuery>,
    token: Token,
) -> Result<StatusCode, ApiError> {
    playing_item_stop(state, claims, Path(item_id), query, token).await
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

/// Query of the user data routes: Jellyfin 10.9's `userId` (default the caller), `datePlayed` for
/// marking played, and `likes` for the rating.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserItemQuery {
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "DatePlayed")]
    date_played: Option<String>,
    #[serde(default, alias = "Likes")]
    likes: Option<bool>,
}

impl UserItemQuery {
    /// `datePlayed` in any form clients send (`parse_client_date`); one that doesn't parse is
    /// ignored, so the item is still marked, on today's date, as Jellyfin's binder falls back.
    fn date_played(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.date_played
            .as_deref()
            .and_then(rustyfin_model::playback::parse_client_date)
    }
}

/// The user a 10.9 route acts for (`query_user`), and a 404 for an item that is no media item.
/// The API key names no user of its own, so it must pass `userId`.
async fn query_user_item(
    state: &AppState,
    claims: &Claims,
    user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<Uuid, ApiError> {
    let user = query_user(state, claims, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    require_item(state, item_id).await?;
    Ok(user)
}

/// POST /UserPlayedItems/{itemId} and POST /Users/{userId}/PlayedItems/{itemId}: mark played,
/// on `datePlayed` when given.
async fn played(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    date_played: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::mark_played_on(&state.db, user_id, item_id, date_played)
        .await?;
    crate::user_data_changes::played_changed(state, user_id, item_id);
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

async fn unplayed(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::mark_unplayed(&state.db, user_id, item_id).await?;
    crate::user_data_changes::played_changed(state, user_id, item_id);
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

async fn favorite(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    on: bool,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = rustyfin_db::repo_playback::set_favorite(&state.db, user_id, item_id, on).await?;
    crate::user_data_changes::changed(state, user_id, item_id);
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

async fn user_mark_played(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    played(&state, user_id, item_id, q.date_played()).await
}

async fn user_mark_unplayed(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    unplayed(&state, user_id, item_id).await
}

async fn user_set_favorite(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    favorite(&state, user_id, item_id, true).await
}

async fn user_unset_favorite(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    favorite(&state, user_id, item_id, false).await
}

/// GET /UserItems/{itemId}/UserData and its legacy form: the user's data on the item.
async fn user_data(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data =
        rustyfin_db::repo_playback::get_user_item_data(&state.db_ro, user_id, item_id).await?;
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

/// POST /UserItems/{itemId}/UserData and its legacy form: sets the fields the body carries.
async fn update_user_data(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    update: &UpdateUserItemDataDto,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data =
        rustyfin_db::repo_playback::update_user_data(&state.db, user_id, item_id, update).await?;
    crate::user_data_changes::changed(state, user_id, item_id);
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

async fn user_get_user_data(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    user_data(&state, user_id, item_id).await
}

async fn user_update_user_data(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
    Json(update): Json<UpdateUserItemDataDto>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    update_user_data(&state, user_id, item_id, &update).await
}

async fn legacy_get_user_data(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    user_data(&state, user_id, item_id).await
}

async fn legacy_update_user_data(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
    Json(update): Json<UpdateUserItemDataDto>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    update_user_data(&state, user_id, item_id, &update).await
}

/// POST .../Rating?likes=: sets Likes, or clears the rating when `likes` is absent, as Jellyfin's
/// UpdateUserItemRating does.
async fn set_likes(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    likes: Option<bool>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let data = match likes {
        Some(likes) => {
            rustyfin_db::repo_playback::set_user_rating(
                &state.db,
                user_id,
                item_id,
                Some(likes),
                None,
            )
            .await?
        }
        None => return clear_rating(state, user_id, item_id).await,
    };
    crate::user_data_changes::changed(state, user_id, item_id);
    Ok(Json(folder_answer(state, user_id, item_id, data).await?))
}

/// DELETE .../Rating: clears Likes and the rating, and answers the user data.
async fn clear_rating(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<Json<UserItemDataDto>, ApiError> {
    rustyfin_db::repo_playback::clear_user_rating(&state.db, user_id, item_id).await?;
    crate::user_data_changes::changed(state, user_id, item_id);
    user_data(state, user_id, item_id).await
}

async fn user_set_likes(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    set_likes(&state, user_id, item_id, q.likes).await
}

async fn user_clear_rating(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = query_user_item(&state, &claims, q.user_id, item_id).await?;
    clear_rating(&state, user_id, item_id).await
}

async fn legacy_set_likes(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    set_likes(&state, user_id, item_id, q.likes).await
}

async fn legacy_clear_rating(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    clear_rating(&state, user_id, item_id).await
}

async fn mark_played(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<UserItemQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    played(&state, user_id, item_id, q.date_played()).await
}

async fn mark_unplayed(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    unplayed(&state, user_id, item_id).await
}

/// User data as items answer it: a Series or Season with its Played and UnplayedItemCount derived
/// from its episodes, other items with their PlayedPercentage, and every answer with Jellyfin's
/// Key and ItemId.
pub(crate) async fn folder_answer(
    state: &AppState,
    user_id: Uuid,
    item_id: Uuid,
    data: UserItemDataDto,
) -> Result<UserItemDataDto, ApiError> {
    let mut data = data.for_item(item_id);
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id).await?;
    let is_folder = item.as_ref().is_some_and(|item| {
        matches!(
            item.media_type,
            rustyfin_model::media::MediaType::Series | rustyfin_model::media::MediaType::Season
        )
    });
    if let Some(runtime) = item.and_then(|item| item.runtime_ticks).filter(|r| *r > 0)
        && data.playback_position_ticks > 0
    {
        data.played_percentage = Some(100.0 * data.playback_position_ticks as f64 / runtime as f64);
    }
    if is_folder {
        // Only the episodes the user may see count.
        let visible = crate::parental::user_filter(&state.db, Some(user_id)).await;
        let counts = rustyfin_db::repo_playback::get_unplayed_counts(
            &state.db,
            user_id,
            &[item_id],
            visible.as_deref(),
        )
        .await?
        .get(&item_id.to_string())
        .copied()
        .unwrap_or((0, 0));
        data.played = rustyfin_db::repo_playback::folder_played(counts);
        data.unplayed_item_count = Some(counts.1 as i32);
    }
    Ok(data)
}

async fn set_favorite(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    favorite(&state, user_id, item_id, true).await
}

async fn unset_favorite(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<UserItemDataDto>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    require_item(&state, item_id).await?;
    favorite(&state, user_id, item_id, false).await
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
    crate::user_data_changes::changed(&state, user_id, item_id);
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
