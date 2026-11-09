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
    /// Live log message streamed to admin UI
    LogMessage {
        #[serde(rename = "Data")]
        data: LogMessageData,
    },
    /// A library scan's progress: "0" when it is queued, each whole percent, then "100" at its end
    RefreshProgress {
        #[serde(rename = "Data")]
        data: RefreshProgressData,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogMessageData {
    /// Timestamp in ISO 8601
    pub timestamp: String,
    /// Log level: DEBUG, INFO, WARN, ERROR, TRACE
    pub level: String,
    /// The log message
    pub message: String,
    /// Target module (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// The `Data` of a `RefreshProgress` event: one library's scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RefreshProgressData {
    /// The library's id, hyphenated
    pub item_id: String,
    /// The whole percent done, "0" to "100", as text
    pub progress: String,
    /// The scan's state: "Queued" or "Active" while it runs, "Idle" once it has ended
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CommandData {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
}
