//! The audio decision for one source: the play method of an audio item and the HLS transcode that
//! plays it. A port of Jellyfin's `StreamBuilder.GetOptimalAudioStream` and of the direct play
//! check of `GetAudioDirectPlayProfile`, with the `StreamInfo` fields that `ToUrl` reads for audio.
//! The DirectStream branch is not ported: Rustyfin can't remux audio, so such a file is transcoded.

use rustyfin_model::device_profile::{
    CodecProfile, CodecType, DeviceProfile, DirectPlayProfile, EncodingContext, ProfileCondition,
    ProfileType, StreamProtocol, TranscodingProfile,
};

use super::conditions::{self, AudioFacts};
use super::video::{self, StreamInfo, VideoOptions, VideoPlan};
use super::{SourceFacts, StreamFacts, TranscodeReason, TranscodeReasons};

/// Jellyfin's `IsAudioContainerSupported`: the profile's container list takes the source's
/// container. A Matroska source needs a profile that names `mkv`, so a WebM profile doesn't cover
/// it (jellyfin#13344).
fn container_supported(profile: &DirectPlayProfile, container: &str) -> bool {
    conditions::contains_container(&profile.container, container)
        && (!conditions::contains_container("mkv", container)
            || conditions::contains_container(&profile.container, "mkv"))
}

/// Whether `list` equals `codec` as a whole string, ignoring ASCII case. An empty codec equals
/// nothing.
fn equals_codec(list: &str, codec: &str) -> bool {
    !codec.is_empty() && list.eq_ignore_ascii_case(codec)
}

/// Jellyfin's `IsAudioDirectPlaySupported`: the profile takes the container and plays the codec.
fn direct_play_supported(profile: &DirectPlayProfile, container: &str, codec: &str) -> bool {
    container_supported(profile, container) && video::direct_play_audio_codec(profile, codec)
}

/// Jellyfin's `IsAudioDirectStreamSupported`: the profile's container is not the source's, and
/// its audio codec or container is the codec as a whole. Jellyfin would remux such a file.
fn direct_stream_supported(profile: &DirectPlayProfile, container: &str, codec: &str) -> bool {
    !container_supported(profile, container)
        && (equals_codec(&profile.audio_codec, codec) || equals_codec(&profile.container, codec))
}

/// The audio direct play profiles of the device, in profile order.
fn audio_profiles(profile: &DeviceProfile) -> Vec<&DirectPlayProfile> {
    profile
        .direct_play_profiles
        .iter()
        .filter(|p| p.r#type == ProfileType::Audio)
        .collect()
}

/// Jellyfin's `GetTranscodeReasonsFromDirectPlayProfile` for an audio item (SB 503-550). The
/// container is checked with the plain list match, without the Matroska rule. No profile takes the
/// container: ContainerNotSupported, VideoCodecNotSupported and AudioCodecNotSupported, since
/// Jellyfin's flags start unset. A profile takes the container and plays the codec: no reason. A
/// profile takes the container but none plays the codec: AudioCodecNotSupported.
fn reasons_without_direct_stream(
    profiles: &[&DirectPlayProfile],
    container: &str,
    codec: &str,
) -> TranscodeReasons {
    let taken = profiles
        .iter()
        .filter(|p| conditions::contains_container(&p.container, container))
        .collect::<Vec<_>>();
    if taken.is_empty() {
        return [
            TranscodeReason::ContainerNotSupported,
            TranscodeReason::VideoCodecNotSupported,
            TranscodeReason::AudioCodecNotSupported,
        ]
        .into_iter()
        .fold(TranscodeReasons::default(), |set, reason| set | reason);
    }
    if taken
        .iter()
        .any(|p| video::direct_play_audio_codec(p, codec))
    {
        return TranscodeReasons::default();
    }
    TranscodeReasons::from(TranscodeReason::AudioCodecNotSupported)
}

/// The Audio codec profiles that match `codec` and `container`, and whose apply conditions hold for
/// the stream `facts` describe (SB 1731-1754, `GetProfileConditionsForAudio`).
fn audio_codec_profiles<'a>(
    profile: &'a DeviceProfile,
    codec: &str,
    container: &str,
    facts: &AudioFacts,
) -> Vec<&'a CodecProfile> {
    profile
        .codec_profiles
        .iter()
        .filter(|cp| {
            cp.r#type == CodecType::Audio
                && video::codec_profile_matches(cp, codec, container, false)
                && cp
                    .apply_conditions
                    .iter()
                    .all(|c| conditions::audio_condition_satisfied(c, facts))
        })
        .collect()
}

/// Jellyfin's `GetCompatibilityAudioCodec` for a direct play of `stream` (SB 113-125): the reasons
/// of the failed conditions of the matching Audio codec profiles.
fn direct_play_failures(
    profile: &DeviceProfile,
    container: &str,
    stream: &StreamFacts,
) -> TranscodeReasons {
    let facts = video::audio_facts(stream, false);
    audio_codec_profiles(profile, &stream.codec, container, &facts)
        .into_iter()
        .flat_map(|cp| cp.conditions.iter())
        .filter(|c| !conditions::audio_condition_satisfied(c, &facts))
        .fold(TranscodeReasons::default(), |set, c| {
            set | conditions::reason_for_failed_condition(c)
        })
}

/// The conditions that a transcode to `tp` applies to `stream` (SB 1731-1754): every condition, in
/// profile order, of the Audio codec profiles that match the transcode's codec and container. They
/// are not filtered by failure.
fn transcode_conditions(
    profile: &DeviceProfile,
    tp: &TranscodingProfile,
    stream: &StreamFacts,
) -> Vec<ProfileCondition> {
    let facts = video::audio_facts(stream, false);
    audio_codec_profiles(profile, &tp.audio_codec, &tp.container, &facts)
        .into_iter()
        .flat_map(|cp| cp.conditions.iter().cloned())
        .collect()
}

/// The audio bitrate of a transcode in bits per second (SB 199-212): the profile's music bitrate,
/// else the limit, else 128 kbps. It is never above the limit, nor above the bitrate that the
/// conditions set. The sum is in i64 and clamped to i32.
fn transcode_bitrate(profile: &DeviceProfile, limit: Option<i32>, condition: Option<i32>) -> i32 {
    let limit = limit.map(i64::from);
    let mut bitrate = profile
        .music_streaming_transcoding_bitrate
        .map(i64::from)
        .or(limit)
        .unwrap_or(128_000);
    if let Some(limit) = limit {
        bitrate = bitrate.min(limit);
    }
    bitrate = bitrate.min(condition.map_or(bitrate, i64::from));
    video::saturating_i32(bitrate)
}

/// Jellyfin's `GetOptimalAudioStream` for one source (SB 78-231), as the plan that `device_data`
/// writes out for an audio item. The stream is the default audio stream, and the request's and the
/// source's audio indexes are not read. Rustyfin returns `None` (Jellyfin throws, or returns null)
/// when there is no audio stream, or when the file has to transcode and the source may not.
pub(super) fn plan_audio(source: &SourceFacts, options: &VideoOptions<'_>) -> Option<VideoPlan> {
    let profile = options.profile;
    let request = options.request;
    let stream = video::default_audio_stream(source, None)?;
    let codec = stream.codec.as_str();
    let container = source.container.as_str();
    let profiles = audio_profiles(profile);

    // The direct play profile (SB 437-550). A file that plays no profile is reported with the
    // reasons of its container and codec.
    let plays = profiles
        .iter()
        .any(|p| direct_play_supported(p, container, codec));
    let remuxes = !plays
        && profiles
            .iter()
            .any(|p| direct_stream_supported(p, container, codec));
    let mut reasons = TranscodeReasons::default();
    if remuxes {
        reasons.insert(TranscodeReason::ContainerNotSupported);
    } else if !plays {
        reasons = reasons_without_direct_stream(&profiles, container, codec);
    }

    // The bitrate limit (SB 437-465). Jellyfin checks it only once a profile matched, so a file
    // with neither profile gets no bitrate reason. A file over the limit isn't played directly,
    // and a remuxable one takes the reason too. The direct stream itself is never chosen.
    let matched = plays || remuxes;
    let exceeded =
        video::bitrate_limit_exceeded(source, i64::from(options.max_bitrate.unwrap_or(0)));
    let mut direct_play = false;
    if plays && source.supports_direct_play {
        if exceeded {
            reasons.insert(TranscodeReason::ContainerBitrateExceedsLimit);
        } else {
            let failures = direct_play_failures(profile, container, stream);
            if failures.is_empty() {
                direct_play = true;
            } else {
                reasons |= failures;
            }
        }
    }
    if matched && source.supports_direct_stream && exceeded {
        reasons.insert(TranscodeReason::ContainerBitrateExceedsLimit);
    }
    if direct_play {
        return Some(VideoPlan {
            direct_play: true,
            container: source.container.clone(),
            transcode_reasons: reasons,
            audio_stream_index: None,
            subtitle_stream_index: None,
            transcode: None,
        });
    }

    // The transcode (SB 166-224): the first HLS streaming profile of Type Audio.
    let Some(tp) = profile.transcoding_profiles.iter().find(|tp| {
        tp.r#type == ProfileType::Audio
            && tp.context == EncodingContext::Streaming
            && tp.protocol == StreamProtocol::Hls
    }) else {
        return Some(VideoPlan {
            direct_play: false,
            container: source.container.clone(),
            transcode_reasons: reasons,
            audio_stream_index: None,
            subtitle_stream_index: None,
            transcode: None,
        });
    };
    if !source.supports_transcoding {
        return None;
    }

    let mut info = StreamInfo {
        item_id: request.item_id.clone(),
        device_id: request.device_id.clone(),
        media_source_id: source.id.clone(),
        play_session_id: request.play_session_id.clone(),
        access_token: request.access_token.clone(),
        start_position_ticks: request.start_time_ticks,
        transcode_reasons: reasons,
        audio_only: true,
        ..StreamInfo::default()
    };
    video::set_stream_info_from_transcoding_profile(&mut info, tp);
    let profile_conditions = transcode_conditions(profile, tp, stream);
    video::apply_transcoding_conditions(&mut info, &profile_conditions, None, true, true);
    info.audio_bitrate = Some(transcode_bitrate(
        profile,
        options.max_bitrate,
        info.audio_bitrate,
    ));
    if info.audio_codecs.is_empty() && !tp.audio_codec.trim().is_empty() {
        info.audio_codecs = vec![tp.audio_codec.clone()];
    }

    Some(VideoPlan {
        direct_play: false,
        container: source.container.clone(),
        transcode_reasons: reasons,
        audio_stream_index: None,
        subtitle_stream_index: None,
        transcode: Some(info),
    })
}
