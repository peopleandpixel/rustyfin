//! Tests for the stream builder's subtitle delivery, a port of Jellyfin's
//! `StreamBuilder.GetSubtitleProfile`, its use in `BuildVideoItem` and `ToUrl`, and the
//! per-stream delivery that `MediaInfoHelper.SetDeviceSpecificSubtitleInfo` adds to a source.

use rustyfin_core::stream_builder::{
    DeviceData, PlayMethod, SourceFacts, StreamFacts, StreamKind, StreamRequest, SubtitleDelivery,
    TranscodeReason as R, TranscodeReasons, device_data,
};
use rustyfin_model::device_profile::{DeviceProfile, StreamProtocol, SubtitleDeliveryMethod as M};
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

/// A profile that plays H.264 with AAC from Matroska and transcodes to HLS, with the given
/// subtitle profiles.
fn mkv_player(subtitle_profiles: serde_json::Value) -> DeviceProfile {
    DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 40_000_000,
        "DirectPlayProfiles": [
            {"Container": "mkv", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"}
        ],
        "SubtitleProfiles": subtitle_profiles
    }))
    .unwrap()
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

/// A request that chooses subtitle stream `index` of source `src1`.
fn choosing(index: i32) -> StreamRequest {
    StreamRequest {
        media_source_id: "src1".to_string(),
        subtitle_stream_index: Some(index),
        ..request()
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

/// An English subtitle stream in the media file, neither default nor forced.
fn embedded(index: i32, codec: &str) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Subtitle,
        codec: codec.to_string(),
        language: Some("eng".to_string()),
        ..StreamFacts::default()
    }
}

/// An English subtitle file next to the media file.
fn external(index: i32, codec: &str) -> StreamFacts {
    StreamFacts {
        is_external: true,
        path: Some(format!("/media/movie.en.{codec}")),
        ..embedded(index, codec)
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

/// `source` with `subtitles` after its streams and `default` as its default subtitle stream.
fn plus(mut source: SourceFacts, subtitles: Vec<StreamFacts>, default: Option<i32>) -> SourceFacts {
    source.streams.extend(subtitles);
    source.default_subtitle_stream_index = default;
    source
}

/// A set of transcode reasons.
fn reasons(list: &[R]) -> TranscodeReasons {
    list.iter()
        .fold(TranscodeReasons::default(), |set, reason| set | *reason)
}

/// Subtitle stream `index` of source `src1` delivered as a WebVTT file that starts at `ticks`.
fn vtt(index: i32, ticks: i64) -> SubtitleDelivery {
    SubtitleDelivery {
        index,
        method: M::External,
        url: Some(format!(
            "/Videos/{ITEM}/src1/Subtitles/{index}/{ticks}/Stream.vtt?api_key=tok"
        )),
    }
}

/// Subtitle stream `index` delivered by `method`, without a URL of its own.
fn by(index: i32, method: M) -> SubtitleDelivery {
    SubtitleDelivery {
        index,
        method,
        url: None,
    }
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

/// chrome-linux's transcode of [`h264_8bit`] for `why`: the video to 8-bit H.264 High, the audio
/// copied.
fn chrome_8bit_url(why: &str) -> String {
    let why = format!("TranscodeReasons={why}");
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
            "h264-videobitdepth=8",
            "h264-profile=high",
            "h264-audiochannels=2",
            "aac-profile=lc",
            "h264-rangetype=SDR",
            "h264-level=52",
            "h264-deinterlace=true",
            why.as_str(),
        ],
    )
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

/// `url` with `param` inserted right after the parameter `after`.
fn insert_after(url: &str, after: &str, param: &str) -> String {
    let (path, params) = parts(url);
    assert!(params.iter().any(|p| p == after), "no parameter {after}");
    let mut out = Vec::new();
    for p in params {
        let hit = p == after;
        out.push(p);
        if hit {
            out.push(param.to_string());
        }
    }
    format!("{path}?{}", out.join("&"))
}

/// `url` with its TranscodeReasons set to `names`.
fn with_reasons(url: &str, names: &str) -> String {
    let (path, params) = parts(url);
    let params: Vec<String> = params
        .into_iter()
        .map(|p| {
            if p.starts_with("TranscodeReasons=") {
                format!("TranscodeReasons={names}")
            } else {
                p
            }
        })
        .collect();
    format!("{path}?{}", params.join("&"))
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

/// `data` with `default` as the default subtitle stream and `subtitles` as the delivery.
fn with_subtitles(
    data: DeviceData,
    default: Option<i32>,
    subtitles: Vec<SubtitleDelivery>,
) -> DeviceData {
    DeviceData {
        default_subtitle_stream_index: default,
        subtitles,
        ..data
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
    assert_eq!(actual.subtitles, expected.subtitles, "subtitles");
    assert_eq!(actual, expected);
}

#[test]
fn chrome_takes_a_chosen_srt_subtitle_as_webvtt_and_plays_directly() {
    let movie = plus(h264_8bit("mkv"), vec![embedded(2, "subrip")], None);
    let data = device_data(&movie, &profile("chrome-linux"), &choosing(2));
    let expected = with_subtitles(direct_play("mkv", Some(1)), Some(2), vec![vtt(2, 0)]);
    assert_data(&data, &expected);
}

#[test]
fn an_image_subtitle_nobody_chose_does_not_stop_direct_play() {
    let movie = plus(
        h264_8bit("mkv"),
        vec![embedded(2, "hdmv_pgs_subtitle")],
        None,
    );
    let data = device_data(&movie, &profile("chrome-linux"), &request());
    let expected = with_subtitles(direct_play("mkv", Some(1)), None, vec![by(2, M::Encode)]);
    assert_data(&data, &expected);
}

#[test]
fn tv_plays_embedded_srt_and_pgs_subtitles() {
    let hdr = StreamFacts {
        range_type: Some("HDR10".to_string()),
        ..video("hevc", "Main 10", 10, 3840, 2160)
    };
    let streams = vec![
        hdr,
        aac(1),
        embedded(2, "subrip"),
        embedded(3, "hdmv_pgs_subtitle"),
    ];
    let uhd = source("mkv", Some(15_000_000), streams);
    let data = device_data(&uhd, &profile("tv-hevc10-mkv"), &choosing(3));
    let expected = with_subtitles(
        direct_play("mkv", Some(1)),
        Some(3),
        vec![by(2, M::Embed), by(3, M::Embed)],
    );
    assert_data(&data, &expected);
}

#[test]
fn chrome_burns_in_a_chosen_pgs_subtitle() {
    // Chrome can't show PGS, so the subtitle fails the mkv direct play profile (SB 1319-1332 and
    // 1377), and the HLS transcode burns it in.
    let subtitles = || vec![embedded(2, "hdmv_pgs_subtitle"), embedded(3, "subrip")];
    let movie = plus(h264_8bit("mkv"), subtitles(), None);
    let data = device_data(&movie, &profile("chrome-linux"), &choosing(2));
    let url = chrome_8bit_url("SubtitleCodecNotSupported");
    let url = insert_after(&url, "AudioStreamIndex=1", "SubtitleStreamIndex=2");
    let url = insert_after(&url, "EnableAudioVbrEncoding=true", "SubtitleMethod=Encode");
    let why = reasons(&[R::SubtitleCodecNotSupported]);
    let expected = with_subtitles(
        transcode("mkv", why, Some(1), url),
        Some(2),
        vec![by(2, M::Encode), vtt(3, 0)],
    );
    assert_data(&data, &expected);

    // The same when the PGS track is the source's default and the client chooses none.
    let flagged = plus(h264_8bit("mkv"), subtitles(), Some(2));
    let data = device_data(&flagged, &profile("chrome-linux"), &request());
    assert_data(&data, &expected);
}

#[test]
fn the_subtitle_reason_joins_the_video_reasons() {
    let movie = plus(
        h264_hi10("mp4"),
        vec![embedded(2, "hdmv_pgs_subtitle")],
        None,
    );
    let data = device_data(&movie, &profile("chrome-linux"), &choosing(2));
    let url = with_reasons(
        &chrome_hi10_url(),
        "SubtitleCodecNotSupported,VideoProfileNotSupported",
    );
    let url = insert_after(&url, "AudioStreamIndex=1", "SubtitleStreamIndex=2");
    let url = insert_after(&url, "EnableAudioVbrEncoding=true", "SubtitleMethod=Encode");
    let why = reasons(&[R::SubtitleCodecNotSupported, R::VideoProfileNotSupported]);
    let expected = with_subtitles(
        transcode("mp4", why, Some(1), url),
        Some(2),
        vec![by(2, M::Encode)],
    );
    assert_data(&data, &expected);
}

#[test]
fn chrome_takes_an_external_ass_file_as_webvtt() {
    // Jellyfin can't convert ASS; Rustyfin's subtitle endpoint can, so External delivery allows it.
    let movie = plus(h264_8bit("mp4"), vec![external(2, "ass")], Some(2));
    let data = device_data(&movie, &profile("chrome-linux"), &request());
    let expected = with_subtitles(direct_play("mp4", Some(1)), Some(2), vec![vtt(2, 0)]);
    assert_data(&data, &expected);
}

#[test]
fn a_webvtt_subtitle_stays_out_of_the_transcode_url() {
    let movie = plus(h264_hi10("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&movie, &profile("chrome-linux"), &choosing(2));
    let why = reasons(&[R::VideoProfileNotSupported]);
    let expected = with_subtitles(
        transcode("mp4", why, Some(1), chrome_hi10_url()),
        Some(2),
        vec![vtt(2, 0)],
    );
    assert_data(&data, &expected);
}

#[test]
fn always_burn_in_names_the_subtitle_in_the_url() {
    // SI 955-959 writes the index for a burn-in; SubtitleMethod stays out because the method is
    // External (SI 1103-1107).
    let movie = plus(h264_hi10("mp4"), vec![embedded(2, "subrip")], None);
    let burn_in = StreamRequest {
        always_burn_in_subtitle: true,
        ..choosing(2)
    };
    let data = device_data(&movie, &profile("chrome-linux"), &burn_in);
    let url = insert_after(
        &chrome_hi10_url(),
        "AudioStreamIndex=1",
        "SubtitleStreamIndex=2",
    );
    let url = format!("{url}&alwaysBurnInSubtitleWhenTranscoding=true");
    let why = reasons(&[R::VideoProfileNotSupported]);
    let expected = with_subtitles(
        transcode("mp4", why, Some(1), url),
        Some(2),
        vec![vtt(2, 0)],
    );
    assert_data(&data, &expected);
}

#[test]
fn turning_subtitles_off_writes_no_subtitle_parameters() {
    // Jellyfin writes SubtitleMethod=Encode for index -1; Rustyfin's transcoder would read that
    // as "burn in the first subtitle", so the URL names no subtitle at all.
    let forced = StreamFacts {
        is_default: true,
        is_forced: true,
        ..embedded(2, "hdmv_pgs_subtitle")
    };
    let movie = plus(h264_hi10("mp4"), vec![forced], Some(2));
    let data = device_data(&movie, &profile("chrome-linux"), &choosing(-1));
    let why = reasons(&[R::VideoProfileNotSupported]);
    let expected = with_subtitles(
        transcode("mp4", why, Some(1), chrome_hi10_url()),
        Some(-1),
        vec![by(2, M::Encode)],
    );
    assert_data(&data, &expected);
}

#[test]
fn an_index_naming_no_subtitle_falls_back_to_the_source_default() {
    let flagged = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], Some(2));
    for index in [0, 1, 99, i32::MIN] {
        let data = device_data(&flagged, &profile("chrome-linux"), &choosing(index));
        let expected = with_subtitles(direct_play("mp4", Some(1)), Some(2), vec![vtt(2, 0)]);
        assert_data(&data, &expected);
    }

    let plain = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&plain, &profile("chrome-linux"), &choosing(1));
    let expected = with_subtitles(direct_play("mp4", Some(1)), None, vec![vtt(2, 0)]);
    assert_data(&data, &expected);
}

#[test]
fn a_subtitle_index_for_another_source_is_ignored() {
    let movie = plus(
        h264_8bit("mkv"),
        vec![embedded(2, "hdmv_pgs_subtitle")],
        None,
    );
    let other = StreamRequest {
        media_source_id: "other".to_string(),
        ..choosing(2)
    };
    let data = device_data(&movie, &profile("chrome-linux"), &other);
    let expected = with_subtitles(direct_play("mkv", Some(1)), None, vec![by(2, M::Encode)]);
    assert_data(&data, &expected);
}

#[test]
fn subtitle_profiles_match_the_stream_language() {
    let streams = |languages: [Option<&str>; 3]| {
        let mut movie = h264_8bit("mkv");
        for (index, language) in (2..).zip(languages) {
            movie.streams.push(StreamFacts {
                language: language.map(str::to_string),
                ..embedded(index, "subrip")
            });
        }
        movie
    };
    let movie = streams([Some("eng"), Some("ger"), None]);

    let embeds_two = mkv_player(json!([
        {"Format": "srt", "Method": "Embed", "Language": "eng,fre"},
        {"Format": "vtt", "Method": "External"}
    ]));
    let data = device_data(&movie, &embeds_two, &request());
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    assert_eq!(data.subtitles, vec![by(2, M::Embed), vtt(3, 0), vtt(4, 0)]);

    // A track without a language counts as "und".
    let english_files = mkv_player(json!([
        {"Format": "vtt", "Method": "External", "Language": "eng"}
    ]));
    let data = device_data(&movie, &english_files, &request());
    assert_eq!(
        data.subtitles,
        vec![vtt(2, 0), by(3, M::Encode), by(4, M::Encode)]
    );
    let undetermined = mkv_player(json!([
        {"Format": "vtt", "Method": "External", "Language": "und"}
    ]));
    let data = device_data(&movie, &undetermined, &request());
    assert_eq!(
        data.subtitles,
        vec![by(2, M::Encode), by(3, M::Encode), vtt(4, 0)]
    );
}

#[test]
fn subtitle_formats_match_their_ffmpeg_codec_names() {
    let codecs = [
        "subrip",
        "hdmv_pgs_subtitle",
        "dvd_subtitle",
        "dvb_subtitle",
        "webvtt",
        "mov_text",
        "ass",
    ];
    let mut movie = h264_8bit("mkv");
    movie.streams.extend(
        (2..)
            .zip(codecs)
            .map(|(index, codec)| embedded(index, codec)),
    );
    let embeds = mkv_player(json!([
        {"Format": "srt", "Method": "Embed"},
        {"Format": "PGSSUB", "Method": "Embed"},
        {"Format": "dvdsub", "Method": "Embed"},
        {"Format": "dvbsub", "Method": "Embed"},
        {"Format": "vtt", "Method": "Embed"},
        {"Format": "vtt", "Method": "External"}
    ]));
    let data = device_data(&movie, &embeds, &request());
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    // mov_text converts to the srt profile (the second pass). ASS converts to nothing that is
    // embedded, as in Jellyfin, so it goes out as a WebVTT file.
    assert_eq!(
        data.subtitles,
        vec![
            by(2, M::Embed),
            by(3, M::Embed),
            by(4, M::Embed),
            by(5, M::Embed),
            by(6, M::Embed),
            by(7, M::Embed),
            vtt(8, 0),
        ]
    );
}

#[test]
fn embed_profiles_match_the_container() {
    let profile = DeviceProfile::from_json(&json!({
        "MaxStreamingBitrate": 40_000_000,
        "DirectPlayProfiles": [
            {"Container": "mkv,mp4", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac"}
        ],
        "SubtitleProfiles": [
            {"Format": "srt", "Method": "Embed", "Container": "mkv"},
            {"Format": "vtt", "Method": "External"}
        ]
    }))
    .unwrap();
    let mkv = plus(h264_8bit("mkv"), vec![embedded(2, "subrip")], None);
    let data = device_data(&mkv, &profile, &request());
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    assert_eq!(data.subtitles, vec![by(2, M::Embed)]);

    let mp4 = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&mp4, &profile, &request());
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    assert_eq!(data.subtitles, vec![vtt(2, 0)]);
}

#[test]
fn hls_subtitle_profiles_are_not_used() {
    // Jellyfin would put the subtitle into the HLS manifest; Rustyfin's transcoder doesn't, so the
    // profile is skipped and the subtitle can only be burned in.
    let manifest_only = mkv_player(json!([{"Format": "vtt", "Method": "Hls"}]));
    let movie = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&movie, &manifest_only, &request());
    assert_eq!(data.play_method, Some(PlayMethod::Transcode));
    assert_eq!(data.subtitles, vec![by(2, M::Encode)]);
}

#[test]
fn only_webvtt_files_are_delivered() {
    let srt_files = mkv_player(json!([{"Format": "srt", "Method": "External"}]));
    let movie = plus(h264_8bit("mkv"), vec![embedded(2, "subrip")], None);
    let data = device_data(&movie, &srt_files, &request());
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    assert_eq!(data.subtitles, vec![by(2, M::Encode)]);
}

#[test]
fn a_subtitle_without_a_codec_is_text_only_as_a_file() {
    // As Jellyfin's IsTextSubtitleStream: an embedded stream needs a codec to count as text.
    let movie = plus(
        h264_8bit("mp4"),
        vec![embedded(2, ""), external(3, "")],
        None,
    );
    let data = device_data(&movie, &profile("chrome-linux"), &request());
    let expected = with_subtitles(
        direct_play("mp4", Some(1)),
        None,
        vec![by(2, M::Encode), vtt(3, 0)],
    );
    assert_data(&data, &expected);
}

#[test]
fn subtitle_files_start_at_the_start_time_only_without_a_stream() {
    // SI 1177-1179: HLS keeps the timestamps and a direct play starts at 0, so only a transcode
    // that isn't HLS starts the file late. With no transcoding profile applied, the subtitles are
    // computed with StreamInfo's defaults: a transcode without a container over http.
    let late = |request: StreamRequest| StreamRequest {
        start_time_ticks: 600_000_000,
        ..request
    };

    let av1 = source(
        "mkv",
        Some(10_000_000),
        vec![
            video("av1", "Main", 8, 1920, 1080),
            aac(1),
            embedded(2, "subrip"),
        ],
    );
    let neither = StreamRequest {
        enable_transcoding: false,
        enable_direct_stream: false,
        ..late(request())
    };
    let data = device_data(&av1, &profile("tv-hevc10-mkv"), &neither);
    assert_eq!(data.play_method, None);
    assert_eq!(data.subtitles, vec![vtt(2, 600_000_000)]);

    let direct = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&direct, &profile("chrome-linux"), &late(request()));
    assert_eq!(data.play_method, Some(PlayMethod::DirectPlay));
    assert_eq!(data.subtitles, vec![vtt(2, 0)]);

    let hls = plus(h264_hi10("mp4"), vec![embedded(2, "subrip")], None);
    let data = device_data(&hls, &profile("chrome-linux"), &late(request()));
    assert_eq!(data.play_method, Some(PlayMethod::Transcode));
    assert_eq!(data.subtitles, vec![vtt(2, 0)]);
}

#[test]
fn tv_transcode_delivers_text_as_webvtt_and_burns_in_images() {
    let movie = plus(
        h264_hi10("mkv"),
        vec![embedded(2, "subrip"), embedded(3, "hdmv_pgs_subtitle")],
        None,
    );
    let data = device_data(&movie, &profile("tv-hevc10-mkv"), &request());
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
    let expected = with_subtitles(
        transcode("mkv", why, Some(1), url),
        None,
        vec![vtt(2, 0), by(3, M::Encode)],
    );
    assert_data(&data, &expected);
}

#[test]
fn without_a_token_the_subtitle_url_has_no_api_key() {
    let movie = plus(h264_8bit("mp4"), vec![embedded(2, "subrip")], None);
    let anonymous = StreamRequest {
        access_token: String::new(),
        ..request()
    };
    let data = device_data(&movie, &profile("chrome-linux"), &anonymous);
    let url = format!("/Videos/{ITEM}/src1/Subtitles/2/0/Stream.vtt");
    assert_eq!(
        data.subtitles,
        vec![SubtitleDelivery {
            index: 2,
            method: M::External,
            url: Some(url),
        }]
    );
}

#[test]
fn odd_subtitle_streams_do_not_panic_and_keep_their_order() {
    let odd = |index: i32, codec: &str| StreamFacts {
        language: Some(String::new()),
        ..embedded(index, codec)
    };
    let unnamed = StreamFacts {
        language: None,
        path: None,
        ..external(-1, "")
    };
    let streams = vec![
        video("h264", "High", 8, 1920, 1080),
        aac(1),
        odd(9, "subrip"),
        odd(3, "hdmv_pgs_subtitle"),
        unnamed,
        odd(i32::MAX, "weird codec"),
        external(i32::MIN, "ASS"),
    ];
    let movie = source("mkv", Some(10_000_000), streams);
    let order = [9, 3, -1, i32::MAX, i32::MIN];
    let requests = [
        request(),
        StreamRequest {
            start_time_ticks: i64::MAX,
            always_burn_in_subtitle: true,
            ..choosing(i32::MAX)
        },
        StreamRequest {
            start_time_ticks: -1,
            ..choosing(i32::MIN)
        },
        StreamRequest {
            enable_transcoding: false,
            enable_direct_stream: false,
            start_time_ticks: i64::MIN,
            ..choosing(-1)
        },
        StreamRequest {
            access_token: String::new(),
            ..choosing(3)
        },
    ];
    let names = [
        "chrome-linux",
        "chrome-linux-high10",
        "minimal-mp4",
        "tv-hevc10-mkv",
    ];
    for name in names {
        let profile = profile(name);
        for (n, request) in requests.iter().enumerate() {
            for default in [None, Some(3), Some(-1), Some(42)] {
                let movie = SourceFacts {
                    default_subtitle_stream_index: default,
                    ..movie.clone()
                };
                let data = device_data(&movie, &profile, request);
                let indexes: Vec<i32> = data.subtitles.iter().map(|s| s.index).collect();
                assert_eq!(indexes, order, "{name}, request {n}, default {default:?}");
            }
        }
    }
}
