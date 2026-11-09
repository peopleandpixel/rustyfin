//! The video decision for one source: which play method BuildVideoItem picks, the transcoding
//! profile it applies, and the stream info that the transcoding URL is written from. A port of the
//! video path of Jellyfin's `StreamBuilder` and of the `StreamInfo` fields that `ToUrl` reads.

use rustyfin_model::device_profile::{
    CodecProfile, CodecType, ConditionProperty, ConditionType, DeviceProfile, DirectPlayProfile,
    EncodingContext, ProfileCondition, ProfileType, StreamProtocol, SubtitleDeliveryMethod,
    TranscodeSeekInfo, TranscodingProfile,
};

use super::conditions::{self, AudioFacts, VideoFacts};
use super::subtitles;
use super::url::StreamOptions;
use super::{
    PlayMethod, SourceFacts, StreamFacts, StreamKind, StreamRequest, TranscodeReason,
    TranscodeReasons,
};

/// The stream info that a transcoding URL is written from: Jellyfin's `StreamInfo`, for the fields
/// that `ToUrl` reads. Defaults are Jellyfin's unset values, so an optional field the profile
/// doesn't set is left out of the URL.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct StreamInfo {
    /// The item being played.
    pub(super) item_id: String,
    /// The requesting device.
    pub(super) device_id: String,
    /// The id of the media source.
    pub(super) media_source_id: String,
    /// The play session.
    pub(super) play_session_id: String,
    /// The access token, written as `api_key`.
    pub(super) access_token: String,
    /// The video codecs the transcode may produce.
    pub(super) video_codecs: Vec<String>,
    /// The audio codecs the transcode may produce.
    pub(super) audio_codecs: Vec<String>,
    /// The audio stream the transcode uses.
    pub(super) audio_stream_index: Option<i32>,
    /// The plan's subtitle stream, when its index names one (-1 never does).
    pub(super) subtitle_stream_index: Option<i32>,
    /// How that subtitle stream is delivered by this transcode (SB 818-824), when it has one.
    pub(super) subtitle_method: Option<SubtitleDeliveryMethod>,
    /// The request's AlwaysBurnInSubtitleWhenTranscoding, which names the subtitle in the URL even
    /// when it is delivered as a file.
    pub(super) always_burn_in_subtitle: bool,
    /// The video bitrate in bits per second.
    pub(super) video_bitrate: Option<i32>,
    /// The audio bitrate in bits per second.
    pub(super) audio_bitrate: Option<i32>,
    /// The audio sample rate.
    pub(super) audio_sample_rate: Option<i32>,
    /// The largest width the transcode may produce.
    pub(super) max_width: Option<i32>,
    /// The largest height the transcode may produce.
    pub(super) max_height: Option<i32>,
    /// The largest frame rate the transcode may produce.
    pub(super) max_framerate: Option<f32>,
    /// The segment container: the transcoding profile's container.
    pub(super) container: Option<String>,
    /// The protocol of the transcode: the transcoding profile's.
    pub(super) sub_protocol: StreamProtocol,
    /// The segment length in seconds, when the profile sets one above zero.
    pub(super) segment_length: Option<i32>,
    /// The minimum number of segments, when the profile sets one above zero.
    pub(super) min_segments: Option<i32>,
    /// The start position in ticks.
    pub(super) start_position_ticks: i64,
    /// The most audio channels the transcode may produce.
    pub(super) transcoding_max_audio_channels: Option<i32>,
    /// Jellyfin's `GlobalMaxAudioChannels`: the channel limit that the audio bitrate and the
    /// audio profile conditions use. It is not written to the URL.
    pub(super) global_max_audio_channels: Option<i32>,
    /// Whether the output must not be anamorphic.
    pub(super) require_non_anamorphic: bool,
    /// Whether subtitles are written into the manifest.
    pub(super) enable_subtitles_in_manifest: bool,
    /// Whether the MPEG-TS and M2TS mode is on.
    pub(super) enable_mpegts_m2ts_mode: bool,
    /// Whether the content length is estimated.
    pub(super) estimate_content_length: bool,
    /// Whether the timestamps are copied.
    pub(super) copy_timestamps: bool,
    /// Whether the output must be AVC.
    pub(super) require_avc: bool,
    /// Whether the audio is encoded with variable bitrate.
    pub(super) enable_audio_vbr_encoding: bool,
    /// How the transcode seeks: only `Bytes` is written to the URL.
    pub(super) transcode_seek_info: TranscodeSeekInfo,
    /// The codec options that go into the URL, in insertion order.
    pub(super) stream_options: StreamOptions,
    /// Why the stream can't be copied as it is.
    pub(super) transcode_reasons: TranscodeReasons,
    /// Whether the transcode is audio only. `to_url` writes `AudioOnly=true` as the last parameter.
    pub(super) audio_only: bool,
}

/// The options that BuildVideoItem reads: Jellyfin's `MediaOptions` after the request has been
/// applied to one source (MIH steps 1 and 2).
pub(super) struct VideoOptions<'a> {
    /// The device profile.
    pub(super) profile: &'a DeviceProfile,
    /// The request, for its identifiers, its flags and its audio channel limit.
    pub(super) request: &'a StreamRequest,
    /// The audio stream the request chose for this source, if any.
    pub(super) audio_stream_index: Option<i32>,
    /// The subtitle stream the request chose for this source, if any.
    pub(super) subtitle_stream_index: Option<i32>,
    /// The bitrate limit in bits per second: the request's, else the profile's streaming limit.
    pub(super) max_bitrate: Option<i32>,
}

/// What the video decision makes of one source: BuildVideoItem's outcome.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct VideoPlan {
    /// Whether the file plays as it is.
    pub(super) direct_play: bool,
    /// The source container, normalized to one container (SB 843).
    pub(super) container: String,
    /// Why the source can't play as it is.
    pub(super) transcode_reasons: TranscodeReasons,
    /// The audio stream that plays.
    pub(super) audio_stream_index: Option<i32>,
    /// The subtitle stream index (SB 660): the chosen one, or the source's default. -1 means none.
    pub(super) subtitle_stream_index: Option<i32>,
    /// The transcode's stream info, when a transcoding profile was applied.
    pub(super) transcode: Option<StreamInfo>,
}

/// The direct play choice for a video (Jellyfin's `GetVideoDirectPlayProfile`): the profile that
/// matched and the audio stream it plays, or else the reasons it can't be played.
struct DirectPlayChoice<'a> {
    /// The direct play profile that matched, with the audio stream index it plays.
    matched: Option<(&'a DirectPlayProfile, Option<i32>)>,
    /// The reasons the video can't be played directly, when no profile matched.
    reasons: TranscodeReasons,
}

/// One video direct play profile, analysed for a source.
struct Analysis<'a> {
    /// The profile.
    profile: &'a DirectPlayProfile,
    /// Whether the profile plays the source directly.
    matched: bool,
    /// The audio stream the profile plays.
    audio_stream_index: Option<i32>,
    /// The reasons the profile fails.
    reasons: TranscodeReasons,
    /// How serious the failure is: lower is more fixable.
    rank: usize,
    /// The profile's position among the video profiles.
    order: usize,
}

/// A set of transcode reasons from a list of them.
fn reasons_of(list: &[TranscodeReason]) -> TranscodeReasons {
    list.iter()
        .fold(TranscodeReasons::default(), |set, reason| set | *reason)
}

/// Jellyfin's `ContainerReasons`.
fn container_reasons() -> TranscodeReasons {
    reasons_of(&[
        TranscodeReason::ContainerNotSupported,
        TranscodeReason::ContainerBitrateExceedsLimit,
    ])
}

/// Jellyfin's `VideoCodecReasons`.
fn video_codec_reasons() -> TranscodeReasons {
    reasons_of(&[
        TranscodeReason::VideoResolutionNotSupported,
        TranscodeReason::AnamorphicVideoNotSupported,
        TranscodeReason::InterlacedVideoNotSupported,
        TranscodeReason::VideoBitDepthNotSupported,
        TranscodeReason::VideoBitrateNotSupported,
        TranscodeReason::VideoFramerateNotSupported,
        TranscodeReason::VideoLevelNotSupported,
        TranscodeReason::RefFramesNotSupported,
        TranscodeReason::VideoRangeTypeNotSupported,
        TranscodeReason::VideoProfileNotSupported,
        TranscodeReason::VideoRotationNotSupported,
    ])
}

/// Jellyfin's `AudioCodecReasons`.
fn audio_codec_reasons() -> TranscodeReasons {
    reasons_of(&[
        TranscodeReason::AudioBitrateNotSupported,
        TranscodeReason::AudioChannelsNotSupported,
        TranscodeReason::AudioProfileNotSupported,
        TranscodeReason::AudioSampleRateNotSupported,
        TranscodeReason::SecondaryAudioNotSupported,
        TranscodeReason::AudioBitDepthNotSupported,
        TranscodeReason::AudioIsExternal,
    ])
}

/// The rankings that `GetRank` uses for a direct play, in order.
fn direct_play_rankings() -> [TranscodeReasons; 5] {
    [
        TranscodeReason::VideoCodecNotSupported.into(),
        video_codec_reasons(),
        TranscodeReason::AudioCodecNotSupported.into(),
        audio_codec_reasons(),
        container_reasons(),
    ]
}

/// Jellyfin's `GetRank`: the 1-based index of the first ranking that `reasons` hits, or one past
/// the last ranking when none does.
fn rank_of(reasons: TranscodeReasons, rankings: &[TranscodeReasons]) -> usize {
    rankings
        .iter()
        .position(|flag| reasons.intersects(*flag))
        .map_or(rankings.len() + 1, |index| index + 1)
}

/// Splits a leading '-' off a profile list, which reverses its meaning.
fn split_negation(profile: &str) -> (bool, &str) {
    match profile.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, profile),
    }
}

/// Jellyfin's span overload of `ContainerHelper.ContainsContainer(profile, negative, input)`, for a
/// profile with its '-' already removed. An empty profile matches everything, even an empty input.
/// Otherwise an entry of `input` that equals an entry of `profile`, ignoring ASCII case, gives
/// `!negative`, and no match, an empty input included, gives `negative`.
fn list_match(profile: &str, negative: bool, input: &str) -> bool {
    if profile.is_empty() {
        return true;
    }
    let found = conditions::split(input).iter().any(|entry| {
        profile
            .split(',')
            .any(|name| !name.is_empty() && name.eq_ignore_ascii_case(entry))
    });
    if found { !negative } else { negative }
}

/// Jellyfin's `ContainerProfile.ContainsContainer`: a profile with a '-' is negated, and an empty
/// profile matches every container, even an empty one.
fn container_profile_matches(profile: &str, input: &str) -> bool {
    let (negative, names) = split_negation(profile);
    list_match(names, negative, input)
}

/// Jellyfin's `ContainsContainer(codec profile, false, codec)` for one codec of a list. An empty
/// codec never matches, and the profile's '-' has no meaning here.
fn codec_entry_matches(profile: &str, codec: &str) -> bool {
    !codec.is_empty() && list_match(profile, false, codec)
}

/// Jellyfin's `ContainsContainer(codec profile, false, codec)` for one codec. An empty profile
/// matches any codec, and an empty codec matches only an empty profile.
fn codec_matches(profile: &str, codec: &str) -> bool {
    list_match(profile, false, codec)
}

/// Jellyfin's `ContainsContainer(list, false, input)`: an entry of `input` equals an entry of the
/// list. An empty list matches nothing.
fn list_contains(list: &[String], input: &str) -> bool {
    conditions::split(input)
        .iter()
        .any(|entry| list.iter().any(|item| item.eq_ignore_ascii_case(entry)))
}

/// The container that a codec profile checks: its sub-container when `use_sub` is set and its
/// container is HLS, else its container.
fn codec_container(profile: &CodecProfile, use_sub: bool) -> &str {
    if use_sub && profile.container.eq_ignore_ascii_case("hls") {
        &profile.sub_container
    } else {
        &profile.container
    }
}

/// Jellyfin's `CodecProfile.ContainsAnyCodec(codec, container)` for one codec.
pub(super) fn codec_profile_matches(
    profile: &CodecProfile,
    codec: &str,
    container: &str,
    use_sub: bool,
) -> bool {
    conditions::contains_container(codec_container(profile, use_sub), container)
        && codec_matches(&profile.codec, codec)
}

/// Jellyfin's `CodecProfile.ContainsAnyCodec(codecs, container)` for a list of codecs.
fn codec_profile_list_matches(
    profile: &CodecProfile,
    codecs: &[String],
    container: &str,
    use_sub: bool,
) -> bool {
    conditions::contains_container(codec_container(profile, use_sub), container)
        && codecs
            .iter()
            .any(|codec| codec_entry_matches(&profile.codec, codec))
}

/// The number of streams of `kind`, or `None` when the source has no streams (Jellyfin's
/// `GetStreamCount`).
fn stream_count(source: &SourceFacts, kind: StreamKind) -> Option<i32> {
    if source.streams.is_empty() {
        return None;
    }
    let count = source.streams.iter().filter(|s| s.kind == kind).count();
    Some(i32::try_from(count).unwrap_or(i32::MAX))
}

/// The video facts that the profile conditions test, for `video` of `source`. A missing video
/// stream gives no facts, except a frame rate of zero, as in Jellyfin.
fn video_facts(source: &SourceFacts, video: Option<&StreamFacts>) -> VideoFacts {
    VideoFacts {
        width: video.and_then(|v| v.width),
        height: video.and_then(|v| v.height),
        bit_depth: video.and_then(|v| v.bit_depth),
        bitrate: video.and_then(|v| v.bit_rate),
        profile: video.and_then(|v| v.profile.clone()),
        range_type: video.and_then(|v| v.range_type.clone()),
        level: video.and_then(|v| v.level),
        framerate: Some(video.and_then(|v| v.frame_rate).unwrap_or(0.0)),
        packet_length: None,
        is_anamorphic: None,
        is_interlaced: video.and_then(|v| v.is_interlaced),
        ref_frames: None,
        num_streams: i32::try_from(source.streams.len()).unwrap_or(i32::MAX),
        num_video_streams: stream_count(source, StreamKind::Video),
        num_audio_streams: stream_count(source, StreamKind::Audio),
        codec_tag: video.and_then(|v| v.codec_tag.clone()),
        is_avc: None,
        rotation: None,
    }
}

/// The audio facts that the profile conditions test, for an audio stream.
pub(super) fn audio_facts(audio: &StreamFacts, is_secondary: bool) -> AudioFacts {
    AudioFacts {
        channels: audio.channels,
        bitrate: audio.bit_rate,
        sample_rate: audio.sample_rate,
        bit_depth: audio.bit_depth,
        profile: audio.profile.clone(),
        is_secondary: Some(is_secondary),
    }
}

/// Jellyfin's `MediaSourceInfo.VideoStream`: the first video stream.
fn video_stream(source: &SourceFacts) -> Option<&StreamFacts> {
    source.streams.iter().find(|s| s.kind == StreamKind::Video)
}

/// Jellyfin's `MediaSourceInfo.GetDefaultAudioStream`: the audio stream at `index` when there is
/// one, else the first default audio stream, else the first audio stream.
pub(super) fn default_audio_stream(
    source: &SourceFacts,
    index: Option<i32>,
) -> Option<&StreamFacts> {
    let audio = |s: &&StreamFacts| s.kind == StreamKind::Audio;
    index
        .filter(|i| *i != -1)
        .and_then(|i| source.streams.iter().filter(audio).find(|s| s.index == i))
        .or_else(|| source.streams.iter().filter(audio).find(|s| s.is_default))
        .or_else(|| source.streams.iter().find(audio))
}

/// Jellyfin's `MediaSourceInfo.IsSecondaryAudio`: an external stream is never secondary, and an
/// internal stream is secondary unless it is the first internal audio stream. `None` when the
/// source has no internal audio stream.
fn is_secondary_audio(source: &SourceFacts, stream: &StreamFacts) -> Option<bool> {
    if stream.is_external {
        return Some(false);
    }
    source
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Audio && !s.is_external)
        .map(|first| first.index != stream.index)
}

/// Jellyfin's `IsBitrateLimitExceeded`: a remote source is never over the limit, a limit of zero or
/// less is no limit, and an unknown bitrate counts as 40 Mbps.
pub(super) fn bitrate_limit_exceeded(source: &SourceFacts, max_bitrate: i64) -> bool {
    if source.is_remote {
        return false;
    }
    let requested = if max_bitrate > 0 {
        max_bitrate
    } else {
        i64::from(i32::MAX)
    };
    source.bitrate.unwrap_or(40_000_000) > requested
}

/// The reasons of the video conditions that `facts` fails. Jellyfin's `AggregateFailureConditions`
/// over `CheckVideoConditions`.
fn video_failures(profile_conditions: &[ProfileCondition], facts: &VideoFacts) -> TranscodeReasons {
    profile_conditions
        .iter()
        .filter(|condition| !conditions::video_condition_satisfied(condition, facts))
        .fold(TranscodeReasons::default(), |set, condition| {
            set | conditions::reason_for_failed_condition(condition)
        })
}

/// The reasons of the video-audio conditions that the audio of a video fails, for the video-audio
/// codec profiles that match `codec` and `container` (Jellyfin's `GetProfileConditionsForVideoAudio`).
fn video_audio_failures(
    profile: &DeviceProfile,
    container: &str,
    codec: &str,
    facts: &AudioFacts,
) -> TranscodeReasons {
    profile
        .codec_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == CodecType::VideoAudio
                && codec_profile_matches(cp, codec, container, false)
                && cp
                    .apply_conditions
                    .iter()
                    .all(|c| conditions::video_audio_condition_satisfied(c, facts))
        })
        .flat_map(|cp| cp.conditions.iter())
        .filter(|c| !conditions::video_audio_condition_satisfied(c, facts))
        .fold(TranscodeReasons::default(), |set, c| {
            set | conditions::reason_for_failed_condition(c)
        })
}

/// Jellyfin's `GetCompatibilityContainer`: the reasons the video container profiles that match
/// `container` fail for `video`.
fn compat_container(
    profile: &DeviceProfile,
    source: &SourceFacts,
    container: &str,
    video: Option<&StreamFacts>,
) -> TranscodeReasons {
    let facts = video_facts(source, video);
    profile
        .container_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == ProfileType::Video && container_profile_matches(&cp.container, container)
        })
        .fold(TranscodeReasons::default(), |set, cp| {
            set | video_failures(&cp.conditions, &facts)
        })
}

/// Jellyfin's `GetCompatibilityVideoCodec`: the reasons the video codec profiles that match
/// `video` and `container`, and whose apply conditions hold, fail for `video`.
fn compat_video_codec(
    profile: &DeviceProfile,
    source: &SourceFacts,
    container: &str,
    video: &StreamFacts,
) -> TranscodeReasons {
    let facts = video_facts(source, Some(video));
    profile
        .codec_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == CodecType::Video
                && codec_profile_matches(cp, &video.codec, container, false)
                && cp
                    .apply_conditions
                    .iter()
                    .all(|c| conditions::video_condition_satisfied(c, &facts))
        })
        .fold(TranscodeReasons::default(), |set, cp| {
            set | video_failures(&cp.conditions, &facts)
        })
}

/// Jellyfin's `GetCompatibilityAudioCodec` for a video. `transcoding_codec` replaces the stream's
/// codec when a transcode would produce a different one.
fn compat_audio(
    profile: &DeviceProfile,
    container: &str,
    audio: &StreamFacts,
    transcoding_codec: Option<&str>,
    is_secondary: bool,
) -> TranscodeReasons {
    let codec = transcoding_codec.unwrap_or(audio.codec.as_str());
    video_audio_failures(profile, container, codec, &audio_facts(audio, is_secondary))
}

/// Jellyfin's `GetCompatibilityAudioCodecDirect`: `compat_audio` for a direct play, plus the reason
/// that an external audio stream is never played directly.
fn compat_audio_direct(
    profile: &DeviceProfile,
    source: &SourceFacts,
    container: &str,
    audio: &StreamFacts,
) -> TranscodeReasons {
    let is_secondary = is_secondary_audio(source, audio).unwrap_or(false);
    let mut failures = compat_audio(profile, container, audio, None, is_secondary);
    if audio.is_external {
        failures.insert(TranscodeReason::AudioIsExternal);
    }
    failures
}

/// Whether a video direct play profile plays the video's codec.
fn direct_play_video_codec(profile: &DirectPlayProfile, codec: &str) -> bool {
    profile.r#type == ProfileType::Video
        && conditions::contains_container(&profile.video_codec, codec)
}

/// Whether a direct play profile plays the audio's codec.
pub(super) fn direct_play_audio_codec(profile: &DirectPlayProfile, codec: &str) -> bool {
    conditions::contains_container(&profile.audio_codec, codec)
}

/// Jellyfin's `NormalizeMediaSourceFormatIntoSingleContainer`: the first container of a comma list
/// that a video direct play profile supports, where `direct` is the profile that was chosen, or
/// else every video direct play profile. The input is returned when there is no comma or no
/// profile supports any entry.
fn normalize_container(
    input: &str,
    profile: &DeviceProfile,
    direct: Option<&DirectPlayProfile>,
) -> String {
    if !input.contains(',') {
        return input.to_string();
    }
    let play_profiles: Vec<&DirectPlayProfile> = match direct {
        Some(chosen) => vec![chosen],
        None => profile.direct_play_profiles.iter().collect(),
    };
    conditions::split(input)
        .into_iter()
        .find(|format| {
            play_profiles.iter().any(|p| {
                p.r#type == ProfileType::Video
                    && conditions::contains_container(&p.container, format)
            })
        })
        .unwrap_or_else(|| input.to_string())
}

/// Jellyfin's `GetVideoDirectPlayProfile`. A profile plays the video directly when its container,
/// video codec and audio codec all match, the video and audio conditions hold, the request allows
/// a direct play, and the source's own direct play flag is set. `subtitle_found` is the chosen
/// subtitle's failure (SB 1319-1332), which every profile shares (SB 1377). When nothing plays,
/// the reasons of the best-ranked failure are returned, or `DirectPlayError` when there are none.
fn direct_play_choice<'a>(
    profile: &'a DeviceProfile,
    source: &SourceFacts,
    video: Option<&StreamFacts>,
    candidates: &[&StreamFacts],
    eligible: bool,
    subtitle_found: TranscodeReasons,
) -> DirectPlayChoice<'a> {
    let container = source.container.as_str();
    let container_found = compat_container(profile, source, container, video);
    let video_found = video.map_or_else(TranscodeReasons::default, |v| {
        compat_video_codec(profile, source, container, v)
    });
    let audio_found: Vec<TranscodeReasons> = candidates
        .iter()
        .map(|audio| compat_audio_direct(profile, source, container, audio))
        .collect();
    let video_codec = video.map_or("", |v| v.codec.as_str());
    let rankings = direct_play_rankings();

    let mut contained = false;
    let mut analyses: Vec<Analysis<'a>> = profile
        .direct_play_profiles
        .iter()
        .filter(|dp| dp.r#type == ProfileType::Video)
        .enumerate()
        .map(|(order, dp)| {
            let mut failure = TranscodeReasons::default();
            if conditions::contains_container(&dp.container, container) {
                contained = true;
            } else {
                failure.insert(TranscodeReason::ContainerNotSupported);
            }
            if !direct_play_video_codec(dp, video_codec) {
                failure.insert(TranscodeReason::VideoCodecNotSupported);
            }
            let mut selected = None;
            let mut audio_found_for_profile = TranscodeReasons::default();
            if !candidates.is_empty() {
                match candidates
                    .iter()
                    .position(|audio| direct_play_audio_codec(dp, &audio.codec))
                {
                    Some(index) => {
                        selected = Some(index);
                        audio_found_for_profile = audio_found[index];
                    }
                    None => failure.insert(TranscodeReason::AudioCodecNotSupported),
                }
            }

            let mut failure = failure | container_found | subtitle_found;
            if !failure.contains(TranscodeReason::VideoCodecNotSupported) {
                failure |= video_found;
            }
            if !failure.contains(TranscodeReason::AudioCodecNotSupported) {
                failure |= audio_found_for_profile;
            }

            Analysis {
                profile: dp,
                matched: failure.is_empty() && eligible && source.supports_direct_play,
                audio_stream_index: selected.map(|index| candidates[index].index),
                reasons: failure,
                rank: rank_of(failure, &rankings),
                order,
            }
        })
        .collect();

    analyses.sort_by(|a, b| {
        b.matched
            .cmp(&a.matched)
            .then(b.rank.cmp(&a.rank))
            .then(a.order.cmp(&b.order))
    });

    if let Some(first) = analyses.iter().find(|a| a.matched) {
        return DirectPlayChoice {
            matched: Some((first.profile, first.audio_stream_index)),
            reasons: first.reasons,
        };
    }

    let reasons = analyses
        .iter()
        .find(|a| !contained || !a.reasons.contains(TranscodeReason::ContainerNotSupported))
        .map_or_else(TranscodeReasons::default, |a| a.reasons);
    let reasons = if reasons.is_empty() {
        TranscodeReason::DirectPlayError.into()
    } else {
        reasons
    };
    DirectPlayChoice {
        matched: None,
        reasons,
    }
}

impl StreamInfo {
    /// Jellyfin's `GetTargetAudioChannels`: the channels that `qualifier` sets, or the default limit
    /// (the global limit, else the transcoding limit) when it sets none. A set value is capped at
    /// the default limit.
    fn target_audio_channels(&self, qualifier: Option<&str>) -> Option<i32> {
        let default = self
            .global_max_audio_channels
            .or(self.transcoding_max_audio_channels);
        let value = self
            .stream_options
            .get_qualified(qualifier, "audiochannels")
            .unwrap_or("");
        if value.is_empty() {
            return default;
        }
        match conditions::try_int(value) {
            Some(channels) => Some(default.map_or(channels, |limit| channels.min(limit))),
            None => default,
        }
    }
}

/// The names of Jellyfin's `VideoRangeType` members, in their declaration order, which `NotEquals`
/// takes the rest from. The order is that of `Jellyfin.Data/Enums/VideoRangeType.cs`.
const VIDEO_RANGE_TYPES: [&str; 13] = [
    "Unknown",
    "SDR",
    "HDR10",
    "HLG",
    "DOVI",
    "DOVIWithHDR10",
    "DOVIWithHLG",
    "DOVIWithSDR",
    "DOVIWithEL",
    "DOVIWithHDR10Plus",
    "DOVIWithELHDR10Plus",
    "DOVIInvalid",
    "HDR10Plus",
];

/// Whether a condition on `property` may apply. `qualified` says whether the condition is for a
/// codec. The gates are Jellyfin's `enableQualifiedConditions` and `enableNonQualifiedConditions`,
/// and each property is gated the way Jellyfin gates it.
fn condition_allowed(
    property: ConditionProperty,
    qualified: bool,
    enable_qualified: bool,
    enable_non_qualified: bool,
) -> bool {
    match property {
        ConditionProperty::AudioBitrate
        | ConditionProperty::AudioSampleRate
        | ConditionProperty::IsAvc
        | ConditionProperty::IsAnamorphic
        | ConditionProperty::Height
        | ConditionProperty::VideoBitrate
        | ConditionProperty::VideoFramerate
        | ConditionProperty::Width => enable_non_qualified,
        ConditionProperty::AudioChannels
        | ConditionProperty::IsInterlaced
        | ConditionProperty::RefFrames
        | ConditionProperty::VideoBitDepth => {
            if qualified {
                enable_qualified
            } else {
                enable_non_qualified
            }
        }
        ConditionProperty::VideoProfile
        | ConditionProperty::VideoRangeType
        | ConditionProperty::VideoCodecTag
        | ConditionProperty::VideoRotation
        | ConditionProperty::VideoLevel => qualified,
        _ => true,
    }
}

/// The value that a condition of `kind` gives a property. `Equals` sets `num`, and `LessThanEqual`
/// sets the smaller of `num` and `current`, or `num` when there is no current value. Other kinds
/// give nothing, so the property keeps its value.
fn cap<T: PartialOrd + Copy>(kind: ConditionType, num: T, current: Option<T>) -> Option<T> {
    match kind {
        ConditionType::Equals => Some(num),
        ConditionType::LessThanEqual => Some(match current {
            Some(current) if current < num => current,
            _ => num,
        }),
        _ => None,
    }
}

/// The entries of a list condition's value: the value split at `|`, without the empty entries.
/// Each entry is trimmed first when `trim` is set.
fn list_values(value: &str, trim: bool) -> Vec<&str> {
    value
        .split('|')
        .map(|entry| if trim { entry.trim() } else { entry })
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Sets the list option `name` for an `Equals` or `EqualsAny` condition, as Jellyfin's
/// `ApplyTranscodingConditions` does. `Equals` sets the listed values. `EqualsAny` keeps the
/// current value when it is set and one of the listed values matches it under `same`, and sets the
/// listed values otherwise. Other kinds change nothing.
fn set_list_option(
    stream: &mut StreamInfo,
    qualifier: Option<&str>,
    name: &str,
    kind: ConditionType,
    values: &[&str],
    same: fn(&str, &str) -> bool,
) {
    let value = match kind {
        ConditionType::Equals => values.join(","),
        ConditionType::EqualsAny => {
            let current = stream
                .stream_options
                .get_qualified(qualifier, name)
                .map(str::to_owned);
            match current {
                Some(current)
                    if !current.is_empty() && values.iter().any(|v| same(v, &current)) =>
                {
                    current
                }
                _ => values.join(","),
            }
        }
        _ => return,
    };
    stream.stream_options.set_qualified(qualifier, name, &value);
}

/// Jellyfin's `ApplyTranscodingConditions`: applies the conditions of the matching codec profiles to
/// the stream info. `qualifier` is the codec the conditions are for, if any. A condition with an
/// empty value or a `GreaterThanEqual` type is skipped, as Jellyfin skips it. A property that
/// Jellyfin doesn't read yet changes nothing.
pub(super) fn apply_transcoding_conditions(
    stream: &mut StreamInfo,
    profile_conditions: &[ProfileCondition],
    qualifier: Option<&str>,
    enable_qualified: bool,
    enable_non_qualified: bool,
) {
    let qualifier = qualifier.filter(|q| !q.is_empty());
    for condition in profile_conditions {
        let value = condition.value.as_str();
        let kind = condition.condition;
        if value.is_empty() || matches!(kind, ConditionType::GreaterThanEqual) {
            continue;
        }
        if !condition_allowed(
            condition.property,
            qualifier.is_some(),
            enable_qualified,
            enable_non_qualified,
        ) {
            continue;
        }
        match condition.property {
            ConditionProperty::AudioBitrate => {
                if let Some(num) = conditions::try_int(value) {
                    stream.audio_bitrate =
                        cap(kind, num, stream.audio_bitrate).or(stream.audio_bitrate);
                }
            }
            ConditionProperty::AudioSampleRate => {
                if let Some(num) = conditions::try_int(value) {
                    stream.audio_sample_rate =
                        cap(kind, num, stream.audio_sample_rate).or(stream.audio_sample_rate);
                }
            }
            ConditionProperty::AudioChannels => {
                if let Some(num) = conditions::try_int(value) {
                    let target = stream.target_audio_channels(qualifier);
                    if let Some(set) = cap(kind, num, target) {
                        stream.stream_options.set_qualified(
                            qualifier,
                            "audiochannels",
                            &set.to_string(),
                        );
                    }
                }
            }
            ConditionProperty::IsAvc => {
                let wanted = conditions::try_bool(value).is_some_and(|is_avc| {
                    (is_avc && matches!(kind, ConditionType::Equals))
                        || (!is_avc && matches!(kind, ConditionType::NotEquals))
                });
                if wanted {
                    stream.require_avc = true;
                }
            }
            ConditionProperty::IsAnamorphic => {
                let wanted = conditions::try_bool(value).is_some_and(|is_anamorphic| {
                    (is_anamorphic && matches!(kind, ConditionType::Equals))
                        || (!is_anamorphic && matches!(kind, ConditionType::NotEquals))
                });
                if wanted {
                    stream.require_non_anamorphic = true;
                }
            }
            ConditionProperty::IsInterlaced => {
                let wanted = conditions::try_bool(value).is_some_and(|is_interlaced| {
                    (!is_interlaced && matches!(kind, ConditionType::Equals))
                        || (is_interlaced && matches!(kind, ConditionType::NotEquals))
                });
                if wanted {
                    stream
                        .stream_options
                        .set_qualified(qualifier, "deinterlace", "true");
                }
            }
            ConditionProperty::RefFrames => {
                if let Some(num) = conditions::try_int(value) {
                    let target = stream
                        .stream_options
                        .get_qualified(qualifier, "maxrefframes")
                        .and_then(conditions::try_int);
                    if let Some(set) = cap(kind, num, target) {
                        stream.stream_options.set_qualified(
                            qualifier,
                            "maxrefframes",
                            &set.to_string(),
                        );
                    }
                }
            }
            ConditionProperty::VideoBitDepth => {
                if let Some(num) = conditions::try_int(value) {
                    let target = stream
                        .stream_options
                        .get_qualified(qualifier, "videobitdepth")
                        .and_then(conditions::try_int);
                    if let Some(set) = cap(kind, num, target) {
                        stream.stream_options.set_qualified(
                            qualifier,
                            "videobitdepth",
                            &set.to_string(),
                        );
                    }
                }
            }
            ConditionProperty::Height => {
                if let Some(num) = conditions::try_int(value) {
                    stream.max_height = cap(kind, num, stream.max_height).or(stream.max_height);
                }
            }
            ConditionProperty::Width => {
                if let Some(num) = conditions::try_int(value) {
                    stream.max_width = cap(kind, num, stream.max_width).or(stream.max_width);
                }
            }
            ConditionProperty::VideoBitrate => {
                if let Some(num) = conditions::try_int(value) {
                    stream.video_bitrate =
                        cap(kind, num, stream.video_bitrate).or(stream.video_bitrate);
                }
            }
            ConditionProperty::VideoFramerate => {
                if let Some(num) = conditions::try_double(value).map(|v| v as f32) {
                    stream.max_framerate =
                        cap(kind, num, stream.max_framerate).or(stream.max_framerate);
                }
            }
            ConditionProperty::VideoLevel => {
                if let Some(num) = conditions::try_int(value).map(f64::from) {
                    let target = stream
                        .stream_options
                        .get_qualified(qualifier, "level")
                        .and_then(conditions::try_double);
                    if let Some(set) = cap(kind, num, target) {
                        stream
                            .stream_options
                            .set_qualified(qualifier, "level", &set.to_string());
                    }
                }
            }
            ConditionProperty::VideoProfile => {
                let values = list_values(value, false);
                set_list_option(stream, qualifier, "profile", kind, &values, |a, b| a == b);
            }
            ConditionProperty::VideoRangeType => {
                let values = list_values(value, true);
                if matches!(kind, ConditionType::NotEquals) {
                    let rest: Vec<&str> = VIDEO_RANGE_TYPES
                        .into_iter()
                        .filter(|name| !values.contains(name))
                        .collect();
                    stream
                        .stream_options
                        .set_qualified(qualifier, "rangetype", &rest.join(","));
                } else {
                    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
                    set_list_option(stream, qualifier, "rangetype", kind, &values, same);
                }
            }
            ConditionProperty::VideoCodecTag => {
                let values = list_values(value, true);
                let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
                set_list_option(stream, qualifier, "codectag", kind, &values, same);
            }
            ConditionProperty::VideoRotation => {
                let values = list_values(value, true);
                let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
                set_list_option(stream, qualifier, "rotation", kind, &values, same);
            }
            _ => {}
        }
    }
}

/// The manifest containers, which can't be played directly (Jellyfin's `ManifestContainers`).
const MANIFEST_CONTAINERS: &str = "hls,applehttp,dash";

/// The video codecs an HLS transcode may produce (Jellyfin's `_supportedHlsVideoCodecs`).
const HLS_VIDEO_CODECS: [&str; 4] = ["h264", "hevc", "vp9", "av1"];

/// The audio codecs an HLS transcode in a TS segment may produce (Jellyfin's
/// `_supportedHlsAudioCodecsTs`).
const HLS_AUDIO_CODECS_TS: [&str; 4] = ["aac", "ac3", "eac3", "mp3"];

/// The audio codecs an HLS transcode in an MP4 segment may produce (Jellyfin's
/// `_supportedHlsAudioCodecsMp4`).
const HLS_AUDIO_CODECS_MP4: [&str; 9] = [
    "aac", "ac3", "eac3", "mp3", "alac", "flac", "opus", "dts", "truehd",
];

/// Clamps an `i64` into the `i32` range, so that no conversion can panic or wrap.
pub(super) fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

/// The audio stream of `source` with the given index.
fn audio_stream_at(source: &SourceFacts, index: i32) -> Option<&StreamFacts> {
    source
        .streams
        .iter()
        .find(|s| s.kind == StreamKind::Audio && s.index == index)
}

/// The audio streams that the transcode may pick from (Jellyfin's `candidateAudioStreams` when
/// the source has no audio index source). An explicit request index, or no audio stream, leaves
/// only the chosen stream. Otherwise every audio stream counts, or only the default ones when the
/// chosen stream is a default.
fn audio_candidates<'a>(
    source: &'a SourceFacts,
    audio: Option<&'a StreamFacts>,
    requested: Option<i32>,
) -> Vec<&'a StreamFacts> {
    let Some(first) = audio else {
        return Vec::new();
    };
    if requested.is_some_and(|index| index >= 0) {
        return vec![first];
    }
    let audio_streams = source
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::Audio);
    if first.is_default {
        audio_streams.filter(|s| s.is_default).collect()
    } else {
        audio_streams.collect()
    }
}

/// Jellyfin's `SetStreamInfoOptionsFromTranscodingProfile`: copies the transcoding profile's
/// container, protocol, seek mode, manifest flags and segment settings into the stream info.
pub(super) fn set_stream_info_from_transcoding_profile(
    stream: &mut StreamInfo,
    profile: &TranscodingProfile,
) {
    stream.container = Some(profile.container.clone());
    stream.sub_protocol = profile.protocol;
    stream.transcode_seek_info = profile.transcode_seek_info;
    let channels = profile
        .max_audio_channels
        .as_deref()
        .and_then(|text| text.trim().parse::<i32>().ok());
    if let Some(channels) = channels {
        stream.transcoding_max_audio_channels = Some(channels);
    }
    stream.estimate_content_length = profile.estimate_content_length;
    stream.copy_timestamps = profile.copy_timestamps;
    stream.enable_subtitles_in_manifest = profile.enable_subtitles_in_manifest;
    stream.enable_mpegts_m2ts_mode = profile.enable_mpegts_m2ts_mode;
    stream.enable_audio_vbr_encoding = profile.enable_audio_vbr_encoding;
    if profile.min_segments > 0 {
        stream.min_segments = Some(profile.min_segments);
    }
    if profile.segment_length > 0 {
        stream.segment_length = Some(profile.segment_length);
    }
}

/// Jellyfin's rank of one transcoding profile for a source, as a pair (video, audio). The video
/// is 1 when it is copied, 2 when the codec is compatible but not copied, else 3. The audio is the
/// best of the profile's codecs, with the same scale, and the pair orders the profiles.
fn transcoding_rank(
    options: &VideoOptions<'_>,
    source: &SourceFacts,
    video: Option<&StreamFacts>,
    audio: Option<&StreamFacts>,
    profile: &TranscodingProfile,
) -> (u8, u8) {
    let container = profile.container.as_str();
    let mut video_rank = 3;
    let copy_video = video.filter(|v| {
        options.request.allow_video_stream_copy
            && conditions::contains_container(&profile.video_codec, &v.codec)
    });
    if let Some(v) = copy_video {
        video_rank = if compat_video_codec(options.profile, source, container, v).is_empty() {
            1
        } else {
            2
        };
    }
    let mut audio_rank = 3;
    if let Some(a) = audio.filter(|_| options.request.allow_audio_stream_copy) {
        for codec in conditions::split(&profile.audio_codec) {
            let rank =
                if compat_audio(options.profile, container, a, Some(&codec), false).is_empty() {
                    if codec.eq_ignore_ascii_case(&a.codec) {
                        1
                    } else {
                        2
                    }
                } else {
                    3
                };
            audio_rank = audio_rank.min(rank);
            if audio_rank == 1 {
                break;
            }
        }
    }
    (video_rank, audio_rank)
}

/// Jellyfin's `GetVideoTranscodeProfile`: the HLS streaming video profile that the transcode uses.
/// Only the profiles of the video type, in the streaming context and with the HLS protocol count.
/// Ties keep the profile order. Nothing is chosen when the source can't be transcoded or copied.
fn video_transcode_profile<'a>(
    options: &VideoOptions<'a>,
    source: &SourceFacts,
    video: Option<&StreamFacts>,
    audio: Option<&StreamFacts>,
) -> Option<&'a TranscodingProfile> {
    if !(source.supports_transcoding || source.supports_direct_stream) {
        return None;
    }
    let device: &'a DeviceProfile = options.profile;
    device
        .transcoding_profiles
        .iter()
        .filter(|tp| {
            tp.r#type == ProfileType::Video
                && tp.context == EncodingContext::Streaming
                && tp.protocol == StreamProtocol::Hls
        })
        .map(|tp| (tp, transcoding_rank(options, source, video, audio, tp)))
        .min_by_key(|(_, rank)| *rank)
        .map(|(tp, _)| tp)
}

/// Jellyfin's default audio bitrate for `codec` with `channels` channels, which is higher for the
/// lossless codecs and for stream copies of the lossy ones.
fn default_audio_bitrate(codec: Option<&str>, channels: Option<i32>) -> i32 {
    let channels = channels.unwrap_or(0);
    if let Some(codec) = codec.filter(|c| !c.is_empty()) {
        match codec.to_ascii_lowercase().as_str() {
            "aac" | "mp3" | "ac3" | "eac3" => {
                return if channels < 2 {
                    128_000
                } else if channels >= 6 {
                    640_000
                } else {
                    384_000
                };
            }
            "flac" | "alac" => {
                return if channels < 2 {
                    768_000
                } else if channels >= 6 {
                    3_584_000
                } else {
                    1_536_000
                };
            }
            _ => {}
        }
    }
    192_000
}

/// Jellyfin's `GetMaxAudioBitrateForTotalBitrate`: the audio bitrate that a total bitrate allows.
fn max_audio_bitrate_for_total(total: i64) -> i32 {
    match total {
        t if t <= 640_000 => 128_000,
        t if t <= 2_000_000 => 384_000,
        t if t <= 3_000_000 => 448_000,
        t if t <= 4_000_000 => 640_000,
        t if t <= 5_000_000 => 768_000,
        t if t <= 10_000_000 => 1_536_000,
        t if t <= 15_000_000 => 2_304_000,
        t if t <= 20_000_000 => 3_584_000,
        _ => 7_168_000,
    }
}

/// Jellyfin's `TargetAudioCodec`: the codec the transcode writes. When the input's codec is one of
/// the stream's audio codecs, it is that codec; otherwise every audio codec is a target.
fn target_audio_codecs(input: Option<&str>, audio_codecs: &[String]) -> Vec<String> {
    let matched =
        input.and_then(|input| audio_codecs.iter().find(|c| c.eq_ignore_ascii_case(input)));
    match matched {
        Some(codec) if codec.is_empty() => Vec::new(),
        Some(codec) => vec![codec.clone()],
        None => audio_codecs.to_vec(),
    }
}

/// Jellyfin's `GetAudioBitrate`: the bitrate of the audio the transcode writes. `channels` are the
/// input's channels after any channel override, and `target_channels` are the channels the target
/// allows. The total bitrate limit is `max_total` in bits per second, and 0 means no limit.
fn get_audio_bitrate(
    max_total: i64,
    target_codecs: &[String],
    audio: Option<&StreamFacts>,
    channels: Option<i32>,
    target_channels: Option<i32>,
) -> i32 {
    let target_codec = target_codecs.first().map(String::as_str);
    let mut rate = 192_000;
    let mut encoder_limit = i32::MAX;
    if let Some(a) = audio {
        rate = match (target_channels, channels) {
            (Some(target), Some(c)) if c > target => {
                default_audio_bitrate(target_codec, Some(target))
            }
            (Some(_), Some(c))
                if !a.codec.is_empty()
                    && !target_codecs.is_empty()
                    && !target_codecs
                        .iter()
                        .any(|codec| codec.eq_ignore_ascii_case(&a.codec)) =>
            {
                default_audio_bitrate(target_codec, Some(c))
            }
            _ => a
                .bit_rate
                .unwrap_or_else(|| default_audio_bitrate(target_codec, target_channels)),
        };
        // Webm encoding fails on a 1 channel source below 64 kbps, so the encoder is held at 64 kbps.
        if channels == Some(1) && a.bit_rate.unwrap_or(0) < 64_000 {
            encoder_limit = 64_000;
        }
    }
    if max_total > 0 {
        rate = rate.min(max_audio_bitrate_for_total(max_total));
    }
    rate.min(encoder_limit)
}

/// Jellyfin's `BuildStreamVideoItem`: fills in the copy options of a transcode from the video and
/// audio streams, the codec profiles that apply, and the audio and video bitrates. The copy check
/// of the audio passes `is_secondary = false`. The channel override of an audio stream that is too
/// wide feeds both `get_audio_bitrate` and the audio facts of the VideoAudio apply conditions, as
/// Jellyfin's changed `audioStream.Channels` does (SB 1028 and 1104), but the source's streams are
/// never changed.
fn build_stream_video_item(
    stream: &mut StreamInfo,
    options: &VideoOptions<'_>,
    source: &SourceFacts,
    video: Option<&StreamFacts>,
    audio: Option<&StreamFacts>,
    candidates: &[&StreamFacts],
    transcoding: &TranscodingProfile,
) {
    let profile = options.profile;
    let container = transcoding.container.as_str();

    // Prefer the profile's video codecs, or the source's when it names none, then keep the codecs
    // that an HLS transcode can produce.
    let mut video_codecs = conditions::split(&transcoding.video_codec);
    if video_codecs.is_empty() {
        video_codecs.extend(video.map(|v| v.codec.clone()));
    }
    video_codecs.retain(|codec| HLS_VIDEO_CODECS.contains(&codec.as_str()));
    if video.is_some_and(|v| !list_contains(&video_codecs, &v.codec)) {
        stream
            .transcode_reasons
            .insert(TranscodeReason::VideoCodecNotSupported);
    }
    stream.video_codecs = video_codecs;

    // The copy options of the video. Their qualifier is the video codec.
    let qualifier = video.map(|v| v.codec.as_str());
    stream.max_framerate = video.and_then(|v| v.frame_rate);
    if let Some(level) = video.and_then(|v| v.level) {
        stream
            .stream_options
            .set_qualified(qualifier, "level", &level.to_string());
    }
    if let Some(depth) = video.and_then(|v| v.bit_depth) {
        stream
            .stream_options
            .set_qualified(qualifier, "videobitdepth", &depth.to_string());
    }
    if let Some(name) = video
        .and_then(|v| v.profile.as_deref())
        .filter(|p| !p.is_empty())
    {
        stream
            .stream_options
            .set_qualified(qualifier, "profile", &name.to_lowercase());
    }

    // The audio codecs, then the first candidate that one of them can copy.
    let mut audio_codecs = conditions::split(&transcoding.audio_codec);
    if audio_codecs.is_empty() {
        audio_codecs.extend(audio.map(|a| a.codec.clone()));
    }
    if container.eq_ignore_ascii_case("mp4") {
        audio_codecs.retain(|codec| HLS_AUDIO_CODECS_MP4.contains(&codec.as_str()));
    } else {
        audio_codecs.retain(|codec| HLS_AUDIO_CODECS_TS.contains(&codec.as_str()));
    }
    let audio_with_codec = candidates
        .iter()
        .copied()
        .find(|s| list_contains(&audio_codecs, &s.codec));
    let limit = stream.transcoding_max_audio_channels.unwrap_or(i32::MAX);
    let channels_exceed = audio_with_codec
        .and_then(|s| s.channels)
        .is_some_and(|channels| channels > limit);
    let direct_failures = audio_with_codec.map_or_else(TranscodeReasons::default, |s| {
        compat_audio(profile, container, s, None, false)
    });
    stream.transcode_reasons |= direct_failures;
    if audio.is_some() && audio_with_codec.is_none() {
        stream
            .transcode_reasons
            .insert(TranscodeReason::AudioCodecNotSupported);
    }
    let direct_audio = audio_with_codec.filter(|_| {
        !channels_exceed
            && direct_failures.is_empty()
            && !stream
                .transcode_reasons
                .contains(TranscodeReason::ContainerBitrateExceedsLimit)
    });

    // The planned audio stream, when it is too wide, is overwritten for the bitrate only.
    let mut channel_override: Option<(i32, i32)> = None;
    if channels_exceed {
        let planned = stream
            .audio_stream_index
            .and_then(|index| audio_stream_at(source, index));
        if let Some(target) = planned {
            stream
                .transcode_reasons
                .insert(TranscodeReason::AudioChannelsNotSupported);
            channel_override = stream
                .transcoding_max_audio_channels
                .map(|channels| (target.index, channels));
        }
    }

    stream.audio_codecs = audio_codecs.clone();
    let audio = match direct_audio {
        Some(direct) => {
            stream.audio_stream_index = Some(direct.index);
            stream.audio_codecs = vec![direct.codec.clone()];
            stream.audio_sample_rate = direct.sample_rate;
            // The channels of a copied audio are keyed by the video codec, as Jellyfin does.
            let channels = direct.channels.map(|c| c.to_string()).unwrap_or_default();
            stream
                .stream_options
                .set_qualified(qualifier, "audiochannels", &channels);
            if let Some(name) = direct.profile.as_deref().filter(|p| !p.is_empty()) {
                stream.stream_options.set_qualified(
                    Some(direct.codec.as_str()),
                    "profile",
                    &name.to_lowercase(),
                );
            }
            if let Some(level) = direct.level.filter(|level| *level != 0.0) {
                stream.stream_options.set_qualified(
                    Some(direct.codec.as_str()),
                    "level",
                    &level.to_string(),
                );
            }
            Some(direct)
        }
        None => audio,
    };

    // The video codec profiles that apply, in reverse order: the first profile has priority. A
    // video-only condition applies to every matching codec.
    let facts = video_facts(source, video);
    let applied_video: Vec<&CodecProfile> = profile
        .codec_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == CodecType::Video
                && codec_profile_list_matches(cp, &stream.video_codecs, container, true)
                && cp
                    .apply_conditions
                    .iter()
                    .all(|c| conditions::video_condition_satisfied(c, &facts))
        })
        .collect();
    for cp in applied_video.into_iter().rev() {
        for codec in stream.video_codecs.clone() {
            if codec_profile_matches(cp, &codec, container, true) {
                apply_transcoding_conditions(stream, &cp.conditions, Some(&codec), true, true);
            }
        }
    }

    stream.global_max_audio_channels = if channels_exceed {
        stream.transcoding_max_audio_channels
    } else {
        options.request.max_audio_channels
    };

    // The audio bitrate: the target codec's, from the channels after any override.
    let target_codecs = target_audio_codecs(audio.map(|a| a.codec.as_str()), &stream.audio_codecs);
    let target_channels = stream.target_audio_channels(target_codecs.first().map(String::as_str));
    let channels = audio.and_then(|a| match channel_override {
        Some((index, channels)) if index == a.index => Some(channels),
        _ => a.channels,
    });
    let computed = get_audio_bitrate(
        i64::from(options.max_bitrate.unwrap_or(0)),
        &target_codecs,
        audio,
        channels,
        target_channels,
    );
    stream.audio_bitrate = Some(
        stream
            .audio_bitrate
            .map_or(computed, |current| current.min(computed)),
    );

    // The audio codec profiles that apply, in reverse order, with the audio's facts after any
    // channel override.
    let secondary = audio
        .and_then(|a| is_secondary_audio(source, a))
        .unwrap_or(false);
    let audio_now = audio.map_or_else(AudioFacts::default, |a| AudioFacts {
        channels,
        ..audio_facts(a, secondary)
    });
    let applied_audio: Vec<&CodecProfile> = profile
        .codec_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == CodecType::VideoAudio
                && codec_profile_list_matches(cp, &stream.audio_codecs, container, false)
                && cp
                    .apply_conditions
                    .iter()
                    .all(|c| conditions::video_audio_condition_satisfied(c, &audio_now))
        })
        .collect();
    for cp in applied_audio.into_iter().rev() {
        for codec in stream.audio_codecs.clone() {
            if codec_profile_matches(cp, &codec, container, false) {
                apply_transcoding_conditions(stream, &cp.conditions, Some(&codec), true, true);
                break;
            }
        }
    }

    // The video bitrate is the limit minus the audio, but at least 64 kbps.
    if let Some(max) = options.max_bitrate {
        let mut available = i64::from(max);
        if let Some(audio_bitrate) = stream.audio_bitrate {
            available -= i64::from(audio_bitrate);
        }
        let current = stream.video_bitrate.map_or(available, i64::from);
        stream.video_bitrate = Some(saturating_i32(available.min(current).max(64_000)));
    }
}

/// Jellyfin's `BuildVideoItem` for one source: whether the file plays as it is, and when it does
/// not, the HLS transcode that plays it. The DVD and Blu-ray folder rules and the DirectStream
/// branch are not ported. The DirectStream branch is unreachable, because the request clears
/// `EnableDirectStream` (MIH step 2).
pub(super) fn plan_video(source: &SourceFacts, options: &VideoOptions<'_>) -> VideoPlan {
    let profile = options.profile;
    let request = options.request;
    let video = video_stream(source);
    let audio = default_audio_stream(
        source,
        options
            .audio_stream_index
            .or(source.default_audio_stream_index),
    );
    let candidates = audio_candidates(source, audio, options.audio_stream_index);
    let subtitle_index = subtitles::chosen_index(source, options.subtitle_stream_index);
    let subtitle = subtitles::subtitle_stream(source, subtitle_index);

    let exceeded = bitrate_limit_exceeded(source, i64::from(options.max_bitrate.unwrap_or(0)));
    let eligible =
        !exceeded && !conditions::contains_container(MANIFEST_CONTAINERS, &source.container);
    let mut reasons = TranscodeReasons::default();
    if exceeded {
        reasons.insert(TranscodeReason::ContainerBitrateExceedsLimit);
    }
    let mut direct_play = false;
    let mut container = normalize_container(&source.container, profile, None);
    let mut audio_index = audio.map(|a| a.index);
    if eligible {
        let subtitle_found = subtitles::direct_play_reason(&source.container, profile, subtitle);
        let choice = direct_play_choice(profile, source, video, &candidates, true, subtitle_found);
        reasons |= choice.reasons;
        if let Some((direct, index)) = choice.matched {
            direct_play = true;
            container = normalize_container(&source.container, profile, Some(direct));
            audio_index = index.or(audio_index);
        }
    }

    let mut transcode = None;
    if let Some(tp) =
        video_transcode_profile(options, source, video, audio).filter(|_| !direct_play)
    {
        let mut stream = StreamInfo {
            item_id: request.item_id.clone(),
            device_id: request.device_id.clone(),
            media_source_id: source.id.clone(),
            play_session_id: request.play_session_id.clone(),
            access_token: request.access_token.clone(),
            audio_stream_index: audio_index,
            subtitle_stream_index: subtitle.map(|s| s.index),
            subtitle_method: subtitle.map(|s| {
                subtitles::subtitle_method(
                    s,
                    &profile.subtitle_profiles,
                    PlayMethod::Transcode,
                    Some(tp.container.as_str()),
                    tp.protocol,
                )
            }),
            always_burn_in_subtitle: request.always_burn_in_subtitle,
            start_position_ticks: request.start_time_ticks,
            transcode_reasons: reasons,
            ..StreamInfo::default()
        };
        set_stream_info_from_transcoding_profile(&mut stream, tp);
        build_stream_video_item(&mut stream, options, source, video, audio, &candidates, tp);
        let mut video_reasons = video_codec_reasons();
        video_reasons.insert(TranscodeReason::VideoCodecNotSupported);
        video_reasons.insert(TranscodeReason::ContainerBitrateExceedsLimit);
        if stream.transcode_reasons.intersects(video_reasons) {
            apply_transcoding_conditions(&mut stream, &tp.conditions, None, true, true);
        }
        reasons = stream.transcode_reasons;
        transcode = Some(stream);
    }

    let audio_stream_index = transcode
        .as_ref()
        .map_or(audio_index, |stream| stream.audio_stream_index);
    VideoPlan {
        direct_play,
        container,
        transcode_reasons: reasons,
        audio_stream_index,
        subtitle_stream_index: subtitle_index,
        transcode,
    }
}
