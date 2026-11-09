//! J2: DeviceProfile and PlaybackInfoDto parsing, with Jellyfin's lenient JSON rules.

use rustyfin_model::device_profile::{
    CodecType, ConditionProperty, ConditionType, DeviceProfile, EncodingContext, ProfileCondition,
    ProfileType, StreamProtocol, SubtitleDeliveryMethod, TranscodeSeekInfo, TranscodingProfile,
};
use rustyfin_model::items_playback_info::{
    ItemsPlaybackInfoResponse, PlaybackErrorCode, PlaybackInfoDto,
};
use serde_json::{Value, json};

const USER_ID: &str = "6b3c3c3a-5d5e-4f4f-9a9a-0123456789ab";

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../rustyfin-core/tests/fixtures/device_profiles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn profile(v: Value) -> DeviceProfile {
    DeviceProfile::from_json(&v).expect("profile parses")
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn enums_parse_names_ignoring_case_and_defined_numbers() {
    assert_eq!(ProfileType::from_json(&json!("Video")), ProfileType::Video);
    assert_eq!(ProfileType::from_json(&json!("video")), ProfileType::Video);
    assert_eq!(ProfileType::from_json(&json!("AUDIO")), ProfileType::Audio);
    assert_eq!(ProfileType::from_json(&json!(1)), ProfileType::Video);
    assert_eq!(ProfileType::from_json(&json!("4")), ProfileType::Lyric);
    assert_eq!(ProfileType::from_json(&Value::Null), ProfileType::Audio);
    assert_eq!(
        ProfileType::from_json(&json!("Banana")),
        ProfileType::Unknown
    );
    assert_eq!(ProfileType::from_json(&json!(9)), ProfileType::Unknown);
    assert_eq!(ProfileType::from_json(&json!(-1)), ProfileType::Unknown);
    assert_eq!(ProfileType::from_json(&json!([1])), ProfileType::Unknown);
    assert_eq!(ProfileType::default(), ProfileType::Audio);
    assert_eq!(ProfileType::Video.as_str(), "Video");
    assert_eq!(ProfileType::Unknown.as_str(), "Unknown");

    assert_eq!(
        CodecType::from_json(&json!("videoaudio")),
        CodecType::VideoAudio
    );
    assert_eq!(CodecType::from_json(&json!(2)), CodecType::Audio);
    assert_eq!(CodecType::default(), CodecType::Video);
    assert_eq!(
        EncodingContext::from_json(&json!("static")),
        EncodingContext::Static
    );
    assert_eq!(EncodingContext::default(), EncodingContext::Streaming);
    assert_eq!(
        StreamProtocol::from_json(&json!("hls")),
        StreamProtocol::Hls
    );
    assert_eq!(
        StreamProtocol::from_json(&json!("HTTP")),
        StreamProtocol::Http
    );
    assert_eq!(StreamProtocol::from_json(&json!(1)), StreamProtocol::Hls);
    assert_eq!(
        StreamProtocol::from_json(&json!("rtsp")),
        StreamProtocol::Unknown
    );
    assert_eq!(StreamProtocol::Hls.as_str(), "hls");
    assert_eq!(StreamProtocol::Http.as_str(), "http");
    assert_eq!(
        SubtitleDeliveryMethod::from_json(&json!("external")),
        SubtitleDeliveryMethod::External
    );
    assert_eq!(
        SubtitleDeliveryMethod::from_json(&json!(4)),
        SubtitleDeliveryMethod::Drop
    );
    assert_eq!(SubtitleDeliveryMethod::Embed.as_str(), "Embed");
    assert_eq!(SubtitleDeliveryMethod::Hls.as_str(), "Hls");
    assert_eq!(
        ConditionType::from_json(&json!("equalsany")),
        ConditionType::EqualsAny
    );
    assert_eq!(
        ConditionType::from_json(&json!("LessThan")),
        ConditionType::Unknown
    );
    assert_eq!(
        TranscodeSeekInfo::from_json(&json!("bytes")),
        TranscodeSeekInfo::Bytes
    );
    assert_eq!(TranscodeSeekInfo::default(), TranscodeSeekInfo::Auto);
}

#[test]
fn condition_properties_follow_jellyfin_numbering() {
    use ConditionProperty as P;
    let named = [
        (0, P::AudioChannels),
        (1, P::AudioBitrate),
        (2, P::AudioProfile),
        (3, P::Width),
        (4, P::Height),
        (5, P::Has64BitOffsets),
        (6, P::PacketLength),
        (7, P::VideoBitDepth),
        (8, P::VideoBitrate),
        (9, P::VideoFramerate),
        (10, P::VideoLevel),
        (11, P::VideoProfile),
        (12, P::VideoTimestamp),
        (13, P::IsAnamorphic),
        (14, P::RefFrames),
        (16, P::NumAudioStreams),
        (17, P::NumVideoStreams),
        (18, P::IsSecondaryAudio),
        (19, P::VideoCodecTag),
        (20, P::IsAvc),
        (21, P::IsInterlaced),
        (22, P::AudioSampleRate),
        (23, P::AudioBitDepth),
        (24, P::VideoRangeType),
        (25, P::NumStreams),
        (26, P::VideoRotation),
    ];
    for (n, p) in named {
        assert_eq!(P::from_json(&json!(n)), p, "{n}");
        assert_eq!(P::from_json(&json!(p.as_str())), p);
        assert_eq!(P::from_json(&json!(p.as_str().to_lowercase())), p);
    }
    assert_eq!(P::from_json(&json!(15)), P::Unknown);
    assert_eq!(P::from_json(&json!(27)), P::Unknown);
    assert_eq!(P::from_json(&json!("Smell")), P::Unknown);
    assert_eq!(P::VideoBitDepth.as_str(), "VideoBitDepth");
    assert_eq!(P::default(), P::AudioChannels);
}

#[test]
fn chrome_fixture_parses() {
    let p = profile(fixture("chrome-linux"));
    assert_eq!(p.name, None);
    assert_eq!(p.id, None);
    assert_eq!(p.max_streaming_bitrate, Some(120_000_000));
    assert_eq!(p.max_static_bitrate, Some(100_000_000));
    assert_eq!(p.music_streaming_transcoding_bitrate, Some(384_000));
    assert_eq!(p.max_static_music_bitrate, Some(8_000_000));
    assert_eq!(p.direct_play_profiles.len(), 17);
    assert_eq!(p.transcoding_profiles.len(), 10);
    assert!(p.container_profiles.is_empty());
    assert_eq!(p.codec_profiles.len(), 6);
    assert_eq!(p.subtitle_profiles.len(), 3);

    let mp4 = &p.direct_play_profiles[1];
    assert_eq!(mp4.container, "mp4,m4v");
    assert_eq!(mp4.r#type, ProfileType::Video);
    assert_eq!(mp4.video_codec, "h264,vp9,av1");
    assert_eq!(mp4.audio_codec, "aac,mp3,mp2,opus,flac,vorbis");
    let opus = &p.direct_play_profiles[4];
    assert_eq!(opus.container, "opus");
    assert_eq!(opus.r#type, ProfileType::Audio);
    assert_eq!(opus.audio_codec, "");
    assert_eq!(opus.video_codec, "");
    assert_eq!(p.direct_play_profiles[16].container, "hls");

    let tp = &p.transcoding_profiles[9];
    assert_eq!(tp.container, "ts");
    assert_eq!(tp.r#type, ProfileType::Video);
    assert_eq!(tp.video_codec, "h264");
    assert_eq!(tp.audio_codec, "aac,mp3,mp2");
    assert_eq!(tp.protocol, StreamProtocol::Hls);
    assert_eq!(tp.context, EncodingContext::Streaming);
    assert_eq!(tp.max_audio_channels.as_deref(), Some("2"));
    assert_eq!(tp.min_segments, 1);
    assert_eq!(tp.segment_length, 0);
    assert_eq!(tp.break_on_non_key_frames, Some(true));
    assert!(tp.enable_audio_vbr_encoding);
    assert_eq!(tp.transcode_seek_info, TranscodeSeekInfo::Auto);
    assert!(!tp.copy_timestamps);
    assert!(!tp.estimate_content_length);
    assert!(!tp.enable_mpegts_m2ts_mode);
    assert!(!tp.enable_subtitles_in_manifest);
    assert!(tp.conditions.is_empty());
    assert_eq!(p.transcoding_profiles[0].r#type, ProfileType::Audio);
    assert_eq!(p.transcoding_profiles[0].protocol, StreamProtocol::Hls);
    assert_eq!(p.transcoding_profiles[1].protocol, StreamProtocol::Http);
    assert_eq!(p.transcoding_profiles[5].context, EncodingContext::Static);

    let h264 = &p.codec_profiles[2];
    assert_eq!(h264.r#type, CodecType::Video);
    assert_eq!(h264.codec, "h264");
    assert_eq!(h264.container, "");
    assert_eq!(h264.sub_container, "");
    assert!(h264.apply_conditions.is_empty());
    assert_eq!(h264.conditions.len(), 4);
    assert_eq!(
        h264.conditions[0],
        ProfileCondition {
            condition: ConditionType::EqualsAny,
            property: ConditionProperty::VideoProfile,
            value: "high|main|baseline|constrained baseline".to_string(),
            is_required: false,
        }
    );
    assert_eq!(
        h264.conditions[1].property,
        ConditionProperty::VideoRangeType
    );
    assert_eq!(h264.conditions[2].property, ConditionProperty::VideoLevel);
    assert_eq!(h264.conditions[2].condition, ConditionType::LessThanEqual);
    assert_eq!(h264.conditions[2].value, "52");
    assert_eq!(h264.conditions[3].property, ConditionProperty::IsInterlaced);
    assert_eq!(h264.conditions[3].condition, ConditionType::NotEquals);
    let any_audio = &p.codec_profiles[1];
    assert_eq!(any_audio.r#type, CodecType::VideoAudio);
    assert_eq!(any_audio.codec, "");
    assert_eq!(
        any_audio.conditions[0].property,
        ConditionProperty::IsSecondaryAudio
    );

    let vtt = &p.subtitle_profiles[0];
    assert_eq!(vtt.format, "vtt");
    assert_eq!(vtt.method, SubtitleDeliveryMethod::External);
    assert_eq!(vtt.language, "");
    assert_eq!(vtt.container, "");
    assert_eq!(p.subtitle_profiles[2].format, "ssa");
}

#[test]
fn chrome_high10_fixture_differs_only_in_the_h264_profiles() {
    let plain = profile(fixture("chrome-linux"));
    let high10 = profile(fixture("chrome-linux-high10"));
    assert_eq!(
        high10.codec_profiles[2].conditions[0].value,
        "high|main|baseline|constrained baseline|high 10"
    );
    let mut patched = high10.clone();
    patched.codec_profiles[2].conditions[0].value =
        plain.codec_profiles[2].conditions[0].value.clone();
    assert_eq!(patched, plain);
}

#[test]
fn minimal_and_tv_fixtures_parse() {
    let mp4 = profile(fixture("minimal-mp4"));
    assert_eq!(mp4.name.as_deref(), Some("Minimal MP4"));
    assert_eq!(mp4.max_streaming_bitrate, Some(8_000_000));
    assert_eq!(mp4.max_static_bitrate, Some(8_000_000));
    assert_eq!(mp4.music_streaming_transcoding_bitrate, Some(128_000));
    assert_eq!(mp4.codec_profiles[0].conditions.len(), 4);
    assert_eq!(mp4.codec_profiles[1].r#type, CodecType::VideoAudio);
    assert_eq!(mp4.transcoding_profiles[0].min_segments, 0);
    assert_eq!(mp4.transcoding_profiles[0].break_on_non_key_frames, None);

    let tv = profile(fixture("tv-hevc10-mkv"));
    assert_eq!(tv.max_streaming_bitrate, Some(40_000_000));
    assert_eq!(tv.music_streaming_transcoding_bitrate, Some(192_000));
    let tp = &tv.transcoding_profiles[0];
    assert_eq!(tp.min_segments, 2);
    assert_eq!(tp.break_on_non_key_frames, Some(false));
    assert_eq!(tp.max_audio_channels.as_deref(), Some("6"));
    assert_eq!(tv.transcoding_profiles[1].protocol, StreamProtocol::Http);
    assert_eq!(tv.codec_profiles[2].codec, "ac3,eac3");
    assert_eq!(tv.subtitle_profiles[2].format, "pgssub");
    assert_eq!(
        tv.subtitle_profiles[2].method,
        SubtitleDeliveryMethod::Embed
    );
}

#[test]
fn an_empty_profile_takes_jellyfin_defaults() {
    let p = profile(json!({}));
    assert_eq!(p, DeviceProfile::default());
    assert_eq!(p.max_streaming_bitrate, Some(8_000_000));
    assert_eq!(p.max_static_bitrate, Some(8_000_000));
    assert_eq!(p.music_streaming_transcoding_bitrate, Some(128_000));
    assert_eq!(p.max_static_music_bitrate, Some(8_000_000));
    assert!(p.direct_play_profiles.is_empty());
    assert!(p.transcoding_profiles.is_empty());
    assert!(p.container_profiles.is_empty());
    assert!(p.codec_profiles.is_empty());
    assert!(p.subtitle_profiles.is_empty());
}

#[test]
fn an_explicit_null_bitrate_is_none() {
    let p = profile(json!({"MaxStreamingBitrate": null, "MaxStaticBitrate": null}));
    assert_eq!(p.max_streaming_bitrate, None);
    assert_eq!(p.max_static_bitrate, None);
    assert_eq!(p.music_streaming_transcoding_bitrate, Some(128_000));
}

#[test]
fn element_defaults_match_jellyfin() {
    let p = profile(json!({
        "TranscodingProfiles": [{"SegmentLength": null}],
        "CodecProfiles": [{"Conditions": [{}]}],
        "SubtitleProfiles": [{}],
        "DirectPlayProfiles": [{"Container": null, "Type": null}]
    }));
    let tp = &p.transcoding_profiles[0];
    assert_eq!(*tp, TranscodingProfile::default());
    assert_eq!(tp.r#type, ProfileType::Audio);
    assert_eq!(tp.protocol, StreamProtocol::Http);
    assert_eq!(tp.context, EncodingContext::Streaming);
    assert!(tp.enable_audio_vbr_encoding);
    assert_eq!(tp.max_audio_channels, None);
    assert_eq!(tp.min_segments, 0);
    assert_eq!(tp.segment_length, 0);
    assert_eq!(tp.break_on_non_key_frames, None);
    let c = &p.codec_profiles[0].conditions[0];
    assert_eq!(*c, ProfileCondition::default());
    assert!(c.is_required);
    assert_eq!(c.condition, ConditionType::Equals);
    assert_eq!(c.property, ConditionProperty::AudioChannels);
    assert_eq!(c.value, "");
    assert_eq!(p.codec_profiles[0].r#type, CodecType::Video);
    assert_eq!(
        p.subtitle_profiles[0].method,
        SubtitleDeliveryMethod::Encode
    );
    assert_eq!(p.direct_play_profiles[0].container, "");
    assert_eq!(p.direct_play_profiles[0].r#type, ProfileType::Audio);
}

#[test]
fn keys_match_ignoring_case() {
    let p = profile(json!({
        "name": "lower",
        "ID": "abc",
        "maxStreamingBitrate": 2000000,
        "maxstaticbitrate": 3000000,
        "musicStreamingTranscodingBitrate": 192000,
        "maxStaticMusicBitrate": 320000,
        "directplayprofiles": [
            {"container": "mp4", "type": "Video", "videoCodec": "h264", "AUDIOCODEC": "aac"}
        ],
        "codecProfiles": [{
            "type": "Video",
            "codec": "h264",
            "container": "mkv",
            "subContainer": "ts",
            "conditions": [
                {"condition": "LessThanEqual", "property": "Width", "value": "1280", "isRequired": false}
            ],
            "applyConditions": [
                {"condition": "Equals", "property": "IsInterlaced", "value": "false"}
            ]
        }],
        "transcodingProfiles": [{
            "container": "ts",
            "type": "video",
            "videoCodec": "h264",
            "audioCodec": "aac",
            "protocol": "hls",
            "context": "streaming",
            "minSegments": 3,
            "segmentLength": 6,
            "breakOnNonKeyFrames": true,
            "maxAudioChannels": "6",
            "enableAudioVbrEncoding": false,
            "copyTimestamps": true,
            "transcodeSeekInfo": "Bytes",
            "estimateContentLength": true,
            "enableMpegtsM2TsMode": true,
            "enableSubtitlesInManifest": true,
            "conditions": [{"condition": "LessThanEqual", "property": "VideoBitDepth", "value": "8"}]
        }],
        "containerProfiles": [{
            "type": "Video",
            "container": "mkv",
            "subContainer": "x",
            "conditions": [{"condition": "Equals", "property": "NumStreams", "value": "3"}]
        }],
        "subtitleProfiles": [
            {"format": "srt", "method": "Embed", "language": "eng,fre", "container": "mkv", "didlMode": "x"}
        ]
    }));
    assert_eq!(p.name.as_deref(), Some("lower"));
    assert_eq!(p.id.as_deref(), Some("abc"));
    assert_eq!(p.max_streaming_bitrate, Some(2_000_000));
    assert_eq!(p.max_static_bitrate, Some(3_000_000));
    assert_eq!(p.music_streaming_transcoding_bitrate, Some(192_000));
    assert_eq!(p.max_static_music_bitrate, Some(320_000));

    let dp = &p.direct_play_profiles[0];
    assert_eq!(
        (
            dp.container.as_str(),
            dp.video_codec.as_str(),
            dp.audio_codec.as_str()
        ),
        ("mp4", "h264", "aac")
    );
    assert_eq!(dp.r#type, ProfileType::Video);

    let cp = &p.codec_profiles[0];
    assert_eq!(cp.container, "mkv");
    assert_eq!(cp.sub_container, "ts");
    assert_eq!(cp.conditions[0].property, ConditionProperty::Width);
    assert_eq!(cp.conditions[0].value, "1280");
    assert!(!cp.conditions[0].is_required);
    assert_eq!(
        cp.apply_conditions[0].property,
        ConditionProperty::IsInterlaced
    );
    assert!(cp.apply_conditions[0].is_required);

    let tp = &p.transcoding_profiles[0];
    assert_eq!(tp.r#type, ProfileType::Video);
    assert_eq!(tp.video_codec, "h264");
    assert_eq!(tp.audio_codec, "aac");
    assert_eq!(tp.protocol, StreamProtocol::Hls);
    assert_eq!(tp.context, EncodingContext::Streaming);
    assert_eq!(tp.min_segments, 3);
    assert_eq!(tp.segment_length, 6);
    assert_eq!(tp.break_on_non_key_frames, Some(true));
    assert_eq!(tp.max_audio_channels.as_deref(), Some("6"));
    assert!(!tp.enable_audio_vbr_encoding);
    assert!(tp.copy_timestamps);
    assert_eq!(tp.transcode_seek_info, TranscodeSeekInfo::Bytes);
    assert!(tp.estimate_content_length);
    assert!(tp.enable_mpegts_m2ts_mode);
    assert!(tp.enable_subtitles_in_manifest);
    assert_eq!(tp.conditions[0].property, ConditionProperty::VideoBitDepth);

    let ctp = &p.container_profiles[0];
    assert_eq!(ctp.r#type, ProfileType::Video);
    assert_eq!(ctp.container, "mkv");
    assert_eq!(ctp.sub_container, "x");
    assert_eq!(ctp.conditions[0].property, ConditionProperty::NumStreams);

    let sp = &p.subtitle_profiles[0];
    assert_eq!(sp.format, "srt");
    assert_eq!(sp.method, SubtitleDeliveryMethod::Embed);
    assert_eq!(sp.language, "eng,fre");
    assert_eq!(sp.container, "mkv");
    assert_eq!(sp.didl_mode, "x");
}

#[test]
fn numbers_bools_and_text_convert_where_jellyfin_reads_them() {
    let p = profile(json!({
        "MaxStreamingBitrate": "120000000",
        "TranscodingProfiles": [{
            "MinSegments": "2",
            "CopyTimestamps": "TRUE",
            "EnableAudioVbrEncoding": "false",
            "BreakOnNonKeyFrames": "True",
            "MaxAudioChannels": 6
        }],
        "CodecProfiles": [{
            "Type": "Video",
            "Codec": "h264",
            "Conditions": [
                {"Condition": "LessThanEqual", "Property": "VideoLevel", "Value": 52, "IsRequired": "False"},
                {"Condition": "Equals", "Property": "IsAvc", "Value": true}
            ]
        }]
    }));
    assert_eq!(p.max_streaming_bitrate, Some(120_000_000));
    let tp = &p.transcoding_profiles[0];
    assert_eq!(tp.min_segments, 2);
    assert!(tp.copy_timestamps);
    assert!(!tp.enable_audio_vbr_encoding);
    assert_eq!(tp.break_on_non_key_frames, Some(true));
    assert_eq!(tp.max_audio_channels.as_deref(), Some("6"));
    let conds = &p.codec_profiles[0].conditions;
    assert_eq!(conds[0].value, "52");
    assert!(!conds[0].is_required);
    assert_eq!(conds[1].value, "true");
    assert!(conds[1].is_required);
}

#[test]
fn unknown_keys_enum_names_and_null_entries_are_tolerated() {
    let p = profile(json!({
        "ResponseProfiles": [{"Container": "m4v"}],
        "SomethingNew": {"nested": true},
        "DirectPlayProfiles": [null, {"Container": "mp4", "Type": "Hologram", "Extra": 1}],
        "TranscodingProfiles": null,
        "ContainerProfiles": [],
        "CodecProfiles": [{
            "Type": "Banana",
            "Conditions": [null, {"Condition": "Sometimes", "Property": "Smell", "Value": "x"}]
        }],
        "SubtitleProfiles": [{"Format": "srt", "Method": "Carrier pigeon"}]
    }));
    assert_eq!(p.direct_play_profiles.len(), 1);
    assert_eq!(p.direct_play_profiles[0].container, "mp4");
    assert_eq!(p.direct_play_profiles[0].r#type, ProfileType::Unknown);
    assert!(p.transcoding_profiles.is_empty());
    assert!(p.container_profiles.is_empty());
    assert_eq!(p.codec_profiles[0].r#type, CodecType::Unknown);
    assert_eq!(p.codec_profiles[0].conditions.len(), 1);
    assert_eq!(
        p.codec_profiles[0].conditions[0].condition,
        ConditionType::Unknown
    );
    assert_eq!(
        p.codec_profiles[0].conditions[0].property,
        ConditionProperty::Unknown
    );
    assert_eq!(
        p.subtitle_profiles[0].method,
        SubtitleDeliveryMethod::Unknown
    );
}

#[test]
fn wrongly_typed_known_fields_are_errors() {
    let bad = [
        Value::Null,
        json!("not an object"),
        json!([]),
        json!(5),
        json!({"MaxStreamingBitrate": "fast"}),
        json!({"MaxStreamingBitrate": 1.5}),
        json!({"MaxStreamingBitrate": 3_000_000_000u64}),
        json!({"MaxStreamingBitrate": {}}),
        json!({"MaxStreamingBitrate": true}),
        json!({"DirectPlayProfiles": {}}),
        json!({"DirectPlayProfiles": "mp4"}),
        json!({"DirectPlayProfiles": [5]}),
        json!({"DirectPlayProfiles": [{"Container": ["mp4"]}]}),
        json!({"TranscodingProfiles": [{"MinSegments": "two"}]}),
        json!({"TranscodingProfiles": [{"CopyTimestamps": "yes"}]}),
        json!({"TranscodingProfiles": [{"BreakOnNonKeyFrames": 1}]}),
        json!({"CodecProfiles": [{"Conditions": [{"IsRequired": "maybe"}]}]}),
        json!({"CodecProfiles": [{"Conditions": {"Property": "Width"}}]}),
        json!({"CodecProfiles": [{"ApplyConditions": [[]]}]}),
        json!({"Name": {"first": "x"}}),
    ];
    for v in bad {
        let err = DeviceProfile::from_json(&v);
        assert!(err.is_err(), "{v} should not parse");
    }
    let err = DeviceProfile::from_json(&json!({"MaxStreamingBitrate": "fast"})).unwrap_err();
    assert!(!err.to_string().is_empty());
    let _: &dyn std::error::Error = &err;
}

#[test]
fn empty_bodies_give_the_default_dto() {
    for body in ["", "   \n", "null", "{}", " { } "] {
        assert_eq!(
            PlaybackInfoDto::from_body(body.as_bytes()).unwrap(),
            PlaybackInfoDto::default(),
            "{body:?}"
        );
    }
    let nulls = br#"{"UserId": "", "DeviceProfile": null, "MediaSourceId": null,
        "EnableDirectPlay": null, "MaxStreamingBitrate": null, "Unknown": [1, 2]}"#;
    assert_eq!(
        PlaybackInfoDto::from_body(nulls).unwrap(),
        PlaybackInfoDto::default()
    );
}

#[test]
fn malformed_bodies_are_errors() {
    let bad: [&[u8]; 13] = [
        b"[]",
        b"5",
        b"\"x\"",
        b"{",
        b"not json",
        &[0xff, 0xfe, 0x7b, 0x7d],
        br#"{"MaxStreamingBitrate": "fast"}"#,
        br#"{"StartTimeTicks": 1.5}"#,
        br#"{"UserId": "nope"}"#,
        br#"{"DeviceProfile": 5}"#,
        br#"{"DeviceProfile": []}"#,
        br#"{"EnableDirectPlay": "maybe"}"#,
        br#"{"DeviceProfile": {"DirectPlayProfiles": 3}}"#,
    ];
    for body in bad {
        assert!(
            PlaybackInfoDto::from_body(body).is_err(),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
}

fn full_dto() -> PlaybackInfoDto {
    PlaybackInfoDto {
        user_id: Some(USER_ID.parse().unwrap()),
        max_streaming_bitrate: Some(8_000_000),
        start_time_ticks: Some(90_000_000_000),
        audio_stream_index: Some(1),
        subtitle_stream_index: Some(-1),
        max_audio_channels: Some(6),
        media_source_id: Some("abc".to_string()),
        live_stream_id: Some("live".to_string()),
        device_profile: Some(profile(fixture("minimal-mp4"))),
        enable_direct_play: Some(false),
        enable_direct_stream: Some(true),
        enable_transcoding: Some(true),
        allow_video_stream_copy: Some(false),
        allow_audio_stream_copy: Some(true),
        auto_open_live_stream: Some(false),
        always_burn_in_subtitle_when_transcoding: Some(true),
    }
}

#[test]
fn body_fields_parse_in_pascal_case() {
    let body = json!({
        "UserId": USER_ID,
        "MaxStreamingBitrate": 8000000,
        "StartTimeTicks": 90_000_000_000i64,
        "AudioStreamIndex": 1,
        "SubtitleStreamIndex": -1,
        "MaxAudioChannels": "6",
        "MediaSourceId": "abc",
        "LiveStreamId": "live",
        "DeviceProfile": fixture("minimal-mp4"),
        "EnableDirectPlay": false,
        "EnableDirectStream": true,
        "EnableTranscoding": "true",
        "AllowVideoStreamCopy": false,
        "AllowAudioStreamCopy": true,
        "AutoOpenLiveStream": false,
        "AlwaysBurnInSubtitleWhenTranscoding": true
    });
    let dto = PlaybackInfoDto::from_body(body.to_string().as_bytes()).unwrap();
    assert_eq!(dto, full_dto());
}

#[test]
fn body_fields_parse_in_camel_case_as_the_web_client_sends_them() {
    let body = json!({
        "userId": USER_ID.replace('-', ""),
        "maxStreamingBitrate": 8000000,
        "startTimeTicks": 90_000_000_000i64,
        "audioStreamIndex": 1,
        "subtitleStreamIndex": -1,
        "maxAudioChannels": 6,
        "mediaSourceId": "abc",
        "liveStreamId": "live",
        "deviceProfile": fixture("minimal-mp4"),
        "enableDirectPlay": false,
        "enableDirectStream": true,
        "enableTranscoding": true,
        "allowVideoStreamCopy": false,
        "allowAudioStreamCopy": true,
        "autoOpenLiveStream": false,
        "alwaysBurnInSubtitleWhenTranscoding": true
    });
    let dto = PlaybackInfoDto::from_body(body.to_string().as_bytes()).unwrap();
    assert_eq!(dto, full_dto());
}

#[test]
fn query_names_ignore_case_and_the_first_occurrence_wins() {
    let simple_user_id = USER_ID.replace('-', "");
    let dto = PlaybackInfoDto::from_query_pairs(&pairs(&[
        ("maxstreamingbitrate", "3000000"),
        ("MaxStreamingBitrate", "fast"),
        ("STARTTIMETICKS", "90000000000"),
        ("audioStreamIndex", "2"),
        ("subtitleStreamIndex", "-1"),
        ("maxAudioChannels", "2"),
        ("mediaSourceId", "abc"),
        ("liveStreamId", "live"),
        ("autoOpenLiveStream", "TRUE"),
        ("enableDirectPlay", "False"),
        ("enableDirectStream", "true"),
        ("enableTranscoding", "false"),
        ("allowVideoStreamCopy", "false"),
        ("allowAudioStreamCopy", "true"),
        ("userId", simple_user_id.as_str()),
        ("api_key", "whatever"),
        ("DeviceProfile", "{}"),
        ("AlwaysBurnInSubtitleWhenTranscoding", "true"),
    ]))
    .unwrap();
    assert_eq!(
        dto,
        PlaybackInfoDto {
            user_id: Some(USER_ID.parse().unwrap()),
            max_streaming_bitrate: Some(3_000_000),
            start_time_ticks: Some(90_000_000_000),
            audio_stream_index: Some(2),
            subtitle_stream_index: Some(-1),
            max_audio_channels: Some(2),
            media_source_id: Some("abc".to_string()),
            live_stream_id: Some("live".to_string()),
            device_profile: None,
            enable_direct_play: Some(false),
            enable_direct_stream: Some(true),
            enable_transcoding: Some(false),
            allow_video_stream_copy: Some(false),
            allow_audio_stream_copy: Some(true),
            auto_open_live_stream: Some(true),
            always_burn_in_subtitle_when_transcoding: None,
        }
    );
}

#[test]
fn empty_query_values_are_absent() {
    let dto = PlaybackInfoDto::from_query_pairs(&pairs(&[
        ("MaxStreamingBitrate", ""),
        ("MaxStreamingBitrate", "5"),
        ("UserId", ""),
        ("MediaSourceId", ""),
        ("EnableDirectPlay", ""),
        ("StartTimeTicks", ""),
    ]))
    .unwrap();
    assert_eq!(dto, PlaybackInfoDto::default());
    assert_eq!(
        PlaybackInfoDto::from_query_pairs(&[]).unwrap(),
        PlaybackInfoDto::default()
    );
}

#[test]
fn bad_query_values_are_errors() {
    for (k, v) in [
        ("MaxStreamingBitrate", "fast"),
        ("MaxStreamingBitrate", "1.5"),
        ("MaxStreamingBitrate", "3000000000"),
        ("StartTimeTicks", "soon"),
        ("AudioStreamIndex", "x"),
        ("SubtitleStreamIndex", "1e3"),
        ("MaxAudioChannels", "two"),
        ("EnableDirectPlay", "1"),
        ("EnableTranscoding", "yes"),
        ("AllowAudioStreamCopy", "on"),
        ("UserId", "nope"),
    ] {
        assert!(
            PlaybackInfoDto::from_query_pairs(&pairs(&[(k, v)])).is_err(),
            "{k}={v}"
        );
    }
}

#[test]
fn query_values_win_over_the_body() {
    let query = PlaybackInfoDto {
        max_streaming_bitrate: Some(1_000_000),
        enable_direct_play: Some(false),
        media_source_id: Some("from-query".to_string()),
        ..Default::default()
    };
    let body = PlaybackInfoDto {
        max_streaming_bitrate: Some(8_000_000),
        enable_direct_play: Some(true),
        media_source_id: Some("from-body".to_string()),
        audio_stream_index: Some(3),
        device_profile: Some(DeviceProfile::default()),
        always_burn_in_subtitle_when_transcoding: Some(true),
        ..Default::default()
    };
    let merged = query.or_body(body);
    assert_eq!(merged.max_streaming_bitrate, Some(1_000_000));
    assert_eq!(merged.enable_direct_play, Some(false));
    assert_eq!(merged.media_source_id.as_deref(), Some("from-query"));
    assert_eq!(merged.audio_stream_index, Some(3));
    assert_eq!(merged.device_profile, Some(DeviceProfile::default()));
    assert_eq!(merged.always_burn_in_subtitle_when_transcoding, Some(true));
    assert_eq!(merged.enable_transcoding, None);
}

#[test]
fn responses_serialise_like_jellyfin() {
    let none = ItemsPlaybackInfoResponse {
        media_sources: vec![],
        play_session_id: None,
        error_code: Some(PlaybackErrorCode::NoCompatibleStream),
    };
    assert_eq!(
        serde_json::to_value(none).unwrap(),
        json!({"MediaSources": [], "ErrorCode": "NoCompatibleStream"})
    );
    let some = ItemsPlaybackInfoResponse {
        media_sources: vec![json!({"Id": "a"})],
        play_session_id: Some("0123abcd".to_string()),
        error_code: None,
    };
    assert_eq!(
        serde_json::to_value(some).unwrap(),
        json!({"MediaSources": [{"Id": "a"}], "PlaySessionId": "0123abcd"})
    );
    assert_eq!(
        serde_json::to_value(PlaybackErrorCode::NotAllowed).unwrap(),
        json!("NotAllowed")
    );
    assert_eq!(
        serde_json::to_value(PlaybackErrorCode::RateLimitExceeded).unwrap(),
        json!("RateLimitExceeded")
    );
}
