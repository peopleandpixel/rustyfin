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

    let token = token.ok_or_else(|| {
        tracing::warn!("WebSocket: missing authentication token");
        ApiError::Unauthorized
    })?;

    // Check if the token matches a configured API key
    let authenticated = if let Some(ref api_key) = state.api_key {
        token == **api_key
    } else {
        // Otherwise validate as JWT
        rustyfin_auth::validate_token(&token, &state.jwt_secret).is_ok()
    };

    if !authenticated {
        tracing::warn!("WebSocket: unauthenticated connection attempt");
        return Err(ApiError::Unauthorized);
    }

    tracing::info!("WebSocket: authenticated connection");
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state)))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to the global broadcast channel for server events
    let mut broadcast_rx = state.ws_broadcast.subscribe();

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
    if let Err(e) = state.ws_broadcast.send(event) {
        tracing::debug!("WebSocket: failed to broadcast event: {}", e);
    }
}
