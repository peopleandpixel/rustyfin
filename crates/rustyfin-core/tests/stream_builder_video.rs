//! Tests for the stream builder's video decision, a port of Jellyfin's `StreamBuilder.BuildVideoItem`,
//! its transcoding URL, a port of `StreamInfo.ToUrl`, and the device data that
//! `MediaInfoHelper.SetDeviceSpecificData` adds to a media source.

use rustyfin_core::stream_builder::{
    DeviceData, MediaKind, PlayMethod, SourceFacts, StreamFacts, StreamKind, StreamRequest,
    TranscodeReason as R, TranscodeReasons, device_data,
};
use rustyfin_model::device_profile::{
    DeviceProfile, EncodingContext, StreamProtocol, TranscodeSeekInfo,
};
use serde_json::json;

/// The item every request asks about.
const ITEM: &str = "11111111-2222-3333-4444-555555555555";

/// A device profile from `tests/fixtures/device_profiles`.
fn profile(name: &str) -> DeviceProfile {
    let path = format!(
        "{}/tests/fixtures/device_profiles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    DeviceProfile::from_json(&serde_json::from_str(&text).unwrap()).unwrap()
}

/// A request from device `dev1` in play session `ps1` with token `tok`.
fn request() -> StreamRequest {
    StreamRequest {
        item_id: ITEM.to_string(),
        device_id: "dev1".to_string(),
        play_session_id: "ps1".to_string(),
        access_token: "tok".to_string(),
        ..StreamRequest::default()
    }
}

/// A default SDR, progressive, 24 fps video stream at index 0.
fn video(codec: &str, profile: &str, bit_depth: i32, width: i32, height: i32) -> StreamFacts {
    StreamFacts {
        index: 0,
        kind: StreamKind::Video,
        codec: codec.to_string(),
        profile: Some(profile.to_string()),
        width: Some(width),
        height: Some(height),
        bit_depth: Some(bit_depth),
        frame_rate: Some(24.0),
        range_type: Some("SDR".to_string()),
        is_interlaced: Some(false),
        is_default: true,
        ..StreamFacts::default()
    }
}

/// A default English stereo AAC LC stream at 128 kbps and 48 kHz.
fn aac(index: i32) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Audio,
        codec: "aac".to_string(),
        profile: Some("LC".to_string()),
        channels: Some(2),
        sample_rate: Some(48000),
        bit_rate: Some(128_000),
        language: Some("eng".to_string()),
        is_default: true,
        ..StreamFacts::default()
    }
}

/// An audio stream at 48 kHz with no profile that isn't marked default.
fn audio(index: i32, codec: &str, channels: i32, bit_rate: i32) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Audio,
        codec: codec.to_string(),
        channels: Some(channels),
        sample_rate: Some(48000),
        bit_rate: Some(bit_rate),
        ..StreamFacts::default()
    }
}

/// A local source `src1` that allows every playback method.
fn source(container: &str, bitrate: Option<i64>, streams: Vec<StreamFacts>) -> SourceFacts {
    SourceFacts {
        id: "src1".to_string(),
        container: container.to_string(),
        bitrate,
        streams,
        ..SourceFacts::default()
    }
}

/// A 10 Mbps source with 8-bit 1080p H.264 High video and a stereo AAC track at index 1.
fn h264_8bit(container: &str) -> SourceFacts {
    let streams = vec![video("h264", "High", 8, 1920, 1080), aac(1)];
    source(container, Some(10_000_000), streams)
}

/// The same with 10-bit H.264 High 10 video, like B25's Hi10 clip.
fn h264_hi10(container: &str) -> SourceFacts {
    let streams = vec![video("h264", "High 10", 10, 1920, 1080), aac(1)];
    source(container, Some(10_000_000), streams)
}

/// A set of transcode reasons.
fn reasons(list: &[R]) -> TranscodeReasons {
    list.iter()
        .fold(TranscodeReasons::default(), |set, reason| set | *reason)
}

/// The URL of an HLS transcode for `dev1`, source `src1`, session `ps1` and token `tok`. `media`
/// holds the parameters between MediaSourceId and PlaySessionId, `rest` those after `api_key`.
fn hls_url(media: &[&str], rest: &[&str]) -> String {
    let mut params = vec!["DeviceId=dev1", "MediaSourceId=src1"];
    params.extend_from_slice(media);
    params.extend_from_slice(&["PlaySessionId=ps1", "api_key=tok"]);
    params.extend_from_slice(rest);
    format!("/Videos/{ITEM}/main.m3u8?{}", params.join("&"))
}

/// chrome-linux's transcode of [`h264_hi10`]: the video to 8-bit H.264, the audio copied.
fn chrome_hi10_url() -> String {
    hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=119872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=10",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=VideoProfileNotSupported",
        ],
    )
}

/// A URL's path and its parameters in order, so that a mismatch names the parameter.
fn parts(url: &str) -> (String, Vec<String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let params = query
        .split('&')
        .filter(|param| !param.is_empty())
        .map(str::to_string)
        .collect();
    (path.to_string(), params)
}

/// The device data of a direct play, with `audio` as the default audio stream.
fn direct_play(container: &str, audio: Option<i32>) -> DeviceData {
    DeviceData {
        supports_direct_play: true,
        supports_direct_stream: true,
        supports_transcoding: true,
        play_method: Some(PlayMethod::DirectPlay),
        container: container.to_string(),
        transcoding_url: None,
        transcoding_sub_protocol: StreamProtocol::Http,
        transcoding_container: None,
        transcode_reasons: TranscodeReasons::default(),
        default_audio_stream_index: audio,
        default_subtitle_stream_index: None,
        subtitles: Vec::new(),
    }
}

/// The device data of an HLS transcode to MPEG-TS segments.
fn transcode(
    container: &str,
    why: TranscodeReasons,
    audio: Option<i32>,
    url: String,
) -> DeviceData {
    DeviceData {
        supports_direct_play: false,
        supports_direct_stream: false,
        supports_transcoding: true,
        play_method: Some(PlayMethod::Transcode),
        container: container.to_string(),
        transcoding_url: Some(url),
        transcoding_sub_protocol: StreamProtocol::Hls,
        transcoding_container: Some("ts".to_string()),
        transcode_reasons: why,
        default_audio_stream_index: audio,
        default_subtitle_stream_index: None,
        subtitles: Vec::new(),
    }
}

/// The device data when nothing can play: no method and no URL.
fn unplayable(container: &str, why: TranscodeReasons, audio: Option<i32>) -> DeviceData {
    DeviceData {
        supports_direct_play: false,
        supports_direct_stream: false,
        supports_transcoding: false,
        play_method: None,
        container: container.to_string(),
        transcoding_url: None,
        transcoding_sub_protocol: StreamProtocol::Http,
        transcoding_container: None,
        transcode_reasons: why,
        default_audio_stream_index: audio,
        default_subtitle_stream_index: None,
        subtitles: Vec::new(),
    }
}

/// Asserts the device data. The URLs are compared parameter by parameter first.
#[track_caller]
fn assert_data(actual: &DeviceData, expected: &DeviceData) {
    assert_eq!(
        actual.transcoding_url.as_deref().map(parts),
        expected.transcoding_url.as_deref().map(parts),
        "transcoding URL"
    );
    assert_eq!(actual, expected);
}

#[test]
fn request_defaults_allow_every_method() {
    let expected = StreamRequest {
        item_id: String::new(),
        media_kind: MediaKind::Video,
        max_bitrate: None,
        start_time_ticks: 0,
        media_source_id: String::new(),
        audio_stream_index: None,
        subtitle_stream_index: None,
        max_audio_channels: None,
        enable_direct_play: true,
        enable_direct_stream: true,
        enable_transcoding: true,
        allow_video_stream_copy: true,
        allow_audio_stream_copy: true,
        always_burn_in_subtitle: false,
        device_id: String::new(),
        play_session_id: String::new(),
        access_token: String::new(),
    };
    assert_eq!(StreamRequest::default(), expected);
}

#[test]
fn chrome_transcodes_high_10_h264_to_8_bit_h264() {
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &request());
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), chrome_hi10_url()));
}

#[test]
fn chrome_with_high_10_plays_high_10_directly() {
    let data = device_data(
        &h264_hi10("mp4"),
        &profile("chrome-linux-high10"),
        &request(),
    );
    assert_data(&data, &direct_play("mp4", Some(1)));
}

#[test]
fn chrome_plays_8_bit_h264_directly() {
    let data = device_data(&h264_8bit("mp4"), &profile("chrome-linux"), &request());
    assert_data(&data, &direct_play("mp4", Some(1)));
}

#[test]
fn chrome_transcodes_hevc_main_10_to_h264() {
    let streams = vec![video("hevc", "Main 10", 10, 1920, 1080), aac(1)];
    let hevc = source("mkv", Some(10_000_000), streams);
    let data = device_data(&hevc, &profile("chrome-linux"), &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=119872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "hevc-videobitdepth=10",
            "hevc-profile=main10",
            "hevc-audiochannels=2",
            "aac-profile=lc",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=VideoCodecNotSupported",
        ],
    );
    let why = reasons(&[R::VideoCodecNotSupported]);
    assert_data(&data, &transcode("mkv", why, Some(1), url));
}

#[test]
fn chrome_transcodes_av1_with_eac3_and_names_both_codecs() {
    // AV1 plays in mkv, but E-AC-3 doesn't. Only H.264 can be transcoded to, so the video
    // can't be copied either, and the 5.1 audio is mixed down for AAC's stereo bitrate.
    let streams = vec![
        video("av1", "Main", 8, 1920, 1080),
        audio(1, "eac3", 6, 640_000),
    ];
    let av1 = source("mkv", Some(10_000_000), streams);
    let data = device_data(&av1, &profile("chrome-linux"), &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac,mp3",
            "AudioStreamIndex=1",
            "VideoBitrate=119616",
            "AudioBitrate=384",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "av1-videobitdepth=8",
            "av1-profile=main",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=VideoCodecNotSupported,AudioCodecNotSupported",
        ],
    );
    let why = reasons(&[R::VideoCodecNotSupported, R::AudioCodecNotSupported]);
    assert_data(&data, &transcode("mkv", why, Some(1), url));
}

#[test]
fn a_profile_without_play_or_transcoding_profiles_plays_nothing() {
    let empty = DeviceProfile::default();

    // Under the default 8 Mbps limit, the direct play check finds no profile at all.
    let streams = vec![video("h264", "High 10", 10, 1920, 1080), aac(1)];
    let small = source("mp4", Some(5_000_000), streams);
    let data = device_data(&small, &empty, &request());
    let why = reasons(&[R::DirectPlayError]);
    assert_data(&data, &unplayable("mp4", why, Some(1)));

    // Over it, direct play isn't tried.
    let data = device_data(&h264_hi10("mp4"), &empty, &request());
    let why = reasons(&[R::ContainerBitrateExceedsLimit]);
    assert_data(&data, &unplayable("mp4", why, Some(1)));
}

#[test]
fn minimal_profile_plays_1080p_directly() {
    let streams = vec![video("h264", "High", 8, 1920, 1080), aac(1)];
    let hd = source("mp4", Some(5_000_000), streams);
    let data = device_data(&hd, &profile("minimal-mp4"), &request());
    assert_data(&data, &direct_play("mp4", Some(1)));
}

#[test]
fn bitrate_over_the_limit_skips_direct_play_and_audio_copy() {
    let streams = vec![video("h264", "High", 8, 3840, 2160), aac(1)];
    let uhd = source("mp4", Some(20_000_000), streams);
    let data = device_data(&uhd, &profile("minimal-mp4"), &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=7872",
            "AudioBitrate=128",
            "MaxFramerate=24",
            "MaxWidth=1920",
            "MaxHeight=1080",
            "SegmentContainer=ts",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "aac-audiochannels=2",
            "TranscodeReasons=ContainerBitrateExceedsLimit",
        ],
    );
    let why = reasons(&[R::ContainerBitrateExceedsLimit]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn resolution_over_the_limit_scales_down() {
    let streams = vec![video("h264", "High", 8, 3840, 2160), aac(1)];
    let uhd = source("mp4", Some(6_000_000), streams);
    let data = device_data(&uhd, &profile("minimal-mp4"), &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=7872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "MaxWidth=1920",
            "MaxHeight=1080",
            "SegmentContainer=ts",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "aac-audiochannels=2",
            "TranscodeReasons=VideoResolutionNotSupported",
        ],
    );
    let why = reasons(&[R::VideoResolutionNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn unknown_bitrate_counts_as_40_mbps() {
    let streams = vec![video("h264", "High", 8, 1920, 1080), aac(1)];
    let unknown = source("mp4", None, streams);
    let data = device_data(&unknown, &profile("minimal-mp4"), &request());
    assert!(!data.supports_direct_play);
    assert_eq!(data.play_method, Some(PlayMethod::Transcode));
    let why = reasons(&[R::ContainerBitrateExceedsLimit]);
    assert_eq!(data.transcode_reasons, why);
}

#[test]
fn requested_max_bitrate_overrides_the_profile() {
    // Bitrates go into the URL in kbps, rounded down.
    let low = StreamRequest {
        max_bitrate: Some(3_000_999),
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &low);
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac,mp3",
            "AudioStreamIndex=1",
            "VideoBitrate=2872",
            "AudioBitrate=128",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=10",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=ContainerBitrateExceedsLimit",
        ],
    );
    let why = reasons(&[R::ContainerBitrateExceedsLimit]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn tv_plays_hevc_main_10_hdr10_directly() {
    let hdr = StreamFacts {
        range_type: Some("HDR10".to_string()),
        ..video("hevc", "Main 10", 10, 3840, 2160)
    };
    let uhd = source("mkv", Some(15_000_000), vec![hdr, aac(1)]);
    let data = device_data(&uhd, &profile("tv-hevc10-mkv"), &request());
    assert_data(&data, &direct_play("mkv", Some(1)));
}

#[test]
fn tv_transcodes_high_10_h264_and_offers_hevc() {
    let data = device_data(&h264_hi10("mkv"), &profile("tv-hevc10-mkv"), &request());
    let url = hls_url(
        &[
            "VideoCodec=hevc,h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=39872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=2",
        ],
        &[
            "TranscodingMaxAudioChannels=6",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "hevc-profile=main,main10",
            "hevc-videobitdepth=10",
            "hevc-rangetype=SDR,HDR10,HLG",
            "TranscodeReasons=VideoProfileNotSupported,VideoBitDepthNotSupported",
        ],
    );
    let why = reasons(&[R::VideoProfileNotSupported, R::VideoBitDepthNotSupported]);
    assert_data(&data, &transcode("mkv", why, Some(1), url));
}

/// An 8-bit H.264 mkv with a DTS track at index 1 and a stereo AAC track at index 2.
fn dts_and_aac(dts_is_default: bool) -> SourceFacts {
    let dts = StreamFacts {
        is_default: dts_is_default,
        ..audio(1, "dts", 6, 1_500_000)
    };
    let stereo = StreamFacts {
        is_default: false,
        ..aac(2)
    };
    let streams = vec![video("h264", "High", 8, 1920, 1080), dts, stereo];
    source("mkv", Some(10_000_000), streams)
}

#[test]
fn a_default_audio_track_limits_the_candidates_to_default_tracks() {
    let data = device_data(&dts_and_aac(true), &profile("tv-hevc10-mkv"), &request());
    let url = hls_url(
        &[
            "VideoCodec=hevc,h264",
            "AudioCodec=aac,ac3",
            "AudioStreamIndex=1",
            "VideoBitrate=39360",
            "AudioBitrate=640",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=2",
        ],
        &[
            "TranscodingMaxAudioChannels=6",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "hevc-profile=main,main10",
            "hevc-videobitdepth=10",
            "hevc-rangetype=SDR,HDR10,HLG",
            "ac3-audiochannels=6",
            "TranscodeReasons=AudioCodecNotSupported",
        ],
    );
    let why = reasons(&[R::AudioCodecNotSupported]);
    assert_data(&data, &transcode("mkv", why, Some(1), url));
}

#[test]
fn without_default_tracks_any_audio_track_can_play() {
    let data = device_data(&dts_and_aac(false), &profile("tv-hevc10-mkv"), &request());
    assert_data(&data, &direct_play("mkv", Some(2)));
}

#[test]
fn chrome_transcodes_when_the_default_audio_is_not_the_first_track() {
    // Chrome's profile only plays the first audio track directly. The copy check in the
    // transcode doesn't ask, so the second track is copied.
    let first = StreamFacts {
        is_default: false,
        ..aac(1)
    };
    let streams = vec![video("h264", "High", 8, 1920, 1080), first, aac(2)];
    let two = source("mp4", Some(10_000_000), streams);
    let data = device_data(&two, &profile("chrome-linux"), &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=2",
            "VideoBitrate=119872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=SecondaryAudioNotSupported",
        ],
    );
    let why = reasons(&[R::SecondaryAudioNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(2), url));
}

#[test]
fn the_audio_index_applies_only_to_the_selected_source() {
    let streams = vec![
        video("h264", "High", 8, 1920, 1080),
        aac(1),
        audio(2, "ac3", 6, 640_000),
    ];
    let two = source("mkv", Some(10_000_000), streams);
    let tv = profile("tv-hevc10-mkv");
    let pick = |media_source_id: &str| StreamRequest {
        media_source_id: media_source_id.to_string(),
        audio_stream_index: Some(2),
        ..request()
    };

    // The id matches ignoring case.
    let data = device_data(&two, &tv, &pick("SRC1"));
    assert_data(&data, &direct_play("mkv", Some(2)));

    let data = device_data(&two, &tv, &pick("other"));
    assert_data(&data, &direct_play("mkv", Some(1)));

    let data = device_data(&two, &tv, &pick(""));
    assert_data(&data, &direct_play("mkv", Some(1)));
}

#[test]
fn disabled_direct_play_transcodes_with_direct_play_error() {
    let no_direct_play = StreamRequest {
        enable_direct_play: false,
        ..request()
    };
    let data = device_data(&h264_8bit("mp4"), &profile("chrome-linux"), &no_direct_play);
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=119872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=DirectPlayError",
        ],
    );
    let why = reasons(&[R::DirectPlayError]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn disabled_stream_copy_and_burn_in_are_added_to_the_url() {
    let no_copy = StreamRequest {
        allow_video_stream_copy: false,
        allow_audio_stream_copy: false,
        always_burn_in_subtitle: true,
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &no_copy);
    let url = format!(
        "{}&allowVideoStreamCopy=false&allowAudioStreamCopy=false&alwaysBurnInSubtitleWhenTranscoding=true",
        chrome_hi10_url()
    );
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn without_transcoding_or_direct_stream_there_is_no_url() {
    // No transcoding profile is chosen, so there is no URL. Jellyfin would still report
    // transcoding as supported and build a URL that doesn't play.
    let neither = StreamRequest {
        enable_transcoding: false,
        enable_direct_stream: false,
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &neither);
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &unplayable("mp4", why, Some(1)));

    // As in Jellyfin, direct stream alone still allows the HLS transcode.
    let no_transcoding = StreamRequest {
        enable_transcoding: false,
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &no_transcoding);
    assert_data(&data, &transcode("mp4", why, Some(1), chrome_hi10_url()));
}

#[test]
fn only_hls_streaming_transcoding_profiles_count() {
    let why = reasons(&[R::VideoProfileNotSupported]);

    let mut http = profile("chrome-linux");
    for tp in &mut http.transcoding_profiles {
        tp.protocol = StreamProtocol::Http;
    }
    let data = device_data(&h264_hi10("mp4"), &http, &request());
    assert_data(&data, &unplayable("mp4", why, Some(1)));

    let mut static_only = profile("chrome-linux");
    for tp in &mut static_only.transcoding_profiles {
        tp.context = EncodingContext::Static;
    }
    let data = device_data(&h264_hi10("mp4"), &static_only, &request());
    assert_data(&data, &unplayable("mp4", why, Some(1)));
}

#[test]
fn start_time_follows_min_segments() {
    let later = StreamRequest {
        start_time_ticks: 600_000_000,
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &later);
    let url =
        chrome_hi10_url().replace("MinSegments=1&", "MinSegments=1&StartTimeTicks=600000000&");
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn max_audio_channels_lowers_the_audio_bitrate() {
    // The 5.1 AAC track is copied either way. With a stereo limit, the bitrate is AAC's
    // stereo default.
    let surround = StreamFacts {
        channels: Some(6),
        bit_rate: Some(640_000),
        ..aac(1)
    };
    let streams = vec![video("h264", "High 10", 10, 1920, 1080), surround];
    let hi10 = source("mkv", Some(10_000_000), streams);
    let tv = profile("tv-hevc10-mkv");
    let url = |video_kbps: &str, audio_kbps: &str| {
        hls_url(
            &[
                "VideoCodec=hevc,h264",
                "AudioCodec=aac",
                "AudioStreamIndex=1",
                video_kbps,
                audio_kbps,
                "AudioSampleRate=48000",
                "MaxFramerate=24",
                "SegmentContainer=ts",
                "MinSegments=2",
            ],
            &[
                "TranscodingMaxAudioChannels=6",
                "RequireAvc=false",
                "EnableAudioVbrEncoding=true",
                "h264-videobitdepth=8",
                "h264-profile=high,main,baseline,constrainedbaseline",
                "h264-audiochannels=6",
                "aac-profile=lc",
                "hevc-profile=main,main10",
                "hevc-videobitdepth=10",
                "hevc-rangetype=SDR,HDR10,HLG",
                "TranscodeReasons=VideoProfileNotSupported,VideoBitDepthNotSupported",
            ],
        )
    };
    let why = reasons(&[R::VideoProfileNotSupported, R::VideoBitDepthNotSupported]);

    let data = device_data(&hi10, &tv, &request());
    let expected = url("VideoBitrate=39360", "AudioBitrate=640");
    assert_data(&data, &transcode("mkv", why, Some(1), expected));

    let stereo = StreamRequest {
        max_audio_channels: Some(2),
        ..request()
    };
    let data = device_data(&hi10, &tv, &stereo);
    let expected = url("VideoBitrate=39616", "AudioBitrate=384");
    assert_data(&data, &transcode("mkv", why, Some(1), expected));
}

#[test]
fn a_container_list_is_narrowed_to_one_container() {
    let chrome = profile("chrome-linux");
    let list = "mov,mp4,m4a,3gp,3g2,mj2";

    // A direct play takes the first entry the chosen profile supports.
    let data = device_data(&h264_8bit(list), &chrome, &request());
    assert_data(&data, &direct_play("mp4", Some(1)));

    // A transcode takes the first entry any video profile supports.
    let data = device_data(&h264_hi10(list), &chrome, &request());
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &transcode("mov", why, Some(1), chrome_hi10_url()));

    // A list that no profile supports stays as it is.
    let data = device_data(&h264_8bit("avi,divx"), &chrome, &request());
    assert_eq!(data.container, "avi,divx");
    assert_eq!(data.play_method, Some(PlayMethod::Transcode));
    assert_eq!(data.transcode_reasons, reasons(&[R::ContainerNotSupported]));
}

#[test]
fn extreme_bitrates_do_not_overflow() {
    let chrome = profile("chrome-linux");

    let huge_audio = StreamFacts {
        bit_rate: Some(i32::MAX),
        ..aac(1)
    };
    let streams = vec![video("h264", "High 10", 10, 1920, 1080), huge_audio];
    let huge = source("mp4", Some(i64::MAX), streams);
    let max = StreamRequest {
        max_bitrate: Some(i32::MAX),
        ..request()
    };
    let data = device_data(&huge, &chrome, &max);
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac,mp3",
            "AudioStreamIndex=1",
            "VideoBitrate=2140315",
            "AudioBitrate=7168",
            "MaxFramerate=24",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=10",
            "h264-profile=high,main,baseline,constrainedbaseline",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            "TranscodeReasons=ContainerBitrateExceedsLimit",
        ],
    );
    let why = reasons(&[R::ContainerBitrateExceedsLimit]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));

    // A limit of zero or less means no limit, and the video gets the 64 kbps floor.
    let why = reasons(&[R::VideoProfileNotSupported]);
    let floor = chrome_hi10_url().replace("VideoBitrate=119872", "VideoBitrate=64");
    for limit in [-5, i32::MIN] {
        let negative = StreamRequest {
            max_bitrate: Some(limit),
            ..request()
        };
        let data = device_data(&h264_hi10("mp4"), &chrome, &negative);
        assert_data(&data, &transcode("mp4", why, Some(1), floor.clone()));
    }

    // With no limit anywhere, the URL has no video bitrate.
    let unlimited = DeviceProfile {
        max_streaming_bitrate: None,
        ..profile("chrome-linux")
    };
    let data = device_data(&h264_hi10("mp4"), &unlimited, &request());
    let url = chrome_hi10_url().replace("VideoBitrate=119872&", "");
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn transcoding_profile_options_reach_the_url() {
    let mut minimal = profile("minimal-mp4");
    let tp = &mut minimal.transcoding_profiles[0];
    tp.estimate_content_length = true;
    tp.enable_mpegts_m2ts_mode = true;
    tp.copy_timestamps = true;
    tp.enable_subtitles_in_manifest = true;
    tp.transcode_seek_info = TranscodeSeekInfo::Bytes;
    tp.segment_length = 6;
    tp.min_segments = 2;
    tp.enable_audio_vbr_encoding = false;

    let streams = vec![video("h264", "High", 8, 3840, 2160), aac(1)];
    let uhd = source("mp4", Some(6_000_000), streams);
    let data = device_data(&uhd, &minimal, &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=7872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "MaxWidth=1920",
            "MaxHeight=1080",
            "SegmentContainer=ts",
            "SegmentLength=6",
            "MinSegments=2",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "EnableSubtitlesInManifest=True",
            "EnableMpegtsM2TsMode=True",
            "EstimateContentLength=True",
            "TranscodeSeekInfo=Bytes",
            "CopyTimestamps=True",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=false",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "aac-audiochannels=2",
            "TranscodeReasons=VideoResolutionNotSupported",
        ],
    );
    let why = reasons(&[R::VideoResolutionNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn transcoding_profiles_are_ranked_by_what_can_be_copied() {
    let two = DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 20_000_000,
        "DirectPlayProfiles": [
            {"Container": "mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"},
            {"Container": "mp4", "Type": "Video", "VideoCodec": "hevc,h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"}
        ]
    }))
    .unwrap();
    let streams = vec![video("hevc", "Main 10", 10, 1920, 1080), aac(1)];
    let hevc = source("mkv", Some(10_000_000), streams);
    let why = reasons(&[R::VideoCodecNotSupported]);

    // The second profile can copy the HEVC video, so it wins.
    let data = device_data(&hevc, &two, &request());
    let url = hls_url(
        &[
            "VideoCodec=hevc,h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=19872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=mp4",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "hevc-videobitdepth=10",
            "hevc-profile=main10",
            "hevc-audiochannels=2",
            "aac-profile=lc",
            "TranscodeReasons=VideoCodecNotSupported",
        ],
    );
    let expected = DeviceData {
        transcoding_container: Some("mp4".to_string()),
        ..transcode("mkv", why, Some(1), url)
    };
    assert_data(&data, &expected);

    // Without video copy the two tie, and the first one wins.
    let no_copy = StreamRequest {
        allow_video_stream_copy: false,
        ..request()
    };
    let data = device_data(&hevc, &two, &no_copy);
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=19872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "hevc-videobitdepth=10",
            "hevc-profile=main10",
            "hevc-audiochannels=2",
            "aac-profile=lc",
            "TranscodeReasons=VideoCodecNotSupported",
            "allowVideoStreamCopy=false",
        ],
    );
    assert_data(&data, &transcode("mkv", why, Some(1), url));
}

#[test]
fn container_profile_conditions_block_direct_play() {
    let capped = DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 20_000_000,
        "DirectPlayProfiles": [
            {"Container": "mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "ContainerProfiles": [
            {"Type": "Video", "Container": "mkv", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "Width", "Value": "1280",
                 "IsRequired": false}
            ]}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"}
        ]
    }))
    .unwrap();

    let data = device_data(&h264_8bit("mkv"), &capped, &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=19872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "SegmentContainer=ts",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "TranscodeReasons=VideoResolutionNotSupported",
        ],
    );
    let why = reasons(&[R::VideoResolutionNotSupported]);
    assert_data(&data, &transcode("mkv", why, Some(1), url));

    let streams = vec![video("h264", "High", 8, 1280, 720), aac(1)];
    let hd = source("mkv", Some(10_000_000), streams);
    let data = device_data(&hd, &capped, &request());
    assert_data(&data, &direct_play("mkv", Some(1)));
}

#[test]
fn empty_ids_are_left_out_of_the_url() {
    let anonymous = StreamRequest {
        device_id: String::new(),
        play_session_id: String::new(),
        access_token: String::new(),
        ..request()
    };
    let data = device_data(&h264_hi10("mp4"), &profile("chrome-linux"), &anonymous);
    let (path, mut params) = parts(&chrome_hi10_url());
    params.retain(|param| {
        !matches!(
            param.as_str(),
            "DeviceId=dev1" | "PlaySessionId=ps1" | "api_key=tok"
        )
    });
    let url = data.transcoding_url.as_deref().expect("a transcoding URL");
    assert_eq!(parts(url), (path, params));
}

#[test]
fn a_fractional_frame_rate_keeps_its_fraction() {
    let ntsc = StreamFacts {
        frame_rate: Some(23.976),
        ..video("h264", "High 10", 10, 1920, 1080)
    };
    let film = source("mp4", Some(10_000_000), vec![ntsc, aac(1)]);
    let data = device_data(&film, &profile("chrome-linux"), &request());
    let url = chrome_hi10_url().replace("MaxFramerate=24", "MaxFramerate=23.976");
    let why = reasons(&[R::VideoProfileNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn downmixed_audio_meets_the_apply_conditions_of_its_new_channels() {
    // The 5.1 track is wider than the profile's stereo transcode, so Jellyfin sets the target
    // audio stream's channels to 2 (SB 1028) before it reads them for the audio codec profiles
    // (SB 1104). The second aac profile then applies, and caps the audio bitrate.
    let stereo = DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 8_000_000,
        "DirectPlayProfiles": [
            {"Container": "mp4", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming", "MaxAudioChannels": "2"}
        ],
        "CodecProfiles": [
            {"Type": "VideoAudio", "Codec": "aac", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "2",
                 "IsRequired": false}
            ]},
            {"Type": "VideoAudio", "Codec": "aac",
             "ApplyConditions": [
                {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "2",
                 "IsRequired": false}
             ],
             "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "256000",
                 "IsRequired": false}
             ]}
        ]
    }))
    .unwrap();
    let surround = StreamFacts {
        channels: Some(6),
        bit_rate: Some(640_000),
        ..aac(1)
    };
    let streams = vec![video("h264", "High", 8, 1920, 1080), surround];
    let movie = source("mp4", Some(6_000_000), streams);

    let data = device_data(&movie, &stereo, &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=7744",
            "AudioBitrate=256",
            "MaxFramerate=24",
            "SegmentContainer=ts",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "aac-audiochannels=2",
            "TranscodeReasons=AudioChannelsNotSupported",
        ],
    );
    let why = reasons(&[R::AudioChannelsNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn a_range_type_not_equals_condition_lists_the_other_types_in_jellyfin_order() {
    // Jellyfin writes the VideoRangeType names that the value doesn't list, in the enum's
    // declaration order (Jellyfin.Data/Enums/VideoRangeType.cs). The entries are trimmed, and the
    // exclusion is case-sensitive, so "hdr10plus" leaves HDR10Plus in. The second h264 profile is
    // for ts only, so it applies to the transcode and not to the mp4 direct play check.
    let no_dovi = DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 8_000_000,
        "DirectPlayProfiles": [
            {"Container": "mp4", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"}
        ],
        "CodecProfiles": [
            {"Type": "Video", "Codec": "h264", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "Width", "Value": "1920",
                 "IsRequired": false}
            ]},
            {"Type": "Video", "Codec": "h264", "Container": "ts", "Conditions": [
                {"Condition": "NotEquals", "Property": "VideoRangeType",
                 "Value": "DOVI | DOVIWithEL|hdr10plus", "IsRequired": false}
            ]}
        ]
    }))
    .unwrap();
    let streams = vec![video("h264", "High", 8, 3840, 2160), aac(1)];
    let uhd = source("mp4", Some(6_000_000), streams);

    let data = device_data(&uhd, &no_dovi, &request());
    let url = hls_url(
        &[
            "VideoCodec=h264",
            "AudioCodec=aac",
            "AudioStreamIndex=1",
            "VideoBitrate=7872",
            "AudioBitrate=128",
            "AudioSampleRate=48000",
            "MaxFramerate=24",
            "MaxWidth=1920",
            "SegmentContainer=ts",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "h264-rangetype=Unknown,SDR,HDR10,HLG,DOVIWithHDR10,DOVIWithHLG,DOVIWithSDR,\
             DOVIWithHDR10Plus,DOVIWithELHDR10Plus,DOVIInvalid,HDR10Plus",
            "TranscodeReasons=VideoResolutionNotSupported",
        ],
    );
    let why = reasons(&[R::VideoResolutionNotSupported]);
    assert_data(&data, &transcode("mp4", why, Some(1), url));
}

#[test]
fn odd_sources_and_requests_do_not_panic() {
    let profiles = [
        profile("chrome-linux"),
        profile("chrome-linux-high10"),
        profile("tv-hevc10-mkv"),
        profile("minimal-mp4"),
        DeviceProfile::default(),
    ];
    let odd_video = StreamFacts {
        index: -1,
        kind: StreamKind::Video,
        codec: "H264".to_string(),
        profile: Some(String::new()),
        width: Some(-1),
        height: Some(i32::MAX),
        bit_depth: Some(i32::MIN),
        bit_rate: Some(-1),
        frame_rate: Some(f32::NAN),
        range_type: Some(String::new()),
        ..StreamFacts::default()
    };
    let odd_audio = StreamFacts {
        index: i32::MAX,
        kind: StreamKind::Audio,
        channels: Some(i32::MAX),
        sample_rate: Some(-1),
        bit_rate: Some(i32::MIN),
        is_default: true,
        ..StreamFacts::default()
    };
    let mono = StreamFacts {
        channels: Some(1),
        bit_rate: Some(0),
        ..aac(2)
    };
    let negative = StreamFacts {
        channels: Some(-1),
        frame_rate: Some(f32::INFINITY),
        ..audio(3, "ac3", 6, 640_000)
    };
    let sources = [
        source("mp4", Some(1_000_000), Vec::new()),
        source("mkv", Some(-1), vec![aac(1)]),
        source(
            "",
            Some(0),
            vec![StreamFacts {
                kind: StreamKind::Video,
                ..StreamFacts::default()
            }],
        ),
        source(",", Some(i64::MIN), vec![odd_video.clone(), odd_audio]),
        source("mp4,", None, vec![odd_video, mono, negative]),
        h264_hi10("mkv"),
    ];
    let requests = [
        request(),
        StreamRequest {
            max_bitrate: Some(0),
            max_audio_channels: Some(-1),
            ..request()
        },
        StreamRequest {
            max_bitrate: Some(i32::MIN),
            max_audio_channels: Some(i32::MAX),
            start_time_ticks: i64::MIN,
            ..request()
        },
        StreamRequest {
            max_bitrate: Some(i32::MAX),
            max_audio_channels: Some(0),
            start_time_ticks: i64::MAX,
            ..request()
        },
        StreamRequest {
            media_source_id: "src1".to_string(),
            audio_stream_index: Some(i32::MIN),
            subtitle_stream_index: Some(-1),
            ..request()
        },
        StreamRequest {
            media_source_id: "SRC1".to_string(),
            audio_stream_index: Some(3),
            subtitle_stream_index: Some(i32::MAX),
            ..request()
        },
        StreamRequest {
            enable_direct_play: false,
            enable_direct_stream: false,
            enable_transcoding: false,
            allow_video_stream_copy: false,
            allow_audio_stream_copy: false,
            always_burn_in_subtitle: true,
            ..request()
        },
    ];
    for profile in &profiles {
        for source in &sources {
            for request in &requests {
                let data = device_data(source, profile, request);
                let transcodes = data.play_method == Some(PlayMethod::Transcode);
                assert_eq!(data.transcoding_url.is_some(), transcodes, "{data:?}");
                if let Some(url) = &data.transcoding_url {
                    assert!(
                        url.starts_with(&format!("/Videos/{ITEM}/main.m3u8?")),
                        "{url}"
                    );
                }
                let plays = data.play_method == Some(PlayMethod::DirectPlay);
                assert_eq!(data.supports_direct_play, plays, "{data:?}");
            }
        }
    }
}
