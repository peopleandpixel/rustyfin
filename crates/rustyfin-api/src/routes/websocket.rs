use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::broadcast;

use crate::error::ApiError;
use crate::state::AppState;

/// Shared state for active WebSocket connections
#[derive(Default)]
pub struct WsConnections {
    /// Map of session ID → broadcast sender for targeted command delivery
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
    #[serde(default, alias = "api_key")]
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

    // The configured API key or a valid user token opens the socket
    let authenticated = state.api_key.as_ref().is_some_and(|key| token == **key)
        || rustyfin_auth::validate_token(&state.db_ro, &token)
            .await
            .is_ok();

    if !authenticated {
        tracing::warn!("WebSocket: unauthenticated connection attempt");
        return Err(ApiError::Unauthorized);
    }

    tracing::info!("WebSocket: authenticated connection");
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, token)))
}

/// Seconds between re-checks of an open websocket's user token.
const WS_TOKEN_RECHECK_SECS: u64 = 5;

/// True only when an API key is configured and the token is that key.
fn is_api_key(api_key: Option<&str>, token: &str) -> bool {
    api_key.is_some_and(|key| token == key)
}

async fn handle_socket(socket: WebSocket, state: AppState, token: String) {
    // Every token but the configured API key is re-checked, also one revoked since the upgrade
    // check; only an invalid token closes the socket.
    let user_token = !is_api_key(state.api_key.as_deref().map(String::as_str), &token);

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to the global broadcast channel for server events
    let mut broadcast_rx = state.ws_broadcast.subscribe();
    let viewer = socket_viewer(&state, &token).await;

    // Create a dedicated channel for targeted command delivery to this session
    let (cmd_tx, mut cmd_rx) = broadcast::channel::<rustyfin_model::ws_event::WsEvent>(16);

    // Generate a session ID for this connection
    let session_id = uuid::Uuid::new_v4().to_string();

    // Register this connection for targeted command delivery
    {
        let mut connections = WS_CONNECTIONS.write().await;
        connections.connections.insert(session_id.clone(), cmd_tx);
    }

    tracing::info!("WebSocket: session {} connected", session_id);

    // Send initial ForceKeepAlive message (Jellyfin protocol)
    let keep_alive = rustyfin_model::ws_event::WsEvent::ForceKeepAlive(60);
    if let Ok(json) = serde_json::to_string(&keep_alive) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    // Send TimeSync event
    let time_sync = rustyfin_model::ws_event::WsEvent::TimeSync {
        data: chrono::Utc::now().timestamp_millis(),
    };
    if let Ok(json) = serde_json::to_string(&time_sync) {
        let _ = sender.send(Message::Text(json.into())).await;
    }

    let period = std::time::Duration::from_secs(WS_TOKEN_RECHECK_SECS);
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + period, period);

    loop {
        tokio::select! {
            // Handle incoming messages from the client
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(incoming) = serde_json::from_str::<WsMessage>(&text) {
                            match incoming.message_type.as_str() {
                                "KeepAlive" => {
                                    // Respond with keep-alive acknowledgement
                                    let resp = rustyfin_model::ws_event::WsEvent::ForceKeepAlive(60);
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = sender.send(Message::Text(json.into())).await;
                                    }
                                }
                                "SessionsStart" | "ScheduledTasksInfoStart" | "ActivityLogEntryStart" => {
                                    // Acknowledge subscription-style messages
                                    tracing::debug!("ws subscription: {}", incoming.message_type);
                                }
                                "SessionsStop" | "ScheduledTasksInfoStop" | "ActivityLogEntryStop" => {
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

            // Handle broadcast events from the server (global events)
            res = broadcast_rx.recv() => {
                match res {
                    // Another user's session and playback events, and logs for non-administrators
                    Ok(event) if !receives(&viewer, &event) => {}
                    Ok(event) => {
                        if let Ok(json) = serde_json::to_string(&event)
                            && sender.send(Message::Text(json.into())).await.is_err() {
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
                        if let Ok(json) = serde_json::to_string(&event)
                            && sender.send(Message::Text(json.into())).await.is_err() {
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

            // Close the socket once its user token stops validating
            _ = recheck.tick(), if user_token => {
                if let Err(rustyfin_auth::AuthError::InvalidToken) =
                    rustyfin_auth::validate_token(&state.db_ro, &token).await
                {
                    tracing::info!("WebSocket: session {} closed: its token is no longer valid", session_id);
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }

    // Clean up: unregister this connection
    {
        let mut connections = WS_CONNECTIONS.write().await;
        connections.connections.remove(&session_id);
        tracing::debug!("WebSocket: unregistered session {}", session_id);
    }
}

/// Send a targeted command to a specific session via WebSocket
pub async fn send_command_to_session(
    session_id: &str,
    command: String,
    arguments: Option<serde_json::Value>,
) -> bool {
    let connections = WS_CONNECTIONS.read().await;
    if let Some(tx) = connections.connections.get(session_id) {
        let event = rustyfin_model::ws_event::WsEvent::Command {
            data: rustyfin_model::ws_event::CommandData {
                command: command.clone(),
                arguments,
            },
        };
        match tx.send(event) {
            Ok(_) => {
                tracing::info!(
                    "WebSocket: command '{}' sent to session {}",
                    command,
                    session_id
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    "WebSocket: failed to send command to session {}: {}",
                    session_id,
                    e
                );
                false
            }
        }
    } else {
        tracing::warn!(
            "WebSocket: session {} not found for command delivery",
            session_id
        );
        false
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
}

/// The socket's user, from the token it connected with. The API key counts as an administrator.
async fn socket_viewer(state: &AppState, token: &str) -> Viewer {
    if state.api_key.as_ref().is_some_and(|key| token == **key) {
        return Viewer {
            user_id: uuid::Uuid::nil(),
            is_admin: true,
        };
    }
    match rustyfin_auth::validate_token(&state.db_ro, token).await {
        Ok(claims) => Viewer {
            user_id: claims.sub,
            is_admin: claims.is_admin,
        },
        Err(_) => Viewer {
            user_id: uuid::Uuid::nil(),
            is_admin: false,
        },
    }
}

/// Whether a socket gets a broadcast event. Session and playback events go to administrators and
/// to the user their `UserId` names, as in Jellyfin's SessionInfoWebSocketListener; log lines go
/// to administrators only. Everything else goes to everyone.
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
        WsEvent::LogMessage { .. } => viewer.is_admin,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::is_api_key;

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
