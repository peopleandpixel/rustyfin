//! The PlaybackInfo request for `GET`/`POST /Items/{itemId}/PlaybackInfo`: request parsing and the
//! response shape.

use std::collections::HashSet;
use std::str::FromStr;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::device_profile::{
    DeviceProfile, ProfileError, bool_text, lookup, opt_bool, opt_int, opt_text,
};

/// Playback parameters of a PlaybackInfo request. The query string and the JSON body share this
/// shape. `None` means the client did not send the field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaybackInfoDto {
    /// Requesting user
    pub user_id: Option<Uuid>,
    /// Maximum streaming bitrate, in bits per second
    pub max_streaming_bitrate: Option<i32>,
    /// Start position, in ticks
    pub start_time_ticks: Option<i64>,
    /// Audio stream to select
    pub audio_stream_index: Option<i32>,
    /// Subtitle stream to select
    pub subtitle_stream_index: Option<i32>,
    /// Maximum audio channels
    pub max_audio_channels: Option<i32>,
    /// Media source to use
    pub media_source_id: Option<String>,
    /// Live stream to open
    pub live_stream_id: Option<String>,
    /// Client capabilities (body only)
    pub device_profile: Option<DeviceProfile>,
    /// Whether direct play is allowed
    pub enable_direct_play: Option<bool>,
    /// Whether direct stream (remux) is allowed
    pub enable_direct_stream: Option<bool>,
    /// Whether transcoding is allowed
    pub enable_transcoding: Option<bool>,
    /// Whether the video stream may be copied
    pub allow_video_stream_copy: Option<bool>,
    /// Whether the audio stream may be copied
    pub allow_audio_stream_copy: Option<bool>,
    /// Whether to open the live stream automatically
    pub auto_open_live_stream: Option<bool>,
    /// Whether subtitles are burnt in when transcoding (body only)
    pub always_burn_in_subtitle_when_transcoding: Option<bool>,
}

impl PlaybackInfoDto {
    /// Parses a request body. An empty, whitespace-only or `null` body gives the default. Invalid
    /// JSON, a non-object, or a field of the wrong type is an error.
    pub fn from_body(body: &[u8]) -> Result<Self, ProfileError> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_slice(body)
            .map_err(|e| ProfileError(format!("invalid JSON body: {e}")))?;
        match value {
            Value::Null => Ok(Self::default()),
            Value::Object(map) => Self::from_object(&map),
            _ => Err(ProfileError("body must be a JSON object".to_string())),
        }
    }

    /// Parses query pairs. Only 14 field names are read, ignoring ASCII case. The first occurrence
    /// of a name decides, even when it is empty or invalid, and an empty value counts as absent.
    pub fn from_query_pairs(pairs: &[(String, String)]) -> Result<Self, ProfileError> {
        let mut dto = Self::default();
        let mut seen = HashSet::new();
        for (key, value) in pairs {
            let name = key.to_ascii_lowercase();
            let is_new = seen.insert(name.clone());
            if !is_new || value.is_empty() {
                continue;
            }
            dto.set_query_value(&name, value)?;
        }
        Ok(dto)
    }

    /// Merges a query-derived DTO over a body-derived one. Each field takes the query value if it
    /// has one, else the body value.
    pub fn or_body(self, body: PlaybackInfoDto) -> PlaybackInfoDto {
        PlaybackInfoDto {
            user_id: self.user_id.or(body.user_id),
            max_streaming_bitrate: self.max_streaming_bitrate.or(body.max_streaming_bitrate),
            start_time_ticks: self.start_time_ticks.or(body.start_time_ticks),
            audio_stream_index: self.audio_stream_index.or(body.audio_stream_index),
            subtitle_stream_index: self.subtitle_stream_index.or(body.subtitle_stream_index),
            max_audio_channels: self.max_audio_channels.or(body.max_audio_channels),
            media_source_id: self.media_source_id.or(body.media_source_id),
            live_stream_id: self.live_stream_id.or(body.live_stream_id),
            device_profile: self.device_profile.or(body.device_profile),
            enable_direct_play: self.enable_direct_play.or(body.enable_direct_play),
            enable_direct_stream: self.enable_direct_stream.or(body.enable_direct_stream),
            enable_transcoding: self.enable_transcoding.or(body.enable_transcoding),
            allow_video_stream_copy: self
                .allow_video_stream_copy
                .or(body.allow_video_stream_copy),
            allow_audio_stream_copy: self
                .allow_audio_stream_copy
                .or(body.allow_audio_stream_copy),
            auto_open_live_stream: self.auto_open_live_stream.or(body.auto_open_live_stream),
            always_burn_in_subtitle_when_transcoding: self
                .always_burn_in_subtitle_when_transcoding
                .or(body.always_burn_in_subtitle_when_transcoding),
        }
    }

    /// Reads the body fields from a JSON object. Keys match ignoring ASCII case.
    fn from_object(map: &Map<String, Value>) -> Result<Self, ProfileError> {
        Ok(Self {
            user_id: body_user_id(map)?,
            max_streaming_bitrate: opt_int(map, "MaxStreamingBitrate")?,
            start_time_ticks: opt_int(map, "StartTimeTicks")?,
            audio_stream_index: opt_int(map, "AudioStreamIndex")?,
            subtitle_stream_index: opt_int(map, "SubtitleStreamIndex")?,
            max_audio_channels: opt_int(map, "MaxAudioChannels")?,
            media_source_id: opt_text(map, "MediaSourceId")?,
            live_stream_id: opt_text(map, "LiveStreamId")?,
            device_profile: match lookup(map, "DeviceProfile") {
                None | Some(Value::Null) => None,
                Some(v) => Some(DeviceProfile::from_json(v)?),
            },
            enable_direct_play: opt_bool(map, "EnableDirectPlay")?,
            enable_direct_stream: opt_bool(map, "EnableDirectStream")?,
            enable_transcoding: opt_bool(map, "EnableTranscoding")?,
            allow_video_stream_copy: opt_bool(map, "AllowVideoStreamCopy")?,
            allow_audio_stream_copy: opt_bool(map, "AllowAudioStreamCopy")?,
            auto_open_live_stream: opt_bool(map, "AutoOpenLiveStream")?,
            always_burn_in_subtitle_when_transcoding: opt_bool(
                map,
                "AlwaysBurnInSubtitleWhenTranscoding",
            )?,
        })
    }

    /// Sets the field that a lowercase query name refers to. Unknown names are ignored.
    fn set_query_value(&mut self, name: &str, value: &str) -> Result<(), ProfileError> {
        match name {
            "userid" => self.user_id = Some(parse_uuid(name, value)?),
            "maxstreamingbitrate" => self.max_streaming_bitrate = Some(query_num(name, value)?),
            "starttimeticks" => self.start_time_ticks = Some(query_num(name, value)?),
            "audiostreamindex" => self.audio_stream_index = Some(query_num(name, value)?),
            "subtitlestreamindex" => self.subtitle_stream_index = Some(query_num(name, value)?),
            "maxaudiochannels" => self.max_audio_channels = Some(query_num(name, value)?),
            "mediasourceid" => self.media_source_id = Some(value.to_string()),
            "livestreamid" => self.live_stream_id = Some(value.to_string()),
            "enabledirectplay" => self.enable_direct_play = Some(query_bool(name, value)?),
            "enabledirectstream" => self.enable_direct_stream = Some(query_bool(name, value)?),
            "enabletranscoding" => self.enable_transcoding = Some(query_bool(name, value)?),
            "allowvideostreamcopy" => self.allow_video_stream_copy = Some(query_bool(name, value)?),
            "allowaudiostreamcopy" => self.allow_audio_stream_copy = Some(query_bool(name, value)?),
            "autoopenlivestream" => self.auto_open_live_stream = Some(query_bool(name, value)?),
            _ => {}
        }
        Ok(())
    }
}

/// `UserId` from a body: `""` and `null` give `None`, and anything else must be a UUID.
fn body_user_id(map: &Map<String, Value>) -> Result<Option<Uuid>, ProfileError> {
    match opt_text(map, "UserId")? {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => parse_uuid("UserId", &s).map(Some),
    }
}

/// A UUID in any form `Uuid::parse_str` accepts.
fn parse_uuid(name: &str, value: &str) -> Result<Uuid, ProfileError> {
    Uuid::parse_str(value).map_err(|_| ProfileError(format!("{name} must be a UUID")))
}

/// A number from a query value, parsed with `str::parse`.
fn query_num<T: FromStr>(name: &str, value: &str) -> Result<T, ProfileError> {
    value
        .parse::<T>()
        .map_err(|_| ProfileError(format!("{name} must be a number")))
}

/// A bool from a query value: "true" or "false" in any case.
fn query_bool(name: &str, value: &str) -> Result<bool, ProfileError> {
    bool_text(value).ok_or_else(|| ProfileError(format!("{name} must be true or false")))
}

/// Why a PlaybackInfo request was refused. Serialises as the variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PlaybackErrorCode {
    /// The client may not play this item
    NotAllowed,
    /// No stream is compatible with the device
    NoCompatibleStream,
    /// Too many requests
    RateLimitExceeded,
}

/// The PlaybackInfo response body. Keys are PascalCase, and unset options are left out.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsPlaybackInfoResponse {
    /// Media sources, as JSON objects
    pub media_sources: Vec<Value>,
    /// Play session id, when a session was created
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_session_id: Option<String>,
    /// Refusal reason, when playback is refused
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<PlaybackErrorCode>,
}
