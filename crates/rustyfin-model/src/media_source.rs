use serde::{Deserialize, Serialize};

/// Represents a media source (file) for a media item, matching Jellyfin's `MediaSourceResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceResponse {
    /// Unique ID for this media source
    pub id: String,
    /// UUID string of the source
    pub uid: String,
    /// File container format (e.g., "mkv", "mp4", "mp3")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Protocol type: "File", "Http", "Rtmp", "Rtsp", "Udp", "Rtp", "Ftp"
    pub protocol: String,
    /// Full path to the media file
    pub path: String,
    /// Source type: "Default", "Gaps"
    #[serde(default = "default_source_type")]
    pub r#type: String,
    /// Runtime in ticks (1 tick = 100 ns)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    /// File size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    /// Total bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    /// Video bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate: Option<u32>,
    /// Audio bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate: Option<u32>,
    /// Video width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Video height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Aspect ratio string (e.g., "16:9")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    /// Whether the source is remote
    #[serde(default)]
    pub is_remote: bool,
    /// Media state: "Missing", "SubtitlesOutOfSync"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_state: Option<String>,
    /// Array of media streams (video, audio, subtitle tracks)
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
    /// Whether direct play is supported
    #[serde(default)]
    pub supports_transcoding: bool,
    #[serde(default)]
    pub supports_direct_stream: bool,
    #[serde(default)]
    pub supports_direct_play: bool,
}

fn default_source_type() -> String {
    "Default".to_string()
}

/// Represents a single media stream (video, audio, or subtitle track), matching Jellyfin's `MediaStream`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    /// Index of the stream within the media source
    pub index: i32,
    /// Codec name (e.g., "h264", "aac", "ass")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Codec type: "Video", "Audio", "Subtitle"
    pub codec_type: String,
    /// Language code (e.g., "eng", "deu")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Time base fraction (e.g., "1/1000")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_base: Option<String>,
    /// Codec time base (e.g., "1/48000")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_time_base: Option<String>,
    /// Codec tag string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_tag: Option<String>,
    /// Whether the stream is external (separate file)
    #[serde(default)]
    pub is_external: bool,
    /// Path to external subtitle file, if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    // Video-specific fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_frame_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_frame_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Subtitle type: "External", "Embedded", "EmbedTimeCodes", "ExternalUrl"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_transfer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_primaries: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_uri: Option<String>,
    /// Whether this is the default stream
    #[serde(default)]
    pub is_default: bool,
    /// Whether this stream is forced
    #[serde(default)]
    pub is_forced: bool,
    /// Whether video is interlaced
    #[serde(default)]
    pub is_interlaced: bool,
    /// Whether subtitles have embedded bitmaps
    #[serde(default)]
    pub embedded_bitmaps: bool,
}

/// What identifies a media stream across rescans. A remembered track choice is offered only where
/// the stream at its index has the same key, so a replaced file whose index now names another
/// stream (a commentary, a dub) does not hand the choice to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamIdentity<'a> {
    pub codec_type: &'a str,
    pub codec: Option<&'a str>,
    pub language: Option<&'a str>,
    pub title: Option<&'a str>,
    pub is_external: bool,
    pub path: Option<&'a str>,
    pub is_default: bool,
    pub is_forced: bool,
}

impl StreamIdentity<'_> {
    /// The key as canonical JSON: the codec type, codec and language lowercased; the title; the
    /// external flag; an external stream's file name (else null); the default and forced flags.
    pub fn key(&self) -> String {
        let file_name = self
            .path
            .filter(|_| self.is_external)
            .and_then(|path| path.rsplit(['/', '\\']).next());
        serde_json::json!([
            self.codec_type.to_ascii_lowercase(),
            self.codec.map(str::to_ascii_lowercase),
            self.language.map(str::to_ascii_lowercase),
            self.title,
            self.is_external,
            file_name,
            self.is_default,
            self.is_forced,
        ])
        .to_string()
    }
}

impl MediaStream {
    /// This stream's identity (see `StreamIdentity`).
    pub fn identity(&self) -> StreamIdentity<'_> {
        StreamIdentity {
            codec_type: &self.codec_type,
            codec: self.codec.as_deref(),
            language: self.language.as_deref(),
            title: self.title.as_deref(),
            is_external: self.is_external,
            path: self.path.as_deref(),
            is_default: self.is_default,
            is_forced: self.is_forced,
        }
    }
}
