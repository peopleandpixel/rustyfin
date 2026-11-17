use crate::query::Query;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::broadcast;

use crate::error::ApiError;
use crate::log_stream;
use crate::state::AppState;
use rustyfin_model::ws_event::LogMessageData;

/// Shared state for active WebSocket connections
#[derive(Default)]
pub struct WsConnections {
    /// Map of session ID → broadcast sender for targeted command delivery. The sockets of one
    /// session (a device's tabs) share its sender.
    connections: HashMap<String, broadcast::Sender<rustyfin_model::ws_event::WsEvent>>,
}

/// Global WebSocket connection registry, accessible from AppState
pub static WS_CONNECTIONS: std::sync::LazyLock<Arc<RwLock<WsConnections>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(WsConnections::default())));

pub fn routes() -> Router<AppState> {
    Router::new().route("/socket", get(ws_handler))
}

/// Query parameters for WebSocket upgrade — Jellyfin clients pass auth token here
#[derive(Debug, Deserialize)]
struct WsQuery {
    #[serde(default)]
    authorization: Option<String>,
    // Jellyfin 10.11 reads `ApiKey`; older clients send `api_key`.
    #[serde(default, alias = "api_key", alias = "ApiKey", alias = "apikey")]
    token: Option<String>,
}

/// Jellyfin WebSocket message envelope (incoming from client)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct WsMessage {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "MessageId", skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(rename = "Data", skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

async fn ws_handler(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    // Extract and validate authentication token from query parameters
    let token = query
        .authorization
        .as_ref()
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or(query.authorization)
        .or(query.token);

    // An empty token never authenticates, whatever the configured key.
    let token = token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            tracing::warn!("WebSocket: missing authentication token");
            ApiError::Unauthorized
        })?;

    // The configured API key or a valid user token opens the socket; a user's token only within
    // their remote access and access schedule, as every other request
    let authenticated = if state.api_key.as_ref().is_some_and(|key| token == **key) {
        true
    } else if let Ok(claims) = rustyfin_auth::validate_token(&state.db_ro, &token).await {
        let local =
            crate::middleware::resolve_client(peer.ip(), &headers, &state.known_proxies).is_local();
        crate::parental::check_access(&state, &claims, local).await?;
        true
    } else {
        false
    };

    if !authenticated {
        tracing::warn!("WebSocket: unauthenticated connection attempt");
        return Err(ApiError::Unauthorized);
    }

    tracing::info!("WebSocket: authenticated connection");
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, token)))
}

/// Seconds between re-checks of an open websocket's user token.
const WS_TOKEN_RECHECK_SECS: u64 = 5;

/// The timer of a `ScheduledTasksInfoStart`, whose data is Jellyfin's "initialDelayMs,intervalMs"
/// (as Jellyfin Web sends "0,1000"). The interval is at least 500 ms; it defaults to 1000.
fn tasks_info_timer(data: Option<&serde_json::Value>) -> tokio::time::Interval {
    let text = data.and_then(serde_json::Value::as_str).unwrap_or("");
    let mut parts = text.split(',').map(|p| p.trim().parse::<u64>().ok());
    let delay = parts.next().flatten().unwrap_or(0);
    let every = parts.next().flatten().unwrap_or(1000).max(500);
    let start = tokio::time::Instant::now() + std::time::Duration::from_millis(delay);
    let mut timer = tokio::time::interval_at(start, std::time::Duration::from_millis(every));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    timer
}

/// True only when an API key is configured and the token is that key.
fn is_api_key(api_key: Option<&str>, token: &str) -> bool {
    api_key.is_some_and(|key| token == key)
}

/// The `RefreshProgress` frame of one library's scan: its id, its percent as text, and its state.
fn refresh_progress(
    library_id: uuid::Uuid,
    progress: u8,
    status: rustyfin_core::scan_progress::ScanStatus,
) -> rustyfin_model::ws_event::WsEvent {
    let state = match status {
        rustyfin_core::scan_progress::ScanStatus::Queued => "Queued",
        rustyfin_core::scan_progress::ScanStatus::Active => "Active",
        rustyfin_core::scan_progress::ScanStatus::Idle => "Idle",
    };
    rustyfin_model::ws_event::WsEvent::RefreshProgress {
        data: rustyfin_model::ws_event::RefreshProgressData {
            item_id: library_id.to_string(),
            progress: progress.to_string(),
            state: state.to_string(),
        },
    }
}

async fn handle_socket(socket: WebSocket, state: AppState, token: String) {
    // Every token but the configured API key is re-checked, also one revoked since the upgrade
    // check; only an invalid token closes the socket.
    let user_token = !is_api_key(state.api_key.as_deref().map(String::as_str), &token);

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to the global broadcast channel for server events
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    // The scans in progress, and the updates after them, taken under one lock before TimeSync
    let (scans_now, mut scans_rx) = state.task_scheduler.scans().subscribe_with_snapshot();
    let mut viewer = socket_viewer(&state, &token).await;

    // The socket joins its token's device session, so that remote control reaches it; the API
    // key, and a token without a device, get a session of their own.
    let device_session = if user_token {
        crate::sessions::for_token(&state, Some(&token))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let session_id = device_session
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Register this connection for targeted command delivery
    let mut cmd_rx = {
        let mut connections = WS_CONNECTIONS.write().await;
        connections
            .connections
            .entry(session_id.clone())
            .or_insert_with(|| broadcast::channel(16).0)
            .subscribe()
    };
    if device_session.is_some() {
        crate::sessions::changed(&state);
    }
    // The Sessions frames a SessionsStart asked for: their timer, and whether the list changed
    let mut sessions_timer: Option<tokio::time::Interval> = None;
    let mut sessions_dirty = false;

    tracing::info!("WebSocket: session {} connected", session_id);

    // The keep-alive interval the client is to keep (Jellyfin protocol)
    let keep_alive = rustyfin_model::ws_event::WsEvent::ForceKeepAlive { data: 60 };
    if let Some(frame) = text_frame(&keep_alive) {
        let _ = sender.send(frame).await;
    }

    // Send TimeSync event
    let time_sync = rustyfin_model::ws_event::WsEvent::TimeSync {
        data: chrono::Utc::now().timestamp_millis(),
    };
    if let Some(frame) = text_frame(&time_sync) {
        let _ = sender.send(frame).await;
    }

    // The scans in progress at their current percent, so this socket starts from them
    for (library_id, scan) in scans_now {
        let event = refresh_progress(library_id, scan.progress.unwrap_or(0), scan.status);
        if let Some(frame) = text_frame(&event) {
            let _ = sender.send(frame).await;
        }
    }

    let period = std::time::Duration::from_secs(WS_TOKEN_RECHECK_SECS);
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    // The ScheduledTasksInfo timer an administrator's ScheduledTasksInfoStart sets
    let mut tasks_info: Option<tokio::time::Interval> = None;
    // The server's log lines, to an administrator's live log view after its LiveLogStart
    let mut live_log: Option<broadcast::Receiver<LogMessageData>> = None;

    loop {
        tokio::select! {
            // Polled in the order written: the scan arm comes before the broadcast arm, so a library's
            // 100 frame goes out before its Completed event.
            biased;

            // Handle incoming messages from the client
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(incoming) = serde_json::from_str::<WsMessage>(&text) {
                            match incoming.message_type.as_str() {
                                "KeepAlive" => {
                                    if device_session.is_some() {
                                        let _ = rustyfin_db::repo_session::touch_session(&state.db, &session_id).await;
                                    }
                                    // Jellyfin answers a client's KeepAlive with a KeepAlive
                                    let resp = rustyfin_model::ws_event::WsEvent::KeepAlive;
                                    if let Some(frame) = text_frame(&resp) {
                                        let _ = sender.send(frame).await;
                                    }
                                }
                                // Only an administrator may follow the tasks; the timer also
                                // checks, for a socket whose user is demoted
                                "ScheduledTasksInfoStart" if viewer.is_admin => {
                                    tasks_info = Some(tasks_info_timer(incoming.data.as_ref()));
                                }
                                "ScheduledTasksInfoStart" => {
                                    tracing::debug!("ws: ScheduledTasksInfoStart from a non-administrator ignored");
                                }
                                "ScheduledTasksInfoStop" => tasks_info = None,
                                // The live log view: the recent lines first, then each new one
                                "LiveLogStart" if viewer.is_admin => {
                                    let (history, rx) = state.live_log.subscribe();
                                    live_log = Some(rx);
                                    if send_log_lines(&mut sender, history).await.is_err() {
                                        break;
                                    }
                                }
                                "LiveLogStart" => {
                                    tracing::debug!("ws: LiveLogStart from a non-administrator ignored");
                                }
                                "LiveLogStop" => live_log = None,
                                // The session list now, then whenever it changes
                                "SessionsStart" => {
                                    sessions_timer = Some(tasks_info_timer(incoming.data.as_ref()));
                                    sessions_dirty = true;
                                }
                                "SessionsStop" => sessions_timer = None,
                                "ActivityLogEntryStart" => {
                                    // Acknowledge subscription-style messages
                                    tracing::debug!("ws subscription: {}", incoming.message_type);
                                }
                                "ActivityLogEntryStop" => {
                                    tracing::debug!("ws unsubscribe: {}", incoming.message_type);
                                }
                                other => {
                                    tracing::debug!("ws unknown message type: {}", other);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = sender.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!("WebSocket: session {} disconnected", session_id);
                        break;
                    }
                    _ => {}
                }
            }

            // Handle scan progress from the registry, sent to every socket. This arm comes before the
            // broadcast arm, so that a library's 100 frame goes out before its Completed event.
            res = scans_rx.recv() => {
                match res {
                    Ok(update) => {
                        let event = refresh_progress(update.library_id, update.progress, update.status);
                        if let Some(frame) = text_frame(&event)
                            && sender.send(frame).await.is_err() {
                                tracing::debug!("WebSocket: session {} sender closed", session_id);
                                break;
                            }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Stale percents would stay on screen, so the socket closes and the client
                        // reconnects for a new snapshot.
                        tracing::warn!("WebSocket: session {} missed {} scan updates, closing so the client resyncs", session_id, n);
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::warn!("WebSocket: scan progress channel closed");
                        break;
                    }
                }
            }

            // Handle broadcast events from the server (global events)
            res = broadcast_rx.recv() => {
                match res {
                    Ok(rustyfin_model::ws_event::WsEvent::SessionsChanged) => sessions_dirty = true,
                    // Another user's session and playback events, and logs for non-administrators
                    Ok(event) if !receives(&viewer, &event) => {}
                    Ok(event) => {
                        if let Some(frame) = text_frame(&event)
                            && sender.send(frame).await.is_err() {
                                tracing::debug!("WebSocket: session {} sender closed", session_id);
                                break;
                            }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("WebSocket: session {} lagged behind broadcast by {} events", session_id, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::warn!("WebSocket: broadcast channel closed");
                        break;
                    }
                }
            }

            // Handle targeted command delivery
            res = cmd_rx.recv() => {
                match res {
                    Ok(event) => {
                        if let Some(frame) = text_frame(&event)
                            && sender.send(frame).await.is_err() {
                                tracing::debug!("WebSocket: session {} sender closed", session_id);
                                break;
                            }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("WebSocket: session {} command lagged by {} events", session_id, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Command sender dropped, ignore
                    }
                }
            }

            // The session list, at most once a timer tick, when it changed since the last one
            _ = async { sessions_timer.as_mut().unwrap().tick().await }, if sessions_timer.is_some() && sessions_dirty => {
                sessions_dirty = false;
                let list = crate::sessions::list(&state, viewer.user_id, viewer.is_admin, &Default::default()).await;
                if let Ok(list) = list {
                    let event = rustyfin_model::ws_event::WsEvent::Sessions { data: serde_json::Value::Array(list) };
                    if let Some(frame) = text_frame(&event)
                        && sender.send(frame).await.is_err() {
                            break;
                        }
                }
            }

            // The tasks and their progress, to an administrator that asked for them
            _ = async { tasks_info.as_mut().unwrap().tick().await }, if tasks_info.is_some() => {
                if viewer.is_admin {
                    let tasks = state.task_scheduler.get_all_tasks().await;
                    let frame = serde_json::json!({
                        "MessageType": "ScheduledTasksInfo",
                        "MessageId": uuid::Uuid::new_v4().simple().to_string(),
                        "Data": tasks,
                    });
                    if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                        tracing::debug!("WebSocket: session {} sender closed", session_id);
                        break;
                    }
                }
            }

            // The server's new log lines, or how many this socket fell behind by
            line = async { log_stream::next_line(live_log.as_mut().unwrap()).await }, if live_log.is_some() => {
                match line {
                    Some(line) if viewer.is_admin => {
                        if send_log_lines(&mut sender, [line]).await.is_err() {
                            break;
                        }
                    }
                    // Closed, or the user is no longer an administrator
                    _ => live_log = None,
                }
            }

            // Close the socket once its user token stops validating; otherwise refresh
            // the socket's administrator rights so a demotion takes effect
            _ = recheck.tick(), if user_token => {
                match rustyfin_auth::validate_token(&state.db_ro, &token).await {
                    Ok(claims) => viewer.is_admin = claims.is_admin,
                    Err(rustyfin_auth::AuthError::InvalidToken) => {
                        tracing::info!("WebSocket: session {} closed: its token is no longer valid", session_id);
                        let _ = sender.send(Message::Close(None)).await;
                        break;
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // Clean up: unregister the session once its last socket is gone
    drop(cmd_rx);
    {
        let mut connections = WS_CONNECTIONS.write().await;
        if connections
            .connections
            .get(&session_id)
            .is_some_and(|tx| tx.receiver_count() == 0)
        {
            connections.connections.remove(&session_id);
            tracing::debug!("WebSocket: unregistered session {}", session_id);
        }
    }
    if device_session.is_some() {
        crate::sessions::changed(&state);
    }
}

/// The sessions that have an open socket.
pub async fn connected_sessions() -> Vec<String> {
    WS_CONNECTIONS
        .read()
        .await
        .connections
        .keys()
        .cloned()
        .collect()
}

/// Sends `event` to the sockets of `session_id`. False when the session has no open socket.
pub async fn send_to_session(session_id: &str, event: rustyfin_model::ws_event::WsEvent) -> bool {
    let connections = WS_CONNECTIONS.read().await;
    connections
        .connections
        .get(session_id)
        .is_some_and(|tx| tx.send(event).is_ok())
}

/// Sends each log line as a `LogMessage` frame.
async fn send_log_lines(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    lines: impl IntoIterator<Item = LogMessageData>,
) -> Result<(), axum::Error> {
    for data in lines {
        let event = rustyfin_model::ws_event::WsEvent::LogMessage { data };
        if let Some(frame) = text_frame(&event) {
            sender.send(frame).await?;
        }
    }
    Ok(())
}

/// `event` as a text frame. An event that does not serialize is logged, not dropped silently.
fn text_frame(event: &impl serde::Serialize) -> Option<Message> {
    match serde_json::to_string(event) {
        Ok(json) => Some(Message::Text(json.into())),
        Err(e) => {
            tracing::warn!(error = %e, "WebSocket: could not serialize an event");
            None
        }
    }
}

/// Broadcast an event to all connected WebSocket clients
pub async fn broadcast_event(state: &AppState, event: rustyfin_model::ws_event::WsEvent) {
    // Silently ignore broadcast failures - this is normal when no WebSocket clients are connected.
    // The browser player does not require WebSocket for playback; it's only used for notifications.
    let _ = state.ws_broadcast.send(event);
}

/// Who a socket belongs to, for deciding which broadcast events it receives.
struct Viewer {
    user_id: uuid::Uuid,
    is_admin: bool,
    /// The libraries the user may see, as stored; `None` for every library.
    libraries: Option<std::collections::HashSet<String>>,
}

/// The socket's user, from the token it connected with. The API key counts as an administrator.
async fn socket_viewer(state: &AppState, token: &str) -> Viewer {
    if state.api_key.as_ref().is_some_and(|key| token == **key) {
        return Viewer {
            user_id: uuid::Uuid::nil(),
            is_admin: true,
            libraries: None,
        };
    }
    match rustyfin_auth::validate_token(&state.db_ro, token).await {
        Ok(claims) => Viewer {
            user_id: claims.sub,
            is_admin: claims.is_admin,
            // An unreadable policy shows no library.
            libraries: match rustyfin_db::repo_parental::get_policy(&state.db_ro, claims.sub).await
            {
                Ok(policy) => policy
                    .enabled_folders()
                    .map(|ids| ids.iter().map(|id| id.to_ascii_lowercase()).collect()),
                Err(_) => Some(Default::default()),
            },
        },
        Err(_) => Viewer {
            user_id: uuid::Uuid::nil(),
            is_admin: false,
            libraries: Some(Default::default()),
        },
    }
}

/// Whether a socket gets a broadcast event. Session and playback events go to administrators and
/// to the user their `UserId` names, as in Jellyfin's SessionInfoWebSocketListener; user data
/// changes only to the user they name; log lines go to administrators only. Everything else goes to
/// everyone.
fn receives(viewer: &Viewer, event: &rustyfin_model::ws_event::WsEvent) -> bool {
    use rustyfin_model::ws_event::WsEvent;
    match event {
        WsEvent::SessionChanged { data }
        | WsEvent::PlaybackStart { data }
        | WsEvent::PlaybackStop { data }
        | WsEvent::PlaybackProgress { data } => {
            viewer.is_admin
                || data["UserId"]
                    .as_str()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .is_some_and(|id| !id.is_nil() && id == viewer.user_id)
        }
        // Jellyfin's UserDataChangeNotifier sends to the user's own sessions only.
        WsEvent::UserDataChanged { data } => data["UserId"]
            .as_str()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .is_some_and(|id| !id.is_nil() && id == viewer.user_id),
        WsEvent::LogMessage { .. } => viewer.is_admin,
        // A library the user's library access leaves out
        WsEvent::LibraryChanged { data } => viewer.libraries.as_ref().is_none_or(|libs| {
            data["LibraryId"]
                .as_str()
                .is_some_and(|id| libs.contains(&id.to_ascii_lowercase()))
        }),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{Viewer, is_api_key, receives};

    #[test]
    fn r3endpoints_user_data_changes_reach_only_their_user() {
        use rustyfin_model::ws_event::WsEvent;
        let user = uuid::Uuid::new_v4();
        let event = WsEvent::UserDataChanged {
            data: serde_json::json!({ "UserId": user.to_string(), "UserDataList": [] }),
        };
        let viewer = |user_id: uuid::Uuid, is_admin: bool| Viewer {
            user_id,
            is_admin,
            libraries: None,
        };
        assert!(receives(&viewer(user, false), &event));
        // Not even an administrator or the API key gets another user's data.
        assert!(!receives(&viewer(uuid::Uuid::new_v4(), true), &event));
        assert!(!receives(&viewer(uuid::Uuid::nil(), true), &event));
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "MessageType": "UserDataChanged",
                "Data": { "UserId": user.to_string(), "UserDataList": [] }
            })
        );
    }

    #[test]
    fn r3parental_library_changes_reach_only_users_who_may_see_the_library() {
        use rustyfin_model::ws_event::WsEvent;
        let event = |id: &str| WsEvent::LibraryChanged {
            data: serde_json::json!({ "LibraryId": id, "LibraryName": "L" }),
        };
        let viewer = |libraries: Option<&[&str]>| Viewer {
            user_id: uuid::Uuid::new_v4(),
            is_admin: false,
            libraries: libraries.map(|ids| ids.iter().map(|id| id.to_string()).collect()),
        };
        assert!(receives(&viewer(None), &event("a")));
        assert!(receives(&viewer(Some(&["a"])), &event("A")));
        assert!(!receives(&viewer(Some(&["a"])), &event("b")));
        assert!(!receives(&viewer(Some(&[])), &event("a")));
    }

    #[test]
    fn g2c_only_the_api_key_skips_the_token_recheck() {
        // handle_socket re-checks every socket whose token is not the configured API key, also one
        // revoked since the upgrade check. 9dd5a44 validated the token again at the start and never
        // re-checked a socket whose token failed there.
        assert!(is_api_key(Some("the-key"), "the-key"));
        assert!(!is_api_key(Some("the-key"), "a-revoked-user-token"));
        assert!(!is_api_key(Some("the-key"), ""));
        assert!(!is_api_key(None, "a-user-token"));
        assert!(!is_api_key(None, ""));
    }
}
