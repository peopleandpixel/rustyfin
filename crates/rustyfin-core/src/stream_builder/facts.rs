//! Facts about a media source and its streams, read from a `MediaSourceResponse`.

use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

use super::StreamKind;

/// Codec tags that mark Dolby Vision, in lowercase.
const DOVI_TAGS: [&str; 5] = ["dvh1", "dvhe", "dav1", "dva1", "dvav"];

/// One stream of a media source, reduced to the fields the conditions test.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamFacts {
    /// The stream's index within the source.
    pub index: i32,
    /// Whether the stream is video, audio, subtitle or something else.
    pub kind: StreamKind,
    /// The codec, lowercased. Empty when the source names none.
    pub codec: String,
    /// The codec profile as the source reports it.
    pub profile: Option<String>,
    /// The codec level. The source doesn't report one here, so this is always `None`.
    pub level: Option<f64>,
    /// The codec tag as the source reports it.
    pub codec_tag: Option<String>,
    /// The width in pixels.
    pub width: Option<i32>,
    /// The height in pixels.
    pub height: Option<i32>,
    /// The bit depth.
    pub bit_depth: Option<i32>,
    /// The bit rate in bits per second.
    pub bit_rate: Option<i32>,
    /// The frame rate in frames per second.
    pub frame_rate: Option<f32>,
    /// The video range type: `SDR`, `HDR10`, `HLG` or `DOVI`. Video streams only.
    pub range_type: Option<String>,
    /// Whether the video is interlaced. Video streams only.
    pub is_interlaced: Option<bool>,
    /// The number of audio channels.
    pub channels: Option<i32>,
    /// The audio sample rate in hertz.
    pub sample_rate: Option<i32>,
    /// The stream's language tag.
    pub language: Option<String>,
    /// Whether the stream is in a separate file.
    pub is_external: bool,
    /// Whether the stream is marked as the default.
    pub is_default: bool,
    /// Whether the stream is marked as forced.
    pub is_forced: bool,
    /// The file path of an external stream.
    pub path: Option<String>,
}

/// A media source, reduced to the facts the conditions test.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFacts {
    /// The source's id.
    pub id: String,
    /// The container, lowercased. Empty when the source names none.
    pub container: String,
    /// The overall bit rate in bits per second.
    pub bitrate: Option<i64>,
    /// Whether the source is remote.
    pub is_remote: bool,
    /// The source's streams, in source order.
    pub streams: Vec<StreamFacts>,
    /// Whether the source can be played as it is.
    pub supports_direct_play: bool,
    /// Whether the source can be remuxed into a new container.
    pub supports_direct_stream: bool,
    /// Whether the source can be transcoded.
    pub supports_transcoding: bool,
    /// The default audio stream index. It isn't derived, so this is always `None`.
    pub default_audio_stream_index: Option<i32>,
    /// The default subtitle stream index, picked from the subtitle streams.
    pub default_subtitle_stream_index: Option<i32>,
}

impl Default for SourceFacts {
    /// An empty source that allows every playback method.
    fn default() -> Self {
        Self {
            id: String::new(),
            container: String::new(),
            bitrate: None,
            is_remote: false,
            streams: Vec::new(),
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: true,
            default_audio_stream_index: None,
            default_subtitle_stream_index: None,
        }
    }
}

impl From<&MediaSourceResponse> for SourceFacts {
    /// Reads the facts that the conditions can test from a source response.
    fn from(source: &MediaSourceResponse) -> Self {
        let streams: Vec<StreamFacts> = source.media_streams.iter().map(stream_facts).collect();
        let default_subtitle_stream_index = default_subtitle(&streams);
        Self {
            id: source.id.clone(),
            container: source
                .container
                .as_deref()
                .unwrap_or_default()
                .to_lowercase(),
            bitrate: source.bitrate.map(i64::from),
            is_remote: source.is_remote,
            streams,
            supports_direct_play: source.supports_direct_play,
            supports_direct_stream: source.supports_direct_stream,
            supports_transcoding: source.supports_transcoding,
            default_audio_stream_index: None,
            default_subtitle_stream_index,
        }
    }
}

/// Reads one stream's facts. Only video streams get a range type and an interlaced flag.
fn stream_facts(stream: &MediaStream) -> StreamFacts {
    let kind = stream_kind(&stream.codec_type);
    let is_video = kind == StreamKind::Video;
    StreamFacts {
        index: stream.index,
        kind,
        codec: stream.codec.as_deref().unwrap_or_default().to_lowercase(),
        profile: stream.profile.clone(),
        level: None,
        codec_tag: stream.codec_tag.clone(),
        width: to_i32(stream.width),
        height: to_i32(stream.height),
        bit_depth: to_i32(stream.bit_depth),
        bit_rate: to_i32(stream.bit_rate),
        frame_rate: frame_rate(stream),
        range_type: is_video.then(|| range_type(stream).to_string()),
        is_interlaced: is_video.then_some(stream.is_interlaced),
        channels: to_i32(stream.channels),
        sample_rate: to_i32(stream.sample_rate),
        language: stream.language.clone(),
        is_external: stream.is_external,
        is_default: stream.is_default,
        is_forced: stream.is_forced,
        path: stream.path.clone(),
    }
}

/// The stream kind from the source's codec type, ignoring case.
fn stream_kind(codec_type: &str) -> StreamKind {
    match codec_type.to_ascii_lowercase().as_str() {
        "video" => StreamKind::Video,
        "audio" => StreamKind::Audio,
        "subtitle" => StreamKind::Subtitle,
        _ => StreamKind::Other,
    }
}

/// A source number as an i32. A value too big for i32 is unknown.
fn to_i32(value: Option<u32>) -> Option<i32> {
    value.and_then(|value| i32::try_from(value).ok())
}

/// The frame rate. The average wins when it parses and is under 1000, which is Jellyfin's
/// cut-off for realistic rates. Otherwise the real rate decides, with no upper limit.
fn frame_rate(stream: &MediaStream) -> Option<f32> {
    stream
        .average_frame_rate
        .as_deref()
        .and_then(parse_frame_rate)
        .filter(|rate| *rate < 1000.0)
        .or_else(|| stream.real_frame_rate.as_deref().and_then(parse_frame_rate))
}

/// Parses `a/b` as a fraction with an f32 division, or a plain decimal. A zero divisor
/// and a non-finite result don't parse.
fn parse_frame_rate(text: &str) -> Option<f32> {
    let rate = match text.split_once('/') {
        Some((numerator, denominator)) => {
            let denominator: f32 = denominator.parse().ok()?;
            if denominator == 0.0 {
                return None;
            }
            numerator.parse::<f32>().ok()? / denominator
        }
        None => text.parse::<f32>().ok()?,
    };
    rate.is_finite().then_some(rate)
}

/// The video range type from the codec tag and the colour transfer. Dolby Vision is
/// recognised by its tag, HDR10 and HLG by the transfer function, and anything else is SDR.
fn range_type(stream: &MediaStream) -> &'static str {
    let tag = stream
        .codec_tag
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    if DOVI_TAGS.contains(&tag.as_str()) {
        return "DOVI";
    }
    let transfer = stream
        .color_transfer
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    match transfer.as_str() {
        "smpte2084" => "HDR10",
        "arib-std-b67" => "HLG",
        _ => "SDR",
    }
}

/// The default subtitle index. Among the subtitles that are external, forced or default, it
/// takes the first by external, then forced, then default, then index. Plain subtitles are
/// never picked.
fn default_subtitle(streams: &[StreamFacts]) -> Option<i32> {
    streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .filter(|s| s.is_external || s.is_forced || s.is_default)
        .min_by_key(|s| (!s.is_external, !s.is_forced, !s.is_default, s.index))
        .map(|s| s.index)
}
