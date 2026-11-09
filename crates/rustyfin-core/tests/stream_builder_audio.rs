//! Tests for the stream builder's audio decision, a port of Jellyfin's
//! `StreamBuilder.GetOptimalAudioStream` and `GetAudioDirectPlayProfile`, and of the device data
//! that `MediaInfoHelper.SetDeviceSpecificData` makes of it for an audio item.

use rustyfin_core::stream_builder::{
    DeviceData, MediaKind, PlayMethod, SourceFacts, StreamFacts, StreamKind, StreamRequest,
    TranscodeReason as R, TranscodeReasons, device_data,
};
use rustyfin_model::device_profile::{DeviceProfile, StreamProtocol};
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

/// A device profile written as JSON.
fn custom(value: serde_json::Value) -> DeviceProfile {
    DeviceProfile::from_json(&value).unwrap()
}

/// An audio request from device `dev1` in play session `ps1` with token `tok`.
fn request() -> StreamRequest {
    StreamRequest {
        item_id: ITEM.to_string(),
        media_kind: MediaKind::Audio,
        device_id: "dev1".to_string(),
        play_session_id: "ps1".to_string(),
        access_token: "tok".to_string(),
        ..StreamRequest::default()
    }
}

/// A default 16-bit, 44.1 kHz audio stream at index 0.
fn audio(codec: &str, channels: i32, bit_rate: i32) -> StreamFacts {
    StreamFacts {
        index: 0,
        kind: StreamKind::Audio,
        codec: codec.to_string(),
        channels: Some(channels),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        bit_rate: Some(bit_rate),
        is_default: true,
        ..StreamFacts::default()
    }
}

/// An embedded cover picture at `index`, which the prober reports as an MJPEG video stream.
fn cover(index: i32) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Video,
        codec: "mjpeg".to_string(),
        width: Some(600),
        height: Some(600),
        ..StreamFacts::default()
    }
}

/// An English SubRip subtitle stream at `index`.
fn subtitle(index: i32) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Subtitle,
        codec: "subrip".to_string(),
        language: Some("eng".to_string()),
        ..StreamFacts::default()
    }
}

/// A local source `src1` in `container` at `bitrate` that allows every playback method. Like the
/// source of a Jellyfin audio item, its default audio stream is its first audio stream.
fn source(container: &str, bitrate: Option<i64>, streams: Vec<StreamFacts>) -> SourceFacts {
    let default_audio_stream_index = streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Audio)
        .map(|stream| stream.index);
    SourceFacts {
        id: "src1".to_string(),
        container: container.to_string(),
        bitrate,
        streams,
        default_audio_stream_index,
        ..SourceFacts::default()
    }
}

/// A source that holds only `stream`, at the stream's bitrate.
fn track(container: &str, stream: StreamFacts) -> SourceFacts {
    let bitrate = stream.bit_rate.map(i64::from);
    source(container, bitrate, vec![stream])
}

/// A set of transcode reasons.
fn reasons(list: &[R]) -> TranscodeReasons {
    list.iter()
        .fold(TranscodeReasons::default(), |set, reason| set | *reason)
}

/// The reasons when no audio direct play profile supports the container. Jellyfin reports the
/// video codec too, because its check starts from "unsupported" (SB 503-550).
fn unsupported() -> TranscodeReasons {
    reasons(&[
        R::ContainerNotSupported,
        R::VideoCodecNotSupported,
        R::AudioCodecNotSupported,
    ])
}

/// A direct play of a file in `container`. `supports_transcoding` says whether the profile has an
/// HLS audio transcoding profile for streaming.
fn direct(container: &str, supports_transcoding: bool) -> DeviceData {
    DeviceData {
        supports_direct_play: true,
        supports_direct_stream: true,
        supports_transcoding,
        play_method: Some(PlayMethod::DirectPlay),
        container: container.to_string(),
        transcoding_url: None,
        transcoding_sub_protocol: StreamProtocol::Http,
        transcoding_container: None,
        transcode_reasons: TranscodeReasons::default(),
        default_audio_stream_index: None,
        default_subtitle_stream_index: None,
        subtitles: Vec::new(),
    }
}

/// An HLS transcode of a file in `container` for `why`, into MPEG-TS segments, at `url`.
fn transcode(container: &str, why: TranscodeReasons, url: String) -> DeviceData {
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
        default_audio_stream_index: None,
        default_subtitle_stream_index: None,
        subtitles: Vec::new(),
    }
}

/// A plan that doesn't play a file in `container`: the flags (direct play, direct stream,
/// transcoding) and the reasons `why`, but no method and no URL.
fn no_play(container: &str, flags: (bool, bool, bool), why: TranscodeReasons) -> DeviceData {
    DeviceData {
        supports_direct_play: flags.0,
        supports_direct_stream: flags.1,
        supports_transcoding: flags.2,
        play_method: None,
        transcoding_url: None,
        transcode_reasons: why,
        ..direct(container, false)
    }
}

/// No plan at all, as when `GetOptimalAudioStream` returns null: MIH leaves `source` as its step 2
/// made it, with `flags` (direct play, direct stream, transcoding) and the source's own default
/// streams.
fn untouched(source: &SourceFacts, flags: (bool, bool, bool)) -> DeviceData {
    DeviceData {
        default_audio_stream_index: source.default_audio_stream_index,
        default_subtitle_stream_index: source.default_subtitle_stream_index,
        ..no_play(&source.container, flags, TranscodeReasons::default())
    }
}

/// The URL of an audio transcode for `dev1`, source `src1`, session `ps1` and token `tok`. `media`
/// holds the parameters between MediaSourceId and PlaySessionId, `rest` those after `api_key`.
/// `AudioOnly=true` comes last.
fn audio_url(media: &[&str], rest: &[&str]) -> String {
    let mut params = vec!["DeviceId=dev1", "MediaSourceId=src1"];
    params.extend_from_slice(media);
    params.extend_from_slice(&["PlaySessionId=ps1", "api_key=tok"]);
    params.extend_from_slice(rest);
    params.push("AudioOnly=true");
    format!("/Videos/{ITEM}/main.m3u8?{}", params.join("&"))
}

/// chrome-linux's transcode to AAC at `kbps` in MPEG-TS segments, for `why`.
fn chrome_aac_url(kbps: u32, why: &str) -> String {
    let bitrate = format!("AudioBitrate={kbps}");
    let why = format!("TranscodeReasons={why}");
    audio_url(
        &[
            "AudioCodec=aac",
            bitrate.as_str(),
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            why.as_str(),
        ],
    )
}

/// A player that direct plays only MP3 and transcodes to AAC over HLS, with the given streaming
/// and music bitrates (JSON `null` clears a default).
fn mp3_player(max: serde_json::Value, music: serde_json::Value) -> DeviceProfile {
    custom(json!({
        "MaxStreamingBitrate": max,
        "MusicStreamingTranscodingBitrate": music,
        "DirectPlayProfiles": [{"Container": "mp3", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Streaming"}
        ]
    }))
}

/// A player that direct plays only AAC in M4A and transcodes to AAC over HLS, with
/// `max_audio_channels` on the transcoding profile. Its Audio codec profiles for AAC are one that
/// always applies, one whose ApplyConditions need stereo, a VideoAudio one and one for MP4 only.
fn aac_player(max_audio_channels: Option<&str>) -> DeviceProfile {
    let mut tp = json!({"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
                        "Context": "Streaming"});
    if let Some(channels) = max_audio_channels {
        tp["MaxAudioChannels"] = json!(channels);
    }
    custom(json!({
        "MaxStreamingBitrate": 120_000_000,
        "MusicStreamingTranscodingBitrate": 384_000,
        "DirectPlayProfiles": [{"Container": "m4a", "AudioCodec": "aac", "Type": "Audio"}],
        "TranscodingProfiles": [tp],
        "CodecProfiles": [
            {"Type": "Audio", "Codec": "aac", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "192000"},
                {"Condition": "LessThanEqual", "Property": "AudioSampleRate", "Value": "44100"},
                {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "6"}
            ]},
            {"Type": "Audio", "Codec": "aac",
             "ApplyConditions": [
                {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "2"}
             ],
             "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "64000"}
             ]},
            {"Type": "VideoAudio", "Codec": "aac", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "96000"}
            ]},
            {"Type": "Audio", "Codec": "aac", "Container": "mp4", "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "32000"}
            ]}
        ]
    }))
}

/// A player that direct plays FLAC and transcodes to stereo AAC over HLS, with an Audio codec
/// profile that limits FLAC in `container` to stereo.
fn flac_player(container: &str) -> DeviceProfile {
    custom(json!({
        "MaxStreamingBitrate": 120_000_000,
        "MusicStreamingTranscodingBitrate": 384_000,
        "DirectPlayProfiles": [{"Container": "flac", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Streaming", "MaxAudioChannels": "2"}
        ],
        "CodecProfiles": [
            {"Type": "Audio", "Codec": "flac", "Container": container, "Conditions": [
                {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "2"}
            ]}
        ]
    }))
}

#[test]
fn chrome_plays_its_audio_formats_directly() {
    let chrome = profile("chrome-linux");
    let cases = [
        ("mp3", "mp3", 320_000),
        ("flac", "flac", 900_000),
        ("ogg", "vorbis", 192_000),
        ("m4a", "aac", 256_000),
        ("webm", "opus", 160_000),
        ("opus", "opus", 160_000),
        ("wav", "pcm_s16le", 1_411_200),
    ];
    for (container, codec, bit_rate) in cases {
        let src = track(container, audio(codec, 2, bit_rate));
        assert_eq!(
            device_data(&src, &chrome, &request()),
            direct(container, true),
            "{codec} in {container}"
        );
    }
}

#[test]
fn an_audio_direct_play_keeps_the_source_container() {
    // Only BuildVideoItem normalizes the source's container (SB 843). An audio item keeps the
    // prober's list.
    let src = track("mov,mp4,m4a,3gp,3g2,mj2", audio("aac", 2, 256_000));
    assert_eq!(
        device_data(&src, &profile("chrome-linux"), &request()),
        direct("mov,mp4,m4a,3gp,3g2,mj2", true)
    );
}

#[test]
fn cover_art_does_not_make_a_song_a_video() {
    let song = StreamFacts {
        index: 1,
        ..audio("mp3", 2, 320_000)
    };
    let src = source("mp3", Some(320_000), vec![cover(0), song]);
    assert_eq!(
        device_data(&src, &profile("chrome-linux"), &request()),
        direct("mp3", true)
    );
}

#[test]
fn chrome_transcodes_alac_to_aac_over_hls() {
    let src = track("m4a", audio("alac", 2, 900_000));
    let expected = transcode(
        "m4a",
        reasons(&[R::AudioCodecNotSupported]),
        chrome_aac_url(384, "AudioCodecNotSupported"),
    );
    assert_eq!(
        device_data(&src, &profile("chrome-linux"), &request()),
        expected
    );

    // The whole URL once, written out, so that the helpers can't hide a mistake.
    let url = format!(
        "/Videos/{ITEM}/main.m3u8?DeviceId=dev1&MediaSourceId=src1&AudioCodec=aac\
         &AudioBitrate=384&SegmentContainer=ts&MinSegments=1&PlaySessionId=ps1&api_key=tok\
         &TranscodingMaxAudioChannels=2&RequireAvc=false&EnableAudioVbrEncoding=true\
         &TranscodeReasons=AudioCodecNotSupported&AudioOnly=true"
    );
    assert_eq!(expected.transcoding_url, Some(url));
}

#[test]
fn a_container_chrome_would_remux_is_transcoded_instead() {
    // These files have a direct stream profile (the codec is a profile's container or audio
    // codec), so Jellyfin would remux them (SB 127-165). Rustyfin can't remux audio, so the HLS
    // transcode takes them, with ContainerNotSupported.
    let chrome = profile("chrome-linux");
    let cases = [
        ("mka", "flac"),
        ("mkv", "opus"),
        ("mkv,webm", "opus"),
        ("mp4", "mp3"),
    ];
    for (container, codec) in cases {
        let src = track(container, audio(codec, 2, 320_000));
        assert_eq!(
            device_data(&src, &chrome, &request()),
            transcode(
                container,
                reasons(&[R::ContainerNotSupported]),
                chrome_aac_url(384, "ContainerNotSupported")
            ),
            "{codec} in {container}"
        );
    }
}

#[test]
fn matroska_plays_only_with_a_profile_that_names_mkv() {
    // jellyfin#13344: a WebM profile doesn't cover a Matroska file. Above, Chrome transcodes opus
    // in "mkv,webm"; a profile that names mkv plays it.
    let src = track("webm", audio("opus", 2, 160_000));
    assert_eq!(
        device_data(&src, &profile("chrome-linux"), &request()),
        direct("webm", true)
    );

    let mkv = custom(json!({
        "DirectPlayProfiles": [{"Container": "webm,mkv", "AudioCodec": "opus", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Streaming"}
        ]
    }));
    let src = track("mkv,webm", audio("opus", 2, 160_000));
    assert_eq!(
        device_data(&src, &mkv, &request()),
        direct("mkv,webm", true)
    );
}

#[test]
fn the_reasons_check_the_container_without_the_matroska_rule() {
    // SB 503-550: a file that no profile plays or remuxes is reported against the profiles whose
    // containers take the source's, without the Matroska rule of jellyfin#13344. chrome-linux's
    // WebM profiles take "mkv,webm", so only the codec is unsupported.
    let vorbis = track("mkv,webm", audio("vorbis", 2, 160_000));
    assert_eq!(
        device_data(&vorbis, &profile("chrome-linux"), &request()),
        transcode(
            "mkv,webm",
            reasons(&[R::AudioCodecNotSupported]),
            chrome_aac_url(384, "AudioCodecNotSupported")
        )
    );

    // A WebM profile that plays the codec leaves no reason, though it can't play the Matroska
    // file directly.
    let webm = custom(json!({
        "MusicStreamingTranscodingBitrate": 192_000,
        "DirectPlayProfiles": [{"Container": "webm", "AudioCodec": "vorbis,opus", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Streaming"}
        ]
    }));
    assert_eq!(
        device_data(&vorbis, &webm, &request()),
        transcode(
            "mkv,webm",
            TranscodeReasons::default(),
            audio_url(
                &["AudioCodec=aac", "AudioBitrate=192", "SegmentContainer=ts"],
                &["RequireAvc=false", "EnableAudioVbrEncoding=true"],
            )
        )
    );
}

#[test]
fn a_file_over_the_bitrate_limit_is_transcoded_at_the_limit() {
    let chrome = profile("chrome-linux");
    let capped = StreamRequest {
        max_bitrate: Some(192_000),
        ..request()
    };
    let over = transcode(
        "mp3",
        reasons(&[R::ContainerBitrateExceedsLimit]),
        chrome_aac_url(192, "ContainerBitrateExceedsLimit"),
    );

    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    assert_eq!(device_data(&mp3, &chrome, &capped), over);

    // An unknown bitrate counts as 40 Mbps: over this limit, under chrome-linux's own.
    let unknown = SourceFacts {
        bitrate: None,
        ..mp3.clone()
    };
    assert_eq!(device_data(&unknown, &chrome, &capped), over);
    assert_eq!(
        device_data(&unknown, &chrome, &request()),
        direct("mp3", true)
    );

    // A remote source is never over the limit.
    let remote = SourceFacts {
        is_remote: true,
        ..mp3.clone()
    };
    assert_eq!(device_data(&remote, &chrome, &capped), direct("mp3", true));

    // A bitrate at the limit is within it, and a limit of 0 or less is no limit.
    for limit in [320_000, 0, -1] {
        let req = StreamRequest {
            max_bitrate: Some(limit),
            ..request()
        };
        assert_eq!(
            device_data(&mp3, &chrome, &req),
            direct("mp3", true),
            "limit {limit}"
        );
    }

    // A container that Jellyfin would remux gets both reasons.
    let mka = track("mka", audio("flac", 2, 900_000));
    assert_eq!(
        device_data(&mka, &chrome, &capped),
        transcode(
            "mka",
            reasons(&[R::ContainerNotSupported, R::ContainerBitrateExceedsLimit]),
            chrome_aac_url(192, "ContainerNotSupported,ContainerBitrateExceedsLimit")
        )
    );
}

#[test]
fn the_transcode_bitrate_falls_back_to_the_limit_and_then_128k() {
    // SB 199-212: the music bitrate, else the limit, else 128 kbps; never above the limit. FLAC
    // has no direct play or direct stream profile here, so the bitrate check never runs and the
    // reasons have no ContainerBitrateExceedsLimit.
    let flac = track("flac", audio("flac", 2, 900_000));
    let url = |kbps: &str| {
        audio_url(
            &["AudioCodec=aac", kbps, "SegmentContainer=ts"],
            &[
                "RequireAvc=false",
                "EnableAudioVbrEncoding=true",
                "TranscodeReasons=ContainerNotSupported,VideoCodecNotSupported,\
                 AudioCodecNotSupported",
            ],
        )
    };
    let cases = [
        (json!(256_000), json!(null), "AudioBitrate=256"),
        (json!(null), json!(null), "AudioBitrate=128"),
        (json!(256_000), json!(192_000), "AudioBitrate=192"),
        (json!(160_000), json!(192_000), "AudioBitrate=160"),
        (json!(null), json!(320_000), "AudioBitrate=320"),
    ];
    for (max, music, kbps) in cases {
        let player = mp3_player(max.clone(), music.clone());
        assert_eq!(
            device_data(&flac, &player, &request()),
            transcode("flac", unsupported(), url(kbps)),
            "max {max}, music {music}"
        );
    }

    // The request's limit replaces the profile's.
    let capped = StreamRequest {
        max_bitrate: Some(64_000),
        ..request()
    };
    let player = mp3_player(json!(256_000), json!(192_000));
    assert_eq!(
        device_data(&flac, &player, &capped),
        transcode("flac", unsupported(), url("AudioBitrate=64"))
    );
}

#[test]
fn audio_codec_profiles_shape_the_transcode() {
    // The conditions of every Audio codec profile for AAC in TS apply, in order. The channel
    // limit becomes a plain `audiochannels` option, capped by the transcoding profile's
    // MaxAudioChannels.
    let why = "TranscodeReasons=AudioCodecNotSupported";
    let alac = reasons(&[R::AudioCodecNotSupported]);
    let surround = track(
        "m4a",
        StreamFacts {
            sample_rate: Some(96_000),
            ..audio("alac", 6, 2_000_000)
        },
    );
    let capped = [
        "AudioCodec=aac",
        "AudioBitrate=192",
        "AudioSampleRate=44100",
        "SegmentContainer=ts",
    ];

    let url = audio_url(
        &capped,
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "audiochannels=6",
            why,
        ],
    );
    assert_eq!(
        device_data(&surround, &aac_player(None), &request()),
        transcode("m4a", alac, url)
    );

    let url = audio_url(
        &capped,
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "audiochannels=2",
            why,
        ],
    );
    assert_eq!(
        device_data(&surround, &aac_player(Some("2")), &request()),
        transcode("m4a", alac, url)
    );

    // ApplyConditions read the source's stream: a stereo file also gets the 64 kbps profile.
    let stereo = track(
        "m4a",
        StreamFacts {
            sample_rate: Some(96_000),
            ..audio("alac", 2, 2_000_000)
        },
    );
    let url = audio_url(
        &[
            "AudioCodec=aac",
            "AudioBitrate=64",
            "AudioSampleRate=44100",
            "SegmentContainer=ts",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "audiochannels=6",
            why,
        ],
    );
    assert_eq!(
        device_data(&stereo, &aac_player(None), &request()),
        transcode("m4a", alac, url)
    );
}

#[test]
fn a_failed_audio_codec_profile_blocks_direct_play() {
    // GetCompatibilityAudioCodec checks the Audio codec profiles for the source's codec and
    // container. A failure is a reason, and the file is transcoded.
    let surround = track("flac", audio("flac", 6, 2_000_000));
    let url = audio_url(
        &["AudioCodec=aac", "AudioBitrate=384", "SegmentContainer=ts"],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "TranscodeReasons=AudioChannelsNotSupported",
        ],
    );
    assert_eq!(
        device_data(&surround, &flac_player(""), &request()),
        transcode("flac", reasons(&[R::AudioChannelsNotSupported]), url)
    );

    // A stereo file passes, and a profile for another container doesn't apply.
    let stereo = track("flac", audio("flac", 2, 900_000));
    assert_eq!(
        device_data(&stereo, &flac_player(""), &request()),
        direct("flac", true)
    );
    assert_eq!(
        device_data(&surround, &flac_player("ogg"), &request()),
        direct("flac", true)
    );
}

#[test]
fn only_an_hls_streaming_audio_profile_transcodes() {
    // The first Audio transcoding profile for streaming over HLS. HTTP and Static profiles are
    // skipped, and a Video profile doesn't count for audio.
    let player = custom(json!({
        "MaxStreamingBitrate": 120_000_000,
        "MusicStreamingTranscodingBitrate": 192_000,
        "DirectPlayProfiles": [{"Container": "mp3", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "mp3", "Type": "Audio", "AudioCodec": "mp3", "Protocol": "http",
             "Context": "Streaming"},
            {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Static"},
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"},
            {"Container": "mp4", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
             "Context": "Streaming", "MinSegments": 2, "SegmentLength": 6},
            {"Container": "ts", "Type": "Audio", "AudioCodec": "mp3", "Protocol": "hls",
             "Context": "Streaming"}
        ]
    }));
    let flac = track("flac", audio("flac", 2, 900_000));
    let url = audio_url(
        &[
            "AudioCodec=aac",
            "AudioBitrate=192",
            "SegmentContainer=mp4",
            "SegmentLength=6",
            "MinSegments=2",
        ],
        &[
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "TranscodeReasons=ContainerNotSupported,VideoCodecNotSupported,AudioCodecNotSupported",
        ],
    );
    assert_eq!(
        device_data(&flac, &player, &request()),
        DeviceData {
            transcoding_container: Some("mp4".to_string()),
            ..transcode("flac", unsupported(), url)
        }
    );

    // With only a Video HLS profile, a song plays directly or not at all.
    let video_only = custom(json!({
        "DirectPlayProfiles": [{"Container": "mp3", "Type": "Audio"}],
        "TranscodingProfiles": [
            {"Container": "ts", "Type": "Video", "VideoCodec": "h264", "AudioCodec": "aac",
             "Protocol": "hls", "Context": "Streaming"}
        ]
    }));
    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    assert_eq!(
        device_data(&mp3, &video_only, &request()),
        direct("mp3", false)
    );
    assert_eq!(
        device_data(&flac, &video_only, &request()),
        no_play("flac", (false, false, false), unsupported())
    );
}

#[test]
fn without_transcoding_a_file_chrome_cant_play_gets_no_plan() {
    // A transcoding profile exists, but the source may not transcode: GetOptimalAudioStream
    // returns null, and MIH leaves the source as step 2 made it, default audio stream included.
    let no_transcode = StreamRequest {
        enable_transcoding: false,
        ..request()
    };
    let alac = track("m4a", audio("alac", 2, 900_000));
    assert_eq!(
        device_data(&alac, &profile("chrome-linux"), &no_transcode),
        untouched(&alac, (true, true, false))
    );

    // Without an HLS audio transcoding profile, the plan stands: its reasons and no method.
    assert_eq!(
        device_data(&alac, &profile("tv-hevc10-mkv"), &no_transcode),
        no_play("m4a", (false, false, false), unsupported())
    );

    // A file that plays directly still does, and MIH step 4 sets the transcoding flag again.
    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    assert_eq!(
        device_data(&mp3, &profile("chrome-linux"), &no_transcode),
        direct("mp3", true)
    );
}

#[test]
fn with_direct_play_off_a_playable_file_is_transcoded_without_a_reason() {
    // Unlike video, audio has no DirectPlayError: the reasons stay empty, and the URL has none.
    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    let off = StreamRequest {
        enable_direct_play: false,
        ..request()
    };
    let url = audio_url(
        &[
            "AudioCodec=aac",
            "AudioBitrate=384",
            "SegmentContainer=ts",
            "MinSegments=1",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
        ],
    );
    assert_eq!(
        device_data(&mp3, &profile("chrome-linux"), &off),
        transcode("mp3", TranscodeReasons::default(), url)
    );

    // Turning direct stream off changes nothing for a direct play.
    let no_stream = StreamRequest {
        enable_direct_stream: false,
        ..request()
    };
    assert_eq!(
        device_data(&mp3, &profile("chrome-linux"), &no_stream),
        direct("mp3", true)
    );
}

#[test]
fn the_decision_reads_the_flagged_default_audio_stream() {
    // GetDefaultAudioStream(null): the first audio stream flagged default, else the first audio
    // stream. Neither the request's audio index nor the source's default index counts.
    let chrome = profile("chrome-linux");
    let alac = |is_default| StreamFacts {
        is_default,
        ..audio("alac", 2, 900_000)
    };
    let aac = |is_default| StreamFacts {
        index: 1,
        is_default,
        ..audio("aac", 2, 256_000)
    };
    let m4a = |streams| source("m4a", Some(1_156_000), streams);
    let transcoded = transcode(
        "m4a",
        reasons(&[R::AudioCodecNotSupported]),
        chrome_aac_url(384, "AudioCodecNotSupported"),
    );

    assert_eq!(
        device_data(&m4a(vec![alac(false), aac(true)]), &chrome, &request()),
        direct("m4a", true)
    );
    assert_eq!(
        device_data(&m4a(vec![alac(true), aac(false)]), &chrome, &request()),
        transcoded
    );
    assert_eq!(
        device_data(&m4a(vec![alac(false), aac(false)]), &chrome, &request()),
        transcoded
    );

    let choosing_aac = StreamRequest {
        media_source_id: "src1".to_string(),
        audio_stream_index: Some(1),
        ..request()
    };
    assert_eq!(
        device_data(&m4a(vec![alac(true), aac(false)]), &chrome, &choosing_aac),
        transcoded
    );
    let aac_default = SourceFacts {
        default_audio_stream_index: Some(1),
        ..m4a(vec![alac(true), aac(false)])
    };
    assert_eq!(device_data(&aac_default, &chrome, &request()), transcoded);
}

#[test]
fn a_source_without_an_audio_stream_gets_no_plan() {
    // Jellyfin throws here. Rustyfin answers with the source as step 2 made it.
    let chrome = profile("chrome-linux");
    let off = StreamRequest {
        enable_direct_play: false,
        enable_transcoding: false,
        ..request()
    };
    for streams in [vec![], vec![cover(0)]] {
        let src = source("mp3", Some(320_000), streams);
        assert_eq!(
            device_data(&src, &chrome, &request()),
            untouched(&src, (true, true, true))
        );
        assert_eq!(
            device_data(&src, &chrome, &off),
            untouched(&src, (false, true, false))
        );
    }
}

#[test]
fn the_tv_profile_has_no_hls_audio_transcode() {
    // tv-hevc10-mkv transcodes audio to MP3 over HTTP only, which Rustyfin doesn't serve.
    let tv = profile("tv-hevc10-mkv");
    let alac = track("m4a", audio("alac", 2, 900_000));
    assert_eq!(
        device_data(&alac, &tv, &request()),
        no_play("m4a", (false, false, false), unsupported())
    );

    let flac = track("flac", audio("flac", 2, 900_000));
    assert_eq!(device_data(&flac, &tv, &request()), direct("flac", false));

    let big = track("mp3", audio("mp3", 2, 50_000_000));
    assert_eq!(
        device_data(&big, &tv, &request()),
        no_play(
            "mp3",
            (false, false, false),
            reasons(&[R::ContainerBitrateExceedsLimit])
        )
    );
}

#[test]
fn a_profile_without_audio_profiles_plays_no_audio() {
    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    assert_eq!(
        device_data(&mp3, &profile("minimal-mp4"), &request()),
        no_play("mp3", (false, false, false), unsupported())
    );
}

#[test]
fn the_url_takes_the_copy_suffixes_but_no_subtitle_or_burn_in() {
    // MIH appends the stream-copy flags from the request, but burn-in only from StreamInfo, which
    // the audio decision never sets. It sets no audio or subtitle index either, so the chosen
    // subtitle and the source's default subtitle are gone.
    let mut src = source(
        "m4a",
        Some(900_000),
        vec![audio("alac", 2, 900_000), cover(1), subtitle(2)],
    );
    src.default_subtitle_stream_index = Some(2);
    let req = StreamRequest {
        media_source_id: "src1".to_string(),
        audio_stream_index: Some(0),
        subtitle_stream_index: Some(2),
        max_audio_channels: Some(6),
        start_time_ticks: 600_000_000,
        allow_video_stream_copy: false,
        allow_audio_stream_copy: false,
        always_burn_in_subtitle: true,
        ..request()
    };
    let url = audio_url(
        &[
            "AudioCodec=aac",
            "AudioBitrate=384",
            "SegmentContainer=ts",
            "MinSegments=1",
            "StartTimeTicks=600000000",
        ],
        &[
            "TranscodingMaxAudioChannels=2",
            "RequireAvc=false",
            "EnableAudioVbrEncoding=true",
            "TranscodeReasons=AudioCodecNotSupported",
        ],
    );
    let url = format!("{url}&allowVideoStreamCopy=false&allowAudioStreamCopy=false");
    assert_eq!(
        device_data(&src, &profile("chrome-linux"), &req),
        transcode("m4a", reasons(&[R::AudioCodecNotSupported]), url)
    );
}

#[test]
fn a_video_request_is_not_decided_as_audio() {
    // The audio decision is for MediaKind::Audio only: a video request for a song still gets the
    // video decision, which finds no video direct play profile for mp3 in chrome-linux.
    let mp3 = track("mp3", audio("mp3", 2, 320_000));
    let video = StreamRequest {
        media_kind: MediaKind::Video,
        ..request()
    };
    let data = device_data(&mp3, &profile("chrome-linux"), &video);
    assert_ne!(data, direct("mp3", true));
    let url = data.transcoding_url.unwrap_or_default();
    assert!(!url.contains("AudioOnly"), "{url}");
}

#[test]
fn odd_audio_inputs_do_not_panic() {
    let odd_conditions = json!([
        {"Condition": "LessThanEqual", "Property": "AudioChannels", "Value": "-5"},
        {"Condition": "GreaterThanEqual", "Property": "AudioChannels", "Value": "2147483647"},
        {"Condition": "Equals", "Property": "AudioBitrate", "Value": "99999999999"},
        {"Condition": "LessThanEqual", "Property": "AudioBitrate", "Value": "-2147483648"},
        {"Condition": "LessThanEqual", "Property": "AudioSampleRate", "Value": "NaN"},
        {"Condition": "NotEquals", "Property": "AudioBitDepth", "Value": "abc"},
        {"Condition": "EqualsAny", "Property": "AudioProfile", "Value": "|"},
        {"Condition": "Equals", "Property": "AudioChannels", "Value": ""}
    ]);
    let profiles = vec![
        ("chrome-linux", profile("chrome-linux")),
        ("chrome-linux-high10", profile("chrome-linux-high10")),
        ("minimal-mp4", profile("minimal-mp4")),
        ("tv-hevc10-mkv", profile("tv-hevc10-mkv")),
        ("default", DeviceProfile::default()),
        (
            "odd transcode",
            custom(json!({
                "MaxStreamingBitrate": i32::MIN,
                "MusicStreamingTranscodingBitrate": i32::MAX,
                "TranscodingProfiles": [
                    {"Container": "ts", "Type": "Audio", "AudioCodec": "aac", "Protocol": "hls",
                     "Context": "Streaming", "MaxAudioChannels": "abc", "MinSegments": -5,
                     "SegmentLength": i32::MAX}
                ],
                "CodecProfiles": [
                    {"Type": "Audio", "Conditions": odd_conditions},
                    {"Type": "Audio", "Codec": "aac", "ApplyConditions": odd_conditions,
                     "Conditions": odd_conditions}
                ]
            })),
        ),
        (
            "odd lists",
            custom(json!({
                "MaxStreamingBitrate": 1,
                "MusicStreamingTranscodingBitrate": null,
                "DirectPlayProfiles": [
                    {"Container": "-mp3", "Type": "Audio"},
                    {"Container": ",", "AudioCodec": ",", "Type": "Audio"}
                ],
                "TranscodingProfiles": [
                    {"Container": "", "Type": "Audio", "AudioCodec": "", "Protocol": "hls",
                     "Context": "Streaming", "MaxAudioChannels": "-2147483648"}
                ],
                "CodecProfiles": [
                    {"Type": "Audio", "Codec": ",", "Container": "-", "Conditions": odd_conditions}
                ]
            })),
        ),
    ];
    let containers = ["", ",,", "-mp3", "mp3,", "MP3", "mkv,", "mka,mkv,webm"];
    let streams = [
        audio("", 0, 0),
        audio("MP3", i32::MAX, i32::MAX),
        StreamFacts {
            index: i32::MIN,
            channels: Some(i32::MIN),
            sample_rate: Some(i32::MIN),
            bit_depth: Some(i32::MIN),
            bit_rate: Some(i32::MIN),
            ..audio("alac", 2, 0)
        },
        StreamFacts {
            index: i32::MAX,
            channels: None,
            sample_rate: None,
            bit_depth: None,
            bit_rate: None,
            is_default: false,
            ..audio("flac", 2, 0)
        },
        StreamFacts {
            index: -1,
            is_external: true,
            ..audio("aac", 8, 1)
        },
    ];
    let bitrates = [None, Some(i64::MIN), Some(0), Some(i64::MAX)];
    let requests = [
        request(),
        StreamRequest {
            max_bitrate: Some(i32::MIN),
            start_time_ticks: i64::MIN,
            max_audio_channels: Some(i32::MIN),
            ..request()
        },
        StreamRequest {
            media_source_id: "SRC1".to_string(),
            audio_stream_index: Some(i32::MIN),
            subtitle_stream_index: Some(-1),
            max_bitrate: Some(0),
            start_time_ticks: i64::MAX,
            max_audio_channels: Some(0),
            ..request()
        },
        StreamRequest {
            max_bitrate: Some(i32::MAX),
            max_audio_channels: Some(i32::MAX),
            ..request()
        },
    ];

    for (name, device) in &profiles {
        for container in containers {
            for stream in &streams {
                for bitrate in bitrates {
                    let src = SourceFacts {
                        default_audio_stream_index: None,
                        ..source(container, bitrate, vec![stream.clone()])
                    };
                    for (i, req) in requests.iter().enumerate() {
                        let label = format!(
                            "{name}: {:?} in {container:?} at {bitrate:?}, request {i}",
                            stream.codec
                        );
                        let data = device_data(&src, device, req);
                        assert_eq!(data.container, container, "{label}");
                        assert_eq!(
                            data.transcoding_url.is_some(),
                            data.play_method == Some(PlayMethod::Transcode),
                            "{label}"
                        );
                        if let Some(url) = &data.transcoding_url {
                            assert!(
                                url.starts_with(&format!("/Videos/{ITEM}/main.m3u8?")),
                                "{label}: {url}"
                            );
                            assert!(url.ends_with("&AudioOnly=true"), "{label}: {url}");
                            assert!(!url.contains("StreamIndex="), "{label}: {url}");
                            assert_eq!(
                                data.transcoding_sub_protocol,
                                StreamProtocol::Hls,
                                "{label}"
                            );
                        }
                        assert_eq!(data.default_audio_stream_index, None, "{label}");
                        assert_eq!(data.default_subtitle_stream_index, None, "{label}");
                        assert!(data.subtitles.is_empty(), "{label}");
                    }
                }
            }
        }
    }
}
