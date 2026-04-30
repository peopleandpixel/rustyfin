use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/socket", get(ws_handler))
}

/// Jellyfin WebSocket message envelope
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WsMessage {
    message_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    // Send initial ForceKeepAlive message (Jellyfin protocol)
    let keep_alive = WsMessage {
        message_type: "ForceKeepAlive".to_string(),
        message_id: None,
        data: Some(serde_json::json!(60)),
    };
    if let Ok(json) = serde_json::to_string(&keep_alive) {
        let _ = socket.send(Message::Text(json.into())).await;
    }

    // Main message loop
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                if let Ok(incoming) = serde_json::from_str::<WsMessage>(&text) {
                    match incoming.message_type.as_str() {
                        "KeepAlive" => {
                            // Respond with keep-alive acknowledgement
                            let resp = WsMessage {
                                message_type: "ForceKeepAlive".to_string(),
                                message_id: None,
                                data: Some(serde_json::json!(60)),
                            };
                            if let Ok(json) = serde_json::to_string(&resp) {
                                let _ = socket.send(Message::Text(json.into())).await;
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
            Message::Ping(data) => {
                let _ = socket.send(Message::Pong(data)).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}
