//! The playback decisions for the device profiles that real Jellyfin clients send, held in
//! `tests/fixtures/device_profiles/client-*.json`: Jellyfin Web on Chrome, Firefox and Safari,
//! Jellyfin Android (ExoPlayer), Jellyfin Media Player and a direct-play-all profile from
//! Jellyfin's own test data, and Swiftfin and Findroid built from their sources. Each case's
//! expected answer is what a Jellyfin 12.4.0 server gave for the same file and profile.

use rustyfin_core::stream_builder::{
    DeviceData, SourceFacts, StreamFacts, StreamKind, StreamRequest, TranscodeReason as R,
    device_data,
};
use rustyfin_model::device_profile::{DeviceProfile, SubtitleDeliveryMethod as M};

/// The item every request asks about.
const ITEM: &str = "11111111-2222-3333-4444-555555555555";
/// The id of the item's only media source.
const SOURCE: &str = "11111111222233334444555555555555";

/// The client profile `client-{name}.json`.
fn client(name: &str) -> DeviceProfile {
    let path = format!(
        "{}/tests/fixtures/device_profiles/client-{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    DeviceProfile::from_json(&serde_json::from_str(&text).unwrap()).unwrap()
}

/// A request with token `tok` that names the source, and its subtitle when given.
fn request(subtitle: Option<i32>) -> StreamRequest {
    StreamRequest {
        item_id: ITEM.to_string(),
        media_source_id: SOURCE.to_string(),
        subtitle_stream_index: subtitle,
        device_id: "dev1".to_string(),
        play_session_id: "ps1".to_string(),
        access_token: "tok".to_string(),
        ..StreamRequest::default()
    }
}

/// A source in `container` with these streams.
fn source(container: &str, bitrate: i64, streams: Vec<StreamFacts>) -> SourceFacts {
    let default_subtitle_stream_index = streams
        .iter()
        .find(|s| s.kind == StreamKind::Subtitle && s.is_default)
        .map(|s| s.index);
    SourceFacts {
        id: SOURCE.to_string(),
        container: container.to_string(),
        bitrate: Some(bitrate),
        streams,
        default_subtitle_stream_index,
        ..SourceFacts::default()
    }
}

/// A progressive SDR video stream at index 0, with its codec level and bit depth.
#[allow(clippy::too_many_arguments)]
fn video(
    codec: &str,
    profile: &str,
    level: f64,
    bit_depth: i32,
    width: i32,
    height: i32,
    frame_rate: f32,
) -> StreamFacts {
    StreamFacts {
        index: 0,
        kind: StreamKind::Video,
        codec: codec.to_string(),
        profile: Some(profile.to_string()),
        level: Some(level),
        width: Some(width),
        height: Some(height),
        bit_depth: Some(bit_depth),
        frame_rate: Some(frame_rate),
        range_type: Some("SDR".to_string()),
        is_interlaced: Some(false),
        is_anamorphic: Some(false),
        is_default: true,
        ..StreamFacts::default()
    }
}

/// The default audio stream at index 1.
fn audio(codec: &str, channels: i32, bit_rate: i32) -> StreamFacts {
    StreamFacts {
        index: 1,
        kind: StreamKind::Audio,
        codec: codec.to_string(),
        channels: Some(channels),
        sample_rate: Some(48000),
        bit_rate: Some(bit_rate),
        language: Some("eng".to_string()),
        is_default: true,
        ..StreamFacts::default()
    }
}

/// An embedded English subtitle stream.
fn subtitle(index: i32, codec: &str, is_default: bool) -> StreamFacts {
    StreamFacts {
        index,
        kind: StreamKind::Subtitle,
        codec: codec.to_string(),
        language: Some("eng".to_string()),
        is_default,
        ..StreamFacts::default()
    }
}

/// "Compartment No. 6": 1080p H.264 High 4.1 in Matroska with E-AC-3 5.1, SubRip and ASS.
fn h264_eac3_ass() -> SourceFacts {
    source(
        "mkv",
        10_000_000,
        vec![
            video("h264", "High", 41.0, 8, 1920, 800, 24.0),
            audio("eac3", 6, 640_000),
            subtitle(2, "subrip", false),
            subtitle(3, "ass", false),
        ],
    )
}

/// "Predator": 2160p HEVC Main 10 at level 5.1 in Matroska with DTS 5.1, SubRip (default) and
/// PGS subtitles.
fn hevc10_dts_pgs() -> SourceFacts {
    source(
        "mkv",
        60_000_000,
        vec![
            video("hevc", "Main 10", 153.0, 10, 3840, 2076, 23.976_025),
            audio("dts", 6, 1_536_000),
            subtitle(3, "subrip", true),
            subtitle(5, "PGSSUB", false),
        ],
    )
}

/// The value of query parameter `name` in `url`.
fn param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

/// The delivery method and URL of subtitle stream `index`.
fn delivery(data: &DeviceData, index: i32) -> (M, Option<&str>) {
    let delivery = data.subtitles.iter().find(|d| d.index == index).unwrap();
    (delivery.method, delivery.url.as_deref())
}

#[test]
fn r3decisions_chrome_transcode_url_is_jellyfins() {
    let data = device_data(&h264_eac3_ass(), &client("web-chrome"), &request(None));

    assert!(!data.supports_direct_play);
    let url = data.transcoding_url.as_deref().unwrap();
    assert!(
        url.starts_with(&format!("/Videos/{ITEM}/master.m3u8?")),
        "{url}"
    );
    // Bitrates in bits per second, the token as ApiKey
    assert_eq!(param(url, "VideoBitrate"), Some("119616000"), "{url}");
    assert_eq!(param(url, "AudioBitrate"), Some("384000"), "{url}");
    assert_eq!(param(url, "ApiKey"), Some("tok"), "{url}");
    assert_eq!(param(url, "api_key"), None, "{url}");
    // The source's level and bit depth, not the profile's cap
    assert_eq!(param(url, "h264-level"), Some("41"), "{url}");
    assert_eq!(param(url, "h264-videobitdepth"), Some("8"), "{url}");
    assert_eq!(
        param(url, "TranscodeReasons"),
        Some("ContainerNotSupported,AudioCodecNotSupported")
    );
}

#[test]
fn r3decisions_chrome_hevc10_transcode_keeps_level_and_depth() {
    let data = device_data(&hevc10_dts_pgs(), &client("web-chrome"), &request(None));

    let url = data.transcoding_url.as_deref().unwrap();
    assert_eq!(param(url, "hevc-level"), Some("153"), "{url}");
    assert_eq!(param(url, "hevc-videobitdepth"), Some("10"), "{url}");
    assert_eq!(param(url, "hevc-profile"), Some("main10"), "{url}");
}

#[test]
fn r3decisions_web_ass_goes_out_as_ass_and_subrip_as_vtt() {
    for name in ["web-chrome", "web-firefox", "web-safari"] {
        let data = device_data(&h264_eac3_ass(), &client(name), &request(None));
        let (method, url) = delivery(&data, 3);
        assert_eq!(method, M::External, "{name}");
        assert!(
            url.unwrap()
                .contains("/Subtitles/3/0/Stream.ass?ApiKey=tok"),
            "{name}: {url:?}"
        );
        let (method, url) = delivery(&data, 2);
        assert_eq!(method, M::External, "{name}");
        assert!(
            url.unwrap().contains("/Subtitles/2/0/Stream.vtt?"),
            "{name}: {url:?}"
        );
    }
}

#[test]
fn r3decisions_srt_profiles_get_subrip_as_srt() {
    for name in ["findroid", "jmp", "infuse-direct"] {
        let data = device_data(&hevc10_dts_pgs(), &client(name), &request(None));
        let (method, url) = delivery(&data, 3);
        assert_eq!(method, M::External, "{name}");
        assert!(
            url.unwrap().contains("/Subtitles/3/0/Stream.srt?"),
            "{name}: {url:?}"
        );
        // A picture subtitle has no text to send as a file
        assert_eq!(delivery(&data, 5).0, M::Encode, "{name}");
    }
}

#[test]
fn r3decisions_jmp_sends_ass_as_ass() {
    let data = device_data(&h264_eac3_ass(), &client("jmp"), &request(Some(3)));

    let (method, url) = delivery(&data, 3);
    assert_eq!(method, M::External);
    assert!(url.unwrap().contains("/Stream.ass?"), "{url:?}");
    let url = data.transcoding_url.as_deref().unwrap();
    // An External subtitle isn't named in the transcode
    assert_eq!(param(url, "SubtitleStreamIndex"), None, "{url}");
    assert_eq!(param(url, "SubtitleMethod"), None, "{url}");
}

#[test]
fn r3decisions_android_subrip_keeps_its_name() {
    let data = device_data(
        &hevc10_dts_pgs(),
        &client("android-exoplayer"),
        &request(None),
    );

    let (method, url) = delivery(&data, 3);
    assert_eq!(method, M::External);
    assert!(url.unwrap().contains("/Stream.subrip?"), "{url:?}");
}

#[test]
fn r3decisions_android_burns_in_a_default_ass_subtitle() {
    // "Mushi-Shi": HEVC Main 10 with Opus and a default ASS subtitle, which ExoPlayer can't show
    let mushishi = source(
        "mkv",
        8_000_000,
        vec![
            video("hevc", "Main 10", 150.0, 10, 1920, 1080, 23.976_025),
            StreamFacts {
                language: Some("jpn".to_string()),
                ..audio("opus", 2, 128_000)
            },
            subtitle(2, "ass", true),
        ],
    );
    let data = device_data(&mushishi, &client("android-exoplayer"), &request(None));

    assert!(!data.supports_direct_play);
    assert_eq!(delivery(&data, 2), (M::Encode, None));
    assert!(
        data.transcode_reasons
            .contains(R::SubtitleCodecNotSupported)
    );
    let url = data.transcoding_url.as_deref().unwrap();
    assert_eq!(param(url, "SubtitleStreamIndex"), Some("2"), "{url}");
    assert_eq!(param(url, "SubtitleMethod"), Some("Encode"), "{url}");
}

#[test]
fn r3decisions_swiftfin_transcodes_anamorphic_video() {
    // "Throne of Blood": anamorphic 480p H.264 Main 4.0 with mono AAC
    let mut throne = video("h264", "Main", 40.0, 8, 720, 480, 23.976_023);
    throne.is_anamorphic = Some(true);
    let source = source(
        "mp4",
        2_000_000,
        vec![
            throne,
            StreamFacts {
                profile: Some("LC".to_string()),
                ..audio("aac", 1, 156_500)
            },
        ],
    );
    let data = device_data(&source, &client("swiftfin"), &request(None));

    assert!(!data.supports_direct_play);
    assert!(
        data.transcode_reasons
            .contains(R::AnamorphicVideoNotSupported)
    );
    assert_eq!(data.transcoding_container.as_deref(), Some("mp4"));
    let url = data.transcoding_url.as_deref().unwrap();
    assert_eq!(param(url, "h264-level"), Some("40"), "{url}");
}
