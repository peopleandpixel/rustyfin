//! Jellyfin's `DeviceProfile` and the types inside it, parsed from client JSON.
//!
//! Parsing follows Jellyfin's System.Text.Json rules, made lenient where Jellyfin would fail:
//! keys match field names ignoring ASCII case, unknown keys are ignored, and enums never fail.

use std::str::FromStr;

use serde_json::{Map, Value};

/// Default for `MaxStreamingBitrate`, `MaxStaticBitrate` and `MaxStaticMusicBitrate`, in bits per second.
const DEFAULT_BITRATE: i32 = 8_000_000;
/// Default for `MusicStreamingTranscodingBitrate`, in bits per second.
const DEFAULT_MUSIC_BITRATE: i32 = 128_000;

/// A JSON value that breaks the device profile rules. The message names the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub String);

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

/// Reads an enum from JSON. A variant name (ignoring ASCII case), or a defined number given as a
/// JSON number or numeric string, selects that variant. `null` gives the default and anything else
/// gives `unknown`.
fn enum_from_json<T: Copy + Default>(v: &Value, defined: &[(&str, i64, T)], unknown: T) -> T {
    let number = match v {
        Value::Null => return T::default(),
        Value::String(s) => {
            if let Some((_, _, t)) = defined
                .iter()
                .find(|(name, _, _)| name.eq_ignore_ascii_case(s))
            {
                return *t;
            }
            s.parse::<i64>().ok()
        }
        Value::Number(n) => n.as_i64(),
        _ => None,
    };
    number
        .and_then(|n| defined.iter().find(|(_, value, _)| *value == n))
        .map_or(unknown, |(_, _, t)| *t)
}

/// Jellyfin's name for an enum value. A value missing from `defined` (only `Unknown`) gives "Unknown".
fn enum_as_str<T: Copy + PartialEq>(defined: &[(&'static str, i64, T)], v: T) -> &'static str {
    defined
        .iter()
        .find(|(_, _, t)| *t == v)
        .map_or("Unknown", |(name, _, _)| *name)
}

/// Kind of media a profile applies to (Jellyfin's `DlnaProfileType`: Audio 0, Video 1, Photo 2,
/// Subtitle 3, Lyric 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProfileType {
    #[default]
    Audio,
    Video,
    Photo,
    Subtitle,
    Lyric,
    /// Any value that is not defined
    Unknown,
}

impl ProfileType {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("Audio", 0, Self::Audio),
        ("Video", 1, Self::Video),
        ("Photo", 2, Self::Photo),
        ("Subtitle", 3, Self::Subtitle),
        ("Lyric", 4, Self::Lyric),
    ];

    /// Parses a profile type. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "Video".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Which streams a codec profile applies to (Jellyfin's `CodecType`: Video 0, VideoAudio 1, Audio 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CodecType {
    #[default]
    Video,
    VideoAudio,
    Audio,
    /// Any value that is not defined
    Unknown,
}

impl CodecType {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("Video", 0, Self::Video),
        ("VideoAudio", 1, Self::VideoAudio),
        ("Audio", 2, Self::Audio),
    ];

    /// Parses a codec type. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "VideoAudio".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Whether a transcode is for streaming or for a static file (Jellyfin's `EncodingContext`:
/// Streaming 0, Static 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EncodingContext {
    #[default]
    Streaming,
    Static,
    /// Any value that is not defined
    Unknown,
}

impl EncodingContext {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("Streaming", 0, Self::Streaming),
        ("Static", 1, Self::Static),
    ];

    /// Parses an encoding context. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "Static".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Delivery protocol of a transcode (Jellyfin's `MediaStreamProtocol`: Http 0, Hls 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StreamProtocol {
    #[default]
    Http,
    Hls,
    /// Any value that is not defined
    Unknown,
}

impl StreamProtocol {
    const DEFINED: &[(&str, i64, Self)] = &[("http", 0, Self::Http), ("hls", 1, Self::Hls)];

    /// Parses a stream protocol. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// The protocol's name: "http" or "hls".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// How a subtitle track is delivered (Jellyfin's `SubtitleDeliveryMethod`: Encode 0, Embed 1,
/// External 2, Hls 3, Drop 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SubtitleDeliveryMethod {
    #[default]
    Encode,
    Embed,
    External,
    Hls,
    Drop,
    /// Any value that is not defined
    Unknown,
}

impl SubtitleDeliveryMethod {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("Encode", 0, Self::Encode),
        ("Embed", 1, Self::Embed),
        ("External", 2, Self::External),
        ("Hls", 3, Self::Hls),
        ("Drop", 4, Self::Drop),
    ];

    /// Parses a delivery method. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "External".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Comparison a profile condition makes (Jellyfin's `ProfileConditionType`: Equals 0, NotEquals 1,
/// LessThanEqual 2, GreaterThanEqual 3, EqualsAny 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConditionType {
    #[default]
    Equals,
    NotEquals,
    LessThanEqual,
    GreaterThanEqual,
    EqualsAny,
    /// Any value that is not defined
    Unknown,
}

impl ConditionType {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("Equals", 0, Self::Equals),
        ("NotEquals", 1, Self::NotEquals),
        ("LessThanEqual", 2, Self::LessThanEqual),
        ("GreaterThanEqual", 3, Self::GreaterThanEqual),
        ("EqualsAny", 4, Self::EqualsAny),
    ];

    /// Parses a condition type. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "LessThanEqual".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Stream property a condition tests (Jellyfin's `ProfileConditionValue`; its numbers run 0 to 26
/// and skip 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConditionProperty {
    #[default]
    AudioChannels,
    AudioBitrate,
    AudioProfile,
    Width,
    Height,
    Has64BitOffsets,
    PacketLength,
    VideoBitDepth,
    VideoBitrate,
    VideoFramerate,
    VideoLevel,
    VideoProfile,
    VideoTimestamp,
    IsAnamorphic,
    RefFrames,
    NumAudioStreams,
    NumVideoStreams,
    IsSecondaryAudio,
    VideoCodecTag,
    IsAvc,
    IsInterlaced,
    AudioSampleRate,
    AudioBitDepth,
    VideoRangeType,
    NumStreams,
    VideoRotation,
    /// Any value that is not defined
    Unknown,
}

impl ConditionProperty {
    const DEFINED: &[(&str, i64, Self)] = &[
        ("AudioChannels", 0, Self::AudioChannels),
        ("AudioBitrate", 1, Self::AudioBitrate),
        ("AudioProfile", 2, Self::AudioProfile),
        ("Width", 3, Self::Width),
        ("Height", 4, Self::Height),
        ("Has64BitOffsets", 5, Self::Has64BitOffsets),
        ("PacketLength", 6, Self::PacketLength),
        ("VideoBitDepth", 7, Self::VideoBitDepth),
        ("VideoBitrate", 8, Self::VideoBitrate),
        ("VideoFramerate", 9, Self::VideoFramerate),
        ("VideoLevel", 10, Self::VideoLevel),
        ("VideoProfile", 11, Self::VideoProfile),
        ("VideoTimestamp", 12, Self::VideoTimestamp),
        ("IsAnamorphic", 13, Self::IsAnamorphic),
        ("RefFrames", 14, Self::RefFrames),
        ("NumAudioStreams", 16, Self::NumAudioStreams),
        ("NumVideoStreams", 17, Self::NumVideoStreams),
        ("IsSecondaryAudio", 18, Self::IsSecondaryAudio),
        ("VideoCodecTag", 19, Self::VideoCodecTag),
        ("IsAvc", 20, Self::IsAvc),
        ("IsInterlaced", 21, Self::IsInterlaced),
        ("AudioSampleRate", 22, Self::AudioSampleRate),
        ("AudioBitDepth", 23, Self::AudioBitDepth),
        ("VideoRangeType", 24, Self::VideoRangeType),
        ("NumStreams", 25, Self::NumStreams),
        ("VideoRotation", 26, Self::VideoRotation),
    ];

    /// Parses a condition property. Never fails: `null` gives the default and anything unrecognised
    /// (including 15) `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "VideoBitDepth".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// Whether a transcode seeks by time or by bytes (Jellyfin's `TranscodeSeekInfo`: Auto 0, Bytes 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TranscodeSeekInfo {
    #[default]
    Auto,
    Bytes,
    /// Any value that is not defined
    Unknown,
}

impl TranscodeSeekInfo {
    const DEFINED: &[(&str, i64, Self)] = &[("Auto", 0, Self::Auto), ("Bytes", 1, Self::Bytes)];

    /// Parses a seek mode. Never fails: `null` gives the default and anything unrecognised `Unknown`.
    pub fn from_json(v: &Value) -> Self {
        enum_from_json(v, Self::DEFINED, Self::Unknown)
    }

    /// Jellyfin's name for this value, e.g. "Bytes".
    pub fn as_str(self) -> &'static str {
        enum_as_str(Self::DEFINED, self)
    }
}

/// A format the client plays without transcoding (Jellyfin's `DirectPlayProfile`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DirectPlayProfile {
    /// Containers, comma-separated (e.g. "mp4,m4v")
    pub container: String,
    /// Audio codecs, comma-separated
    pub audio_codec: String,
    /// Video codecs, comma-separated
    pub video_codec: String,
    /// Kind of media
    pub r#type: ProfileType,
}

impl DirectPlayProfile {
    /// Parses one entry. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "direct play profile")?;
        Ok(Self {
            container: text(map, "Container")?,
            audio_codec: text(map, "AudioCodec")?,
            video_codec: text(map, "VideoCodec")?,
            r#type: ProfileType::from_json(field(map, "Type")),
        })
    }
}

/// A transcode target the client accepts (Jellyfin's `TranscodingProfile`).
#[derive(Debug, Clone, PartialEq)]
pub struct TranscodingProfile {
    /// Container of the transcode
    pub container: String,
    /// Kind of media
    pub r#type: ProfileType,
    /// Video codec of the transcode
    pub video_codec: String,
    /// Audio codec of the transcode
    pub audio_codec: String,
    /// Delivery protocol
    pub protocol: StreamProtocol,
    /// Whether the client should estimate the content length
    pub estimate_content_length: bool,
    /// Whether to use MPEG-TS/M2TS mode
    pub enable_mpegts_m2ts_mode: bool,
    /// Whether the transcode seeks by time or by bytes
    pub transcode_seek_info: TranscodeSeekInfo,
    /// Whether to copy timestamps from the source
    pub copy_timestamps: bool,
    /// Streaming or static transcode
    pub context: EncodingContext,
    /// Whether subtitles are included in the manifest
    pub enable_subtitles_in_manifest: bool,
    /// Maximum audio channels, as text
    pub max_audio_channels: Option<String>,
    /// Minimum number of segments
    pub min_segments: i32,
    /// Segment length
    pub segment_length: i32,
    /// Whether to break on non-key frames
    pub break_on_non_key_frames: Option<bool>,
    /// Conditions that must hold for this profile
    pub conditions: Vec<ProfileCondition>,
    /// Whether audio uses variable bitrate encoding (default true)
    pub enable_audio_vbr_encoding: bool,
}

impl Default for TranscodingProfile {
    /// Jellyfin's defaults for a profile with no keys.
    fn default() -> Self {
        Self {
            container: String::new(),
            r#type: ProfileType::default(),
            video_codec: String::new(),
            audio_codec: String::new(),
            protocol: StreamProtocol::default(),
            estimate_content_length: false,
            enable_mpegts_m2ts_mode: false,
            transcode_seek_info: TranscodeSeekInfo::default(),
            copy_timestamps: false,
            context: EncodingContext::default(),
            enable_subtitles_in_manifest: false,
            max_audio_channels: None,
            min_segments: 0,
            segment_length: 0,
            break_on_non_key_frames: None,
            conditions: Vec::new(),
            enable_audio_vbr_encoding: true,
        }
    }
}

impl TranscodingProfile {
    /// Parses one entry. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "transcoding profile")?;
        Ok(Self {
            container: text(map, "Container")?,
            r#type: ProfileType::from_json(field(map, "Type")),
            video_codec: text(map, "VideoCodec")?,
            audio_codec: text(map, "AudioCodec")?,
            protocol: StreamProtocol::from_json(field(map, "Protocol")),
            estimate_content_length: bool_or(map, "EstimateContentLength", false)?,
            enable_mpegts_m2ts_mode: bool_or(map, "EnableMpegtsM2TsMode", false)?,
            transcode_seek_info: TranscodeSeekInfo::from_json(field(map, "TranscodeSeekInfo")),
            copy_timestamps: bool_or(map, "CopyTimestamps", false)?,
            context: EncodingContext::from_json(field(map, "Context")),
            enable_subtitles_in_manifest: bool_or(map, "EnableSubtitlesInManifest", false)?,
            max_audio_channels: opt_text(map, "MaxAudioChannels")?,
            min_segments: opt_int::<i32>(map, "MinSegments")?.unwrap_or(0),
            segment_length: opt_int::<i32>(map, "SegmentLength")?.unwrap_or(0),
            break_on_non_key_frames: opt_bool(map, "BreakOnNonKeyFrames")?,
            conditions: array(map, "Conditions", ProfileCondition::from_json)?,
            enable_audio_vbr_encoding: bool_or(map, "EnableAudioVbrEncoding", true)?,
        })
    }
}

/// Container-level rules (Jellyfin's `ContainerProfile`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContainerProfile {
    /// Kind of media
    pub r#type: ProfileType,
    /// Conditions that must hold
    pub conditions: Vec<ProfileCondition>,
    /// Containers, comma-separated
    pub container: String,
    /// Sub-containers, comma-separated
    pub sub_container: String,
}

impl ContainerProfile {
    /// Parses one entry. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "container profile")?;
        Ok(Self {
            r#type: ProfileType::from_json(field(map, "Type")),
            conditions: array(map, "Conditions", ProfileCondition::from_json)?,
            container: text(map, "Container")?,
            sub_container: text(map, "SubContainer")?,
        })
    }
}

/// Codec-level rules (Jellyfin's `CodecProfile`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodecProfile {
    /// Which streams the rule covers
    pub r#type: CodecType,
    /// Conditions a stream must meet when the rule applies
    pub conditions: Vec<ProfileCondition>,
    /// Conditions that decide whether the rule applies
    pub apply_conditions: Vec<ProfileCondition>,
    /// Codecs, comma-separated
    pub codec: String,
    /// Containers, comma-separated
    pub container: String,
    /// Sub-containers, comma-separated
    pub sub_container: String,
}

impl CodecProfile {
    /// Parses one entry. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "codec profile")?;
        Ok(Self {
            r#type: CodecType::from_json(field(map, "Type")),
            conditions: array(map, "Conditions", ProfileCondition::from_json)?,
            apply_conditions: array(map, "ApplyConditions", ProfileCondition::from_json)?,
            codec: text(map, "Codec")?,
            container: text(map, "Container")?,
            sub_container: text(map, "SubContainer")?,
        })
    }
}

/// One test on a stream property (Jellyfin's `ProfileCondition`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCondition {
    /// Comparison to make
    pub condition: ConditionType,
    /// Property to test
    pub property: ConditionProperty,
    /// Value to compare against
    pub value: String,
    /// Whether the condition must hold (default true)
    pub is_required: bool,
}

impl Default for ProfileCondition {
    /// Jellyfin's defaults for a condition with no keys.
    fn default() -> Self {
        Self {
            condition: ConditionType::default(),
            property: ConditionProperty::default(),
            value: String::new(),
            is_required: true,
        }
    }
}

impl ProfileCondition {
    /// Parses one condition. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "profile condition")?;
        Ok(Self {
            condition: ConditionType::from_json(field(map, "Condition")),
            property: ConditionProperty::from_json(field(map, "Property")),
            value: text(map, "Value")?,
            is_required: bool_or(map, "IsRequired", true)?,
        })
    }
}

/// A subtitle format the client supports (Jellyfin's `SubtitleProfile`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubtitleProfile {
    /// Subtitle format, e.g. "vtt"
    pub format: String,
    /// How the subtitle is delivered
    pub method: SubtitleDeliveryMethod,
    /// DIDL mode
    pub didl_mode: String,
    /// Languages, comma-separated
    pub language: String,
    /// Container the subtitle is carried in
    pub container: String,
}

impl SubtitleProfile {
    /// Parses one entry. `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "subtitle profile")?;
        Ok(Self {
            format: text(map, "Format")?,
            method: SubtitleDeliveryMethod::from_json(field(map, "Method")),
            didl_mode: text(map, "DidlMode")?,
            language: text(map, "Language")?,
            container: text(map, "Container")?,
        })
    }
}

/// A client's playback capabilities (Jellyfin's `DeviceProfile`), as sent with PlaybackInfo.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProfile {
    /// Profile name, if sent
    pub name: Option<String>,
    /// Profile id, if sent
    pub id: Option<String>,
    /// Maximum streaming bitrate, in bits per second
    pub max_streaming_bitrate: Option<i32>,
    /// Maximum bitrate for static media, in bits per second
    pub max_static_bitrate: Option<i32>,
    /// Bitrate for music transcodes, in bits per second
    pub music_streaming_transcoding_bitrate: Option<i32>,
    /// Maximum bitrate for static music, in bits per second
    pub max_static_music_bitrate: Option<i32>,
    /// Formats played without transcoding
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    /// Transcode targets the client accepts
    pub transcoding_profiles: Vec<TranscodingProfile>,
    /// Container-level rules
    pub container_profiles: Vec<ContainerProfile>,
    /// Codec-level rules
    pub codec_profiles: Vec<CodecProfile>,
    /// Subtitle formats the client supports
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

impl Default for DeviceProfile {
    /// Jellyfin's defaults for a profile with no keys.
    fn default() -> Self {
        Self {
            name: None,
            id: None,
            max_streaming_bitrate: Some(DEFAULT_BITRATE),
            max_static_bitrate: Some(DEFAULT_BITRATE),
            music_streaming_transcoding_bitrate: Some(DEFAULT_MUSIC_BITRATE),
            max_static_music_bitrate: Some(DEFAULT_BITRATE),
            direct_play_profiles: Vec::new(),
            transcoding_profiles: Vec::new(),
            container_profiles: Vec::new(),
            codec_profiles: Vec::new(),
            subtitle_profiles: Vec::new(),
        }
    }
}

impl DeviceProfile {
    /// Parses a device profile. A missing bitrate takes Jellyfin's default and `null` gives `None`.
    /// `Err` if the value is not an object or a field has the wrong type.
    pub fn from_json(v: &Value) -> Result<Self, ProfileError> {
        let map = object(v, "device profile")?;
        Ok(Self {
            name: opt_text(map, "Name")?,
            id: opt_text(map, "Id")?,
            max_streaming_bitrate: i32_or_default(map, "MaxStreamingBitrate", DEFAULT_BITRATE)?,
            max_static_bitrate: i32_or_default(map, "MaxStaticBitrate", DEFAULT_BITRATE)?,
            music_streaming_transcoding_bitrate: i32_or_default(
                map,
                "MusicStreamingTranscodingBitrate",
                DEFAULT_MUSIC_BITRATE,
            )?,
            max_static_music_bitrate: i32_or_default(
                map,
                "MaxStaticMusicBitrate",
                DEFAULT_BITRATE,
            )?,
            direct_play_profiles: array(map, "DirectPlayProfiles", DirectPlayProfile::from_json)?,
            transcoding_profiles: array(map, "TranscodingProfiles", TranscodingProfile::from_json)?,
            container_profiles: array(map, "ContainerProfiles", ContainerProfile::from_json)?,
            codec_profiles: array(map, "CodecProfiles", CodecProfile::from_json)?,
            subtitle_profiles: array(map, "SubtitleProfiles", SubtitleProfile::from_json)?,
        })
    }
}

/// The object under `v`, or an error naming `what` when `v` is not an object.
fn object<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, ProfileError> {
    v.as_object()
        .ok_or_else(|| ProfileError(format!("{what} must be a JSON object")))
}

/// The value under `name`, matching keys ignoring ASCII case. `None` when the key is missing.
pub(crate) fn lookup<'a>(map: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// The value under `name`, or `null` when the key is missing, so enums and defaults apply.
fn field<'a>(map: &'a Map<String, Value>, name: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    lookup(map, name).unwrap_or(&NULL)
}

/// A string field. A number or bool is taken as its text. `null` and a missing key give "".
pub(crate) fn text(map: &Map<String, Value>, name: &str) -> Result<String, ProfileError> {
    Ok(opt_text(map, name)?.unwrap_or_default())
}

/// An optional string field, read as [`text`]. `null` and a missing key give `None`.
pub(crate) fn opt_text(
    map: &Map<String, Value>,
    name: &str,
) -> Result<Option<String>, ProfileError> {
    match field(map, name) {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        Value::Number(n) => Ok(Some(n.to_string())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        _ => Err(ProfileError(format!("{name} must be text"))),
    }
}

/// An integer from a JSON integer or a string holding one. `null` gives `None`. A fraction, a
/// value outside `T`, or any other type is an error.
fn int_value<T: FromStr + TryFrom<i64>>(v: &Value, name: &str) -> Result<Option<T>, ProfileError> {
    let parsed = match v {
        Value::Null => return Ok(None),
        Value::Number(n) => n.as_i64().and_then(|n| T::try_from(n).ok()),
        Value::String(s) => s.parse::<T>().ok(),
        _ => None,
    };
    parsed
        .map(Some)
        .ok_or_else(|| ProfileError(format!("{name} must be an integer")))
}

/// An optional integer field. `null` and a missing key give `None`.
pub(crate) fn opt_int<T: FromStr + TryFrom<i64>>(
    map: &Map<String, Value>,
    name: &str,
) -> Result<Option<T>, ProfileError> {
    match lookup(map, name) {
        None => Ok(None),
        Some(v) => int_value(v, name),
    }
}

/// An i32 field whose missing key means `default`. An explicit `null` still gives `None`.
fn i32_or_default(
    map: &Map<String, Value>,
    name: &str,
    default: i32,
) -> Result<Option<i32>, ProfileError> {
    match lookup(map, name) {
        None => Ok(Some(default)),
        Some(v) => int_value(v, name),
    }
}

/// Reads "true" or "false" in any case, as text.
pub(crate) fn bool_text(s: &str) -> Option<bool> {
    if s.eq_ignore_ascii_case("true") {
        Some(true)
    } else if s.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// An optional bool: a JSON bool, or "true" or "false" as text. `null` and a missing key give `None`.
pub(crate) fn opt_bool(map: &Map<String, Value>, name: &str) -> Result<Option<bool>, ProfileError> {
    let bad = || ProfileError(format!("{name} must be true or false"));
    match field(map, name) {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        Value::String(s) => bool_text(s).map(Some).ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// A bool field that is `default` when `null` or missing.
fn bool_or(map: &Map<String, Value>, name: &str, default: bool) -> Result<bool, ProfileError> {
    Ok(opt_bool(map, name)?.unwrap_or(default))
}

/// An array of objects, each parsed by `parse`. `null` and a missing key give an empty Vec, `null`
/// elements are skipped, and any other non-array value is an error.
fn array<T>(
    map: &Map<String, Value>,
    name: &str,
    parse: impl Fn(&Value) -> Result<T, ProfileError>,
) -> Result<Vec<T>, ProfileError> {
    match field(map, name) {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items.iter().filter(|v| !v.is_null()).map(parse).collect(),
        _ => Err(ProfileError(format!("{name} must be an array"))),
    }
}
