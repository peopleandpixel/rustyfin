use std::collections::HashMap;

use crate::query::Query;
use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use serde::Deserialize;
use uuid::Uuid;

use rustyfin_core::subtitle::is_image_subtitle;
use rustyfin_core::transcode::{
    HwAccel, ImageSubtitle, TranscodeError, TranscodeProfile, audio_encoder, video_encoder,
};
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

use super::streaming::{
    Version, requested_source_id, select_version, validate_path_against_library,
};
use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        // Jellyfin standard HLS master playlist endpoint
        .route("/Videos/{id}/main.m3u8", get(hls_master_playlist))
        // The path Jellyfin's TranscodingUrl and its SDKs use; the same playlist
        .route("/Videos/{id}/master.m3u8", get(hls_master_playlist))
        // Jellyfin HLS segment endpoint: /Videos/{id}/hls/{playbackId}/{type}/{segmentId}.{ext}
        .route(
            "/Videos/{id}/hls/{playback_id}/{segment_type}/{segment}",
            get(hls_segment),
        )
        // Legacy segment endpoint (backward compat)
        .route("/Videos/{id}/hls/{segment}", get(hls_segment_legacy))
        .route("/Videos/{id}/Transcode", delete(stop_transcode))
        .route("/Videos/ActiveEncodings", delete(stop_active_encodings))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodeQuery {
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hw_accel: Option<String>,
    pub segment_duration: Option<u32>,
    /// Absolute index of the subtitle stream to burn in: absent is the first, negative is none
    pub subtitle_stream_index: Option<i32>,
    pub burn_subtitles: Option<bool>,
    /// Jellyfin's SubtitleMethod: "Encode" burns the subtitle in, in any case
    pub subtitle_method: Option<String>,
    /// Absolute index of the audio stream to play: absent, negative or not an audio stream of the
    /// source is the default audio stream
    pub audio_stream_index: Option<i32>,
    /// Audio-only transcoding (no video)
    pub audio_only: Option<bool>,
    /// Enable adaptive bitrate transcoding
    pub adaptive_bitrate: Option<bool>,
    /// Jellyfin play session id: the transcode session key
    pub play_session_id: Option<String>,
    /// Jellyfin device id: the session key when there is no play session id
    pub device_id: Option<String>,
    /// Where to start, in ticks: the resume or seek point Universal forwards
    pub start_time_ticks: Option<i64>,
    /// Jellyfin's Profile: the H.264 profiles the client takes, split on commas or pipes
    pub profile: Option<String>,
    /// Jellyfin's MaxVideoBitDepth: the deepest video bit depth the client takes
    pub max_video_bit_depth: Option<u32>,
    /// Jellyfin's h264-profile: the profiles read when Profile is absent or lists none
    #[serde(rename = "h264-profile")]
    pub h264_profile: Option<String>,
    /// Jellyfin's h264-videobitdepth: read when MaxVideoBitDepth is absent. A string, so a value
    /// that is not a number does not fail the whole query.
    #[serde(rename = "h264-videobitdepth")]
    pub h264_video_bit_depth: Option<String>,
    /// Jellyfin's RequireNonAnamorphic: the client needs square pixels. Jellyfin writes "True",
    /// so it is read as text.
    pub require_non_anamorphic: Option<String>,
    /// Jellyfin's DeInterlace: the client needs progressive video.
    pub de_interlace: Option<String>,
    /// Jellyfin's Level: the highest codec level the client takes, when no `{codec}-level` says.
    pub level: Option<String>,
    /// The codec-qualified options (`h264-level`, `h264-deinterlace`, ...), names lowercased.
    /// The route fills them from the raw query (`with_codec_options`).
    #[serde(skip)]
    pub codec_options: HashMap<String, String>,
    /// Jellyfin's hevc-profile: the HEVC profiles the client takes, split on commas or pipes
    #[serde(rename = "hevc-profile")]
    pub hevc_profile: Option<String>,
    /// Jellyfin's hevc-videobitdepth: read when MaxVideoBitDepth is absent
    #[serde(rename = "hevc-videobitdepth")]
    pub hevc_video_bit_depth: Option<String>,
    /// Jellyfin's hevc-level: the highest HEVC level the client takes (general_level_idc)
    #[serde(rename = "hevc-level")]
    pub hevc_level: Option<String>,
    /// Jellyfin's hevc-rangetype: the video ranges the client takes (SDR, HDR10, HLG, DOVI...)
    #[serde(rename = "hevc-rangetype")]
    pub hevc_range_type: Option<String>,
}

/// Whether a query value says true, in any case.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
}

impl TranscodeQuery {
    /// The query with its codec-qualified options (any name with a `-`) from `params`.
    fn with_codec_options(mut self, params: &HashMap<String, String>) -> Self {
        self.codec_options = params
            .iter()
            .filter(|(name, _)| name.contains('-'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        self
    }

    /// The option `{codec}-{name}`.
    fn codec_option(&self, codec: &str, name: &str) -> Option<&str> {
        self.codec_options
            .get(&format!("{codec}-{name}"))
            .map(String::as_str)
    }

    /// The highest level the client takes for `codec`: `{codec}-level`, else `Level`.
    fn max_level(&self, codec: &str) -> Option<f64> {
        self.codec_option(codec, "level")
            .or(self.level.as_deref())
            .and_then(|level| level.trim().parse().ok())
    }

    /// Whether the client needs `codec` deinterlaced: `{codec}-deinterlace` or `DeInterlace`.
    fn wants_deinterlaced(&self, codec: &str) -> bool {
        truthy(self.codec_option(codec, "deinterlace")) || truthy(self.de_interlace.as_deref())
    }

    /// Whether the client needs square pixels (`RequireNonAnamorphic`).
    fn wants_square_pixels(&self) -> bool {
        truthy(self.require_non_anamorphic.as_deref())
    }
}

/// A URL bitrate in kbps. The URL writes bits per second, as Jellyfin's does.
fn kbps(bits_per_second: u32) -> u32 {
    bits_per_second / 1000
}

impl TranscodeQuery {
    fn into_profile(self) -> TranscodeProfile {
        let mut p = TranscodeProfile::default();
        if let Some(v) = self.video_bitrate {
            p.max_video_bitrate_kbps = kbps(v);
        }
        if let Some(v) = self.audio_bitrate {
            p.max_audio_bitrate_kbps = kbps(v);
        }
        if let Some(v) = self.max_width {
            p.max_width = v;
        }
        if let Some(v) = self.max_height {
            p.max_height = v;
        }
        if let Some(v) = self.hw_accel {
            p.hw_accel = HwAccel::from_str(&v);
        }
        if let Some(v) = self.segment_duration {
            p.segment_duration = v;
        }
        if let Some(v) = self.audio_only {
            p.audio_only = v;
        }
        if let Some(v) = self.adaptive_bitrate {
            p.adaptive_bitrate = v;
        }
        if let Some(v) = self.start_time_ticks {
            p.start_time_ticks = v;
        }
        p
    }
}

/// An audio or subtitle stream of the source that a transcode can play.
#[derive(Debug, Clone, Default)]
struct Track {
    index: u32,           // absolute stream index (MediaStream.Index)
    codec: String,        // lowercase
    bitrate: Option<u32>, // bits/s
    is_default: bool,
}

/// What the transcode route knows about the source.
#[derive(Debug, Default, Clone)]
struct SourceInfo {
    video_codec: Option<String>,
    video_profile: Option<String>,
    video_bit_depth: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    video_bitrate: Option<u32>, // bits/s: stream, else source video_bitrate, else source bitrate
    video_level: Option<f64>,   // the codec level, as ffprobe gives it
    interlaced: bool,
    anamorphic: bool,
    audio: Vec<Track>,          // audio streams, in stream order
    audio_bitrate: Option<u32>, // bits/s: the source's audio_bitrate, for a track without its own
    subtitles: Vec<Track>,      // embedded subtitle streams, in stream order
    /// What ffprobe says about an HEVC video that a client might take as it is; None until the
    /// file is probed, and a video without it is never copied as HEVC
    hevc: Option<HevcFacts>,
}

/// What decides whether a client takes an HEVC video as it is, read by ffprobe from the file:
/// the stored streams lack the level, the Dolby Vision configuration and the pixel format.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct HevcFacts {
    /// ffprobe's `profile` ("Main", "Main 10", "Rext")
    profile: Option<String>,
    /// `general_level_idc`, as Jellyfin's `hevc-level` counts it (120 is level 4, 153 is 5.1)
    level: Option<u32>,
    /// ffprobe's `pix_fmt` ("yuv420p10le")
    pix_fmt: Option<String>,
    /// ffprobe's `color_transfer`: smpte2084 is PQ (HDR10), arib-std-b67 is HLG
    transfer: Option<String>,
    /// The Dolby Vision profile, when the stream carries a DV configuration record
    dv_profile: Option<u32>,
    /// The DV base layer's compatibility: 0 none, 1 HDR10, 2 SDR, 4 HLG
    dv_compatibility: Option<u32>,
}

/// How long one HEVC probe may run.
const HEVC_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The HEVC facts of the first video stream of `path`, probed once per file version (path,
/// size, modification time) and kept; None when ffprobe fails, so the video is encoded.
async fn hevc_facts(path: &str) -> Option<HevcFacts> {
    type Key = (String, u64, Option<std::time::SystemTime>);
    static PROBED: std::sync::OnceLock<std::sync::Mutex<HashMap<Key, HevcFacts>>> =
        std::sync::OnceLock::new();
    let meta = tokio::fs::metadata(path).await.ok()?;
    let key = (path.to_string(), meta.len(), meta.modified().ok());
    let probed = PROBED.get_or_init(Default::default);
    if let Some(facts) = probed.lock().unwrap().get(&key) {
        return Some(facts.clone());
    }
    let run = tokio::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-of", "json", "-show_entries"])
        .arg("stream=profile,level,pix_fmt,color_transfer:stream_side_data=dv_profile,dv_bl_signal_compatibility_id")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(HEVC_PROBE_TIMEOUT, run)
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let facts = parse_hevc_facts(&output.stdout)?;
    let mut probed = probed.lock().unwrap();
    if probed.len() > 1000 {
        probed.clear();
    }
    probed.insert(key, facts.clone());
    Some(facts)
}

/// The facts in ffprobe's JSON for one video stream.
fn parse_hevc_facts(json: &[u8]) -> Option<HevcFacts> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let stream = value.get("streams")?.get(0)?;
    let text = |key: &str| stream.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let number = |v: &serde_json::Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
    let dv = stream
        .get("side_data_list")
        .and_then(|list| list.as_array())
        .and_then(|list| list.iter().find(|side| side.get("dv_profile").is_some()));
    Some(HevcFacts {
        profile: text("profile"),
        level: stream.get("level").and_then(number),
        pix_fmt: text("pix_fmt"),
        transfer: text("color_transfer"),
        dv_profile: dv.and_then(|side| side.get("dv_profile")).and_then(number),
        dv_compatibility: dv
            .and_then(|side| side.get("dv_bl_signal_compatibility_id"))
            .and_then(number),
    })
}

impl SourceInfo {
    fn from_source(source: &MediaSourceResponse) -> Self {
        // The probe keeps ffprobe's lowercase stream types, so the types are matched without case
        let video = source
            .media_streams
            .iter()
            .find(|s| s.codec_type.eq_ignore_ascii_case("video"));
        let audio: Vec<Track> = source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("audio"))
            .filter_map(source_track)
            .collect();
        // An external subtitle file is not a stream of the source
        let subtitles: Vec<Track> = source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("subtitle") && !s.is_external)
            .filter_map(source_track)
            .collect();
        Self {
            video_codec: video
                .and_then(|s| s.codec.as_deref())
                .map(str::to_ascii_lowercase),
            video_profile: video.and_then(|s| s.profile.clone()),
            video_bit_depth: video.and_then(|s| s.bit_depth),
            width: video.and_then(|s| s.width).or(source.width),
            height: video.and_then(|s| s.height).or(source.height),
            video_bitrate: video
                .and_then(|s| s.bit_rate)
                .or(source.video_bitrate)
                .or(source.bitrate),
            video_level: video.and_then(|s| s.level),
            interlaced: video.is_some_and(|s| s.is_interlaced),
            anamorphic: video.is_some_and(|s| s.is_anamorphic == Some(true)),
            audio,
            audio_bitrate: source.audio_bitrate,
            subtitles,
            hevc: None,
        }
    }
}

/// A source stream as a track, with its codec lowercased. A negative index names no stream, so it
/// is no track.
fn source_track(stream: &MediaStream) -> Option<Track> {
    Some(Track {
        index: u32::try_from(stream.index).ok()?,
        codec: stream
            .codec
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase(),
        bitrate: stream.bit_rate,
        is_default: stream.is_default,
    })
}

/// Whether a client codec list takes one of `codecs`. An absent list, or one with no non-empty
/// entries, takes them all. Entries are split on commas, trimmed and compared lowercased.
fn client_takes(list: Option<&str>, codecs: &[&str]) -> bool {
    let names: Vec<String> = list
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    names.is_empty() || names.iter().any(|name| codecs.contains(&name.as_str()))
}

/// The names a client lists HEVC by.
const HEVC_NAMES: [&str; 4] = ["hevc", "h265", "hvc1", "hev1"];

/// Whether a client codec list names one of `codecs`. Unlike `client_takes`, an absent or empty
/// list names none: a client must ask for HEVC to get it.
fn client_lists(list: Option<&str>, codecs: &[&str]) -> bool {
    list.unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .any(|name| codecs.contains(&name.as_str()))
}

/// Whether the source is HEVC.
fn is_hevc(source: &SourceInfo) -> bool {
    matches!(source.video_codec.as_deref(), Some("hevc" | "h265"))
}

/// The video range of HEVC `facts`, as Jellyfin names range types: a Dolby Vision stream by its
/// base layer (`DOVI` alone when it has no compatible one, as profile 5 has none), else HDR10
/// for PQ, HLG, or SDR.
fn hevc_range(facts: &HevcFacts) -> &'static str {
    let transfer = facts.transfer.as_deref().unwrap_or_default();
    match (facts.dv_profile, facts.dv_compatibility) {
        (Some(5), _) | (Some(_), None | Some(0)) => "DOVI",
        (Some(_), Some(1)) => "DOVIWithHDR10",
        (Some(_), Some(4)) => "DOVIWithHLG",
        (Some(_), Some(_)) => "DOVIWithSDR",
        (None, _) if transfer == "smpte2084" => "HDR10",
        (None, _) if transfer == "arib-std-b67" => "HLG",
        (None, _) => "SDR",
    }
}

/// Whether a client takes the source's HEVC as it is, by what Jellyfin clients send with the
/// stream URL. Every rule must hold:
/// - the file was probed (see `HevcFacts`);
/// - the bit depth is within MaxVideoBitDepth, else `hevc-videobitdepth`, else 8 (a client that
///   states nothing gets no 10-bit video, as with H.264);
/// - the profile is among those `hevc-profile` lists ("Main 10" matches "main10"), or Main or
///   Main 10 when it lists none; range extensions (4:2:2, 4:4:4) are always encoded;
/// - the level is at most `hevc-level`, when the client gives one;
/// - the video range is SDR, or one `hevc-rangetype` lists: HDR10 or HLG by name, and Dolby
///   Vision only as DOVI, or by its compatible base layer (DOVIWithHDR10 also plays as HDR10).
fn client_takes_hevc(query: &TranscodeQuery, source: &SourceInfo) -> bool {
    let Some(facts) = &source.hevc else {
        return false;
    };
    let max_depth = query
        .max_video_bit_depth
        .or_else(|| {
            query
                .hevc_video_bit_depth
                .as_deref()
                .and_then(|depth| depth.trim().parse().ok())
        })
        .unwrap_or(8);
    let deep = source.video_profile.as_deref().is_some_and(is_deep_profile);
    let depth = source.video_bit_depth.unwrap_or(if deep { 10 } else { 8 });
    if depth > max_depth {
        return false;
    }
    let squash = |profile: &str| -> String {
        profile
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let list = |value: Option<&str>| -> Vec<String> {
        value
            .unwrap_or_default()
            .split([',', '|'])
            .map(squash)
            .filter(|entry| !entry.is_empty())
            .collect()
    };

    // Profile: range extensions never; else the listed ones, or Main and Main 10
    let profile = facts
        .profile
        .as_deref()
        .or(source.video_profile.as_deref())
        .map(squash)
        .unwrap_or_default();
    let pix_fmt = facts.pix_fmt.as_deref().unwrap_or_default();
    if profile.contains("rext") || ["422", "444"].iter().any(|c| pix_fmt.contains(c)) {
        return false;
    }
    let profiles = list(query.hevc_profile.as_deref());
    let allowed = if profiles.is_empty() {
        vec!["main".to_string(), "main10".to_string()]
    } else {
        profiles
    };
    if !allowed.contains(&profile) {
        return false;
    }

    // Level: within the client's, when it gives one; an unknown level then fails
    if let Some(max_level) = query
        .hevc_level
        .as_deref()
        .and_then(|level| level.trim().parse::<u32>().ok())
        && facts.level.is_none_or(|level| level > max_level)
    {
        return false;
    }

    // Range: SDR plays anywhere; anything else only when listed
    let ranges = list(query.hevc_range_type.as_deref());
    let listed = |name: &str| ranges.contains(&squash(name));
    match hevc_range(facts) {
        "SDR" | "DOVIWithSDR" => true,
        "HDR10" => listed("HDR10"),
        "HLG" => listed("HLG"),
        "DOVIWithHDR10" => listed("DOVI") || listed("DOVIWithHDR10") || listed("HDR10"),
        "DOVIWithHLG" => listed("DOVI") || listed("DOVIWithHLG") || listed("HLG"),
        _ => listed("DOVI"),
    }
}

/// Whether a source size is known and within a client limit. No limit always fits.
fn size_fits(limit: Option<u32>, size: Option<u32>) -> bool {
    limit.is_none_or(|max| size.is_some_and(|s| s <= max))
}

/// Whether a source bitrate is known and within a client limit, both in bits per second.
/// No limit always fits.
fn bitrate_fits(limit: Option<u32>, bits_per_sec: Option<u32>) -> bool {
    limit.is_none_or(|max| bits_per_sec.is_some_and(|bps| bps <= max))
}

/// Whether a profile name marks a 10-bit, 4:2:2 or 4:4:4 stream (e.g. "High 10", "High 4:2:2").
fn is_deep_profile(profile: &str) -> bool {
    let profile = profile.to_ascii_lowercase();
    ["10", "422", "4:2:2", "444", "4:4:4"]
        .iter()
        .any(|mark| profile.contains(mark))
}

/// What a request states about the H.264 a client takes, in Jellyfin's parameters.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct H264Request<'a> {
    pub profile: Option<&'a str>,
    pub h264_profile: Option<&'a str>,
    pub max_video_bit_depth: Option<u32>,
    pub h264_video_bit_depth: Option<&'a str>,
}

/// The profiles a request lists: those of the first of `profile` and `h264_profile` that lists any,
/// split on commas and pipes, trimmed, without the empty entries.
fn listed_profiles(request: H264Request<'_>) -> Vec<&str> {
    [request.profile, request.h264_profile]
        .into_iter()
        .flatten()
        .map(|list| {
            list.split([',', '|'])
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>()
        })
        .find(|entries| !entries.is_empty())
        .unwrap_or_default()
}

/// Whether two profile names are the same, ignoring spaces and ASCII case ("High 10" is "high10").
fn same_profile(a: &str, b: &str) -> bool {
    let key = |name: &str| {
        name.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    key(a) == key(b)
}

/// The place of an H.264 profile in Jellyfin's order of profiles, or None for one it does not name.
fn h264_score(profile: &str) -> Option<usize> {
    const ORDER: [&str; 8] = [
        "ConstrainedBaseline",
        "Baseline",
        "Extended",
        "Main",
        "High",
        "ProgressiveHigh",
        "ConstrainedHigh",
        "High10",
    ];
    ORDER.iter().position(|known| same_profile(known, profile))
}

/// Whether a client that makes `request` takes an H.264 stream with `profile` and `bit_depth` as it
/// is. A request that states no profile list and no depth limit (the web client) takes only 8-bit
/// H.264 with a profile that is not deep. Otherwise (Jellyfin's CanStreamCopyVideo) the profile must
/// be listed, or score no higher than the first listed one, and the depth must be within the limit.
/// A profile or depth that is not known passes.
pub(crate) fn client_takes_h264(
    request: H264Request<'_>,
    profile: Option<&str>,
    bit_depth: Option<u32>,
) -> bool {
    // The stored depth, else 10 when the profile name says so ("High 10")
    let depth = bit_depth.or_else(|| profile.filter(|name| name.contains("10")).map(|_| 10));
    let listed = listed_profiles(request);
    let limit = request.max_video_bit_depth.or_else(|| {
        request
            .h264_video_bit_depth
            .and_then(|bits| bits.parse().ok())
    });
    if listed.is_empty() && limit.is_none() {
        return depth.is_none_or(|bits| bits <= 8) && !profile.is_some_and(is_deep_profile);
    }
    // A profile that the list does not name must score no higher than the list's first entry
    let profile_taken = match (profile, listed.first()) {
        (Some(name), Some(first)) if !listed.iter().any(|entry| same_profile(entry, name)) => {
            matches!((h264_score(name), h264_score(first)), (Some(s), Some(top)) if s <= top)
        }
        _ => true,
    };
    let depth_taken = match (limit, depth) {
        (Some(max), Some(bits)) => bits <= max,
        _ => true,
    };
    profile_taken && depth_taken
}

/// What is burned into the picture: a picture subtitle overlaid by its absolute stream index, or a
/// text subtitle drawn by the subtitles filter at its place among the source's subtitles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BurnIn {
    Image(u32),
    Text(u32),
}

/// The tracks a transcode plays: the audio stream by its absolute index, and the burn-in.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tracks {
    audio: Option<u32>,
    burn_in: Option<BurnIn>,
}

/// Whether the request asks for a subtitle to be burned in: BurnSubtitles=true, or a
/// SubtitleMethod of Encode in any case.
fn burn_requested(query: &TranscodeQuery) -> bool {
    query.burn_subtitles == Some(true)
        || query
            .subtitle_method
            .as_deref()
            .is_some_and(|method| method.eq_ignore_ascii_case("encode"))
}

/// The tracks a transcode plays. The audio is the stream the request names, else the default one,
/// else the first. The burn-in is the subtitle the request names, else the first subtitle stream.
fn choose_tracks(query: &TranscodeQuery, source: &SourceInfo) -> Tracks {
    let requested = query
        .audio_stream_index
        .and_then(|index| u32::try_from(index).ok())
        .and_then(|index| source.audio.iter().find(|track| track.index == index));
    let audio = requested
        .or_else(|| source.audio.iter().find(|track| track.is_default))
        .or_else(|| source.audio.first())
        .map(|track| track.index);

    let burn_in = if burn_requested(query) {
        // The subtitle with its place among the subtitle streams. A negative index matches no
        // subtitle stream, so it burns nothing in.
        let subtitle = match query.subtitle_stream_index {
            None => source.subtitles.first().map(|track| (0, track)),
            Some(index) => source
                .subtitles
                .iter()
                .zip(0..)
                .find(|(track, _)| i64::from(track.index) == i64::from(index))
                .map(|(track, place)| (place, track)),
        };
        subtitle.map(|(place, track)| {
            if is_image_subtitle(&track.codec) {
                BurnIn::Image(track.index)
            } else {
                BurnIn::Text(place)
            }
        })
    } else {
        None
    };

    Tracks { audio, burn_in }
}

/// Which streams can be copied as they are, as (video, audio). A copy needs every rule to hold;
/// anything else is encoded.
fn stream_copy(query: &TranscodeQuery, source: &SourceInfo) -> (bool, bool) {
    let tracks = choose_tracks(query, source);
    let audio = tracks
        .audio
        .and_then(|index| source.audio.iter().find(|track| track.index == index));
    let copy_h264 = matches!(source.video_codec.as_deref(), Some("h264" | "avc" | "avc1"))
        && client_takes(query.video_codec.as_deref(), &["h264", "avc", "avc1"])
        && client_takes_h264(
            H264Request {
                profile: query.profile.as_deref(),
                h264_profile: query.h264_profile.as_deref(),
                max_video_bit_depth: query.max_video_bit_depth,
                h264_video_bit_depth: query.h264_video_bit_depth.as_deref(),
            },
            source.video_profile.as_deref(),
            source.video_bit_depth,
        );
    // HEVC is copied only for a client that lists it; it goes out in fragmented MP4
    let copy_hevc = is_hevc(source)
        && client_lists(query.video_codec.as_deref(), &HEVC_NAMES)
        && client_takes_hevc(query, source);
    // The codec options that the copied stream answers to: hevc-level for HEVC, h264-level else
    let codec = if is_hevc(source) { "hevc" } else { "h264" };
    let copy_video = !query.audio_only.unwrap_or(false)
        && tracks.burn_in.is_none()
        && (copy_h264 || copy_hevc)
        && size_fits(query.max_width, source.width)
        && size_fits(query.max_height, source.height)
        && bitrate_fits(query.video_bitrate, source.video_bitrate)
        // Jellyfin's codec conditions: a level over the client's, interlacing it can't show, or
        // non-square pixels it can't scale all need an encode
        && source
            .video_level
            .zip(query.max_level(codec))
            .is_none_or(|(level, max)| level <= max)
        && !(source.interlaced && query.wants_deinterlaced(codec))
        && !(source.anamorphic && query.wants_square_pixels());
    let copy_audio = audio.is_some_and(|track| {
        track.codec == "aac"
            && client_takes(query.audio_codec.as_deref(), &["aac", "mp4a"])
            && bitrate_fits(query.audio_bitrate, track.bitrate.or(source.audio_bitrate))
    });
    (copy_video, copy_audio)
}

/// The profile of a transcode: the query's settings, the streams it copies, and the tracks it
/// plays.
fn transcode_profile(query: TranscodeQuery, source: &SourceInfo) -> TranscodeProfile {
    let (copy_video, copy_audio) = stream_copy(&query, source);
    let tracks = choose_tracks(&query, source);
    // An encode deinterlaces an interlaced source, as Jellyfin's does, and makes the pixels
    // square when the client asks
    let deinterlace = !copy_video && source.interlaced;
    let square_pixels = !copy_video && source.anamorphic && query.wants_square_pixels();
    let mut profile = query.into_profile();
    profile.deinterlace = deinterlace;
    profile.square_pixels = square_pixels;
    profile.copy_video = copy_video;
    profile.copy_audio = copy_audio;
    // Copied HEVC needs fragmented MP4 segments (browsers and Apple players take no HEVC in
    // MPEG-TS) and the hvc1 tag, as Jellyfin sends it
    if copy_video && is_hevc(source) {
        profile.fmp4 = true;
        profile.video_tag = Some("hvc1".to_string());
    }
    profile.audio_stream_index = tracks.audio;
    match tracks.burn_in {
        Some(BurnIn::Image(stream_index)) => {
            profile.image_subtitle = Some(ImageSubtitle {
                stream_index,
                video_size: source.width.zip(source.height),
            });
        }
        Some(BurnIn::Text(place)) => {
            profile.burn_subtitles = true;
            profile.subtitle_stream_index = Some(place);
        }
        None => {}
    }
    profile
}

/// The query's track indexes, which a client writes in Jellyfin's numbering (external subtitle
/// files first), as indexes in `source`'s file. An external subtitle names no stream of the
/// file: no burn-in.
pub(crate) async fn to_file_numbering(
    mut query: TranscodeQuery,
    source: Option<&MediaSourceResponse>,
) -> TranscodeQuery {
    let externals = match source {
        Some(source) => crate::routes::subtitle::external_count(source).await,
        None => 0,
    };
    let convert = |index: Option<i32>| {
        index.map(|index| crate::routes::subtitle::file_index(index, externals).unwrap_or(-1))
    };
    query.audio_stream_index = convert(query.audio_stream_index);
    query.subtitle_stream_index = convert(query.subtitle_stream_index);
    query
}

/// The profile of a progressive video transcode (`/Videos/{id}/stream` without `static=true`):
/// the HLS rules for tracks, burn-in and the H.264/AAC copies, then the client's own codec lists.
/// A listed source codec is copied (the video only without a burn-in and within the size and
/// bitrate limits), and what is encoded uses the first listed codec Rustyfin encodes, else H.264
/// and AAC.
pub(crate) fn progressive_video_profile(
    query: TranscodeQuery,
    params: &HashMap<String, String>,
    source: Option<&MediaSourceResponse>,
) -> TranscodeProfile {
    let query = query.with_codec_options(params);
    let info = source.map_or_else(SourceInfo::default, SourceInfo::from_source);
    let listed = |list: Option<&str>| -> Vec<String> {
        list.unwrap_or_default()
            .split(',')
            .map(|codec| codec.trim().to_ascii_lowercase())
            .filter(|codec| !codec.is_empty())
            .collect()
    };
    let video_codecs = listed(query.video_codec.as_deref());
    let audio_codecs = listed(query.audio_codec.as_deref());
    let tracks = choose_tracks(&query, &info);
    let audio_track = tracks
        .audio
        .and_then(|index| info.audio.iter().find(|track| track.index == index));
    let fits_video = size_fits(query.max_width, info.width)
        && size_fits(query.max_height, info.height)
        && bitrate_fits(query.video_bitrate, info.video_bitrate);
    let audio_fits = audio_track.is_some_and(|track| {
        bitrate_fits(query.audio_bitrate, track.bitrate.or(info.audio_bitrate))
    });
    let mut profile = transcode_profile(query, &info);
    if !profile.copy_video
        && tracks.burn_in.is_none()
        && !profile.audio_only
        && !info.interlaced
        && fits_video
        && info
            .video_codec
            .as_ref()
            .is_some_and(|codec| video_codecs.contains(codec) && !is_hevc(&info))
        && !matches!(info.video_codec.as_deref(), Some("h264" | "avc" | "avc1"))
    {
        profile.copy_video = true;
    }
    if !profile.copy_audio
        && audio_fits
        && audio_track.is_some_and(|track| audio_codecs.contains(&track.codec))
    {
        profile.copy_audio = true;
    }
    if let Some(encoder) = video_codecs.iter().find_map(|c| video_encoder(c)) {
        profile.video_codec = encoder.to_string();
    }
    if let Some(encoder) = audio_codecs.iter().find_map(|c| audio_encoder(c)) {
        profile.audio_codec = encoder.to_string();
    }
    // One output file, no segments
    profile.fmp4 = false;
    profile
}

/// ffmpeg's input for `version` of an item at `item_path`, and the source whose streams decide
/// stream copy.
fn hls_input<'a>(
    item_path: &'a str,
    sources: &'a [MediaSourceResponse],
    version: Version,
) -> (&'a str, Option<&'a MediaSourceResponse>) {
    match version {
        Version::Primary => (item_path, sources.first()),
        Version::Source(index) => (sources[index].path.as_str(), Some(&sources[index])),
    }
}

async fn hls_master_playlist(
    State(state): State<AppState>,
    claims: Option<axum::Extension<rustyfin_model::auth::Claims>>,
    local: Option<axum::Extension<crate::middleware::ClientIsLocal>>,
    Path(id): Path<Uuid>,
    Query(query): Query<TranscodeQuery>,
    Query(params): Query<HashMap<String, String>>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, ApiError> {
    let query = query.with_codec_options(&params);
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    // The version the request names; an id that names none is refused before ffmpeg starts
    let sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .unwrap_or_default();
    let version = select_version(id, &sources, requested_source_id(&params))?;
    let (input_path, source) = hls_input(&item.path, &sources, version);
    // ffmpeg reads only a file that the streams' library check passes, by its resolved path
    let input_path = validate_path_against_library(&state, input_path).await?;
    let input_path = input_path.as_str();
    let mut query = to_file_numbering(query, source).await;

    // Copy only what the client plays; with no source to read, nothing is copied
    let mut info = source.map_or_else(SourceInfo::default, SourceInfo::from_source);
    // An HEVC video a client might take as it is gets probed for what the stored streams lack
    if is_hevc(&info) && client_lists(query.video_codec.as_deref(), &HEVC_NAMES) {
        info.hevc = hevc_facts(input_path).await;
    }
    // The caller's policy: the remote bitrate limit before the copy decision, then whether the
    // stream that decision makes (a transcode or a remux) is allowed
    let access = match &claims {
        Some(axum::Extension(claims)) => crate::parental::caller_access(&state, claims).await?,
        None => Default::default(),
    };
    let local = local.is_none_or(|axum::Extension(l)| l.0);
    if matches!(item.media_type, rustyfin_model::media::MediaType::Audio) {
        query.audio_bitrate = crate::parental::remote_bitrate(&access, local, query.audio_bitrate);
    } else {
        query.video_bitrate = crate::parental::remote_bitrate(&access, local, query.video_bitrate);
    }
    let tracks = choose_tracks(&query, &info);
    let (copy_video, copy_audio) = stream_copy(&query, &info);
    crate::parental::check_conversion(
        &access,
        matches!(item.media_type, rustyfin_model::media::MediaType::Audio),
        copy_video,
        copy_audio,
    )?;
    // Every playlist reload passes here; the manager logs the ffmpeg command when it starts one
    tracing::debug!(
        item = %id,
        copy_video,
        copy_audio,
        audio = ?tracks.audio,
        burn_in = ?tracks.burn_in,
        "HLS playlist requested"
    );
    // Burn-in asked for by a stream index that no subtitle stream has: nothing is burned in
    if tracks.burn_in.is_none()
        && burn_requested(&query)
        && query.subtitle_stream_index.is_some_and(|index| index >= 0)
    {
        tracing::debug!(
            item = %id,
            subtitle = ?query.subtitle_stream_index,
            "Burn-in names no subtitle stream"
        );
    }

    let play_session_id = query.play_session_id.clone().unwrap_or_default();
    let device_id = query.device_id.clone().unwrap_or_default();
    let profile = transcode_profile(query, &info);

    let (playlist_path, playback_id) = match state
        .transcode_manager
        .start_transcode(id, input_path, &profile, &play_session_id, &device_id)
        .await
    {
        Ok(started) => started,
        // ffmpeg has not written the playlist yet; the client should ask again shortly
        Err(TranscodeError::PlaylistTimeout) => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "5")],
            )
                .into_response());
        }
        Err(TranscodeError::Stopped) => {
            return Err(ApiError::NotFound("The play session was stopped".into()));
        }
        Err(TranscodeError::FfmpegFailed(msg)) => {
            return Err(ApiError::Internal(format!("Transcode failed: {msg}")));
        }
        Err(e) => return Err(ApiError::Internal(format!("Transcode failed: {e}"))),
    };

    // Read the playlist and point its segments at the segment route, with this request's query
    let content = tokio::fs::read_to_string(&playlist_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read playlist: {e}")))?;
    let rewritten = rewrite_playlist(&content, &playback_id, raw_query.as_deref());

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        rewritten,
    )
        .into_response())
}

/// Point each segment line of an FFmpeg playlist at the segment route:
/// `hls/{playback_id}/video/{segment}`, with `?{query}` when the query is non-empty. The query is
/// the playlist request's own, so the `api_key` in it authorises the segment requests. The
/// fragmented MP4 init segment (`#EXT-X-MAP:URI="init.mp4"`) points there too; empty lines and
/// the other `#` tags stay as they are.
fn rewrite_playlist(content: &str, playback_id: &Uuid, query: Option<&str>) -> String {
    let suffix = query
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    content
        .split('\n')
        .map(|line| {
            if let Some(uri) = line
                .strip_prefix("#EXT-X-MAP:URI=\"")
                .and_then(|rest| rest.strip_suffix('"'))
            {
                // The fragmented MP4 init segment
                format!("#EXT-X-MAP:URI=\"hls/{playback_id}/video/{uri}{suffix}\"")
            } else if line.is_empty() || line.starts_with('#') {
                line.to_string()
            } else {
                format!("hls/{playback_id}/video/{line}{suffix}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Handle Jellyfin HLS segment requests: /Videos/{id}/hls/{playbackId}/{type}/{segment}
async fn hls_segment(
    State(state): State<AppState>,
    Path((id, playback_id, segment_type, segment)): Path<(Uuid, Uuid, String, String)>,
) -> Result<Response, ApiError> {
    // segment_type is typically "video", "audio", or "sub" — we just ignore it
    // and serve the segment file by name
    let _ = segment_type;

    // The playback id names the session, which must belong to this item
    let segment_path = state
        .transcode_manager
        .segment_path(id, playback_id, &segment)
        .ok_or_else(|| ApiError::NotFound(format!("Segment {segment} not found for item {id}")))?;

    let data = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?;

    let content_type = if segment.ends_with(".ts") {
        "video/mp2t"
    } else if segment.ends_with(".mp4") || segment.ends_with(".m4s") {
        "video/mp4"
    } else if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        data,
    )
        .into_response())
}

/// Legacy segment endpoint for backward compatibility: /Videos/{id}/hls/{segment}
async fn hls_segment_legacy(
    State(state): State<AppState>,
    Path((id, segment)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    // `{id}` must be an item, which the parental gate has already checked for the caller. A
    // session's playback id (whose directory sits beside the items') is not one, so another
    // user's session can't be read through this route by its playback id.
    let not_found = || ApiError::NotFound(format!("Segment {segment} not found for item {id}"));
    rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(not_found)?;
    let segment_path = state
        .transcode_manager
        .legacy_segment_path(id, &segment)
        .ok_or_else(not_found)?;

    let data = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?;

    let content_type = if segment.ends_with(".ts") {
        "video/mp2t"
    } else if segment.ends_with(".mp4") || segment.ends_with(".m4s") {
        "video/mp4"
    } else if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        data,
    )
        .into_response())
}

/// Look up a query parameter by name, ignoring ASCII case (clients send both `PlaySessionId`
/// and `playSessionId`). A missing parameter reads as "".
fn query_value<'a>(query: &'a HashMap<String, String>, name: &str) -> &'a str {
    query
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value.as_str())
}

/// DELETE /Videos/{id}/Transcode: stop this item's transcodes for one play session or device.
async fn stop_transcode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let play_session_id = query_value(&query, "PlaySessionId");
    let device_id = query_value(&query, "DeviceId");
    state
        .transcode_manager
        .stop_play_session(Some(id), play_session_id, device_id)
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// DELETE /Videos/ActiveEncodings: stop the transcodes of a play session or device, any item.
/// A request that names neither is refused.
async fn stop_active_encodings(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let play_session_id = query_value(&query, "PlaySessionId");
    let device_id = query_value(&query, "DeviceId");
    if play_session_id.is_empty() && device_id.is_empty() {
        return Err(ApiError::BadRequest(
            "PlaySessionId or DeviceId is required".to_string(),
        ));
    }
    state
        .transcode_manager
        .stop_play_session(None, play_session_id, device_id)
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An audio or subtitle stream at absolute index `index`.
    fn track(index: u32, codec: &str) -> Track {
        Track {
            index,
            codec: codec.into(),
            ..Default::default()
        }
    }

    /// An 8-bit H.264 High 1920x1080 source at 8 Mbit/s: the video is stream 0, then one audio
    /// stream per `audio` codec from index 1, none flagged default, with 192 kbit/s audio.
    fn h264_with(audio: &[&str]) -> SourceInfo {
        SourceInfo {
            video_codec: Some("h264".into()),
            video_profile: Some("High".into()),
            video_bit_depth: Some(8),
            width: Some(1920),
            height: Some(1080),
            video_bitrate: Some(8_000_000),
            video_level: Some(41.0),
            interlaced: false,
            anamorphic: false,
            audio: audio.iter().zip(1..).map(|(c, i)| track(i, c)).collect(),
            audio_bitrate: Some(192_000),
            subtitles: Vec::new(),
            hevc: None,
        }
    }

    /// Ghost Stories' streams: 1440x1080 H.264; FLAC Japanese (default) and English at 1 and 2;
    /// PGS at 3, 4 and 5; SubRip at 6 (default), 7 and 8.
    fn anime() -> SourceInfo {
        SourceInfo {
            width: Some(1440),
            audio: vec![
                Track {
                    is_default: true,
                    ..track(1, "flac")
                },
                track(2, "flac"),
            ],
            subtitles: vec![
                track(3, "hdmv_pgs_subtitle"),
                track(4, "hdmv_pgs_subtitle"),
                track(5, "hdmv_pgs_subtitle"),
                Track {
                    is_default: true,
                    ..track(6, "subrip")
                },
                track(7, "subrip"),
                track(8, "subrip"),
            ],
            ..h264_with(&[])
        }
    }

    /// The query a playlist request with `params` carries, parsed as the route parses it.
    fn query(params: &str) -> TranscodeQuery {
        let uri = format!("/Videos/item/main.m3u8?{params}").parse().unwrap();
        let map = Query::<HashMap<String, String>>::try_from_uri(&uri)
            .unwrap()
            .0;
        Query::<TranscodeQuery>::try_from_uri(&uri)
            .unwrap()
            .0
            .with_codec_options(&map)
    }

    /// The lists of a browser that plays H.264 but not AC3.
    fn browser_query() -> TranscodeQuery {
        TranscodeQuery {
            video_codec: Some("h264,vp8,vp9,av1".into()),
            audio_codec: Some("aac,mp3,opus,flac".into()),
            ..Default::default()
        }
    }

    #[test]
    fn test_start_time_ticks_reaches_the_profile() {
        // Universal forwards a resume or seek point as StartTimeTicks
        let query: TranscodeQuery =
            serde_json::from_value(serde_json::json!({ "StartTimeTicks": 6_000_000_000_i64 }))
                .unwrap();
        assert_eq!(query.into_profile().start_time_ticks, 6_000_000_000);
        assert_eq!(TranscodeQuery::default().into_profile().start_time_ticks, 0);
    }

    /// The bitrates are bits per second, always, as in Jellyfin: the TranscodingUrl writes them
    /// so, and Universal passes a client's MaxVideoBitrate and MaxAudioBitrate through as
    /// VideoBitrate and AudioBitrate. Before, 4000000 read as kbps capped nothing.
    /// The URLs the stream builder writes for a level the client can't play, interlaced video
    /// or non-square pixels give an encode with the matching filters, down to the ffmpeg
    /// arguments; before, the route read none of those options and copied the video.
    #[test]
    fn r3decisions_level_scan_and_shape_requests_encode() {
        let args_for = |params: &str, source: SourceInfo| {
            let profile = transcode_profile(query(params), &source);
            rustyfin_core::transcode::build_ffmpeg_args(
                "/in.mkv",
                "/out/main.m3u8",
                "/out/seg_%04d.ts",
                &profile,
            )
        };
        let copies = |args: &[String]| args.windows(2).any(|w| w[0] == "-c:v" && w[1] == "copy");
        let filter = |args: &[String]| {
            args.windows(2)
                .find(|w| w[0] == "-vf")
                .map(|w| w[1].clone())
                .unwrap_or_default()
        };
        let lists = "VideoCodec=h264&AudioCodec=aac";

        // Level 4.1 fits h264-level=41 and Level=51, so the video is copied
        for within in ["h264-level=41", "Level=51"] {
            let args = args_for(&format!("{lists}&{within}"), h264_with(&["aac"]));
            assert!(copies(&args), "{within}: {args:?}");
        }
        // Over h264-level=40, or Level=40 with no h264 option: encoded
        for over in ["h264-level=40", "Level=40", "H264-Level=40"] {
            let args = args_for(&format!("{lists}&{over}"), h264_with(&["aac"]));
            assert!(!copies(&args), "{over}: {args:?}");
            assert!(args.iter().any(|a| a == "libx264"), "{over}: {args:?}");
        }

        // Interlaced video for a client that asks for progressive: encoded and deinterlaced
        let interlaced = SourceInfo {
            interlaced: true,
            ..h264_with(&["aac"])
        };
        let args = args_for(&format!("{lists}&h264-deinterlace=true"), interlaced);
        assert!(!copies(&args), "{args:?}");
        assert!(filter(&args).starts_with("bwdif=0,"), "{args:?}");

        // Non-square pixels with RequireNonAnamorphic=True: encoded and scaled to square pixels
        let anamorphic = || SourceInfo {
            anamorphic: true,
            ..h264_with(&["aac"])
        };
        let args = args_for(&format!("{lists}&RequireNonAnamorphic=True"), anamorphic());
        assert!(!copies(&args), "{args:?}");
        assert!(
            filter(&args).starts_with("scale=trunc(iw*sar/2)*2:ih,setsar=1,"),
            "{args:?}"
        );
        // Without the request the same file is copied
        assert!(copies(&args_for(lists, anamorphic())));
    }

    #[test]
    fn r3decisions_url_bitrates_are_bits_per_second() {
        let profile = query("VideoBitrate=4000000&AudioBitrate=192000").into_profile();
        assert_eq!(profile.max_video_bitrate_kbps, 4000);
        assert_eq!(profile.max_audio_bitrate_kbps, 192);
        let profile = query("VideoBitrate=119616000&AudioBitrate=384000").into_profile();
        assert_eq!(profile.max_video_bitrate_kbps, 119_616);
        assert_eq!(profile.max_audio_bitrate_kbps, 384);
        // No guessing from the size: 4000 is 4 kbit/s
        let small = query("VideoBitrate=4000&AudioBitrate=192").into_profile();
        assert_eq!(small.max_video_bitrate_kbps, 4);
        assert_eq!(small.max_audio_bitrate_kbps, 0);

        // A video at 6 Mbit/s is over a 4 Mbit/s cap, so it isn't copied
        let source = SourceInfo {
            video_bitrate: Some(6_000_000),
            ..h264_with(&[])
        };
        let capped = TranscodeQuery {
            video_bitrate: Some(4_000_000),
            ..browser_query()
        };
        assert!(!stream_copy(&capped, &source).0);
        let roomy = TranscodeQuery {
            video_bitrate: Some(8_000_000),
            ..browser_query()
        };
        assert!(stream_copy(&roomy, &source).0);
    }

    #[test]
    fn test_profile_encodes_h264_and_aac_whatever_the_lists_say() {
        let profile = TranscodeQuery {
            video_codec: Some("h264,hevc,vp8,vp9,av1".into()),
            audio_codec: Some("mp3,aac,opus,ac3,flac".into()),
            ..Default::default()
        }
        .into_profile();
        assert_eq!(profile.video_codec, "libx264");
        assert_eq!(profile.audio_codec, "aac");
        assert!(!profile.copy_video && !profile.copy_audio);
    }

    #[test]
    fn test_h264_with_ac3_copies_video_and_encodes_audio() {
        assert_eq!(
            stream_copy(&browser_query(), &h264_with(&["ac3"])),
            (true, false)
        );
    }

    #[test]
    fn test_h264_with_aac_copies_both() {
        assert_eq!(
            stream_copy(&browser_query(), &h264_with(&["aac"])),
            (true, true)
        );
        // A client without lists is assumed to play H.264 and AAC
        assert_eq!(
            stream_copy(&TranscodeQuery::default(), &h264_with(&["aac"])),
            (true, true)
        );
        // Limits the source meets keep the copy
        let query = TranscodeQuery {
            max_width: Some(1920),
            max_height: Some(1080),
            video_bitrate: Some(8_000_000),
            audio_bitrate: Some(192_000),
            ..browser_query()
        };
        assert_eq!(stream_copy(&query, &h264_with(&["aac"])), (true, true));
        // BurnSubtitles on a source without subtitle streams burns nothing in, so the copy stays
        let query = TranscodeQuery {
            burn_subtitles: Some(true),
            ..browser_query()
        };
        assert_eq!(stream_copy(&query, &h264_with(&["aac"])), (true, true));
    }

    #[test]
    fn test_video_is_encoded_when_it_must_be() {
        let encoded = |query: TranscodeQuery, source: SourceInfo| {
            assert!(!stream_copy(&query, &source).0, "{query:?} {source:?}");
        };
        // Not H.264, or a client that does not list it
        for codec in ["hevc", "mpeg4", "vp9"] {
            let source = SourceInfo {
                video_codec: Some(codec.into()),
                ..h264_with(&["aac"])
            };
            encoded(browser_query(), source);
        }
        let query = TranscodeQuery {
            video_codec: Some("hevc,vp9".into()),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        // 10-bit, 4:2:2 and 4:4:4 profiles, or more than 8 bits
        for profile in ["High 10", "High 4:2:2", "High 4:4:4 Predictive"] {
            let source = SourceInfo {
                video_profile: Some(profile.into()),
                ..h264_with(&["aac"])
            };
            encoded(browser_query(), source);
        }
        let source = SourceInfo {
            video_bit_depth: Some(10),
            ..h264_with(&["aac"])
        };
        encoded(browser_query(), source);
        // Larger than MaxWidth or MaxHeight, or of unknown size
        let query = TranscodeQuery {
            max_width: Some(1280),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            max_height: Some(720),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            max_width: Some(1920),
            ..browser_query()
        };
        let source = SourceInfo {
            width: None,
            ..h264_with(&["aac"])
        };
        encoded(query, source);
        // Over VideoBitrate (bits per second), or of unknown bitrate
        let query = TranscodeQuery {
            video_bitrate: Some(4_000_000),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            video_bitrate: Some(20_000_000),
            ..browser_query()
        };
        let source = SourceInfo {
            video_bitrate: None,
            ..h264_with(&["aac"])
        };
        encoded(query, source);
        // Audio only, a subtitle to burn in, or nothing known about the source
        let query = TranscodeQuery {
            audio_only: Some(true),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let with_subtitles = || SourceInfo {
            subtitles: vec![track(2, "subrip"), track(3, "dvd_subtitle")],
            ..h264_with(&["aac"])
        };
        let query = TranscodeQuery {
            burn_subtitles: Some(true),
            ..browser_query()
        };
        encoded(query, with_subtitles());
        for index in [2, 3] {
            let query = TranscodeQuery {
                subtitle_stream_index: Some(index),
                subtitle_method: Some("Encode".into()),
                ..browser_query()
            };
            encoded(query, with_subtitles());
        }
        encoded(browser_query(), SourceInfo::default());
    }

    #[test]
    fn test_audio_is_encoded_when_it_must_be() {
        let copied = |query: TranscodeQuery, source: SourceInfo| stream_copy(&query, &source).1;
        // The audio stream that plays is not AAC: the first when none is flagged, the chosen one,
        // or none at all
        assert!(!copied(browser_query(), h264_with(&["ac3"])));
        assert!(!copied(browser_query(), h264_with(&["ac3", "aac"])));
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac", "ac3"])));
        assert!(!copied(browser_query(), h264_with(&[])));
        // A client that does not list AAC
        let query = TranscodeQuery {
            audio_codec: Some("mp3,opus".into()),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac"])));
        // Over AudioBitrate (bits per second), or of unknown bitrate
        let query = TranscodeQuery {
            audio_bitrate: Some(128_000),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac"])));
        let query = TranscodeQuery {
            audio_bitrate: Some(320_000),
            ..browser_query()
        };
        let source = SourceInfo {
            audio_bitrate: None,
            ..h264_with(&["aac"])
        };
        assert!(!copied(query, source));
        // "mp4a" in the list means AAC
        let query = TranscodeQuery {
            audio_codec: Some("mp4a".into()),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["aac"])));
    }

    #[test]
    fn test_the_aac_stream_that_plays_is_copied() {
        let copied = |query: TranscodeQuery, source: SourceInfo| stream_copy(&query, &source).1;
        // Chosen by its index beside other audio streams, or flagged as the default
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["ac3", "aac"])));
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["aac", "aac"])));
        let source = SourceInfo {
            audio: vec![
                track(1, "ac3"),
                Track {
                    is_default: true,
                    ..track(2, "aac")
                },
            ],
            ..h264_with(&[])
        };
        assert!(copied(browser_query(), source));
        // The stream's own bitrate counts against AudioBitrate before the source's
        let source = SourceInfo {
            audio: vec![Track {
                bitrate: Some(96_000),
                ..track(1, "aac")
            }],
            audio_bitrate: Some(640_000),
            ..h264_with(&[])
        };
        let query = TranscodeQuery {
            audio_bitrate: Some(128_000),
            ..browser_query()
        };
        assert!(copied(query, source));
    }

    #[test]
    fn test_source_info_reads_the_streams() {
        let source: rustyfin_model::media_source::MediaSourceResponse =
            serde_json::from_value(serde_json::json!({
                "Id": "source",
                "Uid": "source",
                "Protocol": "File",
                "Path": "/media/movie.mkv",
                "Bitrate": 9_000_000,
                "VideoBitrate": 8_000_000,
                "AudioBitrate": 448_000,
                "MediaStreams": [
                    { "Index": 0, "CodecType": "Video", "Codec": "h264", "Profile": "High",
                      "BitDepth": 8, "Width": 1920, "Height": 1080 },
                    { "Index": 1, "CodecType": "Audio", "Codec": "AC3", "BitRate": 640_000 },
                    { "Index": 2, "CodecType": "Audio", "Codec": "aac", "IsDefault": true },
                    { "Index": 3, "CodecType": "Subtitle", "Codec": "subrip" },
                    { "Index": 4, "CodecType": "subtitle", "Codec": "hdmv_pgs_subtitle" },
                    { "Index": 5, "CodecType": "subtitle", "Codec": "srt", "IsExternal": true,
                      "Path": "/media/movie.en.srt" },
                ],
            }))
            .unwrap();
        let info = SourceInfo::from_source(&source);
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.video_profile.as_deref(), Some("High"));
        assert_eq!(info.video_bit_depth, Some(8));
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        // No bitrate on the video stream: the source's video bitrate
        assert_eq!(info.video_bitrate, Some(8_000_000));
        // Each audio stream by its index, with a lowercase codec, its bitrate and its flag
        let tracks = |list: &[Track]| -> Vec<(u32, String, Option<u32>, bool)> {
            list.iter()
                .map(|t| (t.index, t.codec.clone(), t.bitrate, t.is_default))
                .collect()
        };
        assert_eq!(
            tracks(&info.audio),
            [
                (1, "ac3".to_string(), Some(640_000), false),
                (2, "aac".to_string(), None, true)
            ]
        );
        assert_eq!(info.audio_bitrate, Some(448_000));
        // The embedded subtitle streams; an external file is not a stream of the source
        assert_eq!(
            tracks(&info.subtitles),
            [
                (3, "subrip".to_string(), None, false),
                (4, "hdmv_pgs_subtitle".to_string(), None, false)
            ]
        );
    }

    #[test]
    fn test_negative_indexes_and_the_subtitle_method_parse() {
        // Jellyfin clients send -1 for "none"; as unsigned numbers that failed the whole query
        let q = query("AudioStreamIndex=-1&SubtitleStreamIndex=-1&SubtitleMethod=Encode");
        assert_eq!(q.audio_stream_index, Some(-1));
        assert_eq!(q.subtitle_stream_index, Some(-1));
        assert_eq!(q.subtitle_method.as_deref(), Some("Encode"));
    }

    #[test]
    fn test_audio_index_picks_that_audio_stream() {
        let audio = |params: &str| choose_tracks(&query(params), &anime()).audio;
        assert_eq!(audio("AudioStreamIndex=2"), Some(2));
        assert_eq!(audio("AudioStreamIndex=1"), Some(1));
        // Absent, negative, or not one of the audio streams: the default audio stream
        for params in [
            "",
            "AudioStreamIndex=-1",
            "AudioStreamIndex=0",
            "AudioStreamIndex=6",
            "AudioStreamIndex=40",
        ] {
            assert_eq!(audio(params), Some(1), "{params}");
        }
        // The default is the flagged stream, else the first; with no audio streams there is none
        let flagged = SourceInfo {
            audio: vec![
                track(1, "ac3"),
                Track {
                    is_default: true,
                    ..track(2, "aac")
                },
            ],
            ..anime()
        };
        assert_eq!(choose_tracks(&query(""), &flagged).audio, Some(2));
        let unflagged = h264_with(&["ac3", "aac"]);
        assert_eq!(choose_tracks(&query(""), &unflagged).audio, Some(1));
        for source in [h264_with(&[]), SourceInfo::default()] {
            let tracks = choose_tracks(&query("AudioStreamIndex=1"), &source);
            assert_eq!(tracks.audio, None);
        }
    }

    #[test]
    fn test_subtitle_burn_in_follows_the_method_and_the_stream() {
        let burn_in = |params: &str| choose_tracks(&query(params), &anime()).burn_in;
        // A picture subtitle is overlaid by its index; a text one goes through the subtitles
        // filter by its place among the subtitle streams
        let image = burn_in("SubtitleStreamIndex=4&SubtitleMethod=Encode");
        assert_eq!(image, Some(BurnIn::Image(4)));
        let text = burn_in("SubtitleStreamIndex=7&SubtitleMethod=Encode");
        assert_eq!(text, Some(BurnIn::Text(4)));
        // The method matches without case; BurnSubtitles=true asks for it too
        let image = burn_in("SubtitleStreamIndex=3&SubtitleMethod=encode");
        assert_eq!(image, Some(BurnIn::Image(3)));
        let text = burn_in("SubtitleStreamIndex=6&BurnSubtitles=true");
        assert_eq!(text, Some(BurnIn::Text(3)));
        // Without an index, the first subtitle stream, as BurnSubtitles did before
        assert_eq!(burn_in("BurnSubtitles=true"), Some(BurnIn::Image(3)));
        assert_eq!(burn_in("SubtitleMethod=Encode"), Some(BurnIn::Image(3)));
        // Nothing: no burn-in asked for, an index of -1, or one that names no embedded subtitle
        // stream (an audio stream, or an external file past the embedded streams)
        for params in [
            "SubtitleStreamIndex=4",
            "SubtitleStreamIndex=4&SubtitleMethod=Hls",
            "SubtitleStreamIndex=4&SubtitleMethod=External",
            "SubtitleStreamIndex=4&BurnSubtitles=false",
            "SubtitleStreamIndex=-1&SubtitleMethod=Encode",
            "SubtitleStreamIndex=1&SubtitleMethod=Encode",
            "SubtitleStreamIndex=9&SubtitleMethod=Encode",
        ] {
            assert_eq!(burn_in(params), None, "{params}");
        }
        let none = choose_tracks(&query("BurnSubtitles=true"), &h264_with(&["aac"]));
        assert_eq!(none.burn_in, None);
    }

    #[test]
    fn test_profile_carries_the_chosen_tracks() {
        // The English audio and the "English Dub PGS" picture subtitle, from 10 minutes in
        let params = "AudioStreamIndex=2&SubtitleStreamIndex=4&SubtitleMethod=Encode\
                      &StartTimeTicks=6000000000";
        let p = transcode_profile(query(params), &anime());
        assert_eq!(p.audio_stream_index, Some(2));
        let overlay = ImageSubtitle {
            stream_index: 4,
            video_size: Some((1440, 1080)),
        };
        assert_eq!(p.image_subtitle, Some(overlay));
        assert!(!p.burn_subtitles);
        assert_eq!(p.subtitle_stream_index, None);
        assert!(!p.copy_video && !p.copy_audio);
        assert_eq!(p.start_time_ticks, 6_000_000_000);
        // A text subtitle: the subtitles filter with its place among the subtitle streams
        let p = transcode_profile(
            query("SubtitleStreamIndex=8&SubtitleMethod=Encode"),
            &anime(),
        );
        assert_eq!(p.audio_stream_index, Some(1));
        assert!(p.burn_subtitles);
        assert_eq!(p.subtitle_stream_index, Some(5));
        assert_eq!(p.image_subtitle, None);
        assert!(!p.copy_video);
        // No subtitle: nothing burned in, so the video is copied
        let p = transcode_profile(query("SubtitleStreamIndex=-1&BurnSubtitles=true"), &anime());
        assert!(!p.burn_subtitles);
        assert_eq!((p.subtitle_stream_index, p.image_subtitle), (None, None));
        assert!(p.copy_video);
        // A picture subtitle on a video of unknown size is not scaled
        let unsized_source = SourceInfo {
            width: None,
            ..anime()
        };
        let p = transcode_profile(
            query("SubtitleStreamIndex=5&SubtitleMethod=Encode"),
            &unsized_source,
        );
        let overlay = ImageSubtitle {
            stream_index: 5,
            video_size: None,
        };
        assert_eq!(p.image_subtitle, Some(overlay));
        // The chosen AAC stream is copied, and its index still maps it
        let p = transcode_profile(query("AudioStreamIndex=2"), &h264_with(&["ac3", "aac"]));
        assert_eq!(p.audio_stream_index, Some(2));
        assert!(p.copy_audio);
    }

    #[test]
    fn test_rewrite_points_segments_at_the_segment_route_with_auth() {
        let playback_id = Uuid::new_v4();
        let content = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n\
                       #EXTINF:6.000000,\nsegment_0000.ts\n#EXTINF:6.000000,\nsegment_0001.ts\n";
        let query = "PlaySessionId=psid&api_key=placeholder";
        let segment = |n: &str| format!("hls/{playback_id}/video/segment_{n}.ts?{query}");
        assert_eq!(
            rewrite_playlist(content, &playback_id, Some(query)),
            format!(
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n\
                 #EXTINF:6.000000,\n{}\n#EXTINF:6.000000,\n{}\n",
                segment("0000"),
                segment("0001")
            )
        );
    }

    #[test]
    fn test_rewrite_without_query() {
        let playback_id = Uuid::new_v4();
        let content = "#EXTM3U\n#EXTINF:6.000000,\nsegment_0000.ts\n\n#EXT-X-ENDLIST\n";
        let expected = format!(
            "#EXTM3U\n#EXTINF:6.000000,\nhls/{playback_id}/video/segment_0000.ts\n\n#EXT-X-ENDLIST\n"
        );
        assert_eq!(rewrite_playlist(content, &playback_id, None), expected);
        assert_eq!(rewrite_playlist(content, &playback_id, Some("")), expected);
    }

    // ── B16: the transcode follows the version ──

    #[test]
    fn test_hls_input_follows_the_version() {
        use crate::routes::streaming::Version;

        let source = |path: &str, video: &str| -> MediaSourceResponse {
            serde_json::from_value(serde_json::json!({
                "Id": Uuid::new_v4().to_string(),
                "Uid": "source",
                "Container": "mkv",
                "Protocol": "File",
                "Path": path,
                "MediaStreams": [
                    { "Index": 0, "CodecType": "Video", "Codec": video,
                      "Width": 1920, "Height": 1080 },
                    { "Index": 1, "CodecType": "Audio", "Codec": "aac" },
                ],
            }))
            .unwrap()
        };
        // The primary is H.264, which the browser takes as it is; the other version is HEVC
        let primary = "/films/Film - 1080p.mkv";
        let sources = vec![
            source(primary, "h264"),
            source("/films/Film - 2160p.mkv", "hevc"),
        ];
        let query = browser_query();

        let (input, chosen) = hls_input(primary, &sources, Version::Primary);
        assert_eq!(input, primary);
        let chosen = chosen.expect("the primary's streams");
        assert_eq!(chosen.id, sources[0].id);
        let info = SourceInfo::from_source(chosen);
        assert_eq!(stream_copy(&query, &info), (true, true));

        let (input, chosen) = hls_input(primary, &sources, Version::Source(1));
        assert_eq!(input, "/films/Film - 2160p.mkv");
        let chosen = chosen.expect("the version's streams");
        assert_eq!(chosen.id, sources[1].id);
        // The HEVC version's video is encoded, not copied
        let info = SourceInfo::from_source(chosen);
        assert_eq!(stream_copy(&query, &info), (false, true));

        // A movie that was never probed: its own file, and no streams to copy
        let (input, chosen) = hls_input("/films/Bare.mkv", &[], Version::Primary);
        assert_eq!(input, "/films/Bare.mkv");
        assert!(chosen.is_none());
    }

    // ── B25: 10-bit video ──

    /// `h264_with(&["aac"])` whose video has `profile` and `bit_depth` (None: not stored).
    fn h264_as(profile: Option<&str>, bit_depth: Option<u32>) -> SourceInfo {
        SourceInfo {
            video_profile: profile.map(Into::into),
            video_bit_depth: bit_depth,
            ..h264_with(&["aac"])
        }
    }

    /// Whether a browser's playlist request with `params` (`&Key=value...`) added copies the
    /// video of `source`.
    fn copies_video(params: &str, source: &SourceInfo) -> bool {
        let lists = "VideoCodec=h264,vp8,vp9,av1&AudioCodec=aac,mp3,opus,flac";
        stream_copy(&query(&format!("{lists}{params}")), source).0
    }

    #[test]
    fn test_deep_h264_is_encoded_for_a_client_that_states_nothing() {
        // With or without a stored bit depth: the profile tells
        for (profile, bits) in [
            (Some("High 10"), Some(10)),
            (Some("High 10"), None),
            (Some("High 10 Intra"), None),
            (Some("High 4:2:2"), Some(8)),
            (Some("High 4:2:2"), None),
            (Some("High 4:4:4 Predictive"), None),
            (Some("High"), Some(10)),
            (None, Some(10)),
        ] {
            let source = h264_as(profile, bits);
            assert!(!copies_video("", &source), "{profile:?} {bits:?}");
        }
        // An empty profile list or an unparsable depth states nothing
        for params in [
            "&Profile=&h264-profile=",
            "&h264-profile=,",
            "&h264-videobitdepth=x",
        ] {
            let source = h264_as(Some("High 10"), None);
            assert!(!copies_video(params, &source), "{params}");
        }
        // 8-bit H.264 is copied as before
        for (profile, bits) in [
            (Some("High"), Some(8)),
            (Some("Main"), None),
            (Some("Constrained Baseline"), Some(8)),
            (None, None),
        ] {
            let source = h264_as(profile, bits);
            assert!(copies_video("", &source), "{profile:?} {bits:?}");
        }
    }

    #[test]
    fn test_deep_h264_is_copied_for_a_client_that_states_it_takes_it() {
        for bits in [Some(10), None] {
            let hi10 = h264_as(Some("High 10"), bits);
            // A depth limit: MaxVideoBitDepth, else h264-videobitdepth; the stored depth, else
            // the profile's
            for params in [
                "&MaxVideoBitDepth=10",
                "&MaxVideoBitDepth=12",
                "&h264-videobitdepth=10",
                "&h264-videobitdepth=x&MaxVideoBitDepth=10",
            ] {
                assert!(copies_video(params, &hi10), "{params} {bits:?}");
            }
            for params in [
                "&MaxVideoBitDepth=8",
                "&h264-videobitdepth=8",
                "&MaxVideoBitDepth=8&h264-videobitdepth=10",
            ] {
                assert!(!copies_video(params, &hi10), "{params} {bits:?}");
            }
            // A profile list: Profile, else h264-profile; split on commas and pipes, matched
            // without case or spaces
            for params in [
                "&Profile=high,main,high10",
                "&h264-profile=high%7Cmain%7CHigh%2010",
                "&Profile=%20High%2010%20",
                "&Profile=&h264-profile=high10",
            ] {
                assert!(copies_video(params, &hi10), "{params} {bits:?}");
            }
            for params in [
                "&Profile=high,main",
                "&Profile=high&h264-profile=high10",
                "&Profile=high,high10&MaxVideoBitDepth=8",
            ] {
                assert!(!copies_video(params, &hi10), "{params} {bits:?}");
            }
        }
        // A profile not listed passes when it scores no higher than the first one listed
        let high = h264_as(Some("High"), Some(8));
        assert!(copies_video("&Profile=high10", &high));
        assert!(!copies_video("&Profile=main", &high));
        let constrained = h264_as(Some("Constrained Baseline"), Some(8));
        assert!(copies_video("&Profile=baseline", &constrained));
        // A profile without a score never passes unlisted
        for profile in ["High 10 Intra", "High 4:2:2", "High 4:4:4 Predictive"] {
            let source = h264_as(Some(profile), Some(8));
            assert!(!copies_video("&Profile=high10", &source), "{profile}");
        }
        // A depth limit alone takes 4:2:2 within it, and what is not known passes a statement
        assert!(copies_video(
            "&MaxVideoBitDepth=10",
            &h264_as(Some("High 4:2:2"), Some(8))
        ));
        assert!(copies_video("&MaxVideoBitDepth=8", &h264_as(None, None)));
        assert!(copies_video("&Profile=high", &h264_as(None, Some(8))));
        // The other rules still hold: a client that does not list H.264 gets it encoded
        let query = query("VideoCodec=hevc&AudioCodec=aac&MaxVideoBitDepth=10");
        assert!(!stream_copy(&query, &h264_as(Some("High 10"), Some(10))).0);
    }

    /// A 1920x1040 HEVC source of `profile` and `bit_depth` with AC3 audio, like a Blu-ray rip.
    fn hevc_as(profile: &str, bit_depth: Option<u32>) -> SourceInfo {
        let deep = bit_depth.is_some_and(|depth| depth > 8) || profile.contains("10");
        SourceInfo {
            video_codec: Some("hevc".into()),
            video_profile: Some(profile.into()),
            video_bit_depth: bit_depth,
            height: Some(1040),
            hevc: Some(HevcFacts {
                profile: Some(profile.into()),
                level: Some(120),
                pix_fmt: Some(if deep { "yuv420p10le" } else { "yuv420p" }.into()),
                transfer: Some("bt709".into()),
                ..Default::default()
            }),
            ..h264_with(&["ac3"])
        }
    }

    /// `source` with its HEVC facts changed by `change`.
    fn with_facts(source: &SourceInfo, change: impl FnOnce(&mut HevcFacts)) -> SourceInfo {
        let mut source = source.clone();
        change(source.hevc.as_mut().unwrap());
        source
    }

    #[test]
    fn r3playback_hevc_copy_follows_profile_level_range_and_dolby_vision() {
        let main10 = hevc_as("Main 10", Some(10));
        let copies = |params: &str, source: &SourceInfo| {
            stream_copy(
                &query(&format!("VideoCodec=hevc&hevc-videobitdepth=10{params}")),
                source,
            )
            .0
        };
        // A file that was not probed is encoded
        let unprobed = SourceInfo {
            hevc: None,
            ..main10.clone()
        };
        assert!(!copies("", &unprobed));

        // Profiles: Main and Main 10 when none are listed; range extensions never
        let still = with_facts(&main10, |f| f.profile = Some("Main Still Picture".into()));
        assert!(!copies("", &still));
        let rext = with_facts(&main10, |f| f.profile = Some("Rext".into()));
        assert!(!copies("", &rext));
        assert!(!copies("&hevc-profile=main10,rext", &rext));
        let chroma422 = with_facts(&main10, |f| f.pix_fmt = Some("yuv422p10le".into()));
        assert!(!copies("", &chroma422));
        let chroma444 = with_facts(&main10, |f| f.pix_fmt = Some("yuv444p".into()));
        assert!(!copies("&hevc-profile=main10", &chroma444));

        // Level: at most the client's; unknown fails only when the client gives one
        let level51 = with_facts(&main10, |f| f.level = Some(153));
        assert!(copies("", &level51));
        assert!(copies("&hevc-level=153", &level51));
        assert!(!copies("&hevc-level=150", &level51));
        assert!(!copies("&hevc-level=120", &level51));
        let unknown = with_facts(&main10, |f| f.level = None);
        assert!(!copies("&hevc-level=153", &unknown));
        assert!(copies("", &unknown));

        // Ranges: HDR10 and HLG only when listed
        let hdr10 = with_facts(&main10, |f| f.transfer = Some("smpte2084".into()));
        let hlg = with_facts(&main10, |f| f.transfer = Some("arib-std-b67".into()));
        assert!(!copies("", &hdr10));
        assert!(!copies("&hevc-rangetype=SDR,HLG", &hdr10));
        assert!(copies("&hevc-rangetype=SDR,HDR10", &hdr10));
        assert!(!copies("&hevc-rangetype=SDR,HDR10", &hlg));
        assert!(copies("&hevc-rangetype=SDR%7CHLG", &hlg));

        // Dolby Vision: profile 5 and DV without a compatible base layer only as DOVI
        let dv = |profile, compatibility| {
            with_facts(&hdr10, |f| {
                f.dv_profile = Some(profile);
                f.dv_compatibility = compatibility;
            })
        };
        for source in [dv(5, Some(0)), dv(5, None), dv(7, Some(0)), dv(8, None)] {
            assert!(
                !copies("&hevc-rangetype=SDR,HDR10,HLG", &source),
                "{source:?}"
            );
            assert!(copies("&hevc-rangetype=SDR,DOVI", &source), "{source:?}");
        }
        // With an HDR10 or HLG base layer it plays as that range
        let dv81 = dv(8, Some(1));
        assert!(!copies("", &dv81));
        assert!(copies("&hevc-rangetype=HDR10", &dv81));
        assert!(copies("&hevc-rangetype=DOVIWithHDR10", &dv81));
        let dv84 = dv(8, Some(4));
        assert!(!copies("&hevc-rangetype=HDR10", &dv84));
        assert!(copies("&hevc-rangetype=HLG", &dv84));
        assert!(copies("", &dv(8, Some(2))));
    }

    #[test]
    fn r3playback_hevc_facts_parse_ffprobe_json() {
        let json = br#"{"programs":[],"streams":[{"profile":"Main 10","level":153,
            "pix_fmt":"yuv420p10le","color_transfer":"smpte2084","side_data_list":[
            {"side_data_type":"DOVI configuration record","dv_profile":8,
             "dv_bl_signal_compatibility_id":1}]}]}"#;
        let facts = parse_hevc_facts(json).unwrap();
        assert_eq!(
            facts,
            HevcFacts {
                profile: Some("Main 10".into()),
                level: Some(153),
                pix_fmt: Some("yuv420p10le".into()),
                transfer: Some("smpte2084".into()),
                dv_profile: Some(8),
                dv_compatibility: Some(1),
            }
        );
        assert_eq!(hevc_range(&facts), "DOVIWithHDR10");
        let plain = br#"{"streams":[{"profile":"Main","level":120,"pix_fmt":"yuv420p"}]}"#;
        assert_eq!(hevc_range(&parse_hevc_facts(plain).unwrap()), "SDR");
        assert_eq!(parse_hevc_facts(b"{\"streams\":[]}"), None);
        assert_eq!(parse_hevc_facts(b"not json"), None);
    }

    #[test]
    fn r3playback_hevc_is_copied_for_a_client_that_lists_it() {
        let main10 = hevc_as("Main 10", Some(10));
        let main = hevc_as("Main", Some(8));
        let copies = |params: &str, source: &SourceInfo| stream_copy(&query(params), source).0;
        for (params, source) in [
            ("VideoCodec=h264,hevc&hevc-videobitdepth=10", &main10),
            ("VideoCodec=hevc&MaxVideoBitDepth=10", &main10),
            ("VideoCodec=h264,h265", &main),
            ("VideoCodec=HEVC", &main),
            (
                "VideoCodec=hevc&hevc-profile=main,main10&hevc-videobitdepth=10",
                &main10,
            ),
            (
                "VideoCodec=hevc&hevc-profile=Main%7CMain%2010&hevc-videobitdepth=10",
                &main10,
            ),
            // VideoBitrate is bits per second, as in Jellyfin
            ("VideoCodec=hevc&MaxWidth=1920&VideoBitrate=10000000", &main),
        ] {
            assert!(copies(params, source), "{params}");
        }
        for (params, source) in [
            // Not listed, or no list at all
            ("", &main),
            ("VideoCodec=h264", &main),
            // A client that states no depth gets no 10-bit video; nor one that states 8
            ("VideoCodec=hevc", &main10),
            ("VideoCodec=hevc&hevc-videobitdepth=8", &main10),
            ("VideoCodec=hevc&hevc-videobitdepth=ten", &main10),
            // A profile the list leaves out
            (
                "VideoCodec=hevc&hevc-profile=main&hevc-videobitdepth=10",
                &main10,
            ),
            // Over the size or bitrate limit, or with a burn-in
            ("VideoCodec=hevc&MaxWidth=1280", &main),
            ("VideoCodec=hevc&VideoBitrate=4000", &main),
            ("VideoCodec=hevc&SubtitleMethod=Encode", &main),
        ] {
            let mut source = source.clone();
            source.subtitles = vec![track(3, "subrip")];
            assert!(!copies(params, &source), "{params}");
        }
        // An unknown depth is read from the profile
        assert!(!copies("VideoCodec=hevc", &hevc_as("Main 10", None)));
        assert!(copies("VideoCodec=hevc", &hevc_as("Main", None)));
    }

    #[test]
    fn r3playback_copied_hevc_goes_out_in_fragmented_mp4() {
        let main10 = hevc_as("Main 10", Some(10));
        let profile = transcode_profile(query("VideoCodec=hevc&hevc-videobitdepth=10"), &main10);
        assert!(profile.copy_video && profile.fmp4, "{profile:?}");
        assert_eq!(profile.video_tag.as_deref(), Some("hvc1"));
        // Encoded HEVC and copied H.264 stay in MPEG-TS
        let encoded = transcode_profile(query("VideoCodec=h264"), &main10);
        assert!(!encoded.copy_video && !encoded.fmp4 && encoded.video_tag.is_none());
        let h264 = transcode_profile(query("VideoCodec=h264"), &h264_with(&["aac"]));
        assert!(h264.copy_video && !h264.fmp4 && h264.video_tag.is_none());
    }

    #[test]
    fn r3playback_rewrite_points_the_init_segment_at_the_segment_route() {
        let playback_id = Uuid::new_v4();
        let content = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\nsegment_0000.mp4\n";
        let expected = format!(
            "#EXTM3U\n#EXT-X-MAP:URI=\"hls/{playback_id}/video/init.mp4?api_key=k\"\n\
             #EXTINF:6.0,\nhls/{playback_id}/video/segment_0000.mp4?api_key=k\n"
        );
        assert_eq!(
            rewrite_playlist(content, &playback_id, Some("api_key=k")),
            expected
        );
    }
}
