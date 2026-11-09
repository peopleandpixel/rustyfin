//! Profile conditions, ported from Jellyfin's `ConditionProcessor` and `ContainerHelper`.
//!
//! A condition compares one property of a stream with the condition's value. An unknown
//! property value satisfies only an optional condition. A property that a processor doesn't
//! read satisfies every condition.

use rustyfin_model::device_profile::{ConditionProperty, ConditionType, ProfileCondition};

use super::{TranscodeReason, TranscodeReasons};

/// The facts about a video stream that the video conditions test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoFacts {
    /// The width in pixels.
    pub width: Option<i32>,
    /// The height in pixels.
    pub height: Option<i32>,
    /// The bit depth.
    pub bit_depth: Option<i32>,
    /// The bit rate in bits per second.
    pub bitrate: Option<i32>,
    /// The codec profile, such as "High 10".
    pub profile: Option<String>,
    /// The range type: "SDR", "HDR10", "HLG" or "DOVI".
    pub range_type: Option<String>,
    /// The codec level.
    pub level: Option<f64>,
    /// The frame rate in frames per second.
    pub framerate: Option<f32>,
    /// The packet length in bytes.
    pub packet_length: Option<i32>,
    /// Whether the video is anamorphic.
    pub is_anamorphic: Option<bool>,
    /// Whether the video is interlaced.
    pub is_interlaced: Option<bool>,
    /// The number of reference frames.
    pub ref_frames: Option<i32>,
    /// The number of streams in the file.
    pub num_streams: i32,
    /// The number of video streams in the file.
    pub num_video_streams: Option<i32>,
    /// The number of audio streams in the file.
    pub num_audio_streams: Option<i32>,
    /// The codec tag, such as "hvc1".
    pub codec_tag: Option<String>,
    /// Whether the video is H.264.
    pub is_avc: Option<bool>,
    /// The rotation in degrees.
    pub rotation: Option<i32>,
}

/// The facts about an audio stream that the audio conditions test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioFacts {
    /// The number of channels.
    pub channels: Option<i32>,
    /// The bit rate in bits per second.
    pub bitrate: Option<i32>,
    /// The sample rate in hertz.
    pub sample_rate: Option<i32>,
    /// The bit depth.
    pub bit_depth: Option<i32>,
    /// The codec profile, such as "LC".
    pub profile: Option<String>,
    /// Whether the audio is a secondary track.
    pub is_secondary: Option<bool>,
}

/// Whether a condition holds for a video stream. A property the video processor doesn't read
/// holds.
pub fn video_condition_satisfied(c: &ProfileCondition, f: &VideoFacts) -> bool {
    match c.property {
        ConditionProperty::IsAnamorphic => bool_satisfied(c, f.is_anamorphic),
        ConditionProperty::IsAvc => bool_satisfied(c, f.is_avc),
        ConditionProperty::IsInterlaced => bool_satisfied(c, f.is_interlaced),
        ConditionProperty::Height => number_satisfied(c, f.height, parse_int),
        ConditionProperty::Width => number_satisfied(c, f.width, parse_int),
        ConditionProperty::NumAudioStreams => number_satisfied(c, f.num_audio_streams, parse_int),
        ConditionProperty::NumStreams => number_satisfied(c, Some(f.num_streams), parse_int),
        ConditionProperty::NumVideoStreams => number_satisfied(c, f.num_video_streams, parse_int),
        ConditionProperty::PacketLength => number_satisfied(c, f.packet_length, parse_int),
        ConditionProperty::RefFrames => number_satisfied(c, f.ref_frames, parse_int),
        ConditionProperty::VideoBitDepth => number_satisfied(c, f.bit_depth, parse_int),
        ConditionProperty::VideoBitrate => number_satisfied(c, f.bitrate, parse_int),
        ConditionProperty::VideoCodecTag => string_satisfied(c, f.codec_tag.as_deref()),
        ConditionProperty::VideoFramerate => {
            number_satisfied(c, f.framerate.map(f64::from), parse_double)
        }
        ConditionProperty::VideoLevel => number_satisfied(c, f.level, parse_double),
        ConditionProperty::VideoProfile => string_satisfied(c, f.profile.as_deref()),
        ConditionProperty::VideoRangeType => range_type_satisfied(c, f.range_type.as_deref()),
        ConditionProperty::VideoRotation => number_satisfied(c, f.rotation, parse_int),
        // The timestamp is never known, so the condition sees an unknown value.
        ConditionProperty::VideoTimestamp => !c.is_required,
        _ => true,
    }
}

/// Whether a condition holds for an audio stream. A property the audio processor doesn't read
/// holds.
pub fn audio_condition_satisfied(c: &ProfileCondition, f: &AudioFacts) -> bool {
    match c.property {
        ConditionProperty::AudioBitDepth => number_satisfied(c, f.bit_depth, parse_int),
        ConditionProperty::AudioBitrate => number_satisfied(c, f.bitrate, parse_int),
        ConditionProperty::AudioChannels => number_satisfied(c, f.channels, parse_int),
        ConditionProperty::AudioSampleRate => number_satisfied(c, f.sample_rate, parse_int),
        _ => true,
    }
}

/// Whether a condition holds for a video stream's audio. It also reads the audio profile and
/// the secondary flag, and passes every other property to [`audio_condition_satisfied`].
pub fn video_audio_condition_satisfied(c: &ProfileCondition, f: &AudioFacts) -> bool {
    match c.property {
        ConditionProperty::AudioProfile => string_satisfied(c, f.profile.as_deref()),
        ConditionProperty::IsSecondaryAudio => bool_satisfied(c, f.is_secondary),
        _ => audio_condition_satisfied(c, f),
    }
}

/// The transcode reason that a failed condition adds, as Jellyfin's `StreamBuilder` maps
/// them. Only the property matters. Properties with no reason give an empty set.
pub fn reason_for_failed_condition(c: &ProfileCondition) -> TranscodeReasons {
    let reason = match c.property {
        ConditionProperty::AudioBitDepth => TranscodeReason::AudioBitDepthNotSupported,
        ConditionProperty::AudioBitrate => TranscodeReason::AudioBitrateNotSupported,
        ConditionProperty::AudioChannels => TranscodeReason::AudioChannelsNotSupported,
        ConditionProperty::AudioProfile => TranscodeReason::AudioProfileNotSupported,
        ConditionProperty::AudioSampleRate => TranscodeReason::AudioSampleRateNotSupported,
        ConditionProperty::Height | ConditionProperty::Width => {
            TranscodeReason::VideoResolutionNotSupported
        }
        ConditionProperty::IsAnamorphic => TranscodeReason::AnamorphicVideoNotSupported,
        ConditionProperty::IsInterlaced => TranscodeReason::InterlacedVideoNotSupported,
        ConditionProperty::IsSecondaryAudio => TranscodeReason::SecondaryAudioNotSupported,
        ConditionProperty::NumStreams => TranscodeReason::StreamCountExceedsLimit,
        ConditionProperty::RefFrames => TranscodeReason::RefFramesNotSupported,
        ConditionProperty::VideoBitDepth => TranscodeReason::VideoBitDepthNotSupported,
        ConditionProperty::VideoBitrate => TranscodeReason::VideoBitrateNotSupported,
        ConditionProperty::VideoCodecTag => TranscodeReason::VideoCodecTagNotSupported,
        ConditionProperty::VideoFramerate => TranscodeReason::VideoFramerateNotSupported,
        ConditionProperty::VideoLevel => TranscodeReason::VideoLevelNotSupported,
        ConditionProperty::VideoProfile => TranscodeReason::VideoProfileNotSupported,
        ConditionProperty::VideoRangeType => TranscodeReason::VideoRangeTypeNotSupported,
        ConditionProperty::VideoRotation => TranscodeReason::VideoRotationNotSupported,
        // Codec flags, the packet length, the timestamp and the audio and video stream
        // counts have no reason of their own.
        _ => return TranscodeReasons::default(),
    };
    TranscodeReasons::from(reason)
}

/// Whether a comma-separated list of containers accepts a container. A leading `-` negates the
/// list. An empty input takes the negative flag, and an empty list accepts everything.
/// Entries match ignoring ASCII case.
pub fn contains_container(profile: &str, input: &str) -> bool {
    let (profile, negative) = match profile.strip_prefix('-') {
        Some(rest) => (rest, true),
        None => (profile, false),
    };
    if input.is_empty() {
        return negative;
    }
    if profile.is_empty() {
        return true;
    }
    let matched =
        entries(input).any(|entry| entries(profile).any(|known| entry.eq_ignore_ascii_case(known)));
    if matched { !negative } else { negative }
}

/// Splits a comma-separated list into its non-empty entries. The entries aren't trimmed.
pub fn split(input: &str) -> Vec<String> {
    entries(input).map(str::to_string).collect()
}

/// The non-empty entries of a comma-separated list.
fn entries(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').filter(|entry| !entry.is_empty())
}

/// Whether a numeric condition holds. An unknown current value satisfies only an optional
/// condition. A value that doesn't parse fails the condition, and `EqualsAny` skips such pieces.
fn number_satisfied<T: PartialOrd>(
    c: &ProfileCondition,
    current: Option<T>,
    parse: fn(&str) -> Option<T>,
) -> bool {
    let Some(current) = current else {
        return !c.is_required;
    };
    match c.condition {
        ConditionType::Equals => parse(&c.value).is_some_and(|v| current == v),
        ConditionType::NotEquals => parse(&c.value).is_some_and(|v| current != v),
        ConditionType::LessThanEqual => parse(&c.value).is_some_and(|v| current <= v),
        ConditionType::GreaterThanEqual => parse(&c.value).is_some_and(|v| current >= v),
        ConditionType::EqualsAny => c.value.split('|').filter_map(parse).any(|v| current == v),
        _ => false,
    }
}

/// Whether a boolean condition holds. Only `Equals` and `NotEquals` apply, and a value that
/// isn't `true` or `false` fails the condition.
fn bool_satisfied(c: &ProfileCondition, current: Option<bool>) -> bool {
    let Some(current) = current else {
        return !c.is_required;
    };
    let Some(expected) = parse_bool(&c.value) else {
        return false;
    };
    match c.condition {
        ConditionType::Equals => current == expected,
        ConditionType::NotEquals => current != expected,
        _ => false,
    }
}

/// Whether a string condition holds. An empty current value counts as unknown. The comparisons
/// ignore ASCII case, and `EqualsAny` splits the value on `|` without trimming the pieces.
fn string_satisfied(c: &ProfileCondition, current: Option<&str>) -> bool {
    let current = match current {
        Some(current) if !current.is_empty() => current,
        _ => return !c.is_required,
    };
    match c.condition {
        ConditionType::Equals => c.value.eq_ignore_ascii_case(current),
        ConditionType::NotEquals => !c.value.eq_ignore_ascii_case(current),
        ConditionType::EqualsAny => c.value.split('|').any(|v| v.eq_ignore_ascii_case(current)),
        _ => false,
    }
}

/// Whether a range-type condition holds. "Unknown" counts as no range type at all.
fn range_type_satisfied(c: &ProfileCondition, current: Option<&str>) -> bool {
    string_satisfied(
        c,
        current.filter(|range| !range.eq_ignore_ascii_case("Unknown")),
    )
}

/// Parses an integer as Jellyfin's `int.TryParse` does, for the transcoding conditions.
pub(super) fn try_int(text: &str) -> Option<i32> {
    parse_int(text)
}

/// Parses a double as Jellyfin's `double.TryParse` does, for the transcoding conditions.
pub(super) fn try_double(text: &str) -> Option<f64> {
    parse_double(text)
}

/// Parses a boolean as Jellyfin's `bool.TryParse` does, for the transcoding conditions.
pub(super) fn try_bool(text: &str) -> Option<bool> {
    parse_bool(text)
}

/// Parses an integer as .NET's `NumberStyles.Integer` does: surrounding white space and a sign.
fn parse_int(text: &str) -> Option<i32> {
    text.trim().parse().ok()
}

/// Parses a double as .NET's `NumberStyles.Float | AllowThousands` does: surrounding white
/// space, a sign, a decimal point, an exponent, and commas in the integer part.
fn parse_double(text: &str) -> Option<f64> {
    let text = text.trim();
    let boundary = text.find(['.', 'e', 'E']).unwrap_or(text.len());
    let (integer, rest) = text.split_at(boundary);
    let digits: String = integer.chars().filter(|c| *c != ',').collect();
    format!("{digits}{rest}").parse().ok()
}

/// Parses a boolean as .NET's `bool.TryParse` does: `true` or `false` in any case, with
/// surrounding white space or nulls.
fn parse_bool(text: &str) -> Option<bool> {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if text.eq_ignore_ascii_case("true") {
        Some(true)
    } else if text.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}
