//! Tests for the stream builder's basic types, its facts about a media source and its profile
//! conditions, a port of Jellyfin's `ConditionProcessor` and `ContainerHelper`.

use rustyfin_core::stream_builder::conditions::{
    AudioFacts, VideoFacts, audio_condition_satisfied, contains_container,
    reason_for_failed_condition, split, video_audio_condition_satisfied, video_condition_satisfied,
};
use rustyfin_core::stream_builder::{
    MediaKind, PlayMethod, SourceFacts, StreamFacts, StreamKind, TranscodeReason, TranscodeReasons,
};
use rustyfin_model::device_profile::{
    ConditionProperty as P, ConditionType as C, ProfileCondition,
};
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};
use serde_json::json;

/// A condition that fails when the value is unknown.
fn req(condition: C, property: P, value: &str) -> ProfileCondition {
    ProfileCondition {
        condition,
        property,
        value: value.to_string(),
        is_required: true,
    }
}

/// A condition that holds when the value is unknown.
fn opt(condition: C, property: P, value: &str) -> ProfileCondition {
    ProfileCondition {
        is_required: false,
        ..req(condition, property, value)
    }
}

/// Whether a required video condition holds.
fn video_ok(condition: C, property: P, value: &str, facts: &VideoFacts) -> bool {
    video_condition_satisfied(&req(condition, property, value), facts)
}

fn bit_depth(depth: i32) -> VideoFacts {
    VideoFacts {
        bit_depth: Some(depth),
        ..VideoFacts::default()
    }
}

/// Video facts with a distinct value for every property.
fn all_video_facts() -> VideoFacts {
    VideoFacts {
        width: Some(1920),
        height: Some(1080),
        bit_depth: Some(10),
        bitrate: Some(8_000_000),
        profile: Some("High 10".to_string()),
        range_type: Some("HDR10".to_string()),
        level: Some(51.0),
        framerate: Some(24.0),
        packet_length: Some(188),
        is_anamorphic: Some(false),
        is_interlaced: Some(true),
        ref_frames: Some(4),
        num_streams: 3,
        num_video_streams: Some(1),
        num_audio_streams: Some(2),
        codec_tag: Some("hvc1".to_string()),
        is_avc: Some(true),
        rotation: Some(90),
    }
}

fn all_audio_facts() -> AudioFacts {
    AudioFacts {
        channels: Some(6),
        bitrate: Some(640_000),
        sample_rate: Some(48_000),
        bit_depth: Some(24),
        profile: Some("LC".to_string()),
        is_secondary: Some(true),
    }
}

// Containers (ContainerHelper)

#[test]
fn containers_match_any_entry_ignoring_case() {
    assert!(contains_container("mp4,m4v", "mp4"));
    assert!(contains_container("MP4,m4v", "m4v"));
    assert!(contains_container("webm", "mkv,webm"));
    assert!(!contains_container("mp4,m4v", "mkv"));
    assert!(contains_container("mp4,,ts", "ts"));
    assert!(!contains_container("mp4", " mp4"));
    assert!(!contains_container(",", ","));
}

#[test]
fn empty_and_negative_container_lists_follow_jellyfin() {
    // An empty profile allows everything, but an empty input is checked first.
    assert!(contains_container("", "mkv"));
    assert!(!contains_container("mp4", ""));
    assert!(!contains_container("", ""));
    // A leading '-' negates the list.
    assert!(contains_container("-mkv", "mp4"));
    assert!(!contains_container("-mkv,webm", "webm"));
    assert!(contains_container("-mkv", ""));
    assert!(contains_container("-", "mkv"));
}

#[test]
fn split_drops_empty_entries_without_trimming() {
    assert_eq!(split("mkv,mp4"), ["mkv", "mp4"]);
    assert_eq!(split("a,,b,"), ["a", "b"]);
    assert_eq!(split(" a , b"), [" a ", " b"]);
    assert!(split("").is_empty());
    assert!(split(",").is_empty());
}

// Value kinds (ConditionProcessor.IsConditionSatisfied overloads)

#[test]
fn integer_conditions_compare_parsed_values() {
    let ten = bit_depth(10);
    assert!(video_ok(C::Equals, P::VideoBitDepth, "10", &ten));
    assert!(!video_ok(C::Equals, P::VideoBitDepth, "8", &ten));
    assert!(video_ok(C::NotEquals, P::VideoBitDepth, "8", &ten));
    assert!(!video_ok(C::NotEquals, P::VideoBitDepth, "10", &ten));
    assert!(video_ok(C::LessThanEqual, P::VideoBitDepth, "10", &ten));
    assert!(!video_ok(C::LessThanEqual, P::VideoBitDepth, "8", &ten));
    assert!(video_ok(C::GreaterThanEqual, P::VideoBitDepth, "10", &ten));
    assert!(!video_ok(C::GreaterThanEqual, P::VideoBitDepth, "12", &ten));
    // EqualsAny splits on '|' and skips entries that don't parse.
    assert!(video_ok(C::EqualsAny, P::VideoBitDepth, "8|10", &ten));
    assert!(video_ok(C::EqualsAny, P::VideoBitDepth, "x|10", &ten));
    assert!(!video_ok(C::EqualsAny, P::VideoBitDepth, "8|12", &ten));
    assert!(!video_ok(C::EqualsAny, P::VideoBitDepth, "", &ten));
}

#[test]
fn a_value_that_does_not_parse_fails_the_condition() {
    let ten = bit_depth(10);
    for condition in [
        C::Equals,
        C::NotEquals,
        C::LessThanEqual,
        C::GreaterThanEqual,
    ] {
        for value in ["ten", "", "10.0", "99999999999"] {
            assert!(
                !video_ok(condition, P::VideoBitDepth, value, &ten),
                "{condition:?} {value:?}"
            );
        }
    }
}

#[test]
fn an_unknown_value_satisfies_only_optional_conditions() {
    let unknown = VideoFacts::default();
    let conditions = [
        C::Equals,
        C::NotEquals,
        C::LessThanEqual,
        C::GreaterThanEqual,
        C::EqualsAny,
        C::Unknown,
    ];
    for condition in conditions {
        for property in [
            P::VideoBitDepth,
            P::VideoLevel,
            P::VideoProfile,
            P::IsInterlaced,
        ] {
            let label = format!("{condition:?} {property:?}");
            assert!(
                !video_condition_satisfied(&req(condition, property, "nonsense"), &unknown),
                "{label}"
            );
            assert!(
                video_condition_satisfied(&opt(condition, property, "nonsense"), &unknown),
                "{label}"
            );
        }
    }
}

#[test]
fn condition_types_a_value_kind_does_not_handle_fail() {
    let facts = all_video_facts();
    let known = [
        (P::VideoBitDepth, "10"),
        (P::VideoLevel, "51"),
        (P::VideoProfile, "high 10"),
        (P::IsInterlaced, "true"),
        (P::VideoRangeType, "HDR10"),
    ];
    for (property, value) in known {
        assert!(
            !video_ok(C::Unknown, property, value, &facts),
            "{property:?}"
        );
    }
    // Strings, bools and range types have no order, and bools have no EqualsAny.
    for (property, value) in [
        (P::VideoProfile, "high 10"),
        (P::IsInterlaced, "true"),
        (P::VideoRangeType, "HDR10"),
    ] {
        assert!(
            !video_ok(C::LessThanEqual, property, value, &facts),
            "{property:?}"
        );
        assert!(
            !video_ok(C::GreaterThanEqual, property, value, &facts),
            "{property:?}"
        );
    }
    assert!(!video_ok(C::EqualsAny, P::IsInterlaced, "true", &facts));
}

#[test]
fn levels_and_frame_rates_compare_as_doubles() {
    let facts = VideoFacts {
        level: Some(51.0),
        framerate: Some(24.0),
        ..VideoFacts::default()
    };
    assert!(video_ok(C::LessThanEqual, P::VideoLevel, "52", &facts));
    assert!(!video_ok(C::LessThanEqual, P::VideoLevel, "50.9", &facts));
    assert!(video_ok(C::Equals, P::VideoLevel, "51", &facts));
    assert!(video_ok(C::EqualsAny, P::VideoLevel, "40|51.0", &facts));
    assert!(video_ok(
        C::GreaterThanEqual,
        P::VideoFramerate,
        "23.976",
        &facts
    ));
    assert!(video_ok(
        C::EqualsAny,
        P::VideoFramerate,
        "23.976|24",
        &facts
    ));
    assert!(!video_ok(C::Equals, P::VideoFramerate, "25", &facts));
}

#[test]
fn a_frame_rate_widens_from_f32_before_comparing() {
    // 23.976f32 is 23.97599983..., just below the double 23.976, as in Jellyfin.
    let facts = VideoFacts {
        framerate: Some(23.976),
        ..VideoFacts::default()
    };
    assert!(!video_ok(C::Equals, P::VideoFramerate, "23.976", &facts));
    assert!(video_ok(
        C::LessThanEqual,
        P::VideoFramerate,
        "23.976",
        &facts
    ));
    assert!(!video_ok(
        C::GreaterThanEqual,
        P::VideoFramerate,
        "23.976",
        &facts
    ));
}

#[test]
fn values_parse_as_dotnet_does() {
    let facts = VideoFacts {
        bit_depth: Some(10),
        level: Some(1000.0),
        is_interlaced: Some(true),
        ..VideoFacts::default()
    };
    // Integers (NumberStyles.Integer) allow surrounding white space and a sign.
    assert!(video_ok(C::Equals, P::VideoBitDepth, " 10 ", &facts));
    assert!(video_ok(C::Equals, P::VideoBitDepth, "+10", &facts));
    assert!(video_ok(C::EqualsAny, P::VideoBitDepth, "8 | 10", &facts));
    assert!(video_ok(
        C::GreaterThanEqual,
        P::VideoBitDepth,
        "-1",
        &facts
    ));
    // Doubles (Float | AllowThousands) also allow a decimal point, an exponent and ',' between
    // digits of the integer part.
    assert!(video_ok(C::Equals, P::VideoLevel, "1e3", &facts));
    assert!(video_ok(C::Equals, P::VideoLevel, "1,000", &facts));
    assert!(video_ok(C::Equals, P::VideoLevel, " 1000.0 ", &facts));
    // Bools (bool.TryParse) ignore case and surrounding white space, and nothing else parses.
    assert!(video_ok(C::Equals, P::IsInterlaced, "TRUE", &facts));
    assert!(video_ok(C::Equals, P::IsInterlaced, " true ", &facts));
    assert!(!video_ok(C::Equals, P::IsInterlaced, "1", &facts));
    assert!(!video_ok(C::NotEquals, P::IsInterlaced, "yes", &facts));
}

#[test]
fn string_conditions_ignore_case_but_not_spacing() {
    let facts = all_video_facts();
    let chrome_h264 = "high|main|baseline|constrained baseline";
    assert!(!video_ok(
        C::EqualsAny,
        P::VideoProfile,
        chrome_h264,
        &facts
    ));
    assert!(video_ok(
        C::EqualsAny,
        P::VideoProfile,
        "high|main|high 10",
        &facts
    ));
    assert!(!video_ok(
        C::EqualsAny,
        P::VideoProfile,
        "high| high 10",
        &facts
    ));
    assert!(video_ok(C::Equals, P::VideoProfile, "HIGH 10", &facts));
    assert!(video_ok(C::NotEquals, P::VideoProfile, "high", &facts));
    assert!(!video_ok(C::NotEquals, P::VideoProfile, "high 10", &facts));
    assert!(video_ok(
        C::EqualsAny,
        P::VideoCodecTag,
        "avc1|HVC1",
        &facts
    ));
    assert!(!video_ok(C::Equals, P::VideoCodecTag, "hev1", &facts));

    let empty = VideoFacts {
        profile: Some(String::new()),
        ..VideoFacts::default()
    };
    assert!(!video_ok(C::Equals, P::VideoProfile, "high", &empty));
    assert!(video_condition_satisfied(
        &opt(C::Equals, P::VideoProfile, "high"),
        &empty
    ));
}

#[test]
fn range_types_match_names_ignoring_case() {
    let hdr10 = all_video_facts();
    assert!(video_ok(
        C::EqualsAny,
        P::VideoRangeType,
        "SDR|HDR10|HLG",
        &hdr10
    ));
    assert!(video_ok(
        C::EqualsAny,
        P::VideoRangeType,
        "sdr|hdr10",
        &hdr10
    ));
    assert!(!video_ok(C::EqualsAny, P::VideoRangeType, "SDR", &hdr10));
    assert!(video_ok(C::Equals, P::VideoRangeType, "hdr10", &hdr10));
    assert!(video_ok(C::NotEquals, P::VideoRangeType, "SDR", &hdr10));
    assert!(!video_ok(C::NotEquals, P::VideoRangeType, "HDR10", &hdr10));

    let dovi = VideoFacts {
        range_type: Some("DOVI".to_string()),
        ..VideoFacts::default()
    };
    assert!(!video_ok(
        C::EqualsAny,
        P::VideoRangeType,
        "SDR|HDR10|HLG",
        &dovi
    ));

    for unknown in [None, Some(""), Some("Unknown")] {
        let facts = VideoFacts {
            range_type: unknown.map(str::to_string),
            ..VideoFacts::default()
        };
        assert!(
            !video_ok(C::EqualsAny, P::VideoRangeType, "SDR", &facts),
            "{unknown:?}"
        );
        let optional = opt(C::EqualsAny, P::VideoRangeType, "SDR");
        assert!(video_condition_satisfied(&optional, &facts), "{unknown:?}");
    }
}

// Processors (IsVideoConditionSatisfied, IsVideoAudioConditionSatisfied, IsAudioConditionSatisfied)

#[test]
fn each_video_property_reads_its_own_fact() {
    let facts = all_video_facts();
    let cases = [
        (P::Width, "1920"),
        (P::Height, "1080"),
        (P::VideoBitDepth, "10"),
        (P::VideoBitrate, "8000000"),
        (P::VideoProfile, "high 10"),
        (P::VideoRangeType, "HDR10"),
        (P::VideoLevel, "51"),
        (P::VideoFramerate, "24"),
        (P::PacketLength, "188"),
        (P::RefFrames, "4"),
        (P::NumStreams, "3"),
        (P::NumVideoStreams, "1"),
        (P::NumAudioStreams, "2"),
        (P::VideoCodecTag, "HVC1"),
        (P::VideoRotation, "90"),
    ];
    for (property, value) in cases {
        assert!(
            video_ok(C::Equals, property, value, &facts),
            "{property:?} = {value}"
        );
        assert!(
            !video_ok(C::NotEquals, property, value, &facts),
            "{property:?} != {value}"
        );
    }

    // Three different states, so no two bool properties can be mixed up.
    let flags = VideoFacts {
        is_anamorphic: Some(true),
        is_interlaced: None,
        is_avc: Some(false),
        ..VideoFacts::default()
    };
    assert!(video_ok(C::Equals, P::IsAnamorphic, "true", &flags));
    assert!(!video_ok(C::Equals, P::IsInterlaced, "false", &flags));
    assert!(!video_ok(C::Equals, P::IsInterlaced, "true", &flags));
    assert!(video_ok(C::Equals, P::IsAvc, "false", &flags));
}

#[test]
fn the_video_timestamp_is_always_unknown() {
    let facts = all_video_facts();
    assert!(!video_ok(C::Equals, P::VideoTimestamp, "None", &facts));
    let optional = opt(C::NotEquals, P::VideoTimestamp, "Valid");
    assert!(video_condition_satisfied(&optional, &facts));
}

#[test]
fn each_audio_property_reads_its_own_fact() {
    let facts = all_audio_facts();
    let checks: [fn(&ProfileCondition, &AudioFacts) -> bool; 2] =
        [audio_condition_satisfied, video_audio_condition_satisfied];
    let numbers = [
        (P::AudioChannels, "6"),
        (P::AudioBitrate, "640000"),
        (P::AudioSampleRate, "48000"),
        (P::AudioBitDepth, "24"),
    ];
    for check in checks {
        for (property, value) in numbers {
            assert!(
                check(&req(C::Equals, property, value), &facts),
                "{property:?} = {value}"
            );
            assert!(
                !check(&req(C::NotEquals, property, value), &facts),
                "{property:?} != {value}"
            );
        }
        assert!(check(&req(C::LessThanEqual, P::AudioChannels, "6"), &facts));
        assert!(!check(
            &req(C::LessThanEqual, P::AudioChannels, "2"),
            &facts
        ));
    }

    let profile =
        |value| video_audio_condition_satisfied(&req(C::Equals, P::AudioProfile, value), &facts);
    assert!(profile("lc"));
    assert!(!profile("he-aac"));
    let secondary = |value| {
        video_audio_condition_satisfied(&req(C::Equals, P::IsSecondaryAudio, value), &facts)
    };
    assert!(secondary("true"));
    assert!(!secondary("false"));
}

#[test]
fn properties_a_processor_does_not_handle_pass() {
    // Jellyfin's video processor returns true here; its audio processors throw, which we treat
    // as true too.
    let video = VideoFacts::default();
    let not_video = [
        P::AudioChannels,
        P::AudioBitrate,
        P::AudioProfile,
        P::Has64BitOffsets,
        P::IsSecondaryAudio,
        P::AudioSampleRate,
        P::AudioBitDepth,
        P::Unknown,
    ];
    for property in not_video {
        assert!(
            video_ok(C::Equals, property, "nonsense", &video),
            "{property:?}"
        );
    }

    let audio = AudioFacts::default();
    let not_audio = [
        P::Width,
        P::VideoProfile,
        P::IsInterlaced,
        P::Has64BitOffsets,
        P::Unknown,
    ];
    for property in not_audio {
        let condition = req(C::Equals, property, "nonsense");
        assert!(
            video_audio_condition_satisfied(&condition, &audio),
            "{property:?}"
        );
        assert!(
            audio_condition_satisfied(&condition, &audio),
            "{property:?}"
        );
    }

    // Only the video-audio processor reads the audio profile and the secondary flag.
    for property in [P::AudioProfile, P::IsSecondaryAudio] {
        let condition = req(C::Equals, property, "nonsense");
        assert!(
            audio_condition_satisfied(&condition, &audio),
            "{property:?}"
        );
        assert!(
            !video_audio_condition_satisfied(&condition, &audio),
            "{property:?}"
        );
    }
}

// Transcode reasons

#[test]
fn failed_conditions_map_to_jellyfins_reasons() {
    use TranscodeReason as R;
    let expected = [
        (P::AudioChannels, Some(R::AudioChannelsNotSupported)),
        (P::AudioBitrate, Some(R::AudioBitrateNotSupported)),
        (P::AudioProfile, Some(R::AudioProfileNotSupported)),
        (P::Width, Some(R::VideoResolutionNotSupported)),
        (P::Height, Some(R::VideoResolutionNotSupported)),
        (P::Has64BitOffsets, None),
        (P::PacketLength, None),
        (P::VideoBitDepth, Some(R::VideoBitDepthNotSupported)),
        (P::VideoBitrate, Some(R::VideoBitrateNotSupported)),
        (P::VideoFramerate, Some(R::VideoFramerateNotSupported)),
        (P::VideoLevel, Some(R::VideoLevelNotSupported)),
        (P::VideoProfile, Some(R::VideoProfileNotSupported)),
        (P::VideoTimestamp, None),
        (P::IsAnamorphic, Some(R::AnamorphicVideoNotSupported)),
        (P::RefFrames, Some(R::RefFramesNotSupported)),
        (P::NumAudioStreams, None),
        (P::NumVideoStreams, None),
        (P::IsSecondaryAudio, Some(R::SecondaryAudioNotSupported)),
        (P::VideoCodecTag, Some(R::VideoCodecTagNotSupported)),
        (P::IsAvc, None),
        (P::IsInterlaced, Some(R::InterlacedVideoNotSupported)),
        (P::AudioSampleRate, Some(R::AudioSampleRateNotSupported)),
        (P::AudioBitDepth, Some(R::AudioBitDepthNotSupported)),
        (P::VideoRangeType, Some(R::VideoRangeTypeNotSupported)),
        (P::NumStreams, Some(R::StreamCountExceedsLimit)),
        (P::VideoRotation, Some(R::VideoRotationNotSupported)),
        (P::Unknown, None),
    ];
    for (property, reason) in expected {
        let want = reason.map(TranscodeReasons::from).unwrap_or_default();
        let got = reason_for_failed_condition(&req(C::LessThanEqual, property, "1"));
        assert_eq!(got, want, "{property:?}");
    }
}

#[test]
fn transcode_reasons_use_jellyfins_bits_and_names() {
    let names = [
        "ContainerNotSupported",
        "VideoCodecNotSupported",
        "AudioCodecNotSupported",
        "SubtitleCodecNotSupported",
        "AudioIsExternal",
        "SecondaryAudioNotSupported",
        "VideoProfileNotSupported",
        "VideoLevelNotSupported",
        "VideoResolutionNotSupported",
        "VideoBitDepthNotSupported",
        "VideoFramerateNotSupported",
        "RefFramesNotSupported",
        "AnamorphicVideoNotSupported",
        "InterlacedVideoNotSupported",
        "AudioChannelsNotSupported",
        "AudioProfileNotSupported",
        "AudioSampleRateNotSupported",
        "AudioBitDepthNotSupported",
        "ContainerBitrateExceedsLimit",
        "VideoBitrateNotSupported",
        "AudioBitrateNotSupported",
        "UnknownVideoStreamInfo",
        "UnknownAudioStreamInfo",
        "DirectPlayError",
        "VideoRangeTypeNotSupported",
        "VideoCodecTagNotSupported",
        "StreamCountExceedsLimit",
        "VideoRotationNotSupported",
    ];
    assert_eq!(TranscodeReason::ALL.len(), names.len());
    for (n, (reason, name)) in TranscodeReason::ALL.iter().zip(names).enumerate() {
        assert_eq!(reason.as_str(), name);
        assert_eq!(reason.bit(), 1u32 << n, "{name}");
        assert_eq!(TranscodeReasons::from(*reason).names(), [name]);
    }
}

#[test]
fn reason_sets_combine_and_list_names_in_bit_order() {
    use TranscodeReason as R;
    let mut set = TranscodeReasons::default();
    assert!(set.is_empty());
    assert!(set.names().is_empty());

    set.insert(R::VideoRangeTypeNotSupported);
    set.insert(R::VideoRangeTypeNotSupported);
    set |= R::ContainerNotSupported;
    set |= TranscodeReasons::from(R::AudioCodecNotSupported);
    assert!(!set.is_empty());
    assert_eq!(set.0, 1 | (1 << 2) | (1 << 24));
    assert_eq!(
        set.names(),
        [
            "ContainerNotSupported",
            "AudioCodecNotSupported",
            "VideoRangeTypeNotSupported"
        ]
    );
    assert!(set.contains(R::AudioCodecNotSupported));
    assert!(!set.contains(R::VideoProfileNotSupported));

    let other = TranscodeReasons::from(R::DirectPlayError) | R::ContainerNotSupported;
    assert!(set.intersects(other));
    assert!(!set.intersects(TranscodeReasons::from(R::DirectPlayError)));
    assert!(!set.intersects(TranscodeReasons::default()));

    let joined = TranscodeReasons::from(R::VideoBitDepthNotSupported)
        | TranscodeReasons::from(R::VideoProfileNotSupported);
    assert_eq!(
        joined.names(),
        ["VideoProfileNotSupported", "VideoBitDepthNotSupported"]
    );
}

#[test]
fn play_methods_order_as_jellyfin_numbers_them() {
    assert!(PlayMethod::Transcode < PlayMethod::DirectStream);
    assert!(PlayMethod::DirectStream < PlayMethod::DirectPlay);
    assert_eq!(PlayMethod::default(), PlayMethod::Transcode);
    assert_eq!(MediaKind::default(), MediaKind::Video);
    assert_eq!(StreamKind::default(), StreamKind::Other);
}

// Source facts

/// A source as the media source repository returns it, with the given container and streams.
fn media_source(container: Option<&str>, streams: Vec<MediaStream>) -> MediaSourceResponse {
    let mut source: MediaSourceResponse = serde_json::from_value(json!({
        "Id": "src1",
        "Uid": "src1",
        "Protocol": "File",
        "Path": "/media/Film (2020)/Film.mkv",
        "Bitrate": 10_000_000,
        "SupportsTranscoding": true,
        "SupportsDirectStream": true,
        "SupportsDirectPlay": true
    }))
    .expect("a media source");
    source.container = container.map(str::to_string);
    source.media_streams = streams;
    source
}

fn video_stream() -> MediaStream {
    MediaStream {
        index: 0,
        codec: Some("H264".to_string()),
        codec_type: "Video".to_string(),
        codec_tag: Some("avc1".to_string()),
        width: Some(1920),
        height: Some(1080),
        average_frame_rate: Some("24000/1001".to_string()),
        real_frame_rate: Some("24000/1001".to_string()),
        bit_rate: Some(9_000_000),
        bit_depth: Some(10),
        profile: Some("High 10".to_string()),
        color_transfer: Some("bt709".to_string()),
        is_default: true,
        ..MediaStream::default()
    }
}

fn subtitle_stream(index: i32, external: bool, forced: bool, default: bool) -> MediaStream {
    MediaStream {
        index,
        codec: Some("subrip".to_string()),
        codec_type: "Subtitle".to_string(),
        is_external: external,
        is_forced: forced,
        is_default: default,
        ..MediaStream::default()
    }
}

#[test]
fn source_facts_copy_the_source_and_its_streams() {
    let audio = MediaStream {
        index: 1,
        codec: Some("aac".to_string()),
        codec_type: "Audio".to_string(),
        channels: Some(2),
        sample_rate: Some(48_000),
        bit_rate: Some(128_000),
        profile: Some("LC".to_string()),
        language: Some("eng".to_string()),
        is_default: true,
        ..MediaStream::default()
    };
    let subtitle = MediaStream {
        index: 2,
        codec: Some("ASS".to_string()),
        codec_type: "Subtitle".to_string(),
        language: Some("eng".to_string()),
        is_external: true,
        is_forced: true,
        path: Some("/media/Film (2020)/Film.en.ass".to_string()),
        ..MediaStream::default()
    };
    let source = media_source(Some("MKV"), vec![video_stream(), audio, subtitle]);
    let facts = SourceFacts::from(&source);

    assert_eq!(facts.id, "src1");
    assert_eq!(facts.container, "mkv");
    assert_eq!(facts.bitrate, Some(10_000_000));
    assert!(!facts.is_remote);
    assert!(facts.supports_direct_play);
    assert!(facts.supports_direct_stream);
    assert!(facts.supports_transcoding);
    assert_eq!(facts.default_audio_stream_index, None);
    assert_eq!(facts.default_subtitle_stream_index, Some(2));
    assert_eq!(
        facts.streams,
        [
            StreamFacts {
                index: 0,
                kind: StreamKind::Video,
                codec: "h264".to_string(),
                profile: Some("High 10".to_string()),
                level: None,
                codec_tag: Some("avc1".to_string()),
                width: Some(1920),
                height: Some(1080),
                bit_depth: Some(10),
                bit_rate: Some(9_000_000),
                frame_rate: Some(24000.0 / 1001.0),
                range_type: Some("SDR".to_string()),
                is_interlaced: Some(false),
                is_default: true,
                ..StreamFacts::default()
            },
            StreamFacts {
                index: 1,
                kind: StreamKind::Audio,
                codec: "aac".to_string(),
                profile: Some("LC".to_string()),
                bit_rate: Some(128_000),
                channels: Some(2),
                sample_rate: Some(48_000),
                language: Some("eng".to_string()),
                is_default: true,
                ..StreamFacts::default()
            },
            StreamFacts {
                index: 2,
                kind: StreamKind::Subtitle,
                codec: "ass".to_string(),
                language: Some("eng".to_string()),
                is_external: true,
                is_forced: true,
                path: Some("/media/Film (2020)/Film.en.ass".to_string()),
                ..StreamFacts::default()
            },
        ]
    );
}

#[test]
fn missing_values_and_cleared_flags_carry_over() {
    let data = MediaStream {
        index: 0,
        codec_type: "Data".to_string(),
        ..MediaStream::default()
    };
    let mut source = media_source(None, vec![data]);
    source.bitrate = None;
    source.is_remote = true;
    source.supports_direct_play = false;
    source.supports_transcoding = false;
    let facts = SourceFacts::from(&source);

    assert_eq!(facts.container, "");
    assert_eq!(facts.bitrate, None);
    assert!(facts.is_remote);
    assert!(!facts.supports_direct_play);
    assert!(facts.supports_direct_stream);
    assert!(!facts.supports_transcoding);
    assert_eq!(facts.streams[0].kind, StreamKind::Other);
    assert_eq!(facts.streams[0].codec, "");
    assert_eq!(facts.streams[0].is_interlaced, None);
}

#[test]
fn the_default_source_facts_allow_every_method() {
    let facts = SourceFacts::default();
    assert!(facts.supports_direct_play);
    assert!(facts.supports_direct_stream);
    assert!(facts.supports_transcoding);
    assert!(facts.id.is_empty());
    assert!(facts.container.is_empty());
    assert!(facts.streams.is_empty());
    assert_eq!(facts.bitrate, None);
}

#[test]
fn stream_kinds_come_from_the_codec_type_ignoring_case() {
    let streams = [
        "Video",
        "AUDIO",
        "subtitle",
        "Attachment",
        "EmbeddedImage",
        "",
    ]
    .into_iter()
    .zip(0..)
    .map(|(codec_type, index)| MediaStream {
        index,
        codec_type: codec_type.to_string(),
        ..MediaStream::default()
    })
    .collect();
    let facts = SourceFacts::from(&media_source(Some("mkv"), streams));
    let kinds: Vec<_> = facts.streams.iter().map(|s| (s.index, s.kind)).collect();
    assert_eq!(
        kinds,
        [
            (0, StreamKind::Video),
            (1, StreamKind::Audio),
            (2, StreamKind::Subtitle),
            (3, StreamKind::Other),
            (4, StreamKind::Other),
            (5, StreamKind::Other),
        ]
    );
}

#[test]
fn numbers_too_big_for_i32_are_unknown() {
    let video = MediaStream {
        width: Some(u32::MAX),
        height: Some(2160),
        bit_rate: Some(3_000_000_000),
        ..video_stream()
    };
    let audio = MediaStream {
        index: 1,
        codec_type: "Audio".to_string(),
        channels: Some(u32::MAX),
        sample_rate: Some(48_000),
        ..MediaStream::default()
    };
    let mut source = media_source(Some("mkv"), vec![video, audio]);
    source.bitrate = Some(4_000_000_000);
    let facts = SourceFacts::from(&source);

    assert_eq!(facts.bitrate, Some(4_000_000_000));
    let video = &facts.streams[0];
    assert_eq!(
        (video.width, video.height, video.bit_rate, video.bit_depth),
        (None, Some(2160), None, Some(10))
    );
    let audio = &facts.streams[1];
    assert_eq!((audio.channels, audio.sample_rate), (None, Some(48_000)));
}

#[test]
fn frame_rates_prefer_a_realistic_average() {
    let rate = |average: Option<&str>, real: Option<&str>| {
        let stream = MediaStream {
            average_frame_rate: average.map(str::to_string),
            real_frame_rate: real.map(str::to_string),
            ..video_stream()
        };
        SourceFacts::from(&media_source(Some("mkv"), vec![stream])).streams[0].frame_rate
    };
    assert_eq!(
        rate(Some("24000/1001"), Some("24/1")),
        Some(24000.0 / 1001.0)
    );
    assert_eq!(rate(Some("25/1"), None), Some(25.0));
    assert_eq!(rate(Some("23.976"), None), Some(23.976));
    // An average of 1000 or more is unrealistic, and a zero divisor doesn't parse.
    assert_eq!(rate(Some("1000/1"), Some("24/1")), Some(24.0));
    assert_eq!(rate(Some("0/0"), Some("25/1")), Some(25.0));
    assert_eq!(rate(None, Some("30000/1001")), Some(30000.0 / 1001.0));
    // Once the average is out, the real rate decides even when it is missing or large.
    assert_eq!(rate(Some("2000/1"), None), None);
    assert_eq!(rate(Some("2000/1"), Some("2000/1")), Some(2000.0));
    assert_eq!(rate(Some("abc"), Some("x/y")), None);
    assert_eq!(rate(None, Some("24/0")), None);
}

#[test]
fn range_types_come_from_the_codec_tag_and_transfer() {
    let range = |tag: Option<&str>, transfer: Option<&str>| {
        let stream = MediaStream {
            codec_tag: tag.map(str::to_string),
            color_transfer: transfer.map(str::to_string),
            ..video_stream()
        };
        SourceFacts::from(&media_source(Some("mkv"), vec![stream])).streams[0]
            .range_type
            .clone()
    };
    assert_eq!(range(None, None).as_deref(), Some("SDR"));
    assert_eq!(range(Some("avc1"), Some("bt709")).as_deref(), Some("SDR"));
    assert_eq!(range(None, Some("smpte2084")).as_deref(), Some("HDR10"));
    assert_eq!(
        range(Some("hvc1"), Some("arib-std-b67")).as_deref(),
        Some("HLG")
    );
    for tag in ["dvh1", "DVHE", "dav1", "dva1", "dvav"] {
        assert_eq!(
            range(Some(tag), Some("smpte2084")).as_deref(),
            Some("DOVI"),
            "{tag}"
        );
    }
}

#[test]
fn the_default_subtitle_is_external_then_forced_then_default() {
    let pick = |streams: Vec<MediaStream>| {
        SourceFacts::from(&media_source(Some("mkv"), streams)).default_subtitle_stream_index
    };
    // A default video stream doesn't count, and plain subtitles are never picked.
    let plain = vec![
        video_stream(),
        subtitle_stream(1, false, false, false),
        subtitle_stream(2, false, false, false),
    ];
    assert_eq!(pick(plain), None);
    let forced_over_default = vec![
        subtitle_stream(1, false, false, true),
        subtitle_stream(2, false, true, false),
    ];
    assert_eq!(pick(forced_over_default), Some(2));
    let external_first = vec![
        subtitle_stream(1, false, true, true),
        subtitle_stream(2, true, false, false),
    ];
    assert_eq!(pick(external_first), Some(2));
    let forced_and_default_over_forced = vec![
        subtitle_stream(1, false, true, false),
        subtitle_stream(2, false, true, true),
    ];
    assert_eq!(pick(forced_and_default_over_forced), Some(2));
    let lowest_index_on_a_tie = vec![
        subtitle_stream(1, false, false, true),
        subtitle_stream(2, false, false, true),
    ];
    assert_eq!(pick(lowest_index_on_a_tie), Some(1));
}
