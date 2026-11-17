//! Jellyfin's encoding options (`EncodingOptions`, the named configuration `encoding` that
//! `/System/Configuration/encoding` reads and writes), and what the transcoder takes from them.
//!
//! The options are stored as the JSON a client saved (`encoding.json` in the data directory), so
//! fields Rustyfin doesn't know go back to the client as they came. [`EncodingSettings`] is the
//! typed view of the fields the transcoder acts on; the others are stored only.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::transcode::HwAccel;

/// The file in the data directory that holds the saved options.
pub const ENCODING_FILE: &str = "encoding.json";

/// Jellyfin's `HardwareAccelerationType`. Rustyfin builds ffmpeg arguments for VAAPI, QSV and
/// NVENC; the other types are stored and transcode in software.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HardwareAccelerationType {
    #[default]
    None,
    Amf,
    Qsv,
    Nvenc,
    V4l2m2m,
    Vaapi,
    VideoToolbox,
    Rkmpp,
}

impl HardwareAccelerationType {
    /// The type a client names, without case; an unknown name is `None`.
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "amf" => Self::Amf,
            "qsv" => Self::Qsv,
            "nvenc" => Self::Nvenc,
            "v4l2m2m" => Self::V4l2m2m,
            "vaapi" => Self::Vaapi,
            "videotoolbox" => Self::VideoToolbox,
            "rkmpp" => Self::Rkmpp,
            _ => Self::None,
        }
    }

    /// Jellyfin's name for the type, as `HardwareAccelerationType` stores it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Amf => "amf",
            Self::Qsv => "qsv",
            Self::Nvenc => "nvenc",
            Self::V4l2m2m => "v4l2m2m",
            Self::Vaapi => "vaapi",
            Self::VideoToolbox => "videotoolbox",
            Self::Rkmpp => "rkmpp",
        }
    }

    /// The acceleration the transcoder builds arguments for: VAAPI, QSV and NVENC. The other
    /// types transcode in software.
    pub fn transcoder_accel(self) -> HwAccel {
        match self {
            Self::Vaapi => HwAccel::Vaapi,
            Self::Qsv => HwAccel::Qsv,
            Self::Nvenc => HwAccel::Nvenc,
            _ => HwAccel::None,
        }
    }
}

impl serde::Serialize for HardwareAccelerationType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// The encoding options in effect, for code that runs outside the transcoder (trickplay and
/// chapter images): the transcode manager's settings, updated whenever it takes new ones (a
/// save, or a hardware detection while the type is automatic). Nothing reads it yet: it is the
/// shared setting the trickplay work's hardware decoding path reads (its type, device and
/// decoding codecs), so keep it even while it has no caller here.
pub fn current() -> EncodingSettings {
    current_cell()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub(crate) fn set_current(settings: &EncodingSettings) {
    *current_cell().write().unwrap_or_else(|e| e.into_inner()) = settings.clone();
}

fn current_cell() -> &'static std::sync::RwLock<EncodingSettings> {
    static CURRENT: std::sync::OnceLock<std::sync::RwLock<EncodingSettings>> =
        std::sync::OnceLock::new();
    CURRENT.get_or_init(Default::default)
}

/// The encoding options the transcoder acts on, read from the stored options. A missing or
/// unreadable field takes Jellyfin's default.
#[derive(Debug, Clone, PartialEq)]
pub struct EncodingSettings {
    /// `EncodingThreadCount`: -1 (or 0) lets ffmpeg choose
    pub thread_count: i32,
    pub h264_crf: u32,
    pub h265_crf: u32,
    /// `EncoderPreset`: auto, placebo, veryslow, ... ultrafast
    pub encoder_preset: String,
    pub downmix_audio_boost: f64,
    /// `DownMixStereoAlgorithm`: None, Dave750, NightmodeDialogue, Rfc7845, Ac4
    pub downmix_stereo_algorithm: String,
    pub max_muxing_queue_size: u32,
    pub allow_hevc_encoding: bool,
    pub allow_av1_encoding: bool,
    pub enable_tonemapping: bool,
    pub tonemapping_algorithm: String,
    pub tonemapping_range: String,
    pub tonemapping_desat: f64,
    pub tonemapping_peak: f64,
    pub tonemapping_param: f64,
    pub hardware_acceleration: HardwareAccelerationType,
    pub vaapi_device: String,
    pub qsv_device: String,
    /// `HardwareDecodingCodecs`, lowercase
    pub hardware_decoding_codecs: Vec<String>,
    pub enable_hardware_encoding: bool,
    pub enable_decoding_color_depth_10_hevc: bool,
    pub enable_decoding_color_depth_10_vp9: bool,
    /// `TranscodingTempPath` when set; the transcode manager reads it when the server starts
    pub transcoding_temp_path: Option<PathBuf>,
}

impl Default for EncodingSettings {
    fn default() -> Self {
        Self {
            thread_count: -1,
            h264_crf: 23,
            h265_crf: 28,
            encoder_preset: "auto".to_string(),
            downmix_audio_boost: 2.0,
            downmix_stereo_algorithm: "None".to_string(),
            max_muxing_queue_size: 2048,
            allow_hevc_encoding: false,
            allow_av1_encoding: false,
            enable_tonemapping: false,
            tonemapping_algorithm: "bt2390".to_string(),
            tonemapping_range: "auto".to_string(),
            tonemapping_desat: 0.0,
            tonemapping_peak: 100.0,
            tonemapping_param: 0.0,
            hardware_acceleration: HardwareAccelerationType::None,
            vaapi_device: "/dev/dri/renderD128".to_string(),
            qsv_device: String::new(),
            hardware_decoding_codecs: vec!["h264".to_string(), "vc1".to_string()],
            enable_hardware_encoding: true,
            enable_decoding_color_depth_10_hevc: true,
            enable_decoding_color_depth_10_vp9: true,
            transcoding_temp_path: None,
        }
    }
}

/// The value of `key` in `options`, its name matched without case.
fn field<'a>(options: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    options.get(key).or_else(|| {
        options
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

fn number(options: &Map<String, Value>, key: &str) -> Option<f64> {
    match field(options, key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

fn integer(options: &Map<String, Value>, key: &str) -> Option<i64> {
    number(options, key).map(|n| n as i64)
}

fn boolean(options: &Map<String, Value>, key: &str) -> Option<bool> {
    match field(options, key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(options: &Map<String, Value>, key: &str) -> Option<String> {
    match field(options, key)? {
        Value::String(s) => Some(s.trim().to_string()),
        _ => None,
    }
}

fn list(options: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    let items: Vec<String> = match field(options, key)? {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(s) => s.split(',').map(str::to_string).collect(),
        _ => return None,
    };
    Some(
        items
            .iter()
            .map(|item| item.trim().to_ascii_lowercase())
            .filter(|item| !item.is_empty())
            .collect(),
    )
}

impl EncodingSettings {
    /// The settings in the stored `options` (the defaults with what a client saved).
    pub fn from_options(options: &Map<String, Value>) -> Self {
        let d = Self::default();
        let crf = |key, default| {
            integer(options, key)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n <= 51)
                .unwrap_or(default)
        };
        Self {
            thread_count: integer(options, "EncodingThreadCount")
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(d.thread_count),
            h264_crf: crf("H264Crf", d.h264_crf),
            h265_crf: crf("H265Crf", d.h265_crf),
            encoder_preset: text(options, "EncoderPreset")
                .filter(|p| !p.is_empty())
                .unwrap_or(d.encoder_preset),
            downmix_audio_boost: number(options, "DownMixAudioBoost")
                .filter(|n| *n > 0.0)
                .unwrap_or(d.downmix_audio_boost),
            downmix_stereo_algorithm: text(options, "DownMixStereoAlgorithm")
                .filter(|a| !a.is_empty())
                .unwrap_or(d.downmix_stereo_algorithm),
            max_muxing_queue_size: integer(options, "MaxMuxingQueueSize")
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .unwrap_or(d.max_muxing_queue_size),
            allow_hevc_encoding: boolean(options, "AllowHevcEncoding")
                .unwrap_or(d.allow_hevc_encoding),
            allow_av1_encoding: boolean(options, "AllowAv1Encoding")
                .unwrap_or(d.allow_av1_encoding),
            enable_tonemapping: boolean(options, "EnableTonemapping")
                .unwrap_or(d.enable_tonemapping),
            tonemapping_algorithm: text(options, "TonemappingAlgorithm")
                .filter(|a| !a.is_empty())
                .unwrap_or(d.tonemapping_algorithm),
            tonemapping_range: text(options, "TonemappingRange")
                .filter(|r| !r.is_empty())
                .unwrap_or(d.tonemapping_range),
            tonemapping_desat: number(options, "TonemappingDesat").unwrap_or(d.tonemapping_desat),
            tonemapping_peak: number(options, "TonemappingPeak").unwrap_or(d.tonemapping_peak),
            tonemapping_param: number(options, "TonemappingParam").unwrap_or(d.tonemapping_param),
            hardware_acceleration: text(options, "HardwareAccelerationType")
                .map_or(d.hardware_acceleration, |t| {
                    HardwareAccelerationType::parse(&t)
                }),
            vaapi_device: text(options, "VaapiDevice")
                .filter(|v| !v.is_empty())
                .unwrap_or(d.vaapi_device),
            qsv_device: text(options, "QsvDevice").unwrap_or(d.qsv_device),
            hardware_decoding_codecs: list(options, "HardwareDecodingCodecs")
                .unwrap_or(d.hardware_decoding_codecs),
            enable_hardware_encoding: boolean(options, "EnableHardwareEncoding")
                .unwrap_or(d.enable_hardware_encoding),
            enable_decoding_color_depth_10_hevc: boolean(options, "EnableDecodingColorDepth10Hevc")
                .unwrap_or(d.enable_decoding_color_depth_10_hevc),
            enable_decoding_color_depth_10_vp9: boolean(options, "EnableDecodingColorDepth10Vp9")
                .unwrap_or(d.enable_decoding_color_depth_10_vp9),
            transcoding_temp_path: text(options, "TranscodingTempPath")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        }
    }

    /// The settings stored in `data_dir`, or the defaults.
    pub fn load(data_dir: &Path) -> Self {
        let saved = load_saved(data_dir).unwrap_or_default();
        Self::from_options(&saved)
    }
}

/// The fields that name the server's devices, paths and programs. Users who aren't
/// administrators get the options without them.
pub const PRIVATE_FIELDS: [&str; 6] = [
    "VaapiDevice",
    "QsvDevice",
    "TranscodingTempPath",
    "EncoderAppPath",
    "EncoderAppPathDisplay",
    "FallbackFontPath",
];

/// Why a client's options can't be saved: a CRF outside 0–51, a thread count outside -1–256,
/// an enum value Jellyfin doesn't define (hardware type, preset, downmix, tonemapping), or a
/// device that isn't an absolute path free of `,` and `=` (which would end ffmpeg's
/// `-init_hw_device` value). Missing and null fields are fine; names match without case.
pub fn validate_options(options: &Map<String, Value>) -> Result<(), String> {
    let present = |key: &str| field(options, key).filter(|v| !v.is_null());
    for (key, min, max) in [
        ("H264Crf", 0, 51),
        ("H265Crf", 0, 51),
        ("EncodingThreadCount", -1, 256),
    ] {
        if let Some(value) = present(key) {
            let n = number(options, key)
                .filter(|n| n.fract() == 0.0)
                .ok_or_else(|| format!("{key}: {value} is not a whole number"))?;
            if n < f64::from(min) || n > f64::from(max) {
                return Err(format!("{key}: {n} is outside {min} to {max}"));
            }
        }
    }
    const ENUMS: [(&str, &[&str]); 6] = [
        (
            "HardwareAccelerationType",
            &[
                "none",
                "amf",
                "qsv",
                "nvenc",
                "v4l2m2m",
                "vaapi",
                "videotoolbox",
                "rkmpp",
            ],
        ),
        (
            "EncoderPreset",
            &[
                "auto",
                "placebo",
                "veryslow",
                "slower",
                "slow",
                "medium",
                "fast",
                "faster",
                "veryfast",
                "superfast",
                "ultrafast",
            ],
        ),
        (
            "DownMixStereoAlgorithm",
            &["none", "dave750", "nightmodedialogue", "rfc7845", "ac4"],
        ),
        (
            "TonemappingAlgorithm",
            &[
                "none", "clip", "linear", "gamma", "reinhard", "hable", "mobius", "bt2390",
            ],
        ),
        ("TonemappingRange", &["auto", "tv", "pc"]),
        ("TonemappingMode", &["auto", "max", "rgb", "lum", "itp"]),
    ];
    for (key, allowed) in ENUMS {
        if let Some(value) = present(key) {
            let known = value
                .as_str()
                .is_some_and(|v| allowed.contains(&v.to_ascii_lowercase().as_str()));
            if !known {
                return Err(format!(
                    "{key}: {value} is not one of {}",
                    allowed.join(", ")
                ));
            }
        }
    }
    for key in ["VaapiDevice", "QsvDevice"] {
        if let Some(value) = present(key) {
            let path = value
                .as_str()
                .ok_or_else(|| format!("{key}: {value} is not a path"))?;
            if !path.is_empty()
                && (!path.starts_with('/') || path.contains([',', '=']) || path.contains('\0'))
            {
                return Err(format!(
                    "{key}: {path:?} must be an absolute path without ',' or '='"
                ));
            }
        }
    }
    Ok(())
}

/// Jellyfin 12.4.0's defaults for every `EncodingOptions` field, with Rustyfin's ffmpeg and
/// transcode folder.
pub fn default_options(ffmpeg: &str, transcode_dir: &Path) -> Map<String, Value> {
    [
        ("EncodingThreadCount", json!(-1)),
        (
            "TranscodingTempPath",
            json!(transcode_dir.to_string_lossy()),
        ),
        ("FallbackFontPath", json!("")),
        ("EnableFallbackFont", json!(false)),
        ("EnableAudioVbr", json!(false)),
        ("DownMixAudioBoost", json!(2)),
        ("DownMixStereoAlgorithm", json!("None")),
        ("MaxMuxingQueueSize", json!(2048)),
        ("EnableThrottling", json!(false)),
        ("ThrottleDelaySeconds", json!(180)),
        ("EnableSegmentDeletion", json!(false)),
        ("SegmentKeepSeconds", json!(720)),
        ("HardwareAccelerationType", json!("none")),
        ("EncoderAppPath", json!(ffmpeg)),
        ("EncoderAppPathDisplay", json!(ffmpeg)),
        ("VaapiDevice", json!("/dev/dri/renderD128")),
        ("QsvDevice", json!("")),
        ("EnableTonemapping", json!(false)),
        ("EnableVppTonemapping", json!(false)),
        ("EnableVideoToolboxTonemapping", json!(false)),
        ("TonemappingAlgorithm", json!("bt2390")),
        ("TonemappingMode", json!("auto")),
        ("TonemappingRange", json!("auto")),
        ("TonemappingDesat", json!(0)),
        ("TonemappingPeak", json!(100)),
        ("TonemappingParam", json!(0)),
        ("VppTonemappingBrightness", json!(16)),
        ("VppTonemappingContrast", json!(1)),
        ("H264Crf", json!(23)),
        ("H265Crf", json!(28)),
        ("EncoderPreset", json!("auto")),
        ("DeinterlaceDoubleRate", json!(false)),
        ("DeinterlaceMethod", json!("yadif")),
        ("EnableDecodingColorDepth10Hevc", json!(true)),
        ("EnableDecodingColorDepth10Vp9", json!(true)),
        ("EnableDecodingColorDepth10HevcRext", json!(false)),
        ("EnableDecodingColorDepth12HevcRext", json!(false)),
        ("EnableEnhancedNvdecDecoder", json!(true)),
        ("PreferSystemNativeHwDecoder", json!(true)),
        ("EnableIntelLowPowerH264HwEncoder", json!(false)),
        ("EnableIntelLowPowerHevcHwEncoder", json!(false)),
        ("EnableHardwareEncoding", json!(true)),
        ("AllowHevcEncoding", json!(false)),
        ("AllowAv1Encoding", json!(false)),
        ("EnableSubtitleExtraction", json!(true)),
        ("SubtitleExtractionTimeoutMinutes", json!(30)),
        ("HlsAudioSeekStrategy", json!("TrimCopiedAudio")),
        ("HardwareDecodingCodecs", json!(["h264", "vc1"])),
        (
            "AllowOnDemandMetadataBasedKeyframeExtractionForExtensions",
            json!(["mkv"]),
        ),
    ]
    .into_iter()
    .map(|(field, value)| (field.to_string(), value))
    .collect()
}

/// The options a client saved in `data_dir`; None when there are none or they can't be read.
pub fn load_saved(data_dir: &Path) -> Option<Map<String, Value>> {
    let text = match std::fs::read_to_string(data_dir.join(ENCODING_FILE)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("cannot read {ENCODING_FILE}: {e}");
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(saved)) => Some(saved),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("ignoring an unreadable {ENCODING_FILE}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Encoders, presets and filters
// ---------------------------------------------------------------------------

/// The codec family an ffmpeg video encoder writes (`h264`, `hevc`, `av1`); None for the others.
pub fn encoder_family(encoder: &str) -> Option<&'static str> {
    match encoder {
        "libx264" | "h264_vaapi" | "h264_qsv" | "h264_nvenc" => Some("h264"),
        "libx265" | "hevc_vaapi" | "hevc_qsv" | "hevc_nvenc" => Some("hevc"),
        "libsvtav1" | "libaom-av1" | "av1_vaapi" | "av1_qsv" | "av1_nvenc" => Some("av1"),
        _ => None,
    }
}

/// The hardware encoder of `accel` for a codec family, as Jellyfin names them.
pub fn hardware_encoder(accel: HwAccel, family: &str) -> Option<&'static str> {
    Some(match (accel, family) {
        (HwAccel::Vaapi, "h264") => "h264_vaapi",
        (HwAccel::Vaapi, "hevc") => "hevc_vaapi",
        (HwAccel::Vaapi, "av1") => "av1_vaapi",
        (HwAccel::Qsv, "h264") => "h264_qsv",
        (HwAccel::Qsv, "hevc") => "hevc_qsv",
        (HwAccel::Qsv, "av1") => "av1_qsv",
        (HwAccel::Nvenc, "h264") => "h264_nvenc",
        (HwAccel::Nvenc, "hevc") => "hevc_nvenc",
        (HwAccel::Nvenc, "av1") => "av1_nvenc",
        _ => return None,
    })
}

/// The video codec an encode produces for a client that lists `codecs` (in its order of
/// preference), as Jellyfin picks it: the first of H.264, HEVC (with `AllowHevcEncoding`) and AV1
/// (with `AllowAv1Encoding`); else H.264.
pub fn output_video_codec(codecs: &[String], settings: &EncodingSettings) -> &'static str {
    codecs
        .iter()
        .find_map(|codec| match codec.as_str() {
            "h264" | "avc" | "avc1" => Some("h264"),
            "hevc" | "h265" | "hvc1" | "hev1" if settings.allow_hevc_encoding => Some("hevc"),
            "av1" if settings.allow_av1_encoding => Some("av1"),
            _ => None,
        })
        .unwrap_or("h264")
}

/// The preset arguments of `encoder` for Jellyfin's `EncoderPreset`, as Jellyfin maps them; none
/// for an encoder without presets (VAAPI, the audio and other video encoders).
pub fn preset_args(encoder: &str, preset: &str) -> Vec<String> {
    let preset = preset.to_ascii_lowercase();
    let value = match encoder {
        "libx264" | "libx265" => match preset.as_str() {
            "placebo" | "veryslow" | "slower" | "slow" | "medium" | "fast" | "faster"
            | "veryfast" | "superfast" | "ultrafast" => preset,
            _ => "veryfast".to_string(),
        },
        "libsvtav1" => match preset.as_str() {
            "placebo" | "veryslow" => "2",
            "slower" => "3",
            "slow" => "4",
            "medium" => "5",
            "fast" => "6",
            "faster" => "7",
            _ => "8",
        }
        .to_string(),
        "h264_qsv" | "hevc_qsv" | "av1_qsv" => match preset.as_str() {
            "placebo" | "veryslow" => "veryslow",
            "slower" => "slower",
            "slow" => "slow",
            "medium" => "medium",
            "fast" => "fast",
            "faster" => "faster",
            _ => "veryfast",
        }
        .to_string(),
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => match preset.as_str() {
            "placebo" | "veryslow" => "p7",
            "slower" => "p6",
            "slow" => "p5",
            "medium" => "p4",
            "fast" => "p3",
            "faster" => "p2",
            _ => "p1",
        }
        .to_string(),
        _ => return Vec::new(),
    };
    vec!["-preset".to_string(), value]
}

/// The CRF of a software x264 or x265 encode; None for the other encoders, which keep to the
/// bitrate.
pub fn crf(encoder: &str, settings: &EncodingSettings) -> Option<u32> {
    match encoder {
        "libx264" => Some(settings.h264_crf),
        "libx265" => Some(settings.h265_crf),
        _ => None,
    }
}

/// A number as ffmpeg reads it: `2`, `0.5`.
fn number_text(n: f64) -> String {
    format!("{n}")
}

/// The transfer of an HDR video that tonemapping maps to SDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrTransfer {
    /// SMPTE ST 2084 (HDR10, HDR10+, Dolby Vision with an HDR10 base layer)
    Pq,
    /// ARIB STD-B67 (HLG, Dolby Vision with an HLG base layer)
    Hlg,
}

impl HdrTransfer {
    /// The transfer of Jellyfin's `VideoRangeType`; None for SDR and for Dolby Vision without a
    /// base layer ffmpeg can tonemap.
    pub fn from_range_type(range_type: &str) -> Option<Self> {
        match range_type.to_ascii_lowercase().as_str() {
            "hdr10" | "hdr10plus" | "doviwithhdr10" | "doviwithhdr10plus" => Some(Self::Pq),
            "hlg" | "doviwithhlg" => Some(Self::Hlg),
            _ => None,
        }
    }

    fn zscale_name(self) -> &'static str {
        match self {
            Self::Pq => "smpte2084",
            Self::Hlg => "arib-std-b67",
        }
    }
}

/// The software tonemap (zscale and tonemap), HDR10 or HLG to SDR BT.709 in 8-bit 4:2:0, as
/// Jellyfin built it for ffmpeg without its own tonemap filters. `setparams` names the input's
/// transfer, matrix and primaries, so a stream without color tags tonemaps too. Stock ffmpeg's
/// `tonemap`
/// has no `bt2390`, so that algorithm (Jellyfin's default) runs as `hable`.
pub fn software_tonemap_filter(settings: &EncodingSettings, transfer: HdrTransfer) -> String {
    let algorithm = match settings.tonemapping_algorithm.to_ascii_lowercase().as_str() {
        algorithm @ ("none" | "clip" | "linear" | "gamma" | "reinhard" | "hable" | "mobius") => {
            algorithm.to_string()
        }
        _ => "hable".to_string(),
    };
    let range = match settings.tonemapping_range.to_ascii_lowercase().as_str() {
        "pc" => "pc",
        _ => "tv",
    };
    let peak = if settings.tonemapping_peak > 0.0 {
        settings.tonemapping_peak
    } else {
        100.0
    };
    let param = if settings.tonemapping_param != 0.0 {
        format!(":param={}", number_text(settings.tonemapping_param))
    } else {
        String::new()
    };
    format!(
        "setparams=color_primaries=bt2020:color_trc={}:colorspace=bt2020nc,\
         zscale=t=linear:npl={},format=gbrpf32le,\
         zscale=p=bt709,tonemap=tonemap={algorithm}:desat={}{param},\
         zscale=t=bt709:m=bt709:r={range},format=yuv420p",
        transfer.zscale_name(),
        number_text(peak),
        number_text(settings.tonemapping_desat.max(0.0)),
    )
}

/// The stereo downmix filter of Jellyfin's `DownMixStereoAlgorithm` for a source with `channels`
/// channels; None for `None`, for a layout the algorithm has no matrix for, and for `Ac4`, which
/// Rustyfin stores only (those downmix with ffmpeg's own `-ac 2`).
pub fn downmix_filter(algorithm: &str, channels: u32) -> Option<&'static str> {
    Some(match (algorithm.to_ascii_lowercase().as_str(), channels) {
        ("dave750", 6) => {
            "pan=stereo|c0=0.5*c2+0.707*c0+0.707*c4+0.5*c3|c1=0.5*c2+0.707*c1+0.707*c5+0.5*c3"
        }
        ("dave750", 8) => {
            "pan=stereo|c0=0.5*c2+0.707*c0+0.707*c4+0.707*c6+0.5*c3|\
             c1=0.5*c2+0.707*c1+0.707*c5+0.707*c7+0.5*c3"
        }
        ("nightmodedialogue", 6) => "pan=stereo|c0=c2+0.30*c0+0.30*c4|c1=c2+0.30*c1+0.30*c5",
        ("nightmodedialogue", 8) => {
            "pan=stereo|c0=c2+0.30*c0+0.30*c4+0.30*c6|c1=c2+0.30*c1+0.30*c5+0.30*c7"
        }
        // RFC 7845's 5.1 matrix, in ffmpeg's channel order (FL FR FC LFE BL BR)
        ("rfc7845", 6) => {
            "pan=stereo|c0=0.529067*c0+0.374107*c2+0.374107*c3+0.458186*c4+0.264534*c5|\
             c1=0.529067*c1+0.374107*c2+0.374107*c3+0.264534*c4+0.458186*c5"
        }
        _ => return None,
    })
}

/// The audio filters of a downmix of `source_channels` to `output_channels`: the algorithm's
/// matrix, then the volume boost, as Jellyfin adds them when it downmixes to stereo. Empty when
/// there is no downmix to stereo.
pub fn downmix_filters(
    settings: &EncodingSettings,
    source_channels: Option<u32>,
    output_channels: u32,
) -> Vec<String> {
    let Some(source) = source_channels.filter(|c| *c > 2) else {
        return Vec::new();
    };
    if output_channels != 2 {
        return Vec::new();
    }
    let mut filters = Vec::new();
    if let Some(filter) = downmix_filter(&settings.downmix_stereo_algorithm, source) {
        filters.push(filter.to_string());
    }
    if settings.downmix_audio_boost != 1.0 {
        filters.push(format!(
            "volume={}",
            number_text(settings.downmix_audio_boost)
        ));
    }
    filters
}

// ---------------------------------------------------------------------------
// What the ffmpeg build offers
// ---------------------------------------------------------------------------

/// What `ffmpeg -hwaccels`, `-encoders` and `-filters` list. A list that couldn't be read is
/// None, and everything counts as present in it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FfmpegCapabilities {
    pub hwaccels: Option<Vec<String>>,
    pub encoders: Option<HashSet<String>>,
    pub filters: Option<HashSet<String>>,
}

impl FfmpegCapabilities {
    pub fn has_hwaccel(&self, name: &str) -> bool {
        self.hwaccels
            .as_ref()
            .is_none_or(|list| list.iter().any(|h| h == name))
    }

    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.as_ref().is_none_or(|set| set.contains(name))
    }

    pub fn has_filter(&self, name: &str) -> bool {
        self.filters.as_ref().is_none_or(|set| set.contains(name))
    }
}

/// The methods `ffmpeg -hwaccels` lists; None for output without its header.
pub fn parse_hwaccels(output: &str) -> Option<Vec<String>> {
    let mut lines = output.lines();
    lines.find(|line| {
        line.trim_start()
            .starts_with("Hardware acceleration methods:")
    })?;
    Some(
        lines
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// The names `ffmpeg -encoders` (or `-decoders`) lists after its legend; None for output without
/// it.
pub fn parse_encoders(output: &str) -> Option<HashSet<String>> {
    let mut lines = output.lines();
    lines.find(|line| line.trim() == "------")?;
    Some(
        lines
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let flags = words.next()?;
                let name = words.next()?;
                (flags.len() == 6).then(|| name.to_string())
            })
            .collect(),
    )
}

/// The names `ffmpeg -filters` lists; None for output without its `Filters:` header.
pub fn parse_filters(output: &str) -> Option<HashSet<String>> {
    let mut lines = output.lines();
    lines.find(|line| line.trim() == "Filters:")?;
    Some(
        lines
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let flags = words.next()?;
                let name = words.next()?;
                let io = words.next()?;
                (flags.len() == 3 && io.contains("->")).then(|| name.to_string())
            })
            .collect(),
    )
}

/// Run `ffmpeg -hide_banner <flag>` and return its stdout; None when it doesn't run in 10 s.
async fn ffmpeg_list(ffmpeg: &Path, flag: &str) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(ffmpeg)
            .args(["-hide_banner", flag])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What the ffmpeg at `ffmpeg` offers.
pub async fn probe_capabilities(ffmpeg: &Path) -> FfmpegCapabilities {
    let (hwaccels, encoders, filters) = tokio::join!(
        ffmpeg_list(ffmpeg, "-hwaccels"),
        ffmpeg_list(ffmpeg, "-encoders"),
        ffmpeg_list(ffmpeg, "-filters"),
    );
    FfmpegCapabilities {
        hwaccels: hwaccels.as_deref().and_then(parse_hwaccels),
        encoders: encoders.as_deref().and_then(parse_encoders),
        filters: filters.as_deref().and_then(parse_filters),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3transcoding_defaults_read_as_the_typed_defaults() {
        let options = default_options("ffmpeg", Path::new("/data/transcodes"));
        let settings = EncodingSettings::from_options(&options);
        assert_eq!(
            settings,
            EncodingSettings {
                transcoding_temp_path: Some(PathBuf::from("/data/transcodes")),
                ..EncodingSettings::default()
            }
        );
    }

    #[test]
    fn r3transcoding_settings_read_leniently() {
        let options: Map<String, Value> = serde_json::from_value(json!({
            "encodingthreadcount": "4",
            "H264Crf": 99,
            "H265Crf": 20,
            "EncoderPreset": "slow",
            "HardwareAccelerationType": "VAAPI",
            "HardwareDecodingCodecs": ["H264", "hevc", ""],
            "DownMixAudioBoost": 1.5,
            "AllowHevcEncoding": "true",
            "TranscodingTempPath": "  ",
        }))
        .unwrap();
        let s = EncodingSettings::from_options(&options);
        assert_eq!(s.thread_count, 4);
        assert_eq!(s.h264_crf, 23, "out of range takes the default");
        assert_eq!(s.h265_crf, 20);
        assert_eq!(s.encoder_preset, "slow");
        assert_eq!(s.hardware_acceleration, HardwareAccelerationType::Vaapi);
        assert_eq!(s.hardware_decoding_codecs, ["h264", "hevc"]);
        assert_eq!(s.downmix_audio_boost, 1.5);
        assert!(s.allow_hevc_encoding);
        assert_eq!(s.transcoding_temp_path, None);
    }

    #[test]
    fn r3transcoding_validation_refuses_bad_values() {
        let check = |v: Value| validate_options(v.as_object().unwrap());
        // The defaults, case-insensitive enums and nulls pass
        assert_eq!(
            validate_options(&default_options("ffmpeg", Path::new("/t"))),
            Ok(())
        );
        assert_eq!(
            check(
                json!({ "hardwareaccelerationtype": "VAAPI", "EncoderPreset": null,
                          "DownMixStereoAlgorithm": "Dave750", "QsvDevice": "" })
            ),
            Ok(())
        );
        for bad in [
            json!({ "H264Crf": 52 }),
            json!({ "H265Crf": -1 }),
            json!({ "H264Crf": 20.5 }),
            json!({ "H264Crf": "high" }),
            json!({ "EncodingThreadCount": -2 }),
            json!({ "EncodingThreadCount": 1000 }),
            json!({ "HardwareAccelerationType": "cuda" }),
            json!({ "HardwareAccelerationType": 3 }),
            json!({ "EncoderPreset": "turbo" }),
            json!({ "DownMixStereoAlgorithm": "Surround" }),
            json!({ "TonemappingAlgorithm": "aces" }),
            json!({ "TonemappingRange": "full" }),
            json!({ "TonemappingMode": "rgbx" }),
            json!({ "VaapiDevice": "renderD128" }),
            json!({ "VaapiDevice": "/dev/dri/renderD128,driver=iHD" }),
            json!({ "QsvDevice": "/dev/dri/x=y" }),
            json!({ "QsvDevice": 1 }),
        ] {
            assert!(check(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn r3transcoding_hardware_types_parse() {
        for (name, expected, accel) in [
            ("none", HardwareAccelerationType::None, HwAccel::None),
            ("vaapi", HardwareAccelerationType::Vaapi, HwAccel::Vaapi),
            ("qsv", HardwareAccelerationType::Qsv, HwAccel::Qsv),
            ("nvenc", HardwareAccelerationType::Nvenc, HwAccel::Nvenc),
            (
                "videotoolbox",
                HardwareAccelerationType::VideoToolbox,
                HwAccel::None,
            ),
            ("amf", HardwareAccelerationType::Amf, HwAccel::None),
            ("rkmpp", HardwareAccelerationType::Rkmpp, HwAccel::None),
            ("v4l2m2m", HardwareAccelerationType::V4l2m2m, HwAccel::None),
            ("bogus", HardwareAccelerationType::None, HwAccel::None),
        ] {
            let parsed = HardwareAccelerationType::parse(name);
            assert_eq!(parsed, expected, "{name}");
            assert_eq!(parsed.transcoder_accel(), accel, "{name}");
        }
    }

    #[test]
    fn r3transcoding_presets_map_as_jellyfin() {
        assert_eq!(preset_args("libx264", "auto"), ["-preset", "veryfast"]);
        assert_eq!(preset_args("libx265", "slow"), ["-preset", "slow"]);
        assert_eq!(preset_args("libsvtav1", "medium"), ["-preset", "5"]);
        assert_eq!(preset_args("h264_nvenc", "auto"), ["-preset", "p1"]);
        assert_eq!(preset_args("hevc_nvenc", "veryslow"), ["-preset", "p7"]);
        assert_eq!(
            preset_args("h264_qsv", "ultrafast"),
            ["-preset", "veryfast"]
        );
        assert_eq!(preset_args("h264_qsv", "medium"), ["-preset", "medium"]);
        assert!(preset_args("h264_vaapi", "medium").is_empty());
        assert!(preset_args("aac", "medium").is_empty());
    }

    #[test]
    fn r3transcoding_output_codec_follows_the_allow_flags() {
        let listed = |l: &[&str]| l.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        let mut s = EncodingSettings::default();
        assert_eq!(output_video_codec(&listed(&["hevc", "h264"]), &s), "h264");
        assert_eq!(output_video_codec(&listed(&["av1", "hevc"]), &s), "h264");
        s.allow_hevc_encoding = true;
        assert_eq!(
            output_video_codec(&listed(&["av1", "hevc", "h264"]), &s),
            "hevc"
        );
        s.allow_av1_encoding = true;
        assert_eq!(
            output_video_codec(&listed(&["av1", "hevc", "h264"]), &s),
            "av1"
        );
        assert_eq!(output_video_codec(&listed(&["vp9"]), &s), "h264");
    }

    #[test]
    fn r3transcoding_software_tonemap_filter() {
        let mut s = EncodingSettings::default();
        assert_eq!(
            software_tonemap_filter(&s, HdrTransfer::Pq),
            "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc,\
             zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,\
             zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
        );
        s.tonemapping_algorithm = "reinhard".into();
        s.tonemapping_range = "pc".into();
        s.tonemapping_param = 0.5;
        s.tonemapping_peak = 400.0;
        assert_eq!(
            software_tonemap_filter(&s, HdrTransfer::Hlg),
            "setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc,\
             zscale=t=linear:npl=400,format=gbrpf32le,zscale=p=bt709,\
             tonemap=tonemap=reinhard:desat=0:param=0.5,\
             zscale=t=bt709:m=bt709:r=pc,format=yuv420p"
        );
        assert_eq!(HdrTransfer::from_range_type("HDR10"), Some(HdrTransfer::Pq));
        assert_eq!(HdrTransfer::from_range_type("HLG"), Some(HdrTransfer::Hlg));
        assert_eq!(HdrTransfer::from_range_type("DOVI"), None);
        assert_eq!(HdrTransfer::from_range_type("SDR"), None);
    }

    #[test]
    fn r3transcoding_downmix_filters() {
        let mut s = EncodingSettings::default();
        assert_eq!(downmix_filters(&s, Some(6), 2), ["volume=2"]);
        assert!(downmix_filters(&s, Some(2), 2).is_empty());
        assert!(downmix_filters(&s, Some(6), 6).is_empty());
        assert!(downmix_filters(&s, None, 2).is_empty());
        s.downmix_stereo_algorithm = "Dave750".into();
        s.downmix_audio_boost = 1.0;
        assert_eq!(
            downmix_filters(&s, Some(6), 2),
            ["pan=stereo|c0=0.5*c2+0.707*c0+0.707*c4+0.5*c3|c1=0.5*c2+0.707*c1+0.707*c5+0.5*c3"]
        );
        s.downmix_stereo_algorithm = "Ac4".into();
        assert!(downmix_filters(&s, Some(6), 2).is_empty());
    }

    const HWACCELS: &str = "Hardware acceleration methods:\nvdpau\ncuda\nvaapi\n\n";
    const ENCODERS: &str = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n \
        V....D libx264              libx264 H.264 (codec h264)\n \
        V....D h264_vaapi           H.264/AVC (VAAPI) (codec h264)\n \
        A....D aac                  AAC (Advanced Audio Coding)\n";
    const FILTERS: &str = "Filters:\n  T.. = Timeline support\n  | = Source or sink filter\n \
        ... hwupload          V->V       Upload a normal frame\n \
        .SC zscale            V->V       Apply resizing\n";

    #[test]
    fn r3transcoding_capability_lists_parse() {
        assert_eq!(
            parse_hwaccels(HWACCELS).unwrap(),
            ["vdpau", "cuda", "vaapi"]
        );
        let encoders = parse_encoders(ENCODERS).unwrap();
        assert!(encoders.contains("libx264") && encoders.contains("h264_vaapi"));
        assert!(encoders.contains("aac"));
        assert!(!encoders.contains("="));
        let filters = parse_filters(FILTERS).unwrap();
        assert_eq!(filters.len(), 2);
        assert!(filters.contains("zscale"));
        assert_eq!(parse_hwaccels("r3gaps2"), None);
        assert_eq!(parse_encoders(""), None);
        let unknown = FfmpegCapabilities::default();
        assert!(unknown.has_encoder("h264_nvenc") && unknown.has_filter("zscale"));
    }

    /// The real ffmpeg's lists parse, when it is installed.
    #[tokio::test]
    async fn r3transcoding_probe_reads_the_installed_ffmpeg() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            eprintln!("skipped: no ffmpeg");
            return;
        }
        let caps = probe_capabilities(Path::new("ffmpeg")).await;
        let encoders = caps.encoders.as_ref().expect("encoders");
        assert!(encoders.contains("aac"), "{encoders:?}");
        assert!(caps.hwaccels.is_some());
        assert!(caps.filters.as_ref().expect("filters").contains("scale"));
    }
}
