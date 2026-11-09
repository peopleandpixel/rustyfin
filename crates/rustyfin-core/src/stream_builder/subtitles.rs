//! Subtitle delivery: the subtitle profile that each subtitle stream takes, the subtitle that the
//! plan chooses, and the delivery list that a source gets. A port of Jellyfin's
//! `StreamBuilder.GetSubtitleProfile` (SB 1465-1650), `StreamInfo.GetSubtitleProfiles` (SI
//! 1153-1285) and `MediaInfoHelper.SetDeviceSpecificSubtitleInfo` (MIH 493-516), with the
//! MediaStream and SubtitleProfile helpers that they read.

use rustyfin_model::device_profile::{
    DeviceProfile, StreamProtocol, SubtitleDeliveryMethod, SubtitleProfile,
};

use super::conditions::contains_container;
use super::video::VideoPlan;
use super::{
    PlayMethod, SourceFacts, StreamFacts, StreamKind, StreamRequest, SubtitleDelivery,
    TranscodeReason, TranscodeReasons,
};
use crate::subtitle::is_image_subtitle;

/// The profile format that some FFmpeg subtitle codecs are also known by.
const ALIASES: [(&str, &str); 5] = [
    ("subrip", "srt"),
    ("webvtt", "vtt"),
    ("hdmv_pgs_subtitle", "pgssub"),
    ("dvd_subtitle", "dvdsub"),
    ("dvb_subtitle", "dvbsub"),
];

/// Whether a profile format names `codec`: it equals the codec ignoring ASCII case, or the
/// codec's alias in [`ALIASES`].
fn format_matches(format: &str, codec: &str) -> bool {
    let alias = ALIASES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(codec))
        .map_or(codec, |(_, alias)| alias);
    format.eq_ignore_ascii_case(codec) || format.eq_ignore_ascii_case(alias)
}

/// Whether a codec or format is ASS or SSA, which Jellyfin can't convert to or from.
fn is_ass(name: &str) -> bool {
    name.eq_ignore_ascii_case("ass") || name.eq_ignore_ascii_case("ssa")
}

/// Jellyfin's `MediaStream.IsTextSubtitleStream`: a subtitle stream that is text. An embedded
/// stream needs a codec to count, and an external file is text unless its codec is a picture
/// format.
pub(super) fn is_text(stream: &StreamFacts) -> bool {
    stream.kind == StreamKind::Subtitle
        && (!stream.codec.is_empty() || stream.is_external)
        && !is_image_subtitle(&stream.codec)
}

/// Whether a profile format is a text format, which is the case for every format that isn't a
/// picture format.
fn is_text_format(format: &str) -> bool {
    !is_image_subtitle(format)
}

/// Jellyfin's `SupportsSubtitleConversionTo`: a text stream converts to the format `to` unless
/// ASS or SSA is on either side.
fn converts_to(stream: &StreamFacts, to: &str) -> bool {
    is_text(stream) && !is_ass(&stream.codec) && !is_ass(to)
}

/// Jellyfin's `SubtitleProfile.SupportsLanguage`. A profile without languages takes any stream,
/// and a stream without a language counts as "und".
fn supports_language(profile: &SubtitleProfile, language: Option<&str>) -> bool {
    profile.language.is_empty()
        || contains_container(
            &profile.language,
            language.filter(|l| !l.is_empty()).unwrap_or("und"),
        )
}

/// Jellyfin's `IsSubtitleEmbedSupported`: a subtitle can be embedded only into Matroska. An empty
/// container, or MPEG-TS and MP4, can't carry one.
fn embed_supported(container: Option<&str>) -> bool {
    match container.filter(|c| !c.is_empty()) {
        None => false,
        Some(c) => !contains_container(c, "ts,mpegts,mp4") && contains_container(c, "mkv,matroska"),
    }
}

/// Jellyfin's `CanConsiderEmbedSubtitle`. An embedded stream can be embedded unless the transcode
/// is HLS. An external file can be embedded only into a non-HLS Matroska transcode.
fn can_consider_embed(
    stream: &StreamFacts,
    play: PlayMethod,
    protocol: StreamProtocol,
    container: Option<&str>,
) -> bool {
    if stream.is_external {
        play == PlayMethod::Transcode
            && protocol != StreamProtocol::Hls
            && embed_supported(container)
    } else {
        play != PlayMethod::Transcode || protocol != StreamProtocol::Hls
    }
}

/// The skips shared by both Embed passes of `GetSubtitleProfile`: the profile must be an Embed
/// profile that supports the stream's language and the output container, and a transcode may
/// embed only into a container that supports it.
fn embed_profile(
    profile: &SubtitleProfile,
    stream: &StreamFacts,
    play: PlayMethod,
    container: Option<&str>,
) -> bool {
    profile.method == SubtitleDeliveryMethod::Embed
        && supports_language(profile, stream.language.as_deref())
        && contains_container(&profile.container, container.unwrap_or(""))
        && (play != PlayMethod::Transcode || embed_supported(container))
}

/// Jellyfin's `GetSubtitleProfile`, reduced to the delivery method: how `stream` reaches a client
/// with `profiles`, when it plays with `play` into `container` over `protocol`.
///
/// Embed is tried first, with a profile whose format matches the codec and then one that the
/// codec converts to. Otherwise the stream is External when it is text and a WebVTT External
/// profile supports its language. Otherwise it is Encode.
pub(super) fn subtitle_method(
    stream: &StreamFacts,
    profiles: &[SubtitleProfile],
    play: PlayMethod,
    container: Option<&str>,
    protocol: StreamProtocol,
) -> SubtitleDeliveryMethod {
    if can_consider_embed(stream, play, protocol, container) {
        let embeds = || {
            profiles
                .iter()
                .filter(|p| embed_profile(p, stream, play, container))
        };
        if embeds().any(|p| {
            is_text(stream) == is_text_format(&p.format) && format_matches(&p.format, &stream.codec)
        }) {
            return SubtitleDeliveryMethod::Embed;
        }
        if embeds().any(|p| converts_to(stream, &p.format)) {
            return SubtitleDeliveryMethod::Embed;
        }
    }

    // Only WebVTT External profiles count. Hls profiles are skipped: Rustyfin's transcoder
    // doesn't put subtitles in the manifest. Rustyfin's endpoint converts ASS and SSA to WebVTT,
    // so every text stream may go out as a file.
    let external = profiles.iter().any(|p| {
        p.method == SubtitleDeliveryMethod::External
            && p.format.eq_ignore_ascii_case("vtt")
            && supports_language(p, stream.language.as_deref())
    });
    if is_text(stream) && external {
        SubtitleDeliveryMethod::External
    } else {
        SubtitleDeliveryMethod::Encode
    }
}

/// The subtitle stream with `index`. Index -1 names no stream, and neither does an index that
/// isn't a subtitle stream of `source`.
pub(super) fn subtitle_stream(source: &SourceFacts, index: Option<i32>) -> Option<&StreamFacts> {
    let index = index.filter(|&i| i != -1)?;
    source
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Subtitle && stream.index == index)
}

/// The subtitle index that the plan uses (SB 660-664): the request's index when it is -1 or names
/// a subtitle stream of `source`, else the source's default subtitle index.
pub(super) fn chosen_index(source: &SourceFacts, requested: Option<i32>) -> Option<i32> {
    match requested {
        Some(-1) => Some(-1),
        Some(index) if subtitle_stream(source, Some(index)).is_some() => Some(index),
        _ => source.default_subtitle_stream_index,
    }
}

/// The direct play failure for the plan's subtitle (SB 1319-1332 and 1377): the reason
/// SubtitleCodecNotSupported when the subtitle would need a method other than Drop, External or
/// Embed. It is checked as a direct play of the raw container over HTTP.
pub(super) fn direct_play_reason(
    container: &str,
    profile: &DeviceProfile,
    subtitle: Option<&StreamFacts>,
) -> TranscodeReasons {
    let Some(stream) = subtitle else {
        return TranscodeReasons::default();
    };
    let method = subtitle_method(
        stream,
        &profile.subtitle_profiles,
        PlayMethod::DirectPlay,
        Some(container),
        StreamProtocol::Http,
    );
    match method {
        SubtitleDeliveryMethod::Drop
        | SubtitleDeliveryMethod::External
        | SubtitleDeliveryMethod::Embed => TranscodeReasons::default(),
        _ => TranscodeReason::SubtitleCodecNotSupported.into(),
    }
}

/// The delivery of every subtitle stream of `source`, in source order (SI 1153-1285 with MIH
/// 493-516). The play method, container and protocol are the response's: a direct play's, the
/// applied transcoding profile's, or else StreamInfo's defaults. The URL of an External file
/// carries the start ticks of a transcode that doesn't copy timestamps, and no ticks for HLS.
pub(super) fn deliveries(
    source: &SourceFacts,
    profile: &DeviceProfile,
    request: &StreamRequest,
    plan: &VideoPlan,
) -> Vec<SubtitleDelivery> {
    let (play, container, protocol, copy_timestamps) = match (plan.direct_play, &plan.transcode) {
        (true, _) => (
            PlayMethod::DirectPlay,
            Some(plan.container.as_str()),
            StreamProtocol::Http,
            false,
        ),
        (false, Some(stream)) => (
            PlayMethod::Transcode,
            stream.container.as_deref(),
            stream.sub_protocol,
            stream.copy_timestamps,
        ),
        (false, None) => (PlayMethod::Transcode, None, StreamProtocol::Http, false),
    };
    let ticks = if protocol == StreamProtocol::Hls {
        0
    } else if play == PlayMethod::Transcode && !copy_timestamps {
        request.start_time_ticks
    } else {
        0
    };
    let key = if request.access_token.is_empty() {
        String::new()
    } else {
        format!("?api_key={}", request.access_token)
    };

    source
        .streams
        .iter()
        .filter(|stream| stream.kind == StreamKind::Subtitle)
        .map(|stream| {
            let method = subtitle_method(
                stream,
                &profile.subtitle_profiles,
                play,
                container,
                protocol,
            );
            let url = (method == SubtitleDeliveryMethod::External).then(|| {
                format!(
                    "/Videos/{}/{}/Subtitles/{}/{ticks}/Stream.vtt{key}",
                    request.item_id, source.id, stream.index
                )
            });
            SubtitleDelivery {
                index: stream.index,
                method,
                url,
            }
        })
        .collect()
}
