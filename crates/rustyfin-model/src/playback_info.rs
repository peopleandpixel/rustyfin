use serde::Serialize;

use crate::media_source::MediaSourceResponse;

/// Response from `GET /Audio/{id}/PlaybackInfo` and used internally by the universal endpoint.
/// Matches Jellyfin's `PlaybackInfoResponse`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    /// Media sources for this item
    #[serde(default)]
    pub media_sources: Vec<MediaSourceResponse>,
    /// PlaySessionId for tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
}

/// Response from `GET /Audio/{id}/universal`.
/// Matches Jellyfin's `StreamingResponse` for audio playback.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamingResponse {
    /// Media sources for this item
    #[serde(default)]
    pub media_sources: Vec<MediaSourceResponse>,
    /// URL for direct play/stream
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Analyze progress percent (0-100)
    #[serde(default)]
    pub analyze_duration_linear_percent: u32,
    /// PlaySessionId for tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
}

/// Query parameters for the universal audio endpoint
#[derive(Debug, Default)]
pub struct AudioUniversalQuery {
    /// Max audio bitrate in kbps
    pub max_audio_bitrate: Option<u32>,
    /// Audio stream index to use
    pub audio_stream_index: Option<u32>,
    /// Start time in ticks
    pub start_time_ticks: Option<i64>,
    /// Whether to open the live stream
    pub is_live: bool,
    /// Media source ID (for multi-source items)
    pub media_source_id: Option<String>,
    /// Device ID for profile matching
    pub device_id: Option<String>,
    /// Audio codec preference
    pub audio_codec: Option<String>,
    /// Transcode to HLS
    pub transcode_to_subtitles: Option<bool>,
    /// Enable remote direct play
    pub enable_remote_media: bool,
    /// Enable direct play
    pub enable_direct_play: bool,
    /// Enable direct stream
    pub enable_direct_stream: bool,
}

/// Query parameters for video PlaybackInfo endpoint
#[derive(Debug, Default)]
pub struct VideoPlaybackInfoQuery {
    /// Max video bitrate in kbps
    pub max_video_bitrate: Option<u32>,
    /// Max audio bitrate in kbps
    pub max_audio_bitrate: Option<u32>,
    /// Video stream index to use
    pub video_stream_index: Option<u32>,
    /// Audio stream index to use
    pub audio_stream_index: Option<u32>,
    /// Subtitle stream index to use
    pub subtitle_stream_index: Option<i32>,
    /// Start time in ticks
    pub start_time_ticks: Option<i64>,
    /// Whether to open the live stream
    pub is_live: bool,
    /// Media source ID (for multi-source items)
    pub media_source_id: Option<String>,
    /// Device ID for profile matching
    pub device_id: Option<String>,
    /// Video codec preference
    pub video_codec: Option<String>,
    /// Audio codec preference
    pub audio_codec: Option<String>,
    /// Transcode subtitles
    pub transcode_to_subtitles: Option<String>,
    /// Enable remote direct play
    pub enable_remote_media: bool,
    /// Enable direct play
    pub enable_direct_play: bool,
    /// Enable direct stream
    pub enable_direct_stream: bool,
    /// Enable video stream copying
    pub enable_video_stream_copy: bool,
    /// Enable audio stream copying
    pub enable_audio_stream_copy: bool,
    /// Width for transcoding
    pub width: Option<i32>,
    /// Height for transcoding
    pub height: Option<i32>,
    /// Max streaming bitrate
    pub max_streaming_bitrate: Option<u32>,
    /// Segment time in milliseconds
    pub segment_time: Option<u32>,
}

/// Response from `GET /Videos/{id}/PlaybackInfo`.
/// Matches Jellyfin's `PlaybackInfoResponse` for video playback.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VideoPlaybackInfoResponse {
    /// Media sources for this item
    #[serde(default)]
    pub media_sources: Vec<MediaSourceResponse>,
    /// PlaySessionId for tracking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
    /// Whether the video can be played
    #[serde(default)]
    pub can_play: bool,
}
