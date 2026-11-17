//! Hardware acceleration detection: which video devices this machine has, and which codecs
//! each one really decodes, checked by opening the device and decoding a tiny generated clip of
//! each codec with ffmpeg.
//!
//! Unlike Jellyfin, whose default is no acceleration, Rustyfin decodes on the best detected
//! device while the administrator has never chosen a `HardwareAccelerationType`. Any explicit
//! choice, `none` included, wins. [`effective_options`] lays the detected values under the saved
//! options; [`keep_automatic`] keeps a save of the unchanged automatic values automatic.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::process::Command;
use tracing::{info, warn};

use crate::encoding::{self, EncodingSettings, HardwareAccelerationType};
use crate::transcode::HwAccel;

/// A device to try: an acceleration type and the device it opens (a render node, a CUDA index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub accel: HardwareAccelerationType,
    pub device: String,
}

/// The test clips, one per codec and bit depth the transcoder decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Clip {
    H264,
    Hevc,
    Hevc10,
    Vp9,
    Vp910,
    Av1,
}

impl Clip {
    pub const ALL: [Clip; 6] = [
        Clip::H264,
        Clip::Hevc,
        Clip::Hevc10,
        Clip::Vp9,
        Clip::Vp910,
        Clip::Av1,
    ];

    fn file_name(self) -> &'static str {
        match self {
            Clip::H264 => "h264.mkv",
            Clip::Hevc => "hevc.mkv",
            Clip::Hevc10 => "hevc10.mkv",
            Clip::Vp9 => "vp9.mkv",
            Clip::Vp910 => "vp9-10.mkv",
            Clip::Av1 => "av1.mkv",
        }
    }

    fn ten_bit(self) -> bool {
        matches!(self, Clip::Hevc10 | Clip::Vp910)
    }

    /// The software encoders that can write the clip, the first one ffmpeg has wins, with
    /// their speed options.
    fn encoders(self) -> &'static [&'static [&'static str]] {
        match self {
            Clip::H264 => &[&["-c:v", "libx264", "-preset", "ultrafast"]],
            Clip::Hevc | Clip::Hevc10 => &[&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=none",
            ]],
            Clip::Vp9 | Clip::Vp910 => &[&[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
            ]],
            Clip::Av1 => &[
                &["-c:v", "libsvtav1", "-preset", "12"],
                &["-c:v", "libaom-av1", "-cpu-used", "8", "-usage", "realtime"],
                &["-c:v", "librav1e", "-speed", "10"],
            ],
        }
    }
}

/// One device and what the probe found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DetectedDevice {
    #[serde(rename = "Type")]
    pub accel: HardwareAccelerationType,
    pub device: String,
    /// It opened and decoded at least one clip
    pub usable: bool,
    /// Why it isn't usable
    pub error: Option<String>,
    /// The codecs it decoded, as Jellyfin's `HardwareDecodingCodecs` names them
    pub hardware_decoding_codecs: Vec<String>,
    pub enable_decoding_color_depth_10_hevc: bool,
    pub enable_decoding_color_depth_10_vp9: bool,
}

/// Every device found, and the one Rustyfin uses while the administrator hasn't chosen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct HardwareDetection {
    pub devices: Vec<DetectedDevice>,
    pub selected: Option<DetectedDevice>,
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

/// Opens devices and decodes clips on them. [`FfmpegProber`] runs ffmpeg; tests use a fake.
pub trait Prober {
    /// Ok when the device opens; else why not.
    fn open(&self, candidate: &Candidate) -> impl Future<Output = Result<(), String>> + Send;
    /// Whether the device decodes `clip`.
    fn decodes(&self, candidate: &Candidate, clip: Clip) -> impl Future<Output = bool> + Send;
    /// Ok when the device encodes H.264 with the type's encoder (selecting a device also turns
    /// hardware encoding on); else why not.
    fn encodes(&self, candidate: &Candidate) -> impl Future<Output = Result<(), String>> + Send;
}

/// Probes every candidate, then picks the best usable one (`select`).
pub async fn detect_with(prober: &impl Prober, candidates: &[Candidate]) -> HardwareDetection {
    let mut devices = Vec::new();
    for candidate in candidates {
        let mut found = DetectedDevice {
            accel: candidate.accel,
            device: candidate.device.clone(),
            usable: false,
            error: None,
            hardware_decoding_codecs: Vec::new(),
            enable_decoding_color_depth_10_hevc: false,
            enable_decoding_color_depth_10_vp9: false,
        };
        if let Err(e) = prober.open(candidate).await {
            found.error = Some(e);
            devices.push(found);
            continue;
        }
        let mut decoded = HashSet::new();
        for clip in Clip::ALL {
            if prober.decodes(candidate, clip).await {
                decoded.insert(clip);
            }
        }
        for (clip, name) in [
            (Clip::H264, "h264"),
            (Clip::Hevc, "hevc"),
            (Clip::Vp9, "vp9"),
            (Clip::Av1, "av1"),
        ] {
            if decoded.contains(&clip) {
                found.hardware_decoding_codecs.push(name.to_string());
            }
        }
        found.enable_decoding_color_depth_10_hevc =
            decoded.contains(&Clip::Hevc) && decoded.contains(&Clip::Hevc10);
        found.enable_decoding_color_depth_10_vp9 =
            decoded.contains(&Clip::Vp9) && decoded.contains(&Clip::Vp910);
        found.usable = !found.hardware_decoding_codecs.is_empty();
        if !found.usable {
            found.error = Some("it opened but decoded none of the test clips".to_string());
        } else if let Err(e) = prober.encodes(candidate).await {
            found.usable = false;
            found.error = Some(format!("it decodes but can't encode H.264: {e}"));
        }
        devices.push(found);
    }
    let selected = select(&devices).cloned();
    HardwareDetection {
        devices,
        selected,
        detected_at: chrono::Utc::now(),
    }
}

/// The best usable device the transcoder drives (VAAPI, QSV, NVENC): the one that decodes the
/// most clips, then NVENC before QSV before VAAPI (QSV is Jellyfin's advice for Intel), then the
/// first found.
pub fn select(devices: &[DetectedDevice]) -> Option<&DetectedDevice> {
    let rank = |accel| match accel {
        HardwareAccelerationType::Nvenc => 0,
        HardwareAccelerationType::Qsv => 1,
        _ => 2,
    };
    let score = |d: &DetectedDevice| {
        d.hardware_decoding_codecs.len()
            + usize::from(d.enable_decoding_color_depth_10_hevc)
            + usize::from(d.enable_decoding_color_depth_10_vp9)
    };
    devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.usable && d.accel.transcoder_accel() != HwAccel::None)
        .min_by_key(|(i, d)| (std::cmp::Reverse(score(d)), rank(d.accel), *i))
        .map(|(_, d)| d)
}

// ---------------------------------------------------------------------------
// Automatic and explicit options
// ---------------------------------------------------------------------------

/// The fields detection fills while the administrator hasn't chosen a type.
pub const AUTOMATIC_FIELDS: [&str; 6] = [
    "HardwareAccelerationType",
    "VaapiDevice",
    "QsvDevice",
    "HardwareDecodingCodecs",
    "EnableDecodingColorDepth10Hevc",
    "EnableDecodingColorDepth10Vp9",
];

fn saved_field<'a>(saved: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    saved
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

/// Whether a saved value says nothing: missing, null, or (for the type) an empty string.
fn unset(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        _ => false,
    }
}

/// Whether the administrator has never chosen a `HardwareAccelerationType` (it is missing,
/// null or empty in the saved options). An explicit `none` is a choice.
pub fn type_is_automatic(saved: Option<&Map<String, Value>>) -> bool {
    unset(saved.and_then(|s| saved_field(s, "HardwareAccelerationType")))
}

/// The automatic fields' values for `selected` (Jellyfin's defaults under the device's own);
/// without one, Jellyfin's defaults.
fn automatic_values(selected: Option<&DetectedDevice>) -> Map<String, Value> {
    let defaults = EncodingSettings::default();
    let mut values = Map::new();
    values.insert("HardwareAccelerationType".into(), json!("none"));
    values.insert("VaapiDevice".into(), json!(defaults.vaapi_device));
    values.insert("QsvDevice".into(), json!(defaults.qsv_device));
    values.insert(
        "HardwareDecodingCodecs".into(),
        json!(defaults.hardware_decoding_codecs),
    );
    values.insert(
        "EnableDecodingColorDepth10Hevc".into(),
        json!(defaults.enable_decoding_color_depth_10_hevc),
    );
    values.insert(
        "EnableDecodingColorDepth10Vp9".into(),
        json!(defaults.enable_decoding_color_depth_10_vp9),
    );
    let Some(d) = selected else {
        return values;
    };
    values.insert("HardwareAccelerationType".into(), json!(d.accel.name()));
    match d.accel {
        HardwareAccelerationType::Vaapi => {
            values.insert("VaapiDevice".into(), json!(d.device));
        }
        HardwareAccelerationType::Qsv => {
            values.insert("QsvDevice".into(), json!(d.device));
        }
        _ => {}
    }
    values.insert(
        "HardwareDecodingCodecs".into(),
        json!(d.hardware_decoding_codecs),
    );
    values.insert(
        "EnableDecodingColorDepth10Hevc".into(),
        json!(d.enable_decoding_color_depth_10_hevc),
    );
    values.insert(
        "EnableDecodingColorDepth10Vp9".into(),
        json!(d.enable_decoding_color_depth_10_vp9),
    );
    values
}

/// The options in effect: Jellyfin's `defaults`, then, while the type is automatic, the
/// selected device's values, then what the administrator saved (its unset automatic fields
/// left out, so they don't hide the detected ones).
pub fn effective_options(
    defaults: Map<String, Value>,
    saved: Option<&Map<String, Value>>,
    detection: Option<&HardwareDetection>,
) -> Map<String, Value> {
    let mut options = defaults;
    let selected = detection.and_then(|d| d.selected.as_ref());
    if type_is_automatic(saved)
        && let Some(selected) = selected
    {
        options.extend(automatic_values(Some(selected)));
    }
    if let Some(saved) = saved {
        for (key, value) in saved {
            let automatic = AUTOMATIC_FIELDS.iter().any(|f| f.eq_ignore_ascii_case(key));
            if automatic && unset(Some(value)) {
                continue;
            }
            options.insert(key.clone(), value.clone());
        }
    }
    options
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.trim().eq_ignore_ascii_case(b.trim()),
        (Value::Array(a), Value::Array(b)) => {
            let set = |items: &Vec<Value>| -> HashSet<String> {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_ascii_lowercase())
                    .collect()
            };
            set(a) == set(b)
        }
        (a, b) => a == b,
    }
}

/// A save (`body`) that leaves the type automatic: one without a type (null or empty; the
/// dashboard's "Automatic" sends null). Its automatic fields that carry the automatic value are
/// left out, so they follow the detection; a changed value is kept as the administrator's
/// choice. A save that names any type, `none` included, is stored as it came: a `none` posted
/// while detection hasn't finished (or found nothing) must not turn into hardware later.
pub fn keep_automatic(body: &mut Map<String, Value>, detection: Option<&HardwareDetection>) {
    let automatic = automatic_values(detection.and_then(|d| d.selected.as_ref()));
    if !unset(saved_field(body, "HardwareAccelerationType")) {
        return;
    }
    body.retain(|key, value| {
        let Some((_, auto)) = automatic.iter().find(|(f, _)| f.eq_ignore_ascii_case(key)) else {
            return true;
        };
        !key.eq_ignore_ascii_case("HardwareAccelerationType")
            && !unset(Some(value))
            && !same_value(value, auto)
    });
}

// ---------------------------------------------------------------------------
// The machine's devices
// ---------------------------------------------------------------------------

/// The devices to try: each DRM render node (`/dev/dri/renderD*`) as VAAPI, and also as QSV on an
/// Intel GPU; NVIDIA's first GPU when `/dev/nvidia0` exists or `nvidia-smi` lists one; and
/// VideoToolbox on macOS.
pub async fn candidates() -> Vec<Candidate> {
    let mut found = Vec::new();
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("renderD"))
                })
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    for node in nodes {
        let device = node.to_string_lossy().into_owned();
        let name = node.file_name().unwrap_or_default().to_string_lossy();
        let vendor = std::fs::read_to_string(format!("/sys/class/drm/{name}/device/vendor"))
            .unwrap_or_default();
        found.push(Candidate {
            accel: HardwareAccelerationType::Vaapi,
            device: device.clone(),
        });
        if vendor.trim().eq_ignore_ascii_case("0x8086") {
            found.push(Candidate {
                accel: HardwareAccelerationType::Qsv,
                device,
            });
        }
    }
    if Path::new("/dev/nvidia0").exists() || nvidia_smi_lists_a_gpu().await {
        found.push(Candidate {
            accel: HardwareAccelerationType::Nvenc,
            device: "0".to_string(),
        });
    }
    if cfg!(target_os = "macos") {
        found.push(Candidate {
            accel: HardwareAccelerationType::VideoToolbox,
            device: String::new(),
        });
    }
    found
}

async fn nvidia_smi_lists_a_gpu() -> bool {
    let run = Command::new("nvidia-smi")
        .arg("-L")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(10), run).await {
        Ok(Ok(out)) => out.status.success() && String::from_utf8_lossy(&out.stdout).contains("GPU"),
        _ => false,
    }
}

/// Probes with the machine's ffmpeg, on clips it writes into `clip_dir`.
pub struct FfmpegProber {
    ffmpeg: PathBuf,
    /// The server's data directory, for the drivers' cache (`ffmpeg_env`)
    data_dir: PathBuf,
    clips: Vec<(Clip, PathBuf)>,
}

/// The group id that owns `device`, as `stat -c %g` prints it.
fn device_gid(device: &str) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(device).ok().map(|m| m.gid())
    }
    #[cfg(not(unix))]
    {
        let _ = device;
        None
    }
}

/// What to do about a device the server's user may not open: join its group, or, in a
/// container, be given the group's id.
pub fn permission_hint(device: &str) -> String {
    let gid = device_gid(device).map_or("<gid>".to_string(), |g| g.to_string());
    format!(
        "permission denied on {device}: add the server's user to the device's group (often \
         render or video); in a container, pass the device's group id with --group-add {gid} \
         (stat -c %g {device}), or group_add in docker-compose"
    )
}

/// The folder GPU drivers cache in while ffmpeg runs (Mesa's shader cache, among others):
/// `gpu` in the server's cache folder (`cache` in the data directory).
pub fn gpu_cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("gpu")
}

/// The environment every ffmpeg child gets, so GPU drivers cache in `gpu_cache_dir` (created
/// here when missing) rather than under a `$HOME` that may be unset or not writable, as in a
/// container run as another user (Mesa then prints "Failed to create //.cache for shader
/// cache"). Transcodes, the hardware detection and trickplay pass it with `Command::envs`.
pub fn ffmpeg_env(data_dir: &Path) -> [(&'static str, PathBuf); 2] {
    let dir = gpu_cache_dir(data_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let key = format!("gpu-cache|{}", dir.display());
        if logged()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            warn!(
                "cannot make the GPU drivers' cache folder {}: {e}",
                dir.display()
            );
        }
    }
    [
        ("XDG_CACHE_HOME", dir.clone()),
        ("MESA_SHADER_CACHE_DIR", dir),
    ]
}

/// Whether an ffmpeg stderr line is a GPU driver's notice rather than ffmpeg's own error: the
/// AMD driver's note on shared DRM file descriptors and Mesa's shader cache warning. These are
/// harmless, so they are left out of the reasons a run gives; a run fails only by its exit.
pub fn driver_chatter(line: &str) -> bool {
    let line = line.trim();
    line.starts_with("amdgpu:")
        || line.starts_with("If they do, bad things may happen")
        || (line.contains("shader cache") && line.starts_with("Failed to create"))
}

/// The longest one probe run may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

async fn run(ffmpeg: &Path, data_dir: &Path, args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.envs(ffmpeg_env(data_dir))
        .args(["-nostdin", "-hide_banner", "-v", "error", "-y"])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Err(_) => Err("ffmpeg did not finish in time".to_string()),
        Ok(Err(e)) => Err(format!("could not run ffmpeg: {e}")),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !driver_chatter(l))
                .take(3)
                .collect::<Vec<_>>()
                .join("; ");
            Err(if last.is_empty() {
                format!("ffmpeg exited with {}", out.status)
            } else {
                last
            })
        }
    }
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

impl FfmpegProber {
    /// Writes the test clips (256×144, 3 frames) with the software encoders ffmpeg has; a codec
    /// without one is left out, so no device is said to decode it.
    pub async fn new(ffmpeg: &Path, data_dir: &Path, clip_dir: &Path) -> Self {
        let _ = tokio::fs::create_dir_all(clip_dir).await;
        let mut clips = Vec::new();
        for clip in Clip::ALL {
            let path = clip_dir.join(clip.file_name());
            let pix_fmt = if clip.ten_bit() {
                "yuv420p10le"
            } else {
                "yuv420p"
            };
            for encoder in clip.encoders() {
                let mut args = strings(&["-f", "lavfi", "-i", "testsrc2=s=256x144:r=10:d=0.3"]);
                args.extend(strings(encoder));
                args.extend(strings(&["-pix_fmt", pix_fmt, "-an"]));
                args.push(path.to_string_lossy().into_owned());
                if run(ffmpeg, data_dir, &args).await.is_ok() {
                    clips.push((clip, path));
                    break;
                }
            }
        }
        Self {
            ffmpeg: ffmpeg.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
            clips,
        }
    }

    fn settings(candidate: &Candidate) -> EncodingSettings {
        let mut settings = EncodingSettings::default();
        match candidate.accel {
            HardwareAccelerationType::Vaapi => settings.vaapi_device = candidate.device.clone(),
            HardwareAccelerationType::Qsv => settings.qsv_device = candidate.device.clone(),
            _ => {}
        }
        settings
    }

    /// The device options as the transcoder writes them (VideoToolbox, which it doesn't drive,
    /// as Jellyfin writes them).
    fn device_args(candidate: &Candidate, decode: bool) -> Vec<String> {
        match candidate.accel {
            HardwareAccelerationType::VideoToolbox => {
                let mut args = strings(&["-init_hw_device", "videotoolbox=vt"]);
                if decode {
                    args.extend(strings(&[
                        "-hwaccel",
                        "videotoolbox",
                        "-hwaccel_output_format",
                        "videotoolbox_vld",
                    ]));
                }
                args
            }
            accel => crate::transcode::hw_device_args(
                accel.transcoder_accel(),
                decode,
                &Self::settings(candidate),
            ),
        }
    }
}

impl Prober for FfmpegProber {
    async fn open(&self, candidate: &Candidate) -> Result<(), String> {
        if candidate.device.starts_with('/') {
            // A render node the server's user can't open: say so plainly
            if let Err(e) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&candidate.device)
            {
                return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
                    permission_hint(&candidate.device)
                } else {
                    format!("cannot open {}: {e}", candidate.device)
                });
            }
        }
        let mut args = Self::device_args(candidate, false);
        args.extend(strings(&[
            "-f",
            "lavfi",
            "-i",
            "nullsrc=s=64x64:d=0.1",
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ]));
        run(&self.ffmpeg, &self.data_dir, &args).await
    }

    async fn decodes(&self, candidate: &Candidate, clip: Clip) -> bool {
        let Some((_, path)) = self.clips.iter().find(|(c, _)| *c == clip) else {
            return false;
        };
        // hwdownload takes only frames on the device: a decoder that fell back to software fails
        let format = if clip.ten_bit() { "p010" } else { "nv12" };
        let mut args = Self::device_args(candidate, true);
        args.extend([
            "-i".to_string(),
            path.to_string_lossy().into_owned(),
            "-vf".to_string(),
            format!("hwdownload,format={format}"),
            "-f".to_string(),
            "null".to_string(),
            "-".to_string(),
        ]);
        run(&self.ffmpeg, &self.data_dir, &args).await.is_ok()
    }

    async fn encodes(&self, candidate: &Candidate) -> Result<(), String> {
        let accel = candidate.accel.transcoder_accel();
        // A type the transcoder doesn't drive never encodes on the device
        let Some(encoder) = encoding::hardware_encoder(accel, "h264") else {
            return Ok(());
        };
        // A few generated frames, taken to the device as a transcode takes them
        let mut args = Self::device_args(candidate, false);
        args.extend(strings(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=256x144:r=10:d=0.3",
            "-vf",
        ]));
        args.push(crate::transcode::hw_encoder_upload(accel).to_string());
        args.extend(strings(&["-c:v", encoder, "-f", "null", "-"]));
        run(&self.ffmpeg, &self.data_dir, &args).await
    }
}

/// Removes the probe's work folder when dropped, so a detection that is cancelled (its request
/// gone) leaves nothing behind.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The reasons already logged, so a device that can't be used is explained once per run of the
/// server, not at every detection.
fn logged() -> &'static Mutex<HashSet<String>> {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LOGGED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Finds this machine's devices and probes each with `ffmpeg`, on clips written under
/// `work_dir` (removed afterwards). Logs what was found, and why a device isn't usable once.
pub async fn detect(ffmpeg: &Path, data_dir: &Path, work_dir: &Path) -> HardwareDetection {
    let candidates = candidates().await;
    let detection = if candidates.is_empty() {
        detect_with(&NoDevices, &[]).await
    } else {
        let _cleanup = RemoveOnDrop(work_dir.to_path_buf());
        let prober = FfmpegProber::new(ffmpeg, data_dir, work_dir).await;
        detect_with(&prober, &candidates).await
    };
    for device in &detection.devices {
        let Some(error) = &device.error else { continue };
        let key = format!("{}|{}|{error}", device.accel.name(), device.device);
        if logged()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            warn!(
                "hardware acceleration: {} {} is not usable: {error}",
                device.accel.name(),
                device.device
            );
        }
    }
    match &detection.selected {
        Some(d) => info!(
            "hardware acceleration detected: {} on {}, decodes {}{}{}",
            d.accel.name(),
            if d.device.is_empty() { "-" } else { &d.device },
            d.hardware_decoding_codecs.join(", "),
            if d.enable_decoding_color_depth_10_hevc {
                ", hevc 10-bit"
            } else {
                ""
            },
            if d.enable_decoding_color_depth_10_vp9 {
                ", vp9 10-bit"
            } else {
                ""
            },
        ),
        None => info!("hardware acceleration: no usable device found, transcoding in software"),
    }
    detection
}

struct NoDevices;

impl Prober for NoDevices {
    async fn open(&self, _: &Candidate) -> Result<(), String> {
        Err("no device".to_string())
    }
    async fn decodes(&self, _: &Candidate, _: Clip) -> bool {
        false
    }
    async fn encodes(&self, _: &Candidate) -> Result<(), String> {
        Err("no device".to_string())
    }
}

/// The encoding options in effect for `data_dir`'s saved options and `detection`.
pub fn effective_settings(
    data_dir: &Path,
    ffmpeg: &str,
    transcode_dir: &Path,
    detection: Option<&HardwareDetection>,
) -> EncodingSettings {
    let saved = encoding::load_saved(data_dir);
    let options = effective_options(
        encoding::default_options(ffmpeg, transcode_dir),
        saved.as_ref(),
        detection,
    );
    EncodingSettings::from_options(&options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A prober that answers from a table: the devices that open, and the clips each decodes.
    struct Fake {
        opens: HashMap<String, Result<(), String>>,
        decodes: HashMap<String, Vec<Clip>>,
    }

    impl Prober for Fake {
        async fn open(&self, c: &Candidate) -> Result<(), String> {
            self.opens
                .get(&c.device)
                .cloned()
                .unwrap_or_else(|| Err("missing".into()))
        }
        async fn decodes(&self, c: &Candidate, clip: Clip) -> bool {
            self.decodes
                .get(&format!("{}{}", c.accel.name(), c.device))
                .is_some_and(|clips| clips.contains(&clip))
        }
        async fn encodes(&self, _: &Candidate) -> Result<(), String> {
            Ok(())
        }
    }

    /// `Fake`, but the devices of `fails` (type and device, as `decodes` keys them) can't encode.
    struct NoEncode(Fake, Vec<&'static str>);

    impl Prober for NoEncode {
        async fn open(&self, c: &Candidate) -> Result<(), String> {
            self.0.open(c).await
        }
        async fn decodes(&self, c: &Candidate, clip: Clip) -> bool {
            self.0.decodes(c, clip).await
        }
        async fn encodes(&self, c: &Candidate) -> Result<(), String> {
            let key = format!("{}{}", c.accel.name(), c.device);
            if self.1.contains(&key.as_str()) {
                Err("Function not implemented".into())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn hwaccel_detection_drops_a_device_that_cannot_encode() {
        use HardwareAccelerationType::*;
        let all = vec![Clip::H264, Clip::Hevc, Clip::Av1];
        let fake = Fake {
            opens: [("/a".to_string(), Ok(())), ("/b".to_string(), Ok(()))].into(),
            decodes: [
                ("vaapi/a".to_string(), all.clone()),
                ("vaapi/b".to_string(), vec![Clip::H264]),
            ]
            .into(),
        };
        let prober = NoEncode(fake, vec!["vaapi/a"]);
        let d = detect_with(&prober, &[cand(Vaapi, "/a"), cand(Vaapi, "/b")]).await;
        assert!(!d.devices[0].usable);
        assert!(
            d.devices[0]
                .error
                .as_deref()
                .is_some_and(|e| e.contains("can't encode H.264")),
            "{:?}",
            d.devices[0]
        );
        // The one that decodes less but encodes is selected
        assert_eq!(d.selected.unwrap().device, "/b");
    }

    fn cand(accel: HardwareAccelerationType, device: &str) -> Candidate {
        Candidate {
            accel,
            device: device.into(),
        }
    }

    #[tokio::test]
    async fn hwaccel_detection_picks_the_device_that_decodes_most() {
        use HardwareAccelerationType::*;
        let fake = Fake {
            opens: [
                ("/dev/dri/renderD128".to_string(), Ok(())),
                (
                    "/dev/dri/renderD129".to_string(),
                    Err("permission denied".into()),
                ),
                ("/dev/dri/renderD130".to_string(), Ok(())),
            ]
            .into(),
            decodes: [
                ("vaapi/dev/dri/renderD128".to_string(), vec![Clip::H264]),
                (
                    "vaapi/dev/dri/renderD130".to_string(),
                    vec![Clip::H264, Clip::Hevc, Clip::Hevc10, Clip::Av1],
                ),
            ]
            .into(),
        };
        let candidates = [
            cand(Vaapi, "/dev/dri/renderD128"),
            cand(Vaapi, "/dev/dri/renderD129"),
            cand(Vaapi, "/dev/dri/renderD130"),
        ];
        let d = detect_with(&fake, &candidates).await;
        assert_eq!(d.devices.len(), 3);
        assert!(!d.devices[1].usable);
        assert_eq!(d.devices[1].error.as_deref(), Some("permission denied"));
        let s = d.selected.expect("a device");
        assert_eq!(s.device, "/dev/dri/renderD130");
        assert_eq!(s.hardware_decoding_codecs, ["h264", "hevc", "av1"]);
        assert!(s.enable_decoding_color_depth_10_hevc);
        assert!(!s.enable_decoding_color_depth_10_vp9);
    }

    #[tokio::test]
    async fn hwaccel_detection_skips_devices_that_decode_nothing_or_aren_t_driven() {
        use HardwareAccelerationType::*;
        let fake = Fake {
            opens: [("/a".to_string(), Ok(())), (String::new(), Ok(()))].into(),
            decodes: [("videotoolbox".to_string(), vec![Clip::H264, Clip::Hevc])].into(),
        };
        let d = detect_with(&fake, &[cand(Vaapi, "/a"), cand(VideoToolbox, "")]).await;
        assert!(!d.devices[0].usable);
        assert!(d.devices[0].error.is_some());
        // VideoToolbox decodes, but the transcoder doesn't drive it: found, not selected
        assert!(d.devices[1].usable);
        assert!(d.selected.is_none());
    }

    #[tokio::test]
    async fn hwaccel_detection_breaks_ties_nvenc_then_qsv_then_vaapi() {
        use HardwareAccelerationType::*;
        let all = vec![Clip::H264, Clip::Hevc];
        let fake = Fake {
            opens: [("/r".to_string(), Ok(())), ("0".to_string(), Ok(()))].into(),
            decodes: [
                ("vaapi/r".to_string(), all.clone()),
                ("qsv/r".to_string(), all.clone()),
                ("nvenc0".to_string(), all.clone()),
            ]
            .into(),
        };
        let both = [cand(Vaapi, "/r"), cand(Qsv, "/r")];
        let d = detect_with(&fake, &both).await;
        assert_eq!(d.selected.unwrap().accel, Qsv);
        let three = [cand(Vaapi, "/r"), cand(Qsv, "/r"), cand(Nvenc, "0")];
        assert_eq!(
            detect_with(&fake, &three).await.selected.unwrap().accel,
            Nvenc
        );
    }

    fn detection(accel: HardwareAccelerationType) -> HardwareDetection {
        let device = DetectedDevice {
            accel,
            device: "/dev/dri/renderD128".into(),
            usable: true,
            error: None,
            hardware_decoding_codecs: vec!["h264".into(), "hevc".into(), "vp9".into()],
            enable_decoding_color_depth_10_hevc: true,
            enable_decoding_color_depth_10_vp9: false,
        };
        HardwareDetection {
            devices: vec![device.clone()],
            selected: Some(device),
            detected_at: chrono::Utc::now(),
        }
    }

    fn settings(saved: Option<Value>, det: Option<&HardwareDetection>) -> EncodingSettings {
        let saved = saved.map(|v| v.as_object().unwrap().clone());
        let options = effective_options(
            encoding::default_options("ffmpeg", Path::new("/t")),
            saved.as_ref(),
            det,
        );
        EncodingSettings::from_options(&options)
    }

    #[test]
    fn hwaccel_never_set_takes_the_detected_device() {
        let det = detection(HardwareAccelerationType::Vaapi);
        for saved in [
            None,
            Some(json!({})),
            Some(json!({ "H264Crf": 20 })),
            Some(json!({ "HardwareAccelerationType": null })),
            Some(json!({ "HardwareAccelerationType": "" })),
        ] {
            let s = settings(saved.clone(), Some(&det));
            assert_eq!(
                s.hardware_acceleration,
                HardwareAccelerationType::Vaapi,
                "{saved:?}"
            );
            assert_eq!(s.vaapi_device, "/dev/dri/renderD128");
            assert_eq!(s.hardware_decoding_codecs, ["h264", "hevc", "vp9"]);
            assert!(s.enable_decoding_color_depth_10_hevc);
            assert!(!s.enable_decoding_color_depth_10_vp9);
        }
        // Nothing detected: Jellyfin's default, software
        let s = settings(None, None);
        assert_eq!(s.hardware_acceleration, HardwareAccelerationType::None);
    }

    #[test]
    fn hwaccel_an_explicit_choice_wins_none_included() {
        let det = detection(HardwareAccelerationType::Vaapi);
        let s = settings(
            Some(json!({ "HardwareAccelerationType": "none" })),
            Some(&det),
        );
        assert_eq!(s.hardware_acceleration, HardwareAccelerationType::None);
        assert_eq!(s.hardware_decoding_codecs, ["h264", "vc1"]);
        let s = settings(
            Some(json!({ "HardwareAccelerationType": "nvenc" })),
            Some(&det),
        );
        assert_eq!(s.hardware_acceleration, HardwareAccelerationType::Nvenc);
        // An automatic type with codecs the administrator chose keeps those codecs
        let s = settings(
            Some(json!({ "HardwareDecodingCodecs": ["h264"] })),
            Some(&det),
        );
        assert_eq!(s.hardware_acceleration, HardwareAccelerationType::Vaapi);
        assert_eq!(s.hardware_decoding_codecs, ["h264"]);
    }

    #[test]
    fn hwaccel_a_save_of_the_automatic_values_stays_automatic() {
        let det = detection(HardwareAccelerationType::Vaapi);
        // The page sends back what it read with "Automatic" (a null type) and another change
        let read = effective_options(
            encoding::default_options("ffmpeg", Path::new("/t")),
            None,
            Some(&det),
        );
        let mut body = read.clone();
        body.insert("HardwareAccelerationType".into(), Value::Null);
        body.insert("H264Crf".into(), json!(20));
        keep_automatic(&mut body, Some(&det));
        assert!(type_is_automatic(Some(&body)));
        for field in AUTOMATIC_FIELDS {
            assert!(!body.contains_key(field), "{field} kept");
        }
        assert_eq!(body["H264Crf"], json!(20));

        // Any named type is the administrator's choice, the detected one included
        let mut body = read.clone();
        keep_automatic(&mut body, Some(&det));
        assert_eq!(body["HardwareAccelerationType"], json!("vaapi"));
        assert!(body.contains_key("HardwareDecodingCodecs"));

        // None while detection hasn't finished, or found nothing, stays none: a later
        // detection must not turn the hardware on
        for detected in [None, Some(&det)] {
            let mut body = encoding::default_options("ffmpeg", Path::new("/t"));
            assert_eq!(body["HardwareAccelerationType"], json!("none"));
            keep_automatic(&mut body, detected);
            assert_eq!(body["HardwareAccelerationType"], json!("none"));
            assert!(!type_is_automatic(Some(&body)));
            let s = settings(Some(Value::Object(body)), Some(&det));
            assert_eq!(s.hardware_acceleration, HardwareAccelerationType::None);
        }

        // Back to automatic (a null type) from a choice: a changed codec list stays chosen
        let mut body = read;
        body.insert("HardwareAccelerationType".into(), Value::Null);
        body.insert("HardwareDecodingCodecs".into(), json!(["h264"]));
        keep_automatic(&mut body, Some(&det));
        assert!(type_is_automatic(Some(&body)));
        assert!(!body.contains_key("VaapiDevice"));
        assert_eq!(body["HardwareDecodingCodecs"], json!(["h264"]));
    }

    #[test]
    fn hwaccel_driver_chatter_and_the_permission_hint() {
        for line in [
            "amdgpu: os_same_file_description couldn't determine if two DRM fds reference the same file description.",
            "If they do, bad things may happen!",
            "Failed to create //.cache for shader cache (Permission denied)---disabling.",
        ] {
            assert!(driver_chatter(line), "{line}");
        }
        for line in [
            "Device creation failed: -22.",
            "Failed to initialise VAAPI connection: -1 (unknown libva error).",
            "Failed to create a VAAPI device",
        ] {
            assert!(!driver_chatter(line), "{line}");
        }
        let hint = permission_hint("/dev/null");
        assert!(hint.contains("--group-add"), "{hint}");
        assert!(hint.contains("stat -c %g /dev/null"), "{hint}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let gid = std::fs::metadata("/dev/null").unwrap().gid();
            assert!(hint.contains(&format!("--group-add {gid} ")), "{hint}");
        }
    }

    #[test]
    fn hwaccel_detection_serializes_jellyfin_shaped() {
        let v = serde_json::to_value(detection(HardwareAccelerationType::Vaapi)).unwrap();
        let s = &v["Selected"];
        assert_eq!(s["Type"], json!("vaapi"));
        assert_eq!(s["HardwareDecodingCodecs"], json!(["h264", "hevc", "vp9"]));
        assert_eq!(s["EnableDecodingColorDepth10Hevc"], json!(true));
        assert!(v["DetectedAt"].is_string());
    }
}
