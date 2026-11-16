//! Sessions as Jellyfin's `SessionManager` keeps them: one per device, found from the request's
//! token, with the play state the client reports. The session rows (who, which device, the
//! capabilities, the last activity) are in the database; what is playing lives here in memory,
//! as in Jellyfin, and is gone after a restart.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;
use rustyfin_model::playback::ReportedPlayState;
use rustyfin_model::session::SessionInfo;
use rustyfin_model::ws_event::WsEvent;

/// Minutes without activity after which a session without an open socket ends.
pub const SESSION_IDLE_MINUTES: i64 = 30;
/// Minutes without a progress report or ping after which a playback counts as stopped
/// (Jellyfin's `CheckForInactiveSteams`).
pub const PLAYBACK_IDLE_MINUTES: i64 = 5;
/// Minutes since the last activity within which a session without a socket is active.
const ACTIVE_MINUTES: i64 = 10;
/// Jellyfin's `DateTime.MinValue`, sent for a session that never reported playback.
const NEVER: &str = "0001-01-01T00:00:00.0000000Z";

/// Jellyfin's `PlayerStateInfo`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ticks: Option<i64>,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle_stream_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_method: Option<String>,
    pub repeat_mode: String,
    pub playback_order: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_stream_id: Option<String>,
}

impl Default for PlayState {
    fn default() -> Self {
        Self {
            position_ticks: None,
            can_seek: false,
            is_paused: false,
            is_muted: false,
            volume_level: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
            media_source_id: None,
            play_method: None,
            repeat_mode: "RepeatNone".to_string(),
            playback_order: "Default".to_string(),
            live_stream_id: None,
        }
    }
}

/// One playback report, as `/Sessions/Playing`, `/Progress` and `/Stopped` send it.
pub struct Report<'a> {
    /// Who reported it: the user whose watch data a stalled playback's stop updates.
    pub user_id: Uuid,
    pub item_id: Uuid,
    pub position_ticks: Option<i64>,
    pub is_paused: Option<bool>,
    pub media_source_id: Option<&'a str>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub fields: &'a ReportedPlayState,
}

/// What a session is playing.
#[derive(Debug, Clone)]
struct Playing {
    user_id: Uuid,
    item_id: Uuid,
    play_session_id: Option<String>,
    playlist_item_id: Option<String>,
    queue: Option<serde_json::Value>,
}

#[derive(Debug, Default)]
struct Runtime {
    playing: Option<Playing>,
    play_state: PlayState,
    last_check_in: Option<DateTime<Utc>>,
}

/// What the server keeps in memory about its sessions, by session id. One per `AppState`, so
/// servers in one process (the tests) never see or sweep each other's sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionRuntime(Arc<RwLock<HashMap<String, Runtime>>>);

impl SessionRuntime {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Runtime>> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Runtime>> {
        self.0.write().unwrap_or_else(|e| e.into_inner())
    }
}

fn runtime(state: &AppState) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Runtime>> {
    state.session_runtime.write()
}

/// Tells the sockets that follow the sessions (`SessionsStart`) that the list changed.
pub fn changed(state: &AppState) {
    let _ = state.ws_broadcast.send(WsEvent::SessionsChanged);
}

/// The session of the request's token: its device's session, made again when it has expired
/// (Jellyfin's `LogSessionActivity`), and marked active. `None` for the API key and for a token
/// without a device.
pub async fn for_token(state: &AppState, token: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(token) = token else {
        return Ok(None);
    };
    let Some((user_id, device)) = rustyfin_auth::token_device(&state.db_ro, token).await? else {
        return Ok(None);
    };
    if let Some(id) =
        rustyfin_db::repo_session::find_device_session(&state.db, user_id, &device.device_id)
            .await?
    {
        rustyfin_db::repo_session::touch_session(&state.db, &id).await?;
        return Ok(Some(id));
    }
    let Some(user) = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user_id).await? else {
        return Ok(None);
    };
    let id = Uuid::new_v4();
    rustyfin_db::repo_session::create_session(
        &state.db,
        id,
        user.id,
        &user.name,
        &device.app_name,
        &device.device_name,
        &device.device_id,
        &device.app_version,
        "{}",
    )
    .await?;
    changed(state);
    Ok(Some(id.to_string()))
}

fn apply(play_state: &mut PlayState, report: &Report<'_>) {
    let f = report.fields;
    play_state.position_ticks = report.position_ticks.or(play_state.position_ticks);
    if let Some(v) = report.is_paused {
        play_state.is_paused = v;
    }
    if let Some(v) = f.can_seek {
        play_state.can_seek = v;
    }
    if let Some(v) = f.is_muted {
        play_state.is_muted = v;
    }
    if f.volume_level.is_some() {
        play_state.volume_level = f.volume_level;
    }
    if report.audio_stream_index.is_some() {
        play_state.audio_stream_index = report.audio_stream_index;
    }
    if report.subtitle_stream_index.is_some() {
        play_state.subtitle_stream_index = report.subtitle_stream_index;
    }
    if let Some(v) = report.media_source_id {
        play_state.media_source_id = Some(v.to_string());
    }
    if f.play_method.is_some() {
        play_state.play_method = f.play_method.clone();
    }
    if let Some(v) = &f.repeat_mode {
        play_state.repeat_mode = v.clone();
    }
    if let Some(v) = &f.playback_order {
        play_state.playback_order = v.clone();
    }
    if f.live_stream_id.is_some() {
        play_state.live_stream_id = f.live_stream_id.clone();
    }
}

/// Records a start or progress report of a media item on `session_id`.
pub async fn playing(state: &AppState, session_id: &str, report: Report<'_>, start: bool) {
    let (position, paused) = {
        let mut all = runtime(state);
        let rt = all.entry(session_id.to_string()).or_default();
        let same_item = rt
            .playing
            .as_ref()
            .is_some_and(|p| p.item_id == report.item_id);
        if start || !same_item {
            rt.play_state = PlayState::default();
        }
        apply(&mut rt.play_state, &report);
        let f = report.fields;
        let playing = rt.playing.get_or_insert(Playing {
            user_id: report.user_id,
            item_id: report.item_id,
            play_session_id: None,
            playlist_item_id: None,
            queue: None,
        });
        playing.user_id = report.user_id;
        playing.item_id = report.item_id;
        if f.play_session_id.is_some() {
            playing.play_session_id = f.play_session_id.clone();
        }
        if f.playlist_item_id.is_some() {
            playing.playlist_item_id = f.playlist_item_id.clone();
        }
        if f.now_playing_queue.is_some() {
            playing.queue = f.now_playing_queue.clone();
        }
        rt.last_check_in = Some(Utc::now());
        (rt.play_state.position_ticks, rt.play_state.is_paused)
    };
    let _ = rustyfin_db::repo_session::update_session_now_playing(
        &state.db,
        session_id,
        Some(report.item_id),
        position,
        paused,
    )
    .await;
    changed(state);
}

/// Ends the session's playback: no now-playing item, and the play state back to idle.
pub async fn stopped(state: &AppState, session_id: &str) {
    {
        let mut all = runtime(state);
        let rt = all.entry(session_id.to_string()).or_default();
        rt.playing = None;
        rt.play_state = PlayState::default();
        rt.last_check_in = Some(Utc::now());
    }
    let _ = rustyfin_db::repo_session::update_session_now_playing(
        &state.db, session_id, None, None, false,
    )
    .await;
    changed(state);
}

/// `POST /Sessions/Playing/Ping`: the client is still playing.
pub fn ping(state: &AppState, session_id: &str) {
    if let Some(rt) = runtime(state).get_mut(session_id) {
        rt.last_check_in = Some(Utc::now());
    }
}

/// Drops what the server keeps in memory about a session that has ended.
pub fn forget(state: &AppState, session_id: &str) {
    runtime(state).remove(session_id);
}

/// Whether the server keeps anything in memory for `session_id` (for tests).
pub fn tracks(state: &AppState, session_id: &str) -> bool {
    state.session_runtime.read().contains_key(session_id)
}

/// Forgets the sessions `ids`, after they are deleted.
pub fn forget_all(state: &AppState, ids: &[String]) {
    let mut all = runtime(state);
    for id in ids {
        all.remove(id);
    }
}

/// A playback that stopped reporting, taken out of its session for `stop_stalled`.
#[derive(Debug)]
pub struct Stalled {
    pub session_id: String,
    pub user_id: Uuid,
    pub item_id: Uuid,
    pub play_state: PlayState,
    pub play_session_id: Option<String>,
}

/// Takes out the playbacks of the sessions in `ended`, and those without a report for
/// `PLAYBACK_IDLE_MINUTES`, and forgets the sessions in `ended` and those not in `known`.
fn sweep(
    state: &AppState,
    ended: &[String],
    known: Option<&[String]>,
    now: DateTime<Utc>,
) -> Vec<Stalled> {
    let cutoff = now - chrono::Duration::minutes(PLAYBACK_IDLE_MINUTES);
    let mut all = runtime(state);
    let mut stalled = Vec::new();
    for (id, rt) in all.iter_mut() {
        let gone = ended.contains(id) || known.is_some_and(|k| !k.contains(id));
        let idle = rt.last_check_in.is_some_and(|t| t < cutoff);
        if (gone || idle)
            && let Some(p) = rt.playing.take()
        {
            stalled.push(Stalled {
                session_id: id.clone(),
                user_id: p.user_id,
                item_id: p.item_id,
                play_state: std::mem::take(&mut rt.play_state),
                play_session_id: p.play_session_id,
            });
        }
    }
    all.retain(|id, _| !ended.contains(id) && known.is_none_or(|k| k.contains(id)));
    stalled
}

/// Stops a stalled playback as a `POST /Sessions/Playing/Stopped` at its last known position
/// would (the position saved, played past the threshold, the play count), and stops its
/// transcodes.
async fn stop_stalled(state: &AppState, s: &Stalled, device_id: Option<&str>) {
    let ps = &s.play_state;
    let stop = rustyfin_model::playback::PlaybackStopInfo {
        item_id: s.item_id,
        item_name: None,
        position_ticks: ps.position_ticks,
        media_source_id: ps.media_source_id.clone(),
        audio_stream_index: ps.audio_stream_index,
        subtitle_stream_index: ps.subtitle_stream_index,
        failed: None,
        play_state: Default::default(),
    };
    if let Err(e) = crate::routes::playback::record_stop(state, s.user_id, &stop).await {
        tracing::warn!("stopping a stalled playback failed: {e:?}");
    }
    // The transcodes are found by the reported play session id, else by the device
    let psid = s.play_session_id.as_deref().unwrap_or("");
    let device = if psid.is_empty() {
        device_id.unwrap_or("")
    } else {
        ""
    };
    if !psid.is_empty() || !device.is_empty() {
        state
            .transcode_manager
            .stop_play_session(Some(s.item_id), psid, device)
            .await;
    }
    let _ = rustyfin_db::repo_session::update_session_now_playing(
        &state.db,
        &s.session_id,
        None,
        None,
        false,
    )
    .await;
}

/// Ends idle sessions and stalled playbacks; run once a minute. Sessions with an open socket stay.
pub async fn expire(state: &AppState) {
    expire_at(state, Utc::now()).await;
}

/// `expire` as of `now`, which tests move forward.
pub async fn expire_at(state: &AppState, now: DateTime<Utc>) {
    let open = crate::routes::websocket::connected_sessions().await;
    let ended =
        match rustyfin_db::repo_session::expire_sessions(&state.db, SESSION_IDLE_MINUTES, &open)
            .await
        {
            Ok(ended) => ended,
            Err(e) => {
                tracing::warn!("session cleanup error: {e}");
                return;
            }
        };
    if !ended.is_empty() {
        tracing::info!("ended {} idle sessions", ended.len());
    }
    // The sessions still stored; one deleted elsewhere (a user disabled, say) is forgotten too
    let sessions = rustyfin_db::repo_session::get_active_sessions(&state.db)
        .await
        .ok();
    let known: Option<Vec<String>> = sessions
        .as_ref()
        .map(|s| s.iter().map(|s| s.id.clone()).collect());
    let stalled = sweep(state, &ended, known.as_deref(), now);
    for s in &stalled {
        let device = sessions
            .as_ref()
            .and_then(|all| all.iter().find(|x| x.id == s.session_id))
            .map(|x| x.device_id.as_str());
        stop_stalled(state, s, device).await;
    }
    if !ended.is_empty() || !stalled.is_empty() {
        changed(state);
    }
}

/// `GET /Sessions` filters.
#[derive(Debug, Default)]
pub struct Filter {
    pub controllable_by: Option<Uuid>,
    pub device_id: Option<String>,
    pub active_within_seconds: Option<i64>,
}

/// The sessions `viewer` may see (an administrator sees all, anyone else their own), filtered as
/// Jellyfin's `SessionManager.GetSessions` does, as Jellyfin's `SessionInfoDto`s.
pub async fn list(
    state: &AppState,
    viewer: Uuid,
    viewer_is_admin: bool,
    filter: &Filter,
) -> Result<Vec<serde_json::Value>, ApiError> {
    let mut sessions = rustyfin_db::repo_session::get_active_sessions(&state.db_ro).await?;
    if !viewer_is_admin {
        sessions.retain(|s| s.user_id == viewer);
    }
    if let Some(user) = filter.controllable_by {
        sessions.retain(|s| s.capabilities.supports_media_control == Some(true));
        if user != viewer || !viewer_is_admin {
            sessions.retain(|s| s.user_id == user);
        }
    }
    if let Some(device) = &filter.device_id {
        sessions.retain(|s| &s.device_id == device);
    }
    if let Some(secs) = filter.active_within_seconds.filter(|s| *s > 0) {
        let since = Utc::now() - chrono::Duration::seconds(secs);
        sessions.retain(|s| s.last_activity_date >= since);
    }
    let open = crate::routes::websocket::connected_sessions().await;
    let mut out = Vec::with_capacity(sessions.len());
    for s in &sessions {
        out.push(dto(state, s, viewer, open.contains(&s.id)).await);
    }
    Ok(out)
}

/// One session as Jellyfin's `SessionInfoDto`.
async fn dto(state: &AppState, s: &SessionInfo, viewer: Uuid, socket: bool) -> serde_json::Value {
    let (playing, play_state, check_in) = {
        let all = state.session_runtime.read();
        match all.get(&s.id) {
            Some(rt) => (rt.playing.clone(), rt.play_state.clone(), rt.last_check_in),
            None => (None, PlayState::default(), None),
        }
    };
    let caps = &s.capabilities;
    let media_control = caps.supports_media_control.unwrap_or(false);
    let playable = caps.playable_media_types.clone().unwrap_or_default();
    let commands = caps.supported_commands.clone().unwrap_or_default();
    let active =
        socket || Utc::now() - s.last_activity_date <= chrono::Duration::minutes(ACTIVE_MINUTES);
    let mut out = serde_json::json!({
        "PlayState": play_state,
        "AdditionalUsers": [],
        "Capabilities": {
            "PlayableMediaTypes": playable,
            "SupportedCommands": commands,
            "SupportsMediaControl": media_control,
            "SupportsPersistentIdentifier": caps.supports_persistent_identifier.unwrap_or(true),
        },
        "PlayableMediaTypes": playable,
        "Id": s.id,
        "UserId": s.user_id,
        "UserName": s.user_name,
        "Client": s.app_name,
        "LastActivityDate": s.last_activity_date,
        "LastPlaybackCheckIn": check_in.map_or(serde_json::json!(NEVER), |t| serde_json::json!(t)),
        "DeviceName": s.device_name,
        "DeviceId": s.device_id,
        "ApplicationVersion": s.app_version,
        "IsActive": active,
        "SupportsMediaControl": media_control,
        "SupportsRemoteControl": media_control,
        "NowPlayingQueue": [],
        "HasCustomDeviceName": false,
        "ServerId": state.server_id.to_string(),
        "SupportedCommands": commands,
    });
    if let Some(p) = playing {
        // The full item, as Jellyfin sends it; an item gone since the start is left out.
        if let Ok(item) = crate::routes::items::media_item(state, p.item_id, viewer).await {
            out["NowPlayingItem"] = serde_json::to_value(item).unwrap_or_default();
        }
        if let Some(queue) = p.queue {
            out["NowPlayingQueue"] = queue;
        }
        if let Some(id) = p.playlist_item_id {
            out["PlaylistItemId"] = serde_json::json!(id);
        }
        if let Some(id) = p.play_session_id {
            out["PlaySessionId"] = serde_json::json!(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3sessions_progress_keeps_unreported_fields() {
        let fields = ReportedPlayState {
            can_seek: Some(true),
            play_method: Some("DirectPlay".into()),
            ..Default::default()
        };
        let mut ps = PlayState::default();
        apply(
            &mut ps,
            &Report {
                user_id: Uuid::nil(),
                item_id: Uuid::nil(),
                position_ticks: Some(10),
                is_paused: None,
                media_source_id: Some("m"),
                audio_stream_index: Some(1),
                subtitle_stream_index: None,
                fields: &fields,
            },
        );
        let empty = ReportedPlayState::default();
        apply(
            &mut ps,
            &Report {
                user_id: Uuid::nil(),
                item_id: Uuid::nil(),
                position_ticks: Some(20),
                is_paused: Some(true),
                media_source_id: None,
                audio_stream_index: None,
                subtitle_stream_index: None,
                fields: &empty,
            },
        );
        assert_eq!(ps.position_ticks, Some(20));
        assert!(ps.is_paused && ps.can_seek);
        assert_eq!(ps.play_method.as_deref(), Some("DirectPlay"));
        assert_eq!(ps.media_source_id.as_deref(), Some("m"));
        assert_eq!(ps.audio_stream_index, Some(1));
    }
}
