//! Facts about a media source and its streams, read from a `MediaSourceResponse`.

use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

use super::StreamKind;
use super::selection::{AudioIndexSource, SubtitleMode, default_subtitle_index};

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
    /// The codec level, as ffprobe gives it.
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
    /// Whether the video's pixels are not square. Video streams only.
    pub is_anamorphic: Option<bool>,
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
    /// Jellyfin's subtitle score, for the subtitle streams the user's subtitle mode would load
    /// (`SourceFacts::apply_preferences`); None for the others.
    pub score: Option<i32>,
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
    /// The default audio stream index: None until a user's settings pick one
    /// (`SourceFacts::apply_preferences`).
    pub default_audio_stream_index: Option<i32>,
    /// The default subtitle stream index, picked from the subtitle streams.
    pub default_subtitle_stream_index: Option<i32>,
    /// Which user settings picked the default audio stream; None until they are applied
    /// (`SourceFacts::apply_preferences`).
    pub audio_index_source: Option<AudioIndexSource>,
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
            audio_index_source: None,
        }
    }
}

impl From<&MediaSourceResponse> for SourceFacts {
    /// Reads the facts that the conditions can test from a source response.
    fn from(source: &MediaSourceResponse) -> Self {
        let mut streams: Vec<StreamFacts> = source.media_streams.iter().map(stream_facts).collect();
        estimate_missing_bit_rates(&mut streams, source.bitrate);
        // Jellyfin's pick for a user with the default settings
        let default_subtitle_stream_index =
            default_subtitle_index(&streams, &[], SubtitleMode::Default, None);
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
            audio_index_source: None,
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
        level: stream.level,
        codec_tag: stream.codec_tag.clone(),
        width: to_i32(stream.width),
        height: to_i32(stream.height),
        bit_depth: to_i32(stream.bit_depth),
        bit_rate: to_i32(stream.bit_rate),
        frame_rate: frame_rate(stream),
        range_type: is_video.then(|| range_type(stream).to_string()),
        is_interlaced: is_video.then_some(stream.is_interlaced),
        is_anamorphic: stream.is_anamorphic.filter(|_| is_video),
        channels: to_i32(stream.channels),
        sample_rate: to_i32(stream.sample_rate),
        language: stream.language.clone(),
        is_external: stream.is_external,
        is_default: stream.is_default,
        is_forced: stream.is_forced,
        path: stream.path.clone(),
        score: None,
    }
}

/// Whether a video stream is a picture (cover art), which Jellyfin counts as an embedded image.
fn is_picture(stream: &StreamFacts) -> bool {
    ["mjpeg", "png", "gif", "webp"].contains(&stream.codec.as_str())
}

/// Jellyfin's fallback bit rate for an audio stream that ffprobe gives none, by codec and
/// channels (its probe's `GetEstimatedAudioBitrate`). None for other codecs or no channels.
fn estimated_audio_bit_rate(codec: &str, profile: Option<&str>, channels: i32) -> Option<i32> {
    if channels < 1 {
        return None;
    }
    let multichannel = channels > 2;
    let pick = |many: i32, few: i32| if multichannel { many } else { few };
    let lossless_dts = profile.is_some_and(|p| p.to_ascii_lowercase().contains("hd ma"));
    match codec {
        "aac" | "mp3" | "mp2" => Some(pick(320_000, 192_000)),
        "ac3" | "eac3" => Some(pick(640_000, 192_000)),
        "dts" | "dca" if lossless_dts => channels.checked_mul(700_000),
        "dts" | "dca" => Some(pick(1_509_000, 768_000)),
        "opus" => Some(pick(256_000, 128_000)),
        "vorbis" => Some(pick(320_000, 160_000)),
        "wmav1" | "wmav2" | "wmapro" => Some(pick(384_000, 192_000)),
        "flac" | "alac" => channels.checked_mul(480_000),
        "truehd" | "mlp" => channels.checked_mul(700_000),
        _ => None,
    }
}

/// Fills in the bit rates a video's probe left out, as Jellyfin's probe does, from the stored
/// codecs, channels and container bit rate (so no file is read again): each audio stream gets
/// the estimate for its codec (`estimated_audio_bit_rate`), then a lone video stream gets the
/// container's bit rate less the other embedded streams', when every audio bit rate is known and
/// something is left. A source without a video stream (an audio file) is left as it is.
fn estimate_missing_bit_rates(streams: &mut [StreamFacts], total_bitrate: Option<u32>) {
    let is_video = |s: &StreamFacts| s.kind == StreamKind::Video && !is_picture(s);
    let videos = streams.iter().filter(|s| is_video(s)).count();
    if videos == 0 {
        return;
    }
    for stream in streams.iter_mut() {
        if stream.kind == StreamKind::Audio && stream.bit_rate.is_none() {
            stream.bit_rate = stream.channels.and_then(|channels| {
                estimated_audio_bit_rate(&stream.codec, stream.profile.as_deref(), channels)
            });
        }
    }
    let Some(total) = total_bitrate.filter(|_| videos == 1) else {
        return;
    };
    let others = || streams.iter().filter(|s| !is_video(s) && !s.is_external);
    let audio_known = others()
        .filter(|s| s.kind == StreamKind::Audio)
        .all(|s| s.bit_rate.is_some());
    let used: i64 = others().filter_map(|s| s.bit_rate).map(i64::from).sum();
    let left = i64::from(total) - used;
    if let Some(video) = streams.iter_mut().find(|s| is_video(s))
        && video.bit_rate.is_none()
        && audio_known
        && left > 0
    {
        video.bit_rate = i32::try_from(left).ok();
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

/// The video range type, as Jellyfin's `MediaStream.VideoRangeType` gives it: Dolby Vision by
/// its configuration record (DOVIWithHDR10, DOVIWithEL, ...) or its tag, HDR10 and HLG by the
/// transfer function, and anything else SDR (`MediaStream::video_range`).
fn range_type(stream: &MediaStream) -> &'static str {
    stream.video_range().map_or("SDR", |(_, kind)| kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(
        kind: StreamKind,
        codec: &str,
        channels: Option<i32>,
        bit_rate: Option<i32>,
    ) -> StreamFacts {
        StreamFacts {
            kind,
            codec: codec.to_string(),
            channels,
            bit_rate,
            ..Default::default()
        }
    }

    /// Audio streams without a bit rate get Jellyfin's estimate by codec and channels, and the
    /// lone video stream gets the container's bit rate less the audio, as in Jellyfin 12.4.0
    /// (mono AAC 192 kbps, stereo FLAC 960 kbps). Cover art is no video stream.
    #[test]
    fn r3gaps3_missing_bit_rates_are_estimated_as_jellyfin_does() {
        let mut dts = stream(StreamKind::Audio, "dts", Some(6), None);
        dts.profile = Some("DTS-HD MA".to_string());
        let mut streams = vec![
            stream(StreamKind::Video, "h264", None, None),
            stream(StreamKind::Audio, "aac", Some(1), None),
            stream(StreamKind::Audio, "flac", Some(2), None),
            dts,
            stream(StreamKind::Audio, "truehd", Some(8), None),
            stream(StreamKind::Audio, "eac3", Some(6), Some(256_000)),
            stream(StreamKind::Subtitle, "subrip", None, None),
            stream(StreamKind::Video, "mjpeg", None, None),
        ];
        estimate_missing_bit_rates(&mut streams, Some(20_000_000));
        let rates: Vec<_> = streams.iter().map(|s| s.bit_rate).collect();
        assert_eq!(
            rates,
            vec![
                Some(20_000_000 - 192_000 - 960_000 - 4_200_000 - 5_600_000 - 256_000),
                Some(192_000),
                Some(960_000),
                Some(4_200_000),
                Some(5_600_000),
                Some(256_000),
                None,
                None
            ]
        );
    }

    /// The video keeps no bit rate when an audio bit rate stays unknown, when nothing is left,
    /// or without a container bit rate; a known one is kept; an audio file is left alone.
    #[test]
    fn r3gaps3_video_bit_rate_is_estimated_only_when_it_can_be() {
        let estimate = |audio: StreamFacts, video_rate, total| {
            let mut streams = vec![stream(StreamKind::Video, "h264", None, video_rate), audio];
            estimate_missing_bit_rates(&mut streams, total);
            (streams[0].bit_rate, streams[1].bit_rate)
        };
        let pcm = stream(StreamKind::Audio, "pcm_s16le", Some(2), None);
        let aac = stream(StreamKind::Audio, "aac", Some(2), None);
        assert_eq!(estimate(pcm, None, Some(5_000_000)), (None, None));
        assert_eq!(
            estimate(aac.clone(), None, Some(192_000)),
            (None, Some(192_000))
        );
        assert_eq!(estimate(aac.clone(), None, None), (None, Some(192_000)));
        assert_eq!(
            estimate(aac.clone(), Some(1000), Some(5_000_000)),
            (Some(1000), Some(192_000))
        );
        let mut song = vec![aac];
        estimate_missing_bit_rates(&mut song, Some(320_000));
        assert_eq!(song[0].bit_rate, None);
    }

    /// The estimate reaches the facts the decisions read; the stored source is unchanged.
    #[test]
    fn r3gaps3_source_facts_carry_the_estimates() {
        let mut source: MediaSourceResponse = serde_json::from_value(serde_json::json!({
            "Id": "s", "Uid": "s", "Protocol": "File", "Path": "/m/a.mkv", "Bitrate": 205_621
        }))
        .unwrap();
        source.media_streams = vec![
            MediaStream {
                index: 1,
                codec: Some("h264".to_string()),
                codec_type: "video".to_string(),
                ..Default::default()
            },
            MediaStream {
                index: 2,
                codec: Some("aac".to_string()),
                codec_type: "audio".to_string(),
                channels: Some(1),
                ..Default::default()
            },
        ];
        let facts = SourceFacts::from(&source);
        let rates: Vec<_> = facts.streams.iter().map(|s| s.bit_rate).collect();
        assert_eq!(rates, vec![Some(13_621), Some(192_000)]);
        assert_eq!(source.media_streams[1].bit_rate, None);
    }
}
