use serde::{Deserialize, Serialize};

/// WebSocket event types sent by the server to connected clients.
/// Matches Jellyfin's real-time notification protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "MessageType", rename_all = "PascalCase")]
pub enum WsEvent {
    /// Keep-alive ping from server
    ForceKeepAlive(i64),
    /// Playback started on a session
    PlaybackStart {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// Playback stopped on a session
    PlaybackStop {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// Playback position updated
    PlaybackProgress {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// Library content changed (scan completed, items added/removed)
    LibraryChanged {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// A scheduled task started
    ScheduledTaskStarted {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// A scheduled task completed
    ScheduledTaskCompleted {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// Session state changed (new session, session updated)
    SessionChanged {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// User profile updated
    UserUpdated {
        #[serde(flatten)]
        data: serde_json::Value,
    },
    /// System time sync
    TimeSync {
        #[serde(rename = "Data")]
        data: i64,
    },
    /// Remote command delivered to a session
    Command {
        #[serde(rename = "Data")]
        data: CommandData,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CommandData {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}
