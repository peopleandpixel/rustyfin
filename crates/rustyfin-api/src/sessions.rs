//! Sessions as Jellyfin's `SessionManager` keeps them: one per device, found from the request's
//! token, with the play state the client reports. The session rows (who, which device, the
//! capabilities, the last activity) are in the database; what is playing lives here in memory,
//! as in Jellyfin, and is gone after a restart.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

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

static RUNTIME: LazyLock<RwLock<HashMap<String, Runtime>>> = LazyLock::new(Default::default);

fn runtime() -> std::sync::RwLockWriteGuard<'static, HashMap<String, Runtime>> {
    RUNTIME.write().unwrap_or_else(|e| e.into_inner())
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
        rustyfin_db::repo_session::find_device_session(&state.db, &device.device_id).await?
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
        let mut all = runtime();
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
            item_id: report.item_id,
            play_session_id: None,
            playlist_item_id: None,
            queue: None,
        });
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
        let mut all = runtime();
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
pub fn ping(session_id: &str) {
    if let Some(rt) = runtime().get_mut(session_id) {
        rt.last_check_in = Some(Utc::now());
    }
}

/// Stops the playbacks that have not reported for `PLAYBACK_IDLE_MINUTES`, and forgets the
/// sessions in `ended`. Returns the sessions whose playback it stopped.
pub fn sweep(ended: &[String]) -> Vec<String> {
    let cutoff = Utc::now() - chrono::Duration::minutes(PLAYBACK_IDLE_MINUTES);
    let mut all = runtime();
    for id in ended {
        all.remove(id);
    }
    let mut idle = Vec::new();
    for (id, rt) in all.iter_mut() {
        if rt.playing.is_some() && rt.last_check_in.is_some_and(|t| t < cutoff) {
            rt.playing = None;
            rt.play_state = PlayState::default();
            idle.push(id.clone());
        }
    }
    idle
}

/// Ends idle sessions and idle playbacks; run once a minute. Sessions with an open socket stay.
pub async fn expire(state: &AppState) {
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
    let idle = sweep(&ended);
    for id in &idle {
        let _ =
            rustyfin_db::repo_session::update_session_now_playing(&state.db, id, None, None, false)
                .await;
    }
    if !ended.is_empty() || !idle.is_empty() {
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
        let all = RUNTIME.read().unwrap_or_else(|e| e.into_inner());
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
