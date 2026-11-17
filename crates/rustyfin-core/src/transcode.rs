use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::encoding::{self, EncodingSettings, FfmpegCapabilities};

// ---------------------------------------------------------------------------
// Transcoding profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwAccel {
    None,
    Vaapi,
    Nvenc,
    Qsv,
}

impl HwAccel {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "vaapi" => Self::Vaapi,
            "nvenc" | "nvidia" => Self::Nvenc,
            "qsv" | "intel" => Self::Qsv,
            _ => Self::None,
        }
    }
}

/// An image subtitle (PGS, VobSub, DVB) to burn in with an overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSubtitle {
    /// The subtitle's absolute stream index in the input (MediaStream.Index)
    pub stream_index: u32,
    /// The source video's width and height, which the subtitle is scaled to first; None
    /// leaves it at its own size
    pub video_size: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscodeProfile {
    pub max_video_bitrate_kbps: u32,
    pub max_audio_bitrate_kbps: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub video_codec: String,
    pub audio_codec: String,
    pub hw_accel: HwAccel,
    pub segment_duration: u32,
    pub burn_subtitles: bool,
    pub subtitle_stream_index: Option<u32>,
    /// The audio stream to transcode: its absolute stream index (MediaStream.Index), not its
    /// place among the audio streams. None takes the first audio stream.
    pub audio_stream_index: Option<u32>,
    /// Whether to transcode audio only (no video)
    pub audio_only: bool,
    /// Enable adaptive bitrate transcoding with multiple quality levels
    pub adaptive_bitrate: bool,
    /// Copy the source video (`-c:v copy`) instead of encoding it.
    pub copy_video: bool,
    /// Copy the source audio (`-c:a copy`) instead of encoding it.
    pub copy_audio: bool,
    /// Where the transcode starts, in ticks; 0 or less starts at the beginning.
    /// Output time 0 is this point.
    pub start_time_ticks: i64,
    /// An image subtitle burned in with an overlay. It wins over the text burn-in, and it turns
    /// the video copy off. Hardware acceleration decodes and scales on the device and overlays in
    /// software; a subtitle of unknown video size turns it off.
    pub image_subtitle: Option<ImageSubtitle>,
    /// Deinterlace the video (`bwdif`). It turns the video copy and hardware acceleration off.
    pub deinterlace: bool,
    /// Scale non-square pixels to square ones (`scale=iw*sar:ih,setsar=1`, even width). It turns
    /// the video copy and hardware acceleration off.
    pub square_pixels: bool,
    /// Write fragmented MP4 segments (`init.mp4` and `segment_NNNN.mp4`) instead of MPEG-TS, as
    /// HEVC needs in HLS
    pub fmp4: bool,
    /// The codec tag of a copied video stream, such as `hvc1`, which Apple players need for HEVC
    pub video_tag: Option<String>,
    /// The channels of an encoded audio stream; None is stereo
    pub audio_channels: Option<u32>,
    /// The sample rate of an encoded audio stream; None keeps the source's where the encoder
    /// takes it
    pub audio_sample_rate: Option<u32>,
    /// The server's encoding options (threads, presets, CRF, downmix, tonemapping, hardware
    /// acceleration). The transcode manager sets them when it starts a transcode.
    pub encoding: EncodingSettings,
    /// The source video's codec (lowercase) and bit depth, which decide whether the device
    /// decodes it; an unknown codec is decoded on the device whenever acceleration is on
    pub source_video_codec: Option<String>,
    pub source_bit_depth: Option<u32>,
    /// The source video's HDR transfer (HDR10 or HLG), which tonemapping maps to SDR
    pub source_hdr: Option<encoding::HdrTransfer>,
    /// The channels of the played audio track, which decide the stereo downmix
    pub source_audio_channels: Option<u32>,
    /// The source video's width and height, from which a device scale gets its exact output
    /// size; None scales by expression
    pub source_size: Option<(u32, u32)>,
}

impl Default for TranscodeProfile {
    fn default() -> Self {
        Self {
            max_video_bitrate_kbps: 8000,
            max_audio_bitrate_kbps: 192,
            max_width: 1920,
            max_height: 1080,
            video_codec: "libx264".to_string(),
            audio_codec: "aac".to_string(),
            hw_accel: HwAccel::None,
            segment_duration: 6,
            burn_subtitles: false,
            subtitle_stream_index: None,
            audio_stream_index: None,
            audio_only: false,
            adaptive_bitrate: false,
            copy_video: false,
            copy_audio: false,
            start_time_ticks: 0,
            image_subtitle: None,
            deinterlace: false,
            square_pixels: false,
            fmp4: false,
            video_tag: None,
            audio_channels: None,
            audio_sample_rate: None,
            encoding: EncodingSettings::default(),
            source_video_codec: None,
            source_bit_depth: None,
            source_hdr: None,
            source_audio_channels: None,
            source_size: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Transcode session
// ---------------------------------------------------------------------------

/// One ffmpeg run. Each playback id has its own session, so two viewers of one item get two.
struct TranscodeSession {
    item_id: Uuid,
    playback_id: Uuid,
    /// The file ffmpeg reads: each version of an item has its own.
    input_path: String,
    play_session_id: String, // "" when the request had none
    device_id: String,       // "" when the request had none
    output_dir: PathBuf,     // cache_root/{playback_id}
    child: Option<Child>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
    stderr_tail: Arc<std::sync::Mutex<VecDeque<String>>>,
    last_access: Instant, // last playlist or segment request
    profile: TranscodeProfile,
    /// A progressive transcode, piped to one HTTP response: it ends with the response, so the
    /// idle cleanup and the HLS reuse leave it alone
    progressive: bool,
}

/// What a starting session looked like when it was polled.
enum StartPoll {
    /// Its playlist is written.
    Ready(PathBuf),
    /// ffmpeg exited without writing a playlist.
    Exited(ExitStatus),
    /// ffmpeg is still running and has not written the playlist yet.
    Running,
    /// The session was stopped or cleaned up.
    Gone,
}

/// A session idle for longer than this (no playlist or segment request) is stopped.
const IDLE_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How many of ffmpeg's last stderr lines are kept for error reports.
const STDERR_TAIL_LINES: usize = 20;
/// How often a playlist wait looks at how much ffmpeg has written.
const PROGRESS_CHECK: Duration = Duration::from_millis(500);
/// The longest a playlist wait runs, as a multiple of `playlist_wait`, however ffmpeg progresses.
const MAX_WAIT_FACTOR: u32 = 10;

// ---------------------------------------------------------------------------
// Transcode manager
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TranscodeManager {
    sessions: Arc<DashMap<Uuid, TranscodeSession>>, // keyed by playback id
    cache_root: PathBuf,
    ffmpeg: PathBuf,         // "ffmpeg"; unit tests point it at a fake script
    playlist_wait: Duration, // 30 s; unit tests shorten it
    start_lock: Arc<tokio::sync::Mutex<()>>,
    /// Set under `start_lock` by `shutdown`: no transcode starts after it
    closed: Arc<std::sync::atomic::AtomicBool>,
    /// The PlaySessionIds stopped lately (in ASCII lower case) and when each was stopped
    stopped: Arc<std::sync::Mutex<HashMap<String, Instant>>>,
    stop_memory: Duration, // 60 s; unit tests shorten it
    /// The most progressive transcodes that run at once (`MAX_PROGRESSIVE`); unit tests lower it
    max_progressive: usize,
    /// The server's encoding options, which each new transcode takes
    settings: Arc<std::sync::RwLock<EncodingSettings>>,
    /// What the ffmpeg build offers, probed the first time a transcode needs to know
    capabilities: Arc<tokio::sync::OnceCell<FfmpegCapabilities>>,
    /// Which transcodes failed on the device; forgotten when the options are saved again
    hardware: Arc<std::sync::Mutex<HardwareHealth>>,
    /// The warnings logged once, until the options are saved again
    warned: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// The transcode reasons PlaybackInfo gave, by PlaySessionId (in ASCII lower case)
    reasons: Arc<std::sync::Mutex<HashMap<String, PlannedReasons>>>,
    /// The last hardware detection (`hwaccel::detect`), None until the first one ends
    detection: Arc<std::sync::RwLock<Option<crate::hwaccel::HardwareDetection>>>,
    /// Held while a detection runs, so two never probe the devices at once
    detecting: Arc<tokio::sync::Mutex<()>>,
}

/// The transcode reasons of one PlaybackInfo answer: each media source's that transcodes.
#[derive(Debug)]
struct PlannedReasons {
    at: Instant,
    sources: Vec<(String, Vec<String>)>,
}

/// How long the reasons of a PlaybackInfo answer are kept, and how many answers at most.
const REASONS_KEPT: Duration = Duration::from_secs(12 * 3600);
const REASONS_LIMIT: usize = 2000;

/// What a session's transcode does, for its `TranscodingInfo` in `/Sessions`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TranscodingSummary {
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub is_video_direct: bool,
    pub is_audio_direct: bool,
    pub audio_channels: Option<u32>,
    /// Jellyfin's `HardwareAccelerationType` name, None for software
    pub hardware_acceleration_type: Option<&'static str>,
    /// Jellyfin's `TranscodeReasons` names
    pub reasons: Vec<String>,
}

/// The hardware failures the manager remembers. A file whose hardware transcode failed (and
/// then ran in software) is transcoded in software from then on. Only the device's own setup
/// failing `DEVICE_FAILURE_LIMIT` times in a row turns the hardware off for every transcode; a
/// hardware transcode that starts resets that count.
#[derive(Debug, Default)]
struct HardwareHealth {
    failed_inputs: std::collections::HashSet<String>,
    device_failures: u32,
    off: bool,
}

/// How many device setup failures in a row turn the hardware off.
const DEVICE_FAILURE_LIMIT: u32 = 3;

/// The most files remembered as failing on the device; past it the memory starts again.
const FAILED_INPUTS_LIMIT: usize = 1000;

/// Whether ffmpeg's last stderr lines say the device itself could not be opened (no driver,
/// no render node, no CUDA), as opposed to one file or one encode session failing.
fn device_setup_failed(stderr: &str) -> bool {
    [
        "Device creation failed",
        "Failed to initialise VAAPI connection",
        "No VA display found",
        "Cannot load libcuda",
        "Could not dynamically load CUDA",
        "cuInit(0) failed",
        "Error creating a MFX session",
        "Failed to create a VAAPI device",
    ]
    .iter()
    .any(|marker| stderr.contains(marker))
}

/// The most progressive transcodes that run at once. Each is one client's playback, so this is
/// well above a household's streams and well below what fills the CPU with encodes.
const MAX_PROGRESSIVE: usize = 8;

impl TranscodeManager {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            cache_root: data_dir.join("transcodes"),
            ffmpeg: PathBuf::from("ffmpeg"),
            playlist_wait: Duration::from_secs(30),
            start_lock: Arc::new(tokio::sync::Mutex::new(())),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stopped: Arc::new(std::sync::Mutex::new(HashMap::new())),
            stop_memory: Duration::from_secs(60),
            max_progressive: MAX_PROGRESSIVE,
            settings: Arc::new(std::sync::RwLock::new(EncodingSettings::default())),
            capabilities: Arc::new(tokio::sync::OnceCell::new()),
            hardware: Arc::new(std::sync::Mutex::new(HardwareHealth::default())),
            warned: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            reasons: Arc::new(std::sync::Mutex::new(HashMap::new())),
            detection: Arc::new(std::sync::RwLock::new(None)),
            detecting: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// The ffmpeg the manager runs.
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// The last hardware detection, None until the first one ends.
    pub fn hardware_detection(&self) -> Option<crate::hwaccel::HardwareDetection> {
        self.detection
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Whether a hardware detection is running.
    pub fn detecting_hardware(&self) -> bool {
        self.detecting.try_lock().is_err()
    }

    /// Detects the machine's devices (`hwaccel::detect`, its clips under `data_dir`), keeps the
    /// result and takes the encoding options it gives: while the administrator hasn't chosen a
    /// type, the best device decodes. Detections run one at a time.
    pub async fn detect_hardware(&self, data_dir: &Path) -> crate::hwaccel::HardwareDetection {
        let guard = self.detecting.lock().await;
        self.detect_hardware_locked(data_dir, guard).await
    }

    /// `detect_hardware`, unless a detection is already running: then None at once.
    pub async fn try_detect_hardware(
        &self,
        data_dir: &Path,
    ) -> Option<crate::hwaccel::HardwareDetection> {
        let guard = self.detecting.try_lock().ok()?;
        Some(self.detect_hardware_locked(data_dir, guard).await)
    }

    async fn detect_hardware_locked(
        &self,
        data_dir: &Path,
        _guard: tokio::sync::MutexGuard<'_, ()>,
    ) -> crate::hwaccel::HardwareDetection {
        let work = data_dir.join("hwaccel-probe");
        let detection = crate::hwaccel::detect(&self.ffmpeg, &work).await;
        self.set_hardware_detection(data_dir, detection.clone());
        detection
    }

    /// Keeps `detection` and takes the encoding options it gives with `data_dir`'s saved ones.
    pub fn set_hardware_detection(
        &self,
        data_dir: &Path,
        detection: crate::hwaccel::HardwareDetection,
    ) {
        let settings = crate::hwaccel::effective_settings(
            data_dir,
            &self.ffmpeg.to_string_lossy(),
            &self.cache_root,
            Some(&detection),
        );
        *self.detection.write().unwrap_or_else(|e| e.into_inner()) = Some(detection);
        self.set_encoding_settings(settings);
    }

    /// Keeps the transcode reasons PlaybackInfo gave for `play_session_id`: the reasons of each
    /// media source that transcodes. Sources with no reason are left out.
    pub fn note_reasons(&self, play_session_id: &str, sources: Vec<(String, Vec<String>)>) {
        let sources: Vec<_> = sources.into_iter().filter(|(_, r)| !r.is_empty()).collect();
        if play_session_id.is_empty() || sources.is_empty() {
            return;
        }
        let mut all = self.reasons.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        all.retain(|_, p| now.duration_since(p.at) < REASONS_KEPT);
        if all.len() >= REASONS_LIMIT {
            all.clear();
        }
        all.insert(
            play_session_id.to_ascii_lowercase(),
            PlannedReasons { at: now, sources },
        );
    }

    /// What the transcode of `play_session_id` does: the running transcode's codecs, and the
    /// reasons PlaybackInfo gave for `media_source_id` (else for its first source). None when
    /// neither is known.
    pub fn transcoding_summary(
        &self,
        play_session_id: &str,
        media_source_id: Option<&str>,
    ) -> Option<TranscodingSummary> {
        if play_session_id.is_empty() {
            return None;
        }
        let reasons = {
            let all = self.reasons.lock().unwrap_or_else(|e| e.into_inner());
            all.get(&play_session_id.to_ascii_lowercase()).map(|p| {
                let wanted = media_source_id
                    .and_then(|id| p.sources.iter().find(|(s, _)| s.eq_ignore_ascii_case(id)));
                wanted
                    .or(p.sources.first())
                    .map(|(_, r)| r.clone())
                    .unwrap_or_default()
            })
        };
        let running = self.sessions.iter().find_map(|s| {
            s.play_session_id
                .eq_ignore_ascii_case(play_session_id)
                .then(|| s.profile.clone())
        });
        if reasons.is_none() && running.is_none() {
            return None;
        }
        let mut summary = TranscodingSummary {
            reasons: reasons.unwrap_or_default(),
            ..Default::default()
        };
        if let Some(p) = running {
            summary.video_codec = (!p.audio_only).then(|| p.video_codec.clone());
            summary.audio_codec = Some(p.audio_codec.clone());
            summary.is_video_direct = p.copy_video;
            summary.is_audio_direct = p.copy_audio;
            summary.audio_channels = p.audio_channels;
            summary.hardware_acceleration_type = match p.hw_accel {
                HwAccel::None => None,
                HwAccel::Vaapi => Some("vaapi"),
                HwAccel::Nvenc => Some("nvenc"),
                HwAccel::Qsv => Some("qsv"),
            };
        }
        Some(summary)
    }

    /// A manager that writes its transcodes under `root` instead of the data directory's
    /// `transcodes` (`TranscodingTempPath`). Only the session directories (named by playback id)
    /// in it are ever removed.
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.cache_root = root;
        self
    }

    /// The directory the transcodes are written under.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// The encoding options new transcodes take.
    pub fn encoding_settings(&self) -> EncodingSettings {
        self.settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the encoding options. The hardware failures noted before are forgotten, so the
    /// hardware is tried again, and the warnings may be logged again.
    pub fn set_encoding_settings(&self, settings: EncodingSettings) {
        encoding::set_current(&settings);
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = settings;
        *self.hardware_health() = HardwareHealth::default();
        self.warned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn hardware_health(&self) -> std::sync::MutexGuard<'_, HardwareHealth> {
        self.hardware.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Note how a hardware transcode of `input` went. A failure (whose software retry started)
    /// sends that file to software from then on; a device setup failure also counts towards
    /// turning the hardware off, which a hardware start resets.
    fn note_hardware_result(&self, input: &str, failure: Option<&str>) {
        let mut health = self.hardware_health();
        let Some(message) = failure else {
            health.device_failures = 0;
            return;
        };
        if health.failed_inputs.len() >= FAILED_INPUTS_LIMIT {
            health.failed_inputs.clear();
        }
        health.failed_inputs.insert(input.to_string());
        warn!("hardware transcoding of {input} failed, it is transcoded in software: {message}");
        if device_setup_failed(message) {
            health.device_failures += 1;
            if health.device_failures >= DEVICE_FAILURE_LIMIT && !health.off {
                health.off = true;
                warn!(
                    "the hardware device failed to open {DEVICE_FAILURE_LIMIT} times in a row: \
                     transcoding in software until the encoding options are saved again"
                );
            }
        }
    }

    /// Whether the progressive transcode `playback_id` decodes or encodes on a device.
    pub fn progressive_uses_hardware(&self, playback_id: Uuid) -> bool {
        self.sessions
            .get(&playback_id)
            .is_some_and(|s| uses_hardware(&s.profile))
    }

    /// Log `message` as a warning the first time `key` is seen.
    fn warn_once(&self, key: &str, message: &str) {
        if self
            .warned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string())
        {
            warn!("{message}");
        }
    }

    async fn capabilities(&self) -> &FfmpegCapabilities {
        self.capabilities
            .get_or_init(|| encoding::probe_capabilities(&self.ffmpeg))
            .await
    }

    /// `profile` with the server's encoding options. Hardware acceleration the ffmpeg build
    /// lacks (its hwaccel or its encoder) and a tonemap without zscale fall back to software,
    /// with a warning once. A file that failed on the device, and every file once the device
    /// itself keeps failing to open, runs in software.
    async fn prepare(&self, profile: &TranscodeProfile, input: &str) -> TranscodeProfile {
        let mut p = profile.clone();
        p.encoding = self.encoding_settings();
        let encodes_video = !p.audio_only && !p.copy_video;
        let accel = configured_accel(&p);
        if encodes_video && accel != HwAccel::None {
            {
                let health = self.hardware_health();
                if health.off || health.failed_inputs.contains(input) {
                    return software_profile(&p);
                }
            }
            let caps = self.capabilities().await;
            let hwaccel = match accel {
                HwAccel::Vaapi => "vaapi",
                HwAccel::Qsv => "qsv",
                HwAccel::Nvenc => "cuda",
                HwAccel::None => "",
            };
            if !caps.has_hwaccel(hwaccel) {
                self.warn_once(
                    hwaccel,
                    &format!("ffmpeg has no {hwaccel} hwaccel: transcoding in software"),
                );
                return software_profile(&p);
            }
            let encoder = encoding::encoder_family(&p.video_codec)
                .and_then(|family| encoding::hardware_encoder(accel, family));
            if p.encoding.enable_hardware_encoding
                && let Some(encoder) = encoder
                && !caps.has_encoder(encoder)
            {
                self.warn_once(
                    encoder,
                    &format!("ffmpeg has no {encoder} encoder: encoding in software"),
                );
                p.encoding.enable_hardware_encoding = false;
            }
        }
        if encodes_video && p.encoding.enable_tonemapping && p.source_hdr.is_some() {
            let caps = self.capabilities().await;
            if !caps.has_filter("zscale") || !caps.has_filter("tonemap") {
                self.warn_once(
                    "tonemap",
                    "ffmpeg has no zscale or tonemap filter: HDR video is not tonemapped",
                );
                p.encoding.enable_tonemapping = false;
            }
        }
        p
    }

    /// Spawn a background task that every minute stops idle transcode sessions and removes
    /// the cache directories of sessions that are no longer active.
    pub fn spawn_cleanup_task(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.tick().await; // skip the first immediate tick
            loop {
                interval.tick().await;
                manager
                    .cleanup_idle_sessions(Instant::now(), IDLE_LIMIT)
                    .await;
                if let Err(e) = manager.cleanup_cache().await {
                    warn!("transcode cache cleanup failed: {}", e);
                }
            }
        });
    }

    /// Start the HLS transcode of `input_path` for a request, or reuse the item's transcode of
    /// the same input file, the same play session (else device) and the same profile. Returns the
    /// playlist path and the playback id once ffmpeg has written the playlist. Fails when ffmpeg
    /// exits first, or when ffmpeg writes nothing for `playlist_wait` before the playlist (or the
    /// wait reaches `MAX_WAIT_FACTOR` times that); ffmpeg then keeps running.
    /// A play session stopped less than `stop_memory` ago fails at once with `Stopped`.
    ///
    /// A hardware transcode whose ffmpeg exits with an error is run again in software; when
    /// that one starts, the file is transcoded in software from then on (see `HardwareHealth`).
    /// A stop or the shutdown is never retried.
    pub async fn start_transcode(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<(PathBuf, Uuid), TranscodeError> {
        let profile = self.prepare(profile, input_path).await;
        let started = self
            .start_prepared(item_id, input_path, &profile, play_session_id, device_id)
            .await;
        if !uses_hardware(&profile) {
            return started;
        }
        match started {
            Err(TranscodeError::FfmpegFailed(message)) => {
                let software = software_profile(&profile);
                let retried = self
                    .start_prepared(item_id, input_path, &software, play_session_id, device_id)
                    .await;
                if retried.is_ok() {
                    self.note_hardware_result(input_path, Some(&message));
                }
                retried
            }
            Ok(started) => {
                self.note_hardware_result(input_path, None);
                Ok(started)
            }
            started => started,
        }
    }

    async fn start_prepared(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<(PathBuf, Uuid), TranscodeError> {
        let playback_id = {
            let _lock = self.start_lock.lock().await;
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(TranscodeError::ShuttingDown);
            }
            if !play_session_id.is_empty() && self.recently_stopped(play_session_id) {
                return Err(TranscodeError::Stopped);
            }
            match self.find_session(item_id, input_path, play_session_id, device_id) {
                Some(id) if !self.same_profile(id, profile) => {
                    // Other tracks, another start or other limits: the transcode is replaced
                    self.stop_session(id).await;
                    self.spawn_session(item_id, input_path, profile, play_session_id, device_id)
                        .await?
                }
                Some(id) if self.touch_live_session(id) => id,
                Some(id) => {
                    // ffmpeg exited without a playlist: start again
                    self.stop_session(id).await;
                    self.spawn_session(item_id, input_path, profile, play_session_id, device_id)
                        .await?
                }
                None => {
                    self.spawn_session(item_id, input_path, profile, play_session_id, device_id)
                        .await?
                }
            }
        };

        // Poll without the lock, so other requests can start their transcodes meanwhile. The
        // wait runs while ffmpeg makes progress, as Jellyfin waits while its transcode runs: each
        // time its output grows, the wait starts again. A slow transcode (a burn-in slower than
        // real time) still gets its first segment out; one that writes nothing for
        // `playlist_wait`, or none in `MAX_WAIT_FACTOR` times that, times out.
        let started = std::time::Instant::now();
        let mut last_progress = started;
        let mut written = 0;
        let mut next_check = started + PROGRESS_CHECK;
        loop {
            match self.poll_session(playback_id) {
                StartPoll::Ready(playlist) => return Ok((playlist, playback_id)),
                StartPoll::Exited(status) => {
                    return Err(self.ffmpeg_exited(playback_id, status).await);
                }
                StartPoll::Gone => return Err(TranscodeError::Stopped),
                StartPoll::Running => {}
            }
            let now = std::time::Instant::now();
            if now >= next_check {
                next_check = now + PROGRESS_CHECK;
                let size = self.output_bytes(playback_id).await;
                if size > written {
                    written = size;
                    last_progress = now;
                }
            }
            if now.duration_since(last_progress) >= self.playlist_wait
                || now.duration_since(started) >= self.playlist_wait * MAX_WAIT_FACTOR
            {
                return Err(TranscodeError::PlaylistTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The bytes ffmpeg has written so far in the session's directory; 0 when it is gone.
    async fn output_bytes(&self, playback_id: Uuid) -> u64 {
        let Some(dir) = self
            .sessions
            .get(&playback_id)
            .map(|session| session.output_dir.clone())
        else {
            return 0;
        };
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            return 0;
        };
        let mut total = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                total += meta.len();
            }
        }
        total
    }

    /// The item's session of `input_path` that a request with these names reuses.
    fn find_session(
        &self,
        item_id: Uuid,
        input_path: &str,
        play_session_id: &str,
        device_id: &str,
    ) -> Option<Uuid> {
        self.sessions
            .iter()
            .find(|e| {
                let session = e.value();
                !session.progressive
                    && session.item_id == item_id
                    && session.input_path == input_path
                    && same_key(session, play_session_id, device_id)
            })
            .map(|e| *e.key())
    }

    /// Whether the session `playback_id` runs the transcode of `profile`. A request of the same
    /// play session (else device) with other tracks, another start or other limits replaces the
    /// transcode: its client has moved on from the old one.
    fn same_profile(&self, playback_id: Uuid, profile: &TranscodeProfile) -> bool {
        self.sessions
            .get(&playback_id)
            .is_some_and(|session| session.profile == *profile)
    }

    /// Whether a session can still serve (its playlist exists or its ffmpeg runs). One whose
    /// playlist exists is touched; one still waiting for it is not, so a client that keeps
    /// retrying a hung ffmpeg doesn't keep it from the idle cleanup.
    fn touch_live_session(&self, playback_id: Uuid) -> bool {
        let Some(mut session) = self.sessions.get_mut(&playback_id) else {
            return false;
        };
        let running = session
            .child
            .as_mut()
            .is_some_and(|child| !matches!(child.try_wait(), Ok(Some(_))));
        let ready = session.output_dir.join("main.m3u8").exists();
        if ready {
            session.last_access = Instant::now();
        }
        running || ready
    }

    /// Look at a starting session without waiting.
    fn poll_session(&self, playback_id: Uuid) -> StartPoll {
        let Some(mut session) = self.sessions.get_mut(&playback_id) else {
            return StartPoll::Gone;
        };
        let playlist = session.output_dir.join("main.m3u8");
        if playlist.exists() {
            return StartPoll::Ready(playlist);
        }
        match session.child.as_mut().map(Child::try_wait) {
            Some(Ok(Some(status))) => StartPoll::Exited(status),
            _ => StartPoll::Running,
        }
    }

    /// Start ffmpeg for a new session in its own directory. Call with `start_lock` held.
    async fn spawn_session(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<Uuid, TranscodeError> {
        let playback_id = Uuid::new_v4();
        let output_dir = self.cache_root.join(playback_id.to_string());
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| TranscodeError::Io(format!("Cannot create output dir: {e}")))?;

        let playlist_path = output_dir.join("main.m3u8");
        // Use Jellyfin-compatible segment naming: segment_{id}.ts, or .mp4 for fragmented MP4
        let extension = if profile.fmp4 { "mp4" } else { "ts" };
        let segment_pattern = output_dir.join(format!("segment_%04d.{extension}"));

        let args = build_ffmpeg_args(
            input_path,
            playlist_path.to_str().unwrap(),
            segment_pattern.to_str().unwrap(),
            profile,
        );

        info!(
            item_id = %item_id,
            playback_id = %playback_id,
            "Starting HLS transcode: ffmpeg {}",
            args.join(" ")
        );

        let spawned = self.ffmpeg_command(&args, Stdio::null()).spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&output_dir).await;
                return Err(TranscodeError::FfmpegNotFound(e.to_string()));
            }
        };

        // Drain stderr from the start: a full pipe would block ffmpeg for good
        let stderr_tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(drain_stderr(stderr, playback_id, stderr_tail.clone())));

        self.sessions.insert(
            playback_id,
            TranscodeSession {
                item_id,
                playback_id,
                input_path: input_path.to_string(),
                play_session_id: play_session_id.to_string(),
                device_id: device_id.to_string(),
                output_dir,
                child: Some(child),
                stderr_task,
                stderr_tail,
                last_access: Instant::now(),
                profile: profile.clone(),
                progressive: false,
            },
        );
        Ok(playback_id)
    }

    /// The ffmpeg command for `args`: stderr piped, killed on drop, in its own process group,
    /// which a stop kills whole; and on Linux it dies with the server, even a killed one.
    fn ffmpeg_command(&self, args: &[String], stdout: Stdio) -> Command {
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(target_os = "linux")]
        // SAFETY: prctl is async-signal-safe and touches only the child's own state. The signal
        // follows the spawning thread, a tokio worker that lives as long as the runtime.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
    }

    /// Start a progressive transcode of `input_path`: ffmpeg writes `format` (an ffmpeg muxer)
    /// to its stdout, which the caller sends as the response. Jellyfin's progressive jobs work
    /// so (`TranscodingJobType.Progressive`). Each request gets its own ffmpeg; it is registered
    /// like an HLS session, so a stop by PlaySessionId or DeviceId and the shutdown kill it. The
    /// caller ends it with `end_progressive` when the response ends or the client goes away.
    /// A play session stopped less than `stop_memory` ago fails at once with `Stopped`.
    pub async fn start_progressive(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        format: &str,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<(Uuid, tokio::process::ChildStdout), TranscodeError> {
        let profile = &self.prepare(profile, input_path).await;
        let _lock = self.start_lock.lock().await;
        if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TranscodeError::ShuttingDown);
        }
        if !play_session_id.is_empty() && self.recently_stopped(play_session_id) {
            return Err(TranscodeError::Stopped);
        }
        // A client that asks again (a seek, a retry) for the same item in the same play session
        // (else from the same device) has left its older job: that one ends
        let replaced: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|e| {
                let s = e.value();
                s.progressive
                    && s.item_id == item_id
                    && (!play_session_id.is_empty() || !device_id.is_empty())
                    && same_key(s, play_session_id, device_id)
            })
            .map(|e| *e.key())
            .collect();
        for id in replaced {
            self.stop_session(id).await;
        }
        let running = self
            .sessions
            .iter()
            .filter(|e| e.value().progressive)
            .count();
        if running >= self.max_progressive {
            return Err(TranscodeError::TooMany(self.max_progressive));
        }
        let playback_id = Uuid::new_v4();
        let args = build_progressive_args(input_path, format, profile);
        info!(
            item_id = %item_id,
            playback_id = %playback_id,
            "Starting progressive transcode: ffmpeg {}",
            args.join(" ")
        );
        let mut child = self
            .ffmpeg_command(&args, Stdio::piped())
            .spawn()
            .map_err(|e| TranscodeError::FfmpegNotFound(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| TranscodeError::Io("ffmpeg has no stdout".into()))?;
        let stderr_tail = Arc::new(std::sync::Mutex::new(VecDeque::new()));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(drain_stderr(stderr, playback_id, stderr_tail.clone())));
        self.sessions.insert(
            playback_id,
            TranscodeSession {
                item_id,
                playback_id,
                input_path: input_path.to_string(),
                play_session_id: play_session_id.to_string(),
                device_id: device_id.to_string(),
                // Never created: the output is the pipe
                output_dir: self.cache_root.join(playback_id.to_string()),
                child: Some(child),
                stderr_task,
                stderr_tail,
                last_access: Instant::now(),
                profile: profile.clone(),
                progressive: true,
            },
        );
        Ok((playback_id, stdout))
    }

    /// `start_progressive`, then its first bytes, read within `first_wait`. A hardware transcode
    /// that exits without writing, or writes nothing in time, is ended and run again in software
    /// (the file stays in software from then on, logged once): at most two tries. Answers the
    /// playback id, the rest of the output and the first bytes. Writing nothing in time in
    /// software is `PlaylistTimeout`.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_progressive_first(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        format: &str,
        play_session_id: &str,
        device_id: &str,
        first_wait: Duration,
    ) -> Result<(Uuid, tokio::process::ChildStdout, Vec<u8>), TranscodeError> {
        use tokio::io::AsyncReadExt;
        let mut tries = 0;
        loop {
            tries += 1;
            let (playback_id, mut stdout) = self
                .start_progressive(
                    item_id,
                    input_path,
                    profile,
                    format,
                    play_session_id,
                    device_id,
                )
                .await?;
            let retry = tries < 2 && self.progressive_uses_hardware(playback_id);
            let mut first = vec![0; 64 * 1024];
            let failure = match tokio::time::timeout(first_wait, stdout.read(&mut first)).await {
                Ok(Ok(n)) if n > 0 => {
                    first.truncate(n);
                    return Ok((playback_id, stdout, first));
                }
                Ok(Ok(_)) => self.progressive_failure(playback_id).await,
                Ok(Err(e)) => {
                    self.end_progressive(playback_id).await;
                    return Err(TranscodeError::Io(e.to_string()));
                }
                Err(_) => {
                    self.end_progressive(playback_id).await;
                    if !retry {
                        return Err(TranscodeError::PlaylistTimeout);
                    }
                    TranscodeError::FfmpegFailed(format!(
                        "the hardware transcode wrote nothing in {} s",
                        first_wait.as_secs()
                    ))
                }
            };
            if !retry {
                return Err(failure);
            }
            self.note_hardware_result(input_path, Some(&failure.to_string()));
        }
    }

    /// End the progressive transcode `playback_id`: kill its ffmpeg if it still runs. False
    /// when it was already gone (stopped, or ended before).
    pub async fn end_progressive(&self, playback_id: Uuid) -> bool {
        self.stop_session(playback_id).await
    }

    /// Why the progressive transcode `playback_id` wrote nothing: ffmpeg's exit and its last
    /// stderr lines, once it has exited (waited for up to 2 s). The session is ended.
    pub async fn progressive_failure(&self, playback_id: Uuid) -> TranscodeError {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = match self.sessions.get_mut(&playback_id) {
                Some(mut session) => session
                    .child
                    .as_mut()
                    .and_then(|c| c.try_wait().ok().flatten()),
                None => return TranscodeError::Stopped,
            };
            if let Some(status) = status {
                return self.ffmpeg_exited(playback_id, status).await;
            }
            if Instant::now() >= deadline {
                self.stop_session(playback_id).await;
                return TranscodeError::FfmpegFailed("ffmpeg wrote nothing".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// ffmpeg exited before writing a playlist: drop the session and return its last stderr lines.
    async fn ffmpeg_exited(&self, playback_id: Uuid, status: ExitStatus) -> TranscodeError {
        let Some((stderr_task, tail)) = self
            .sessions
            .get_mut(&playback_id)
            .map(|mut s| (s.stderr_task.take(), s.stderr_tail.clone()))
        else {
            return TranscodeError::Stopped;
        };
        // The reader ends once ffmpeg has closed stderr; the timeout only bounds a stuck reader
        if let Some(task) = stderr_task {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
        let lines: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
        self.stop_session(playback_id).await;
        TranscodeError::FfmpegFailed(format!(
            "ffmpeg exited with {status}: {}",
            lines.join(" | ")
        ))
    }

    /// Stop the transcodes that a stop request names and delete their files. Names are the
    /// PlaySessionId, else the DeviceId (whatever the session's PlaySessionId), else, with no
    /// names, the unnamed sessions. `item_id` restricts the stop to one item; without an item
    /// and without names nothing stops. Returns how many sessions were stopped. A stop by
    /// PlaySessionId also refuses that play session's starts for `stop_memory`.
    pub async fn stop_play_session(
        &self,
        item_id: Option<Uuid>,
        play_session_id: &str,
        device_id: &str,
    ) -> usize {
        // Under the start lock, a start either sees the stop or has its new session collected
        let start_lock = match play_session_id {
            "" => None,
            id => {
                let guard = self.start_lock.lock().await;
                self.remember_stop(id);
                Some(guard)
            }
        };
        let matching: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|e| {
                let s = e.value();
                if item_id.is_some_and(|id| s.item_id != id) {
                    false
                } else if !play_session_id.is_empty() {
                    s.play_session_id.eq_ignore_ascii_case(play_session_id)
                } else if !device_id.is_empty() {
                    s.device_id.eq_ignore_ascii_case(device_id)
                } else {
                    item_id.is_some() && s.play_session_id.is_empty() && s.device_id.is_empty()
                }
            })
            .map(|e| *e.key())
            .collect();
        drop(start_lock);

        let mut stopped = 0;
        for id in matching {
            if self.stop_session(id).await {
                stopped += 1;
            }
        }
        stopped
    }

    /// Record a stop by PlaySessionId, and forget the stops older than `stop_memory`
    fn remember_stop(&self, play_session_id: &str) {
        let mut stopped = self.stopped.lock().unwrap();
        stopped.retain(|_, at| at.elapsed() < self.stop_memory);
        stopped.insert(play_session_id.to_ascii_lowercase(), Instant::now());
    }

    /// Whether a play session was stopped less than `stop_memory` ago
    fn recently_stopped(&self, play_session_id: &str) -> bool {
        self.stopped
            .lock()
            .unwrap()
            .get(&play_session_id.to_ascii_lowercase())
            .is_some_and(|at| at.elapsed() < self.stop_memory)
    }

    /// Stop all running transcodes and start no more: the server is shutting down. The flag is
    /// set under the start lock, so a start in progress either finishes before (and is stopped
    /// here) or sees it.
    pub async fn shutdown(&self) {
        {
            let _lock = self.start_lock.lock().await;
            self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.stop_all().await;
    }

    /// Stop all running transcodes.
    pub async fn stop_all(&self) {
        let ids: Vec<Uuid> = self.sessions.iter().map(|e| *e.key()).collect();
        for id in ids {
            self.stop_session(id).await;
        }
    }

    /// Remove a session, stop its ffmpeg and delete its files. False when it was already gone.
    async fn stop_session(&self, playback_id: Uuid) -> bool {
        match self.sessions.remove(&playback_id) {
            Some((_, session)) => {
                end_session(session).await;
                true
            }
            None => false,
        }
    }

    /// Get the path to a specific HLS segment file.
    ///
    /// Returns `None` unless `segment_name` is a bare file name whose canonical
    /// path is a regular file directly inside the item's canonical transcode
    /// directory. A symlink that points elsewhere fails that check.
    pub fn get_segment_path(&self, item_id: Uuid, segment_name: &str) -> Option<PathBuf> {
        if segment_name.is_empty()
            || segment_name == "."
            || segment_name == ".."
            || segment_name.contains(['/', '\\', '\0'])
        {
            return None;
        }
        let dir = self
            .cache_root
            .join(item_id.to_string())
            .canonicalize()
            .ok()?;
        let path = dir.join(segment_name).canonicalize().ok()?;
        (path.parent() == Some(dir.as_path()) && path.is_file()).then_some(path)
    }

    /// Get the path to a segment of one transcode session of `item_id`. Touches the session.
    pub fn segment_path(
        &self,
        item_id: Uuid,
        playback_id: Uuid,
        segment_name: &str,
    ) -> Option<PathBuf> {
        {
            let mut session = self.sessions.get_mut(&playback_id)?;
            if session.item_id != item_id {
                return None;
            }
            session.last_access = Instant::now();
        }
        self.get_segment_path(playback_id, segment_name)
    }

    /// Get the path to a segment for the legacy `/hls/{segment}` route: the item's most recently
    /// used session if it has the file (touched), else the item's old per-item directory.
    /// A session's playback id in place of the item id finds nothing: session directories are
    /// named by playback id too, and serving one here would bypass the item and its owner.
    pub fn legacy_segment_path(&self, item_id: Uuid, segment_name: &str) -> Option<PathBuf> {
        if self.sessions.contains_key(&item_id) {
            return None;
        }
        let latest = self
            .sessions
            .iter()
            .filter(|e| !e.value().progressive && e.value().item_id == item_id)
            .max_by_key(|e| e.value().last_access)
            .map(|e| *e.key());
        if let Some(playback_id) = latest
            && let Some(path) = self.get_segment_path(playback_id, segment_name)
        {
            if let Some(mut session) = self.sessions.get_mut(&playback_id) {
                session.last_access = Instant::now();
            }
            return Some(path);
        }
        self.get_segment_path(item_id, segment_name)
    }

    /// Check if a transcode session is active for the given item.
    pub async fn is_active(&self, item_id: Uuid) -> bool {
        self.sessions.iter().any(|e| e.value().item_id == item_id)
    }

    /// Clean up transcode cache: remove directories for sessions that are no longer active.
    pub async fn cleanup_cache(&self) -> Result<(), TranscodeError> {
        // Under the start lock, so a session that is starting keeps its directory
        let _lock = self.start_lock.lock().await;
        let active_ids: Vec<Uuid> = self.sessions.iter().map(|e| *e.key()).collect();

        if !self.cache_root.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.cache_root)
            .await
            .map_err(|e| TranscodeError::Io(format!("Cannot read cache dir: {e}")))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| TranscodeError::Io(format!("Cannot read entry: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Ok(id) = Uuid::parse_str(&name)
                && !active_ids.contains(&id)
            {
                info!(playback_id = %id, "Cleaning up transcode cache");
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }

        Ok(())
    }

    /// Stop the sessions whose last playlist or segment request is older than `max_idle`.
    /// Returns how many were stopped.
    pub async fn cleanup_idle_sessions(&self, now: Instant, max_idle: Duration) -> usize {
        let idle: Vec<Uuid> = self
            .sessions
            .iter()
            .filter(|e| {
                !e.value().progressive
                    && now.saturating_duration_since(e.value().last_access) > max_idle
            })
            .map(|e| *e.key())
            .collect();

        let mut stopped = 0;
        for id in idle {
            // Checked again under the map lock: a request may have touched the session since
            let removed = self.sessions.remove_if(&id, |_, s| {
                now.saturating_duration_since(s.last_access) > max_idle
            });
            if let Some((_, session)) = removed {
                warn!(playback_id = %id, "Stopping idle transcode session");
                end_session(session).await;
                stopped += 1;
            }
        }
        stopped
    }
}

/// Whether a session answers to a request's names: the same PlaySessionId, else the same
/// DeviceId among sessions without one, else a session with neither. Case-insensitive.
fn same_key(session: &TranscodeSession, play_session_id: &str, device_id: &str) -> bool {
    if !play_session_id.is_empty() {
        session
            .play_session_id
            .eq_ignore_ascii_case(play_session_id)
    } else if !device_id.is_empty() {
        session.play_session_id.is_empty() && session.device_id.eq_ignore_ascii_case(device_id)
    } else {
        session.play_session_id.is_empty() && session.device_id.is_empty()
    }
}

/// Kill a removed session's ffmpeg and delete its files.
async fn end_session(mut session: TranscodeSession) {
    if let Some(mut child) = session.child.take() {
        // ffmpeg leads its own process group: kill the group, then reap ffmpeg
        #[cfg(unix)]
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: kill only sends a signal; the group is the one ffmpeg was started in
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill().await;
    }
    let _ = tokio::fs::remove_dir_all(&session.output_dir).await;
    debug!(playback_id = %session.playback_id, "Stopped transcode session");
}

/// Read ffmpeg's stderr until it closes. Each line goes to the debug log, and the last
/// `STDERR_TAIL_LINES` lines (cut to 300 chars) are kept for error reports.
async fn drain_stderr<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    playback_id: Uuid,
    tail: Arc<std::sync::Mutex<VecDeque<String>>>,
) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = match reader.read_until(b'\n', &mut buf).await {
            Ok(read) => read,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim_end();
        if line.is_empty() {
            continue;
        }
        debug!(playback_id = %playback_id, "ffmpeg: {line}");
        let kept: String = line.chars().take(300).collect();
        let mut tail = tail.lock().unwrap();
        tail.push_back(kept);
        while tail.len() > STDERR_TAIL_LINES {
            tail.pop_front();
        }
    }
}

// ---------------------------------------------------------------------------
// FFmpeg argument builder
// ---------------------------------------------------------------------------

/// Whether an ffmpeg audio encoder is lossless, so takes no bitrate.
fn is_lossless_encoder(encoder: &str) -> bool {
    matches!(encoder, "flac" | "alac") || encoder.starts_with("pcm_")
}

/// The ffmpeg encoder for an audio codec a client names (`mp3`, `aac`, `opus`, ...), as
/// Jellyfin's `EncodingHelper.GetAudioEncoder` picks it; None for a codec Rustyfin doesn't
/// encode.
pub fn audio_encoder(codec: &str) -> Option<&'static str> {
    Some(match codec.to_ascii_lowercase().as_str() {
        "mp3" => "libmp3lame",
        "aac" | "mp4a" => "aac",
        "opus" => "libopus",
        "vorbis" => "libvorbis",
        "flac" => "flac",
        "alac" => "alac",
        "ac3" => "ac3",
        "eac3" => "eac3",
        "mp2" => "mp2",
        "wav" | "pcm" | "pcm_s16le" => "pcm_s16le",
        _ => return None,
    })
}

/// The ffmpeg encoder for a video codec a client names; None for a codec Rustyfin doesn't
/// encode.
pub fn video_encoder(codec: &str) -> Option<&'static str> {
    Some(match codec.to_ascii_lowercase().as_str() {
        "h264" | "avc" | "avc1" => "libx264",
        "hevc" | "h265" | "hvc1" | "hev1" => "libx265",
        "vp8" => "libvpx",
        "vp9" => "libvpx-vp9",
        "mpeg4" => "mpeg4",
        "mpeg2video" => "mpeg2video",
        "av1" => "libsvtav1",
        _ => return None,
    })
}

/// The ffmpeg muxer and the content type of a progressive transcode into `container`, as
/// Jellyfin names them; None for a container it can't stream.
pub fn progressive_format(
    container: &str,
    audio_only: bool,
) -> Option<(&'static str, &'static str)> {
    Some(
        match (container.to_ascii_lowercase().as_str(), audio_only) {
            ("mp3", _) => ("mp3", "audio/mpeg"),
            ("aac" | "adts", _) => ("adts", "audio/aac"),
            ("m4a" | "m4b", _) | ("mp4", true) => ("mp4", "audio/mp4"),
            ("ogg" | "oga" | "opus", _) => ("ogg", "audio/ogg"),
            ("webma", _) | ("webm", true) => ("webm", "audio/webm"),
            ("flac", _) => ("flac", "audio/flac"),
            ("wav", _) => ("wav", "audio/wav"),
            ("mka", _) | ("mkv" | "matroska", true) => ("matroska", "audio/x-matroska"),
            ("ts" | "mpegts" | "m2ts", _) => ("mpegts", "video/mp2t"),
            ("mkv" | "matroska", false) => ("matroska", "video/x-matroska"),
            ("mp4" | "m4v", false) => ("mp4", "video/mp4"),
            ("webm", false) => ("webm", "video/webm"),
            ("avi", _) => ("avi", "video/x-msvideo"),
            _ => return None,
        },
    )
}

/// Ticks as seconds with milliseconds truncated, in integer math (`600.000`)
fn seconds_text(ticks: i64) -> String {
    format!(
        "{}.{:03}",
        ticks / 10_000_000,
        (ticks % 10_000_000) / 10_000
    )
}

/// Escapes a path for the single quotes of a `subtitles` option, as Jellyfin's
/// `EscapeSubtitleFilterPath` does. Jellyfin turns `\` into `/` (Windows separators); here `\` is
/// a file name character, so it is escaped like the others. The path sits in single quotes in a
/// filter option, and ffmpeg unquotes it twice: for the graph (`'...'` is literal, `\x` is `x`,
/// and an unquoted `[],;` ends the filter) and for the options (`\x` is `x`, and an unquoted `:`
/// ends the value). Inside the quotes `[],;=` are plain. `:`, `\` and `"` get a backslash for the
/// option level. `'` closes the quote, adds `\\\'` (the graph level leaves `\'`, the option level
/// `'`) and opens the quote again.
fn escape_filter_path(path: &str) -> String {
    path.replace('\\', r"\\")
        .replace(':', r"\:")
        .replace('\'', r"'\\\''")
        .replace('"', r#"\""#)
}

/// The text subtitle filter. After an input seek the frames start at 0, but the filter reads
/// the file from its start, so the frames move to the file's time for it and back after.
fn subtitles_filter(input: &str, sub_idx: u32, start_time_ticks: i64) -> String {
    // The f= key keeps a path with `=` from being read as an option name
    let escaped = escape_filter_path(input);
    let subtitles = format!("subtitles=f='{escaped}':si={sub_idx}");
    if start_time_ticks > 0 {
        let secs = seconds_text(start_time_ticks);
        format!("setpts=PTS+{secs}/TB,{subtitles},setpts=PTS-{secs}/TB")
    } else {
        subtitles
    }
}

/// The size the scale step gives a `width`×`height` video within `max_width`×`max_height`: at
/// most its own size, its aspect kept, in even numbers as 4:2:0 frames need.
fn output_size((width, height): (u32, u32), max_width: u32, max_height: u32) -> (u32, u32) {
    let (w, h) = (f64::from(width.max(2)), f64::from(height.max(2)));
    let scale = (f64::from(max_width) / w)
        .min(f64::from(max_height) / h)
        .min(1.0);
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
    (even(w * scale), even(h * scale))
}

/// The filter graph that burns picture subtitle `sub` (input 1) into the video (input 0) and
/// names the result `[v]`.
///
/// With the video's size known, the video is scaled to its output size and made 8-bit 4:2:0
/// first (on the device with `hw_accel`, then downloaded), the subtitle is scaled to that same
/// size, and the overlay runs once per frame at the output size. For a hardware encoder the
/// result is NV12, uploaded again for VAAPI. Without the size, the subtitle is laid over the
/// video as it is and the result scaled like any video (`scale_filter`, in software).
fn image_overlay_graph(
    sub: ImageSubtitle,
    plan: HwPlan,
    profile: &TranscodeProfile,
    tonemap: bool,
    scale_filter: &str,
) -> String {
    let overlay = "overlay=eof_action=pass:repeatlast=0";
    let index = sub.stream_index;
    // Deinterlacing, square pixels and the tonemap run in `scale_filter`, after the overlay: the
    // subtitle is laid over the frame as it is and the whole frame is made progressive, square
    // and SDR
    let Some(size) = sub
        .video_size
        .filter(|_| !profile.deinterlace && !profile.square_pixels && !tonemap)
    else {
        return format!("[0:V:0][1:{index}]{overlay},{scale_filter}[v]");
    };
    let (w, h) = output_size(size, profile.max_width, profile.max_height);
    let main = if plan.decode {
        format!(
            "{},hwdownload,format=nv12",
            hw_scale(plan.accel, &w.to_string(), &h.to_string())
        )
    } else {
        format!("scale={w}:{h},format=yuv420p")
    };
    let after = to_hw_encoder(plan);
    format!("[0:V:0]{main}[main];[1:{index}]scale={w}:{h}[sub];[main][sub]{overlay}{after}[v]")
}

/// The ffmpeg arguments of an HLS transcode of `input` with `profile`.
pub fn build_ffmpeg_args(
    input: &str,
    playlist_output: &str,
    segment_pattern: &str,
    profile: &TranscodeProfile,
) -> Vec<String> {
    ffmpeg_args(
        input,
        Output::Hls {
            playlist: playlist_output,
            segments: segment_pattern,
        },
        profile,
    )
}

/// The ffmpeg arguments of a progressive transcode: the encode of `build_ffmpeg_args`, written
/// to stdout in the muxer `format` (`mp3`, `adts`, `mp4`, `mpegts`, ...). MP4 is fragmented, as
/// a pipe can't be seeked back to write the index. Metadata and chapters are dropped, as
/// Jellyfin's progressive audio does (`-map_metadata -1 -map_chapters -1`).
pub fn build_progressive_args(
    input: &str,
    format: &str,
    profile: &TranscodeProfile,
) -> Vec<String> {
    ffmpeg_args(input, Output::Pipe { format }, profile)
}

/// Where ffmpeg writes.
#[derive(Clone, Copy)]
enum Output<'a> {
    /// An HLS playlist and its segments.
    Hls {
        playlist: &'a str,
        segments: &'a str,
    },
    /// Stdout, in this muxer.
    Pipe { format: &'a str },
}

/// The acceleration a profile asks for: its own (a request's `HwAccel`), else the server's
/// `HardwareAccelerationType` when the transcoder implements it.
fn configured_accel(profile: &TranscodeProfile) -> HwAccel {
    match profile.hw_accel {
        HwAccel::None => profile.encoding.hardware_acceleration.transcoder_accel(),
        accel => accel,
    }
}

/// Whether `profile` decodes or encodes video on a device.
fn uses_hardware(profile: &TranscodeProfile) -> bool {
    !profile.audio_only && !profile.copy_video && configured_accel(profile) != HwAccel::None
}

/// `profile` without hardware acceleration.
fn software_profile(profile: &TranscodeProfile) -> TranscodeProfile {
    let mut software = profile.clone();
    software.hw_accel = HwAccel::None;
    software.encoding.hardware_acceleration = encoding::HardwareAccelerationType::None;
    software
}

/// Whether the device decodes the source: its codec is in `HardwareDecodingCodecs`, and a 10-bit
/// HEVC or VP9 only with `EnableDecodingColorDepth10Hevc`/`Vp9`. An unknown codec is decoded on
/// the device.
fn hardware_decodes(profile: &TranscodeProfile) -> bool {
    let settings = &profile.encoding;
    let Some(codec) = profile.source_video_codec.as_deref() else {
        return true;
    };
    let codec = match codec {
        "h265" => "hevc",
        "avc" => "h264",
        codec => codec,
    };
    let deep = profile.source_bit_depth.is_some_and(|depth| depth > 8);
    // No VAAPI, QSV or NVDEC decoder takes 10-bit H.264 (Hi10P): Jellyfin decodes it in software
    settings.hardware_decoding_codecs.iter().any(|c| c == codec)
        && !(deep && codec == "h264")
        && !(deep && codec == "hevc" && !settings.enable_decoding_color_depth_10_hevc)
        && !(deep && codec == "vp9" && !settings.enable_decoding_color_depth_10_vp9)
}

/// Where the video runs on a device: it is decoded there, encoded there, or both. `accel` is
/// None when neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HwPlan {
    accel: HwAccel,
    decode: bool,
    encode: bool,
}

/// The arguments that open the device of `accel` (and decode on it with `decode`), as a
/// transcode writes them; the hardware detection probes with these.
pub(crate) fn hw_device_args(
    accel: HwAccel,
    decode: bool,
    settings: &EncodingSettings,
) -> Vec<String> {
    hw_input_args(
        HwPlan {
            accel,
            decode,
            encode: false,
        },
        settings,
    )
}

/// The filter that takes software frames to `accel`'s encoder, as a transcode writes it; the
/// hardware detection's encode probe uses it.
pub(crate) fn hw_encoder_upload(accel: HwAccel) -> &'static str {
    to_hw_encoder(HwPlan {
        accel,
        decode: false,
        encode: true,
    })
    .trim_start_matches(',')
}

/// The input arguments that open the device, and decode on it with `plan.decode`, as Jellyfin
/// writes them on Linux.
fn hw_input_args(plan: HwPlan, settings: &EncodingSettings) -> Vec<String> {
    let (devices, hwaccel, name) = match plan.accel {
        HwAccel::Vaapi => (
            vec![
                "-init_hw_device".to_string(),
                format!("vaapi=va:{}", settings.vaapi_device),
            ],
            "vaapi",
            "va",
        ),
        HwAccel::Qsv => {
            let device = if settings.qsv_device.is_empty() {
                "/dev/dri/renderD128"
            } else {
                settings.qsv_device.as_str()
            };
            (
                vec![
                    "-init_hw_device".to_string(),
                    format!("vaapi=va:{device}"),
                    "-init_hw_device".to_string(),
                    "qsv=qs@va".to_string(),
                ],
                "qsv",
                "qs",
            )
        }
        HwAccel::Nvenc => (
            vec!["-init_hw_device".to_string(), "cuda=cu:0".to_string()],
            "cuda",
            "cu",
        ),
        HwAccel::None => return Vec::new(),
    };
    let mut args = devices;
    args.extend(["-filter_hw_device".to_string(), name.to_string()]);
    if plan.decode {
        args.extend(
            [
                "-hwaccel",
                hwaccel,
                "-hwaccel_output_format",
                hwaccel,
                "-hwaccel_device",
                name,
                "-noautorotate",
            ]
            .map(str::to_string),
        );
    }
    args
}

/// The step that takes software frames to the device's encoder: VAAPI's encoder takes device
/// frames only, NVENC's and QSV's take NV12 from memory.
fn to_hw_encoder(plan: HwPlan) -> &'static str {
    match plan.accel {
        _ if !plan.encode => "",
        HwAccel::Vaapi => ",format=nv12,hwupload",
        HwAccel::Nvenc | HwAccel::Qsv => ",format=nv12",
        HwAccel::None => "",
    }
}

/// The scale's width and height expressions for a video of unknown size within
/// `max_width`×`max_height`: at most its own size, its aspect kept, each truncated to an even
/// number as 4:2:0 frames and the encoders need, as Jellyfin's `EncodingHelper` writes them
/// (`trunc(…/2)*2`). Only `iw` and `ih` are used, which every scale filter (software, VAAPI,
/// CUDA, QSV) evaluates.
fn even_fit(max_width: u32, max_height: u32) -> (String, String) {
    (
        format!("'trunc(min(min(iw,{max_width}),{max_height}*iw/ih)/2)*2'"),
        format!("'trunc(min(min(ih,{max_height}),{max_width}*ih/iw)/2)*2'"),
    )
}

/// The device's scale to `w`×`h` (numbers or `even_fit` expressions), to 8-bit NV12.
fn hw_scale(accel: HwAccel, w: &str, h: &str) -> String {
    match accel {
        HwAccel::Vaapi => format!("scale_vaapi=w={w}:h={h}:format=nv12"),
        HwAccel::Nvenc => format!("scale_cuda=w={w}:h={h}:format=nv12"),
        HwAccel::Qsv => format!("scale_qsv=w={w}:h={h}:format=nv12"),
        HwAccel::None => format!("scale={w}:{h}"),
    }
}

fn ffmpeg_args(input: &str, output: Output<'_>, profile: &TranscodeProfile) -> Vec<String> {
    let settings = &profile.encoding;
    let mut args: Vec<String> = Vec::new();

    // Overwrite output without asking; keep the banner and progress lines out of stderr
    args.push("-y".to_string());
    args.extend(["-hide_banner".to_string(), "-nostats".to_string()]);

    // An image subtitle is overlaid in software. With hardware acceleration the device decodes
    // and scales to the output size first; that needs the video's size, so a subtitle without
    // one turns hardware acceleration off. Deinterlacing, square pixels and the tonemap are
    // software filters, so they turn it off too.
    let image_subtitle = profile.image_subtitle.filter(|_| !profile.audio_only);

    // Burn-in, the overlay, deinterlacing and square pixels need a re-encode, so they turn the
    // copy off. The tonemap doesn't: an HDR video a client takes as it is stays copied.
    let copy_video = profile.copy_video
        && !profile.audio_only
        && !profile.burn_subtitles
        && !profile.deinterlace
        && !profile.square_pixels
        && image_subtitle.is_none();
    let tonemap = profile
        .source_hdr
        .filter(|_| settings.enable_tonemapping && !copy_video && !profile.audio_only);
    let software_filters = profile.deinterlace || profile.square_pixels || tonemap.is_some();
    let hw_accel = match image_subtitle {
        _ if software_filters => HwAccel::None,
        Some(sub) if sub.video_size.is_none() => HwAccel::None,
        _ => configured_accel(profile),
    };

    // The encoder: the device's for the profile's codec with `EnableHardwareEncoding`, else the
    // profile's own. The device decodes the codecs `HardwareDecodingCodecs` lists.
    let hw_encoder = (hw_accel != HwAccel::None && settings.enable_hardware_encoding)
        .then(|| {
            encoding::encoder_family(&profile.video_codec)
                .and_then(|family| encoding::hardware_encoder(hw_accel, family))
        })
        .flatten();
    let hw_decode = hw_accel != HwAccel::None && hardware_decodes(profile);
    let plan = HwPlan {
        accel: if hw_decode || hw_encoder.is_some() {
            hw_accel
        } else {
            HwAccel::None
        },
        decode: hw_decode,
        encode: hw_encoder.is_some(),
    };
    let video_encoder = hw_encoder.unwrap_or(profile.video_codec.as_str());

    // Hardware acceleration input flags (skip for audio-only and copied video)
    if !profile.audio_only && !copy_video {
        args.extend(hw_input_args(plan, settings));
    }

    // Resume or seek point: an input seek, so output time 0 is the start point
    let seek: Vec<String> = if profile.start_time_ticks > 0 {
        vec!["-ss".to_string(), seconds_text(profile.start_time_ticks)]
    } else {
        Vec::new()
    };
    args.extend(seek.iter().cloned());

    // Input
    args.extend(["-i".to_string(), input.to_string()]);

    // An image subtitle is read from a second input of the same file. From the first, its sparse
    // packets stall the overlay: ffmpeg then decodes and filters at about half speed (1080p HEVC
    // burn-in ran slower than real time). The second input seeks to the same point.
    if image_subtitle.is_some() {
        args.extend(seek);
        args.extend(["-i".to_string(), input.to_string()]);
    }

    if profile.audio_only {
        // Audio-only transcoding: drop the video only. -an would drop the audio too.
        args.push("-vn".to_string());
    } else if copy_video {
        args.extend(["-c:v".to_string(), "copy".to_string()]);
    } else {
        args.extend(["-c:v".to_string(), video_encoder.to_string()]);

        // Video bitrate: x264 and x265 encode at the CRF under the limit, as Jellyfin's do; the
        // other encoders keep to the bitrate
        let kbps = profile.max_video_bitrate_kbps;
        match encoding::crf(video_encoder, settings) {
            Some(crf) => args.extend(["-crf".to_string(), crf.to_string()]),
            None => args.extend(["-b:v".to_string(), format!("{kbps}k")]),
        }
        // SVT-AV1 takes no -maxrate: equal to -b:v ffmpeg asks it for CBR, which it refuses in
        // its random-access mode
        if video_encoder != "libsvtav1" {
            args.extend(["-maxrate".to_string(), format!("{kbps}k")]);
        }
        args.extend(["-bufsize".to_string(), format!("{}k", kbps * 2)]);
        if settings.thread_count > 0 {
            args.extend(["-threads".to_string(), settings.thread_count.to_string()]);
        }

        // Scale filter. Its last step makes the frames 8-bit 4:2:0 (NV12 on the hardware
        // encoders): libx264 would keep a 10-bit or 4:2:2 source's format, which browsers
        // can't decode. HDR video is tonemapped to SDR after the scale, in software.
        // Even dimensions, the aspect kept: an odd width (1280×720 at 480 lines is 853.3
        // wide) makes libx264 and the hardware encoders refuse to start
        let (w, h) = even_fit(profile.max_width, profile.max_height);
        // The device scales to the exact output size when the source's is known; else by
        // expression
        let device_scale = match profile.source_size {
            Some(size) => {
                let (w, h) = output_size(size, profile.max_width, profile.max_height);
                hw_scale(plan.accel, &w.to_string(), &h.to_string())
            }
            None => hw_scale(plan.accel, &w, &h),
        };
        let software_scale = format!(
            "{}{}scale={w}:{h},{}",
            if profile.deinterlace { "bwdif=0," } else { "" },
            if profile.square_pixels {
                "scale=trunc(iw*sar/2)*2:ih,setsar=1,"
            } else {
                ""
            },
            if let Some(transfer) = tonemap {
                encoding::software_tonemap_filter(settings, transfer)
            } else {
                "format=yuv420p".to_string()
            },
        );
        // The frames at the output size, in memory (`decoded`) for a software filter
        let decoded = if plan.decode {
            format!("{device_scale},hwdownload,format=nv12")
        } else {
            software_scale.clone()
        };

        // Subtitle burn-in: an image subtitle is overlaid, and it wins over a text burn-in
        if let Some(sub) = image_subtitle {
            let graph = image_overlay_graph(sub, plan, profile, tonemap.is_some(), &software_scale);
            args.extend(["-filter_complex".to_string(), graph]);
        } else if profile.burn_subtitles {
            let sub_idx = profile.subtitle_stream_index.unwrap_or(0);
            let burn = subtitles_filter(input, sub_idx, profile.start_time_ticks);
            args.extend([
                "-vf".to_string(),
                format!("{decoded},{burn}{}", to_hw_encoder(plan)),
            ]);
        } else if plan.decode && plan.encode {
            args.extend(["-vf".to_string(), device_scale]);
        } else if plan.decode {
            args.extend(["-vf".to_string(), decoded]);
        } else {
            args.extend([
                "-vf".to_string(),
                format!("{software_scale}{}", to_hw_encoder(plan)),
            ]);
        }
    }

    // Maps: the video first, then the audio. The overlay's output is the video; audio-only has none.
    if image_subtitle.is_some() {
        args.extend(["-map".to_string(), "[v]".to_string()]);
    } else if !profile.audio_only {
        args.extend(["-map".to_string(), "0:V:0?".to_string()]);
    }
    let audio_map = match profile.audio_stream_index {
        Some(idx) => format!("0:{idx}"),
        None => "0:a:0?".to_string(),
    };
    args.extend(["-map".to_string(), audio_map]);

    // Audio encoding

    if profile.copy_audio {
        args.extend(["-c:a".to_string(), "copy".to_string()]);
    } else {
        args.extend(["-c:a".to_string(), profile.audio_codec.clone()]);
        // A lossless encoder has no bitrate to set
        if !is_lossless_encoder(&profile.audio_codec) {
            args.extend([
                "-b:a".to_string(),
                format!("{}k", profile.max_audio_bitrate_kbps),
            ]);
        }
        let channels = profile.audio_channels.unwrap_or(2);
        args.extend(["-ac".to_string(), channels.to_string()]);
        if let Some(rate) = profile.audio_sample_rate {
            args.extend(["-ar".to_string(), rate.to_string()]);
        }
        // A downmix to stereo takes the stereo algorithm's matrix and the volume boost
        let downmix = encoding::downmix_filters(settings, profile.source_audio_channels, channels);
        if !downmix.is_empty() {
            args.extend(["-af".to_string(), downmix.join(",")]);
        }
    }

    // ffmpeg's buffer for packets waiting to be muxed (`MaxMuxingQueueSize`)
    if !profile.audio_only {
        args.extend([
            "-max_muxing_queue_size".to_string(),
            settings.max_muxing_queue_size.to_string(),
        ]);
    }

    // The encoder's preset (`EncoderPreset`); none for audio-only, copied video and VAAPI
    let preset = if !profile.audio_only && !copy_video {
        encoding::preset_args(video_encoder, &settings.encoder_preset)
    } else {
        Vec::new()
    };
    // An encoded HEVC in MP4 segments gets the hvc1 tag Apple players need, as a copied one does
    let encoded_hevc_tag = !profile.audio_only
        && !copy_video
        && profile.fmp4
        && encoding::encoder_family(video_encoder) == Some("hevc");

    let (playlist_output, segment_pattern) = match output {
        Output::Hls { playlist, segments } => (playlist, segments),
        Output::Pipe { format } => {
            args.extend(preset);
            if profile.start_time_ticks > 0 && (copy_video || profile.copy_audio) {
                args.extend(["-copypriorss".to_string(), "0".to_string()]);
            }
            args.extend(
                ["-map_metadata", "-1", "-map_chapters", "-1", "-f", format].map(str::to_string),
            );
            if format == "mp4" {
                args.extend([
                    "-movflags".to_string(),
                    "frag_keyframe+empty_moov+default_base_moof".to_string(),
                ]);
            }
            args.push("pipe:1".to_string());
            return args;
        }
    };

    // An encoded video gets a key frame at each segment start, as Jellyfin forces them. The
    // muxer cuts a segment only at a key frame, so without them the first segment (which the
    // playlist waits for) runs to the encoder's next one: about 10 s of video, not 6.
    if !profile.audio_only && !copy_video {
        args.extend([
            "-force_key_frames:v".to_string(),
            format!("expr:gte(t,n_forced*{})", profile.segment_duration.max(1)),
        ]);
    }
    // A copied video keeps the tag the client needs (hvc1 for HEVC on Apple players)
    if copy_video && let Some(tag) = &profile.video_tag {
        args.extend(["-tag:v".to_string(), tag.clone()]);
    } else if encoded_hevc_tag {
        args.extend(["-tag:v".to_string(), "hvc1".to_string()]);
    }

    // HLS output settings
    args.extend([
        "-f".to_string(),
        "hls".to_string(),
        "-hls_time".to_string(),
        profile.segment_duration.to_string(),
        "-hls_list_size".to_string(),
        "0".to_string(),
        "-hls_flags".to_string(),
        "independent_segments".to_string(),
    ]);
    if profile.fmp4 {
        args.extend([
            "-hls_segment_type".to_string(),
            "fmp4".to_string(),
            "-hls_fmp4_init_filename".to_string(),
            "init.mp4".to_string(),
        ]);
    } else {
        args.extend(["-hls_segment_type".to_string(), "mpegts".to_string()]);
    }
    args.extend([
        "-hls_segment_filename".to_string(),
        segment_pattern.to_string(),
        "-hls_playlist_type".to_string(),
        "event".to_string(),
        "-start_number".to_string(),
        "0".to_string(),
    ]);

    args.extend(preset);

    // A copied stream would start with packets from before the start: drop those
    if profile.start_time_ticks > 0 && (copy_video || profile.copy_audio) {
        args.extend(["-copypriorss".to_string(), "0".to_string()]);
    }

    // Output playlist
    args.push(playlist_output.to_string());

    args
}

// ---------------------------------------------------------------------------
// HLS Playlist generation with #EXT-X-MEDIA tags
// ---------------------------------------------------------------------------

/// Generate a proper HLS master playlist with `#EXT-X-MEDIA` tags for multi-audio/subtitle groups.
/// This wraps the FFmpeg-generated playlist with proper HLS metadata.
pub fn generate_hls_master_playlist(
    playback_id: &Uuid,
    item_id: &Uuid,
    profile: &TranscodeProfile,
) -> String {
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:3\n");
    playlist.push_str("#EXT-X-INDEPENDENT-SEGMENTS\n");

    if profile.audio_only {
        // Audio-only playlist
        playlist.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"default\",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE=\"und\",URI=\"/Videos/{}/hls/{}/audio/segment_{{}}.ts\"\n",
            item_id, playback_id
        ));
        playlist.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},AUDIO=\"audio\"\n",
            profile.max_audio_bitrate_kbps * 1000
        ));
        playlist.push_str(&format!(
            "/Videos/{}/hls/{}/audio/segment_{{}}.m3u8\n",
            item_id, playback_id
        ));
    } else {
        // Video + Audio playlist
        let bandwidth = (profile.max_video_bitrate_kbps + profile.max_audio_bitrate_kbps) * 1000;
        playlist.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"default\",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE=\"und\",URI=\"/Videos/{}/hls/{}/audio/segment_{{}}.ts\"\n",
            item_id, playback_id
        ));

        if let Some(sub_idx) = profile.subtitle_stream_index {
            playlist.push_str(&format!(
                "#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"subtitles\",DEFAULT=YES,AUTOSELECT=YES,FORCED=NO,URI=\"/Videos/{}/hls/{}/sub/segment_{}.ts\"\n",
                item_id, playback_id, sub_idx
            ));
            playlist.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},AUDIO=\"audio\",SUBTITLES=\"subs\"\n",
                bandwidth
            ));
            playlist.push_str(&format!(
                "/Videos/{}/hls/{}/video/segment_{{}}.m3u8\n",
                item_id, playback_id
            ));
        } else {
            playlist.push_str(&format!(
                "#EXT-X-STREAM-INF:BANDWIDTH={},AUDIO=\"audio\"\n",
                bandwidth
            ));
            playlist.push_str(&format!(
                "/Videos/{}/hls/{}/video/segment_{{}}.m3u8\n",
                item_id, playback_id
            ));
        }
    }

    playlist
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    #[error("FFmpeg not found or failed to start: {0}")]
    FfmpegNotFound(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Transcode session not found for item {0}")]
    SessionNotFound(Uuid),

    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),

    #[error("ffmpeg has not written a playlist yet")]
    PlaylistTimeout,

    /// The play session was stopped, or its transcode ended while it started
    #[error("the play session was stopped")]
    Stopped,

    #[error("the server is shutting down")]
    ShuttingDown,

    #[error("{0} progressive transcodes are running, the most the server runs at once")]
    TooMany(usize),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2parental_legacy_segment_refuses_a_playback_id() {
        let data = std::env::temp_dir().join(format!("rustyfin_r2parental_{}", Uuid::new_v4()));
        let manager = TranscodeManager::new(&data);
        let (item, playback) = (Uuid::new_v4(), Uuid::new_v4());
        let output_dir = manager.cache_root.join(playback.to_string());
        std::fs::create_dir_all(&output_dir).unwrap();
        std::fs::write(output_dir.join("main.m3u8"), "#EXTM3U").unwrap();
        manager.sessions.insert(
            playback,
            TranscodeSession {
                item_id: item,
                playback_id: playback,
                input_path: String::new(),
                play_session_id: String::new(),
                device_id: String::new(),
                output_dir,
                child: None,
                stderr_task: None,
                stderr_tail: Arc::new(std::sync::Mutex::new(VecDeque::new())),
                last_access: Instant::now(),
                profile: TranscodeProfile::default(),
                progressive: false,
            },
        );
        // By the item's id the session's file is served; by the playback id it isn't.
        assert!(manager.legacy_segment_path(item, "main.m3u8").is_some());
        assert_eq!(manager.legacy_segment_path(playback, "main.m3u8"), None);
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn test_default_profile() {
        let p = TranscodeProfile::default();
        assert_eq!(p.max_video_bitrate_kbps, 8000);
        assert_eq!(p.video_codec, "libx264");
        assert_eq!(p.audio_codec, "aac");
        assert_eq!(p.segment_duration, 6);
        assert!(!p.burn_subtitles);
    }

    #[test]
    fn test_hw_accel_from_str() {
        assert_eq!(HwAccel::from_str("vaapi"), HwAccel::Vaapi);
        assert_eq!(HwAccel::from_str("nvenc"), HwAccel::Nvenc);
        assert_eq!(HwAccel::from_str("nvidia"), HwAccel::Nvenc);
        assert_eq!(HwAccel::from_str("qsv"), HwAccel::Qsv);
        assert_eq!(HwAccel::from_str("intel"), HwAccel::Qsv);
        assert_eq!(HwAccel::from_str("none"), HwAccel::None);
        assert_eq!(HwAccel::from_str("unknown"), HwAccel::None);
    }

    #[test]
    fn test_build_ffmpeg_args_basic() {
        let profile = TranscodeProfile::default();
        let args = build_ffmpeg_args("/input.mkv", "/out/main.m3u8", "/out/seg_%04d.ts", &profile);

        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.mkv".to_string()));
        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"hls".to_string()));
        assert!(args.contains(&"-c:v".to_string()));
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"aac".to_string()));
        assert!(args.contains(&"-preset".to_string()));
        assert!(args.contains(&"veryfast".to_string()));
        assert!(args.contains(&"/out/main.m3u8".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_vaapi() {
        let profile = TranscodeProfile {
            hw_accel: HwAccel::Vaapi,
            ..Default::default()
        };
        let args = build_ffmpeg_args("/input.mkv", "/out/main.m3u8", "/out/seg_%04d.ts", &profile);

        assert!(args.contains(&"-hwaccel".to_string()));
        assert!(args.contains(&"vaapi".to_string()));
        assert!(args.contains(&"h264_vaapi".to_string()));
        // No -preset for HW accel
        assert!(!args.contains(&"-preset".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_subtitle_burnin() {
        let profile = TranscodeProfile {
            burn_subtitles: true,
            subtitle_stream_index: Some(1),
            ..Default::default()
        };
        let args = build_ffmpeg_args("/input.mkv", "/out/main.m3u8", "/out/seg_%04d.ts", &profile);

        let vf_idx = args.iter().position(|a| a == "-vf").unwrap();
        let vf_val = &args[vf_idx + 1];
        assert!(vf_val.contains("subtitles="));
        assert!(vf_val.contains("si=1"));
    }

    #[tokio::test]
    async fn test_transcode_manager_new() {
        let mgr = TranscodeManager::new(Path::new("/tmp/test_rustyfin"));
        assert!(!mgr.is_active(Uuid::new_v4()).await);
    }

    /// True when `args` holds `flag` directly followed by `value`.
    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    fn args_for(profile: &TranscodeProfile) -> Vec<String> {
        build_ffmpeg_args("/input.mkv", "/out/main.m3u8", "/out/seg_%04d.ts", profile)
    }

    #[test]
    fn test_build_ffmpeg_args_quiet() {
        let args = args_for(&TranscodeProfile::default());
        assert!(args.contains(&"-hide_banner".to_string()), "{args:?}");
        assert!(args.contains(&"-nostats".to_string()), "{args:?}");
        assert_eq!(args.last().unwrap(), "/out/main.m3u8");
    }

    #[test]
    fn test_build_ffmpeg_args_audio_only_drops_video_only() {
        for hw_accel in [HwAccel::None, HwAccel::Vaapi] {
            let profile = TranscodeProfile {
                audio_only: true,
                copy_video: true,
                hw_accel,
                ..Default::default()
            };
            let args = args_for(&profile);
            assert!(args.contains(&"-vn".to_string()), "{args:?}");
            assert!(
                !args.contains(&"-an".to_string()),
                "-an drops the audio too: {args:?}"
            );
            assert!(!args.contains(&"-c:v".to_string()), "{args:?}");
            assert!(!args.contains(&"-hwaccel".to_string()), "{args:?}");
            assert!(has_pair(&args, "-c:a", "aac"), "{args:?}");
            assert_eq!(args.last().unwrap(), "/out/main.m3u8");
        }
    }

    #[test]
    fn test_build_ffmpeg_args_copy_video() {
        for hw_accel in [HwAccel::None, HwAccel::Vaapi, HwAccel::Nvenc, HwAccel::Qsv] {
            let profile = TranscodeProfile {
                copy_video: true,
                hw_accel,
                ..Default::default()
            };
            let args = args_for(&profile);
            assert!(has_pair(&args, "-c:v", "copy"), "{hw_accel:?}: {args:?}");
            for flag in [
                "-b:v",
                "-maxrate",
                "-bufsize",
                "-vf",
                "-filter_complex",
                "-preset",
                "-hwaccel",
                "-hwaccel_output_format",
                "-vaapi_device",
                "-init_hw_device",
            ] {
                assert!(
                    !args.iter().any(|a| a == flag),
                    "{hw_accel:?}: {flag} in {args:?}"
                );
            }
            assert!(
                !args.iter().any(|a| a.starts_with("h264_")),
                "{hw_accel:?}: {args:?}"
            );
            // The audio is still encoded to stereo AAC.
            assert!(has_pair(&args, "-c:a", "aac"), "{args:?}");
            assert!(has_pair(&args, "-b:a", "192k"), "{args:?}");
            assert!(has_pair(&args, "-ac", "2"), "{args:?}");
            assert_eq!(args.last().unwrap(), "/out/main.m3u8");
        }
    }

    #[test]
    fn test_build_ffmpeg_args_burn_in_overrides_copy_video() {
        let profile = TranscodeProfile {
            copy_video: true,
            burn_subtitles: true,
            subtitle_stream_index: Some(0),
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(args.contains(&"-vf".to_string()), "{args:?}");
    }

    #[test]
    fn test_build_ffmpeg_args_copy_audio() {
        let profile = TranscodeProfile {
            copy_audio: true,
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-c:a", "copy"), "{args:?}");
        assert!(!args.contains(&"-b:a".to_string()), "{args:?}");
        assert!(!args.contains(&"-ac".to_string()), "{args:?}");
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
    }

    fn position(args: &[String], flag: &str) -> usize {
        args.iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("no {flag} in {args:?}"))
    }

    #[test]
    fn test_build_ffmpeg_args_start_time_seeks_the_input() {
        let profile = TranscodeProfile {
            start_time_ticks: 6_000_000_000,
            ..Default::default()
        };
        let args = args_for(&profile);
        // An input option: ffmpeg seeks the input, and output time 0 is the start time
        assert!(has_pair(&args, "-ss", "600.000"), "{args:?}");
        assert!(position(&args, "-ss") < position(&args, "-i"), "{args:?}");
        // Nothing is copied, so nothing before the start can reach the output
        assert!(!args.contains(&"-copypriorss".to_string()), "{args:?}");

        let profile = TranscodeProfile {
            start_time_ticks: 12_345_678_901,
            ..Default::default()
        };
        assert!(has_pair(&args_for(&profile), "-ss", "1234.567"));
    }

    #[test]
    fn test_build_ffmpeg_args_start_time_with_a_copy_drops_earlier_packets() {
        for (copy_video, copy_audio) in [(true, false), (false, true), (true, true)] {
            let profile = TranscodeProfile {
                start_time_ticks: 300_000_000,
                copy_video,
                copy_audio,
                ..Default::default()
            };
            let args = args_for(&profile);
            assert!(has_pair(&args, "-ss", "30.000"), "{args:?}");
            // Copied packets before the start would start the output before the start time
            assert!(has_pair(&args, "-copypriorss", "0"), "{args:?}");
            assert!(
                position(&args, "-copypriorss") > position(&args, "-i"),
                "an output option: {args:?}"
            );
        }
    }

    #[test]
    fn test_build_ffmpeg_args_without_start_time_does_not_seek() {
        for start_time_ticks in [0, -10_000_000] {
            let profile = TranscodeProfile {
                start_time_ticks,
                copy_video: true,
                copy_audio: true,
                ..Default::default()
            };
            let args = args_for(&profile);
            assert!(!args.contains(&"-ss".to_string()), "{args:?}");
            assert!(!args.contains(&"-copypriorss".to_string()), "{args:?}");
        }
        assert_eq!(TranscodeProfile::default().start_time_ticks, 0);
    }

    /// The values of every `-map` in `args`, in order.
    fn maps(args: &[String]) -> Vec<&str> {
        args.windows(2)
            .filter(|w| w[0] == "-map")
            .map(|w| w[1].as_str())
            .collect()
    }

    #[test]
    fn test_build_ffmpeg_args_maps_the_video_and_the_chosen_audio() {
        // The audio index is the stream's absolute index (MediaStream.Index), not its place
        // among the audio streams; an explicit map must not drop the video either
        let profile = TranscodeProfile {
            audio_stream_index: Some(2),
            ..Default::default()
        };
        assert_eq!(maps(&args_for(&profile)), ["0:V:0?", "0:2"]);
        // No choice: the first audio stream, if there is one
        assert_eq!(
            maps(&args_for(&TranscodeProfile::default())),
            ["0:V:0?", "0:a:0?"]
        );
        let profile = TranscodeProfile {
            copy_video: true,
            copy_audio: true,
            audio_stream_index: Some(1),
            ..Default::default()
        };
        assert_eq!(maps(&args_for(&profile)), ["0:V:0?", "0:1"]);
    }

    #[test]
    fn test_build_ffmpeg_args_audio_only_maps_only_the_audio() {
        let profile = TranscodeProfile {
            audio_only: true,
            audio_stream_index: Some(3),
            ..Default::default()
        };
        let args = args_for(&profile);
        assert_eq!(maps(&args), ["0:3"]);
        assert!(args.contains(&"-vn".to_string()), "{args:?}");
        let profile = TranscodeProfile {
            audio_only: true,
            ..Default::default()
        };
        assert_eq!(maps(&args_for(&profile)), ["0:a:0?"]);
    }

    /// The values of every `flag` in `args`, in order.
    fn values<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].as_str())
            .collect()
    }

    #[test]
    fn test_build_ffmpeg_args_image_subtitle_is_overlaid() {
        let sub = |video_size| ImageSubtitle {
            stream_index: 5,
            video_size,
        };
        let profile = TranscodeProfile {
            image_subtitle: Some(sub(Some((1440, 1080)))),
            audio_stream_index: Some(1),
            copy_video: true,
            ..Default::default()
        };
        let args = args_for(&profile);
        // The subtitle comes from a second input of the same file. The video is scaled and made
        // 8-bit first, the subtitle scaled once to the same size, and laid over it
        assert_eq!(values(&args, "-i"), ["/input.mkv", "/input.mkv"]);
        assert!(
            has_pair(
                &args,
                "-filter_complex",
                "[0:V:0]scale=1440:1080,format=yuv420p[main];[1:5]scale=1440:1080[sub];\
                 [main][sub]overlay=eof_action=pass:repeatlast=0[v]"
            ),
            "{args:?}"
        );
        // A video copy gives way to the encode
        assert_eq!(maps(&args), ["[v]", "0:1"]);
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        for flag in ["-vf", "-hwaccel", "-vaapi_device"] {
            assert!(!args.contains(&flag.to_string()), "{args:?}");
        }

        // A video over the limits is scaled down, the subtitle to the same size
        let profile = TranscodeProfile {
            image_subtitle: Some(sub(Some((1920, 1040)))),
            max_width: 1280,
            max_height: 720,
            ..Default::default()
        };
        let graph = values(&args_for(&profile), "-filter_complex").concat();
        assert!(
            graph
                .starts_with("[0:V:0]scale=1280:692,format=yuv420p[main];[1:5]scale=1280:692[sub]"),
            "{graph}"
        );

        // Both inputs seek to the start point
        let profile = TranscodeProfile {
            image_subtitle: Some(sub(Some((1920, 1040)))),
            start_time_ticks: 600_000_000,
            ..Default::default()
        };
        let args = args_for(&profile);
        let seeks: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-ss")
            .map(|(i, _)| i)
            .collect();
        let inputs: Vec<usize> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "-i")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(seeks.len(), 2, "{args:?}");
        assert_eq!(inputs, [seeks[0] + 2, seeks[1] + 2], "{args:?}");
        assert_eq!(values(&args, "-ss"), ["60.000", "60.000"]);
    }

    #[test]
    fn test_build_ffmpeg_args_image_subtitle_on_hardware() {
        // The device decodes and scales; the frames come down as NV12 for the overlay, and go
        // back up for VAAPI, whose encoder takes only device frames
        for (hw_accel, scale, after, codec) in [
            (
                HwAccel::Vaapi,
                "scale_vaapi",
                ",format=nv12,hwupload",
                "h264_vaapi",
            ),
            (HwAccel::Nvenc, "scale_cuda", ",format=nv12", "h264_nvenc"),
            (HwAccel::Qsv, "scale_qsv", ",format=nv12", "h264_qsv"),
        ] {
            let profile = TranscodeProfile {
                image_subtitle: Some(ImageSubtitle {
                    stream_index: 4,
                    video_size: Some((1920, 1040)),
                }),
                hw_accel,
                ..Default::default()
            };
            let args = args_for(&profile);
            let graph = format!(
                "[0:V:0]{scale}=w=1920:h=1040:format=nv12,hwdownload,format=nv12[main];\
                 [1:4]scale=1920:1040[sub];[main][sub]overlay=eof_action=pass:repeatlast=0{after}[v]"
            );
            assert!(
                has_pair(&args, "-filter_complex", &graph),
                "{hw_accel:?}: {args:?}"
            );
            assert!(has_pair(&args, "-c:v", codec), "{hw_accel:?}: {args:?}");
            // The device flags belong to the video's input only
            let hwaccel = position(&args, "-hwaccel");
            let inputs: Vec<usize> = args
                .iter()
                .enumerate()
                .filter(|(_, a)| *a == "-i")
                .map(|(i, _)| i)
                .collect();
            assert!(hwaccel < inputs[0], "{hw_accel:?}: {args:?}");
            assert_eq!(values(&args, "-hwaccel").len(), 1, "{hw_accel:?}");
            // VAAPI has no preset; NVENC and QSV take the auto preset's
            let preset = match hw_accel {
                HwAccel::Nvenc => vec!["p1"],
                HwAccel::Qsv => vec!["veryfast"],
                _ => vec![],
            };
            assert_eq!(values(&args, "-preset"), preset, "{hw_accel:?}: {args:?}");
        }
        // Without the video's size the overlay stays in software
        let profile = TranscodeProfile {
            image_subtitle: Some(ImageSubtitle {
                stream_index: 4,
                video_size: None,
            }),
            hw_accel: HwAccel::Vaapi,
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(!args.contains(&"-hwaccel".to_string()), "{args:?}");
    }

    #[test]
    fn test_output_size_fits_the_limits_in_even_numbers() {
        assert_eq!(output_size((1920, 1040), 1920, 1080), (1920, 1040));
        assert_eq!(output_size((1440, 1080), 1920, 1080), (1440, 1080));
        assert_eq!(output_size((1920, 1040), 1280, 720), (1280, 692));
        assert_eq!(output_size((3840, 2160), 1920, 1080), (1920, 1080));
        assert_eq!(output_size((720, 576), 1920, 1080), (720, 576));
        assert_eq!(output_size((1919, 1079), 4000, 4000), (1918, 1078));
        assert_eq!(output_size((1920, 1080), 0, 0), (2, 2));
    }

    #[test]
    fn test_build_ffmpeg_args_force_key_frames_at_segment_starts() {
        let key_frames = |profile: &TranscodeProfile| {
            values(&args_for(profile), "-force_key_frames:v")
                .concat()
                .to_string()
        };
        let encoded = TranscodeProfile::default();
        assert_eq!(key_frames(&encoded), "expr:gte(t,n_forced*6)");
        let short = TranscodeProfile {
            segment_duration: 3,
            hw_accel: HwAccel::Nvenc,
            ..Default::default()
        };
        assert_eq!(key_frames(&short), "expr:gte(t,n_forced*3)");
        // A copied video keeps its own key frames, and audio alone has none
        for profile in [
            TranscodeProfile {
                copy_video: true,
                ..Default::default()
            },
            TranscodeProfile {
                audio_only: true,
                ..Default::default()
            },
        ] {
            assert_eq!(key_frames(&profile), "", "{profile:?}");
        }
    }

    #[test]
    fn test_build_ffmpeg_args_fmp4_segments_and_the_copy_tag() {
        let profile = TranscodeProfile {
            copy_video: true,
            fmp4: true,
            video_tag: Some("hvc1".to_string()),
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-hls_segment_type", "fmp4"), "{args:?}");
        assert!(
            has_pair(&args, "-hls_fmp4_init_filename", "init.mp4"),
            "{args:?}"
        );
        assert!(has_pair(&args, "-tag:v", "hvc1"), "{args:?}");
        assert!(has_pair(&args, "-c:v", "copy"), "{args:?}");
        // MPEG-TS stays the default, and an encoded video gets no copy tag
        let args = args_for(&TranscodeProfile {
            video_tag: Some("hvc1".to_string()),
            ..Default::default()
        });
        assert!(has_pair(&args, "-hls_segment_type", "mpegts"), "{args:?}");
        assert!(!args.contains(&"-tag:v".to_string()), "{args:?}");
        assert!(
            !args.contains(&"-hls_fmp4_init_filename".to_string()),
            "{args:?}"
        );
    }

    #[test]
    fn test_build_ffmpeg_args_image_subtitle_of_unknown_size_is_not_scaled() {
        let profile = TranscodeProfile {
            image_subtitle: Some(ImageSubtitle {
                stream_index: 3,
                video_size: None,
            }),
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(
            has_pair(
                &args,
                "-filter_complex",
                "[0:V:0][1:3]overlay=eof_action=pass:repeatlast=0,scale='trunc(min(min(iw,1920),1080*iw/ih)/2)*2':'trunc(min(min(ih,1080),1920*ih/iw)/2)*2',format=yuv420p[v]"
            ),
            "{args:?}"
        );
        assert_eq!(values(&args, "-i"), ["/input.mkv", "/input.mkv"]);
        assert_eq!(maps(&args), ["[v]", "0:a:0?"]);
        // The image subtitle wins over a text burn-in asked for at the same time
        let profile = TranscodeProfile {
            burn_subtitles: true,
            subtitle_stream_index: Some(0),
            ..profile
        };
        let args = args_for(&profile);
        assert!(!args.iter().any(|a| a.contains("subtitles=")), "{args:?}");
    }

    #[test]
    fn test_build_ffmpeg_args_text_burn_in_after_a_seek_keeps_the_subtitle_times() {
        // After an input seek the frames start at 0, but the subtitle filter reads the file
        // from its start: shift the frames to the file's time for the filter and back
        let profile = TranscodeProfile {
            burn_subtitles: true,
            subtitle_stream_index: Some(1),
            start_time_ticks: 6_000_000_000,
            ..Default::default()
        };
        let args = args_for(&profile);
        let vf = &args[position(&args, "-vf") + 1];
        assert!(
            vf.ends_with(
                ",setpts=PTS+600.000/TB,subtitles=f='/input.mkv':si=1,setpts=PTS-600.000/TB"
            ),
            "{vf}"
        );
        // From the start there is nothing to shift
        let profile = TranscodeProfile {
            start_time_ticks: 0,
            ..profile
        };
        let args = args_for(&profile);
        let vf = &args[position(&args, "-vf") + 1];
        assert!(vf.ends_with(",subtitles=f='/input.mkv':si=1"), "{vf}");
        assert!(!vf.contains("setpts"), "{vf}");
    }

    /// A port of libavutil's `av_get_token` (avstring.c), the parser ffmpeg reads a filter's
    /// options with: the token up to an unquoted char of `term`, and the rest from that char.
    /// Leading whitespace is skipped, `\x` is `x`, text in `'...'` is literal with the quotes
    /// dropped, and whitespace outside quotes at the token's end is trimmed.
    fn ff_token<'a>(s: &'a str, term: &str) -> (String, &'a str) {
        let s = s.trim_start_matches([' ', '\n', '\t', '\r']);
        let mut out = String::new();
        let mut end = 0;
        let mut rest = s;
        while let Some(c) = rest.chars().next() {
            if term.contains(c) {
                break;
            }
            rest = &rest[c.len_utf8()..];
            match (c, rest.chars().next()) {
                ('\\', Some(next)) => {
                    out.push(next);
                    rest = &rest[next.len_utf8()..];
                    end = out.len();
                }
                ('\'', _) => {
                    let close = rest.find('\'').unwrap_or(rest.len());
                    out.push_str(&rest[..close]);
                    rest = &rest[close..];
                    if let Some(after) = rest.strip_prefix('\'') {
                        rest = after;
                        end = out.len();
                    }
                }
                _ => out.push(c),
            }
        }
        let kept = out.trim_end_matches([' ', '\n', '\t', '\r']).len().max(end);
        out.truncate(kept);
        (out, rest)
    }

    /// The options of a graph's `subtitles=` filter as ffmpeg reads them: the graph level, where
    /// `[],;` end the filter and its quotes and escapes are undone, then `key=value` pairs split
    /// by `:`, with the option level's quotes and escapes undone.
    fn ff_subtitles_options(graph: &str) -> Vec<(String, String)> {
        let (_, after) = graph
            .split_once("subtitles=")
            .expect("no subtitles= in the graph");
        let (options, _) = ff_token(after, "[],;");
        let mut pairs = Vec::new();
        let mut rest = options.as_str();
        while !rest.is_empty() {
            let (key, value_text) = rest.split_once('=').expect("an option without =");
            let (value, next) = ff_token(value_text, ":");
            pairs.push((key.to_string(), value));
            rest = next.strip_prefix(':').unwrap_or(next);
        }
        pairs
    }

    #[test]
    fn test_subtitles_filter_escapes_like_jellyfin() {
        assert_eq!(
            subtitles_filter("/m/Ocean's Eleven: Redux.mkv", 2, 0),
            r"subtitles=f='/m/Ocean'\\\''s Eleven\: Redux.mkv':si=2"
        );
        assert_eq!(
            subtitles_filter("/m/Film.mkv", 2, 0),
            "subtitles=f='/m/Film.mkv':si=2"
        );
    }

    #[test]
    fn test_subtitles_filter_paths_reach_ffmpeg_intact() {
        let paths = [
            "/m/Movies/Ocean's Eleven: Redux (2001).mkv",
            r"/m/Movies/back\slash.mkv",
            "/m/Movies/[Group] Show - 01 [1080p].mkv",
            "/m/Movies/Hello, World; Again.mkv",
            "/m/Movies/a=b.mkv",
            r#"/m/Movies/"Quoted" Title.mkv"#,
            r#"/m/Movies/It's: [all], of; them\ = "x".mkv"#,
            "/m/Movies/''two''.mkv",
            "/m/Movies/end'",
        ];
        for path in paths {
            for ticks in [0, 125_000_000] {
                let graph = subtitles_filter(path, 3, ticks);
                let want = vec![
                    ("f".to_string(), path.to_string()),
                    ("si".to_string(), "3".to_string()),
                ];
                assert_eq!(ff_subtitles_options(&graph), want, "{graph}");
            }
        }
    }

    #[test]
    fn test_build_ffmpeg_args_text_burn_in_escapes_the_input() {
        let input = "/m/Ocean's Eleven: Redux.mkv";
        for start_time_ticks in [0, 6_000_000_000] {
            let profile = TranscodeProfile {
                burn_subtitles: true,
                subtitle_stream_index: Some(1),
                start_time_ticks,
                ..Default::default()
            };
            let args = build_ffmpeg_args(input, "/out/main.m3u8", "/out/seg_%04d.ts", &profile);
            let vf = &args[position(&args, "-vf") + 1];
            let burn = subtitles_filter(input, 1, start_time_ticks);
            assert!(vf.contains(&burn), "{vf}");
        }
    }

    // B25: 10-bit video

    /// The software scale for the default 1920x1080 limit.
    const SW_SCALE: &str =
        "scale='trunc(min(min(iw,1920),1080*iw/ih)/2)*2':'trunc(min(min(ih,1080),1920*ih/iw)/2)*2'";

    /// Each hardware encoder with its scale for the default limit, ending in the 8-bit 4:2:0
    /// format the encoder takes.
    const HW_SCALES: [(HwAccel, &str, &str); 3] = [
        (
            HwAccel::Vaapi,
            "h264_vaapi",
            "scale_vaapi=w='trunc(min(min(iw,1920),1080*iw/ih)/2)*2':h='trunc(min(min(ih,1080),1920*ih/iw)/2)*2':format=nv12",
        ),
        (
            HwAccel::Nvenc,
            "h264_nvenc",
            "scale_cuda=w='trunc(min(min(iw,1920),1080*iw/ih)/2)*2':h='trunc(min(min(ih,1080),1920*ih/iw)/2)*2':format=nv12",
        ),
        (
            HwAccel::Qsv,
            "h264_qsv",
            "scale_qsv=w='trunc(min(min(iw,1920),1080*iw/ih)/2)*2':h='trunc(min(min(ih,1080),1920*ih/iw)/2)*2':format=nv12",
        ),
    ];

    #[test]
    fn test_build_ffmpeg_args_encode_8bit_420_in_software() {
        // libx264 keeps the decoder's format, so a 10-bit or 4:2:2 source would come out as
        // High 10 or High 4:2:2, which browsers don't decode
        let args = args_for(&TranscodeProfile::default());
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(
            has_pair(&args, "-vf", &format!("{SW_SCALE},format=yuv420p")),
            "{args:?}"
        );
        assert!(!args.contains(&"-pix_fmt".to_string()), "{args:?}");
        let profile = TranscodeProfile {
            max_width: 1280,
            max_height: 720,
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(
            has_pair(
                &args,
                "-vf",
                "scale='trunc(min(min(iw,1280),720*iw/ih)/2)*2':'trunc(min(min(ih,720),1280*ih/iw)/2)*2',format=yuv420p"
            ),
            "{args:?}"
        );
    }

    /// Evaluates an `even_fit` expression (without its quotes) for an `iw`×`ih` source: the
    /// four operations it uses, enough to check the sizes ffmpeg computes from it.
    fn eval_even(expr: &str, iw: f64, ih: f64) -> f64 {
        fn parse(s: &mut &str, iw: f64, ih: f64) -> f64 {
            let mut value = term(s, iw, ih);
            while let Some(rest) = s.strip_prefix('*').or_else(|| s.strip_prefix('/')) {
                let op = s.as_bytes()[0];
                *s = rest;
                let rhs = term(s, iw, ih);
                value = if op == b'*' { value * rhs } else { value / rhs };
            }
            value
        }
        fn term(s: &mut &str, iw: f64, ih: f64) -> f64 {
            for (name, f) in [("trunc(", 1), ("min(", 2)] {
                if let Some(rest) = s.strip_prefix(name) {
                    *s = rest;
                    let a = parse(s, iw, ih);
                    let v = if f == 2 {
                        *s = s.strip_prefix(',').unwrap();
                        a.min(parse(s, iw, ih))
                    } else {
                        a.trunc()
                    };
                    *s = s.strip_prefix(')').unwrap();
                    return v;
                }
            }
            for (name, v) in [("iw", iw), ("ih", ih)] {
                if let Some(rest) = s.strip_prefix(name) {
                    *s = rest;
                    return v;
                }
            }
            let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            let (n, rest) = s.split_at(end);
            *s = rest;
            n.parse().unwrap()
        }
        let mut s = expr;
        let v = parse(&mut s, iw, ih);
        assert!(s.is_empty(), "left over: {s}");
        v
    }

    /// A 1280×720 source at a 480- and a 360-line limit gets even widths and heights, its
    /// aspect kept, in software and on VAAPI, as Jellyfin 12.4's `trunc(…/2)*2` computes them:
    /// 852×480 and 640×360. A known source size gives the same.
    #[test]
    fn r3hwaccel_scales_give_even_dimensions() {
        for (max_height, want) in [(480, (852.0, 480.0)), (360, (640.0, 360.0))] {
            let (w, h) = even_fit(1920, max_height);
            let got = (
                eval_even(w.trim_matches('\''), 1280.0, 720.0),
                eval_even(h.trim_matches('\''), 1280.0, 720.0),
            );
            assert_eq!(got, want, "{max_height}");
            assert_eq!(
                output_size((1280, 720), 1920, max_height),
                (want.0 as u32, want.1 as u32)
            );
            for accel in [HwAccel::None, HwAccel::Vaapi] {
                let profile = TranscodeProfile {
                    hw_accel: accel,
                    max_height,
                    ..Default::default()
                };
                let args = args_for(&profile);
                let vf = &args[position(&args, "-vf") + 1];
                assert!(
                    vf.contains(&format!("w={w}:h={h}")) || vf.contains(&format!("scale={w}:{h},")),
                    "{accel:?}: {vf}"
                );
                assert!(!vf.contains("force_original_aspect_ratio"), "{vf}");
                // With the source size known, the exact even size
                let sized = TranscodeProfile {
                    source_size: Some((1280, 720)),
                    ..profile
                };
                let args = args_for(&sized);
                let vf = &args[position(&args, "-vf") + 1];
                let exact = format!("{}:{}", want.0, want.1);
                if accel == HwAccel::Vaapi {
                    assert!(
                        vf.contains(&format!("scale_vaapi=w={}:h={}", want.0, want.1)),
                        "{vf}"
                    );
                } else {
                    assert!(
                        vf.contains(&format!("scale={w}:{h}")) || vf.contains(&exact),
                        "{vf}"
                    );
                }
            }
        }
        // A wide source limited by width keeps its aspect: 1920×800 in 1280×720 is 1280×532
        let (w, h) = even_fit(1280, 720);
        assert_eq!(eval_even(w.trim_matches('\''), 1920.0, 800.0), 1280.0);
        assert_eq!(eval_even(h.trim_matches('\''), 1920.0, 800.0), 532.0);
    }

    #[test]
    fn test_build_ffmpeg_args_encode_8bit_420_on_hardware() {
        // The hardware H.264 encoders take 8-bit frames only: the scale converts P010 to NV12
        for (hw_accel, encoder, scale) in HW_SCALES {
            let profile = TranscodeProfile {
                hw_accel,
                ..Default::default()
            };
            let args = args_for(&profile);
            assert!(has_pair(&args, "-c:v", encoder), "{hw_accel:?}: {args:?}");
            assert!(has_pair(&args, "-vf", scale), "{hw_accel:?}: {args:?}");
            assert!(
                !args.contains(&"-pix_fmt".to_string()),
                "{hw_accel:?}: {args:?}"
            );
        }
    }

    #[test]
    fn test_build_ffmpeg_args_burn_in_draws_on_8bit_420() {
        let burn = "subtitles=f='/input.mkv':si=1";
        let profile = TranscodeProfile {
            burn_subtitles: true,
            subtitle_stream_index: Some(1),
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(
            has_pair(&args, "-vf", &format!("{SW_SCALE},format=yuv420p,{burn}")),
            "{args:?}"
        );
        // On hardware the scale makes the frames NV12, which the download keeps; they go back
        // up for VAAPI, whose encoder takes only device frames
        for (hw_accel, _, scale) in HW_SCALES {
            let profile = TranscodeProfile {
                hw_accel,
                ..profile.clone()
            };
            let after = if hw_accel == HwAccel::Vaapi {
                ",format=nv12,hwupload"
            } else {
                ",format=nv12"
            };
            let args = args_for(&profile);
            assert!(
                has_pair(
                    &args,
                    "-vf",
                    &format!("{scale},hwdownload,format=nv12,{burn}{after}")
                ),
                "{hw_accel:?}: {args:?}"
            );
        }
    }

    #[test]
    fn test_build_ffmpeg_args_copy_and_audio_only_have_no_format_step() {
        let profiles = [
            TranscodeProfile {
                copy_video: true,
                ..Default::default()
            },
            TranscodeProfile {
                audio_only: true,
                ..Default::default()
            },
        ];
        for profile in profiles {
            let args = args_for(&profile);
            assert!(
                !args
                    .iter()
                    .any(|a| a == "-vf" || a == "-filter_complex" || a.contains("format=")),
                "{args:?}"
            );
        }
    }

    /// The args the base built for the default profile, before the format step, with the key
    /// frames forced at segment starts and the default encoding options (CRF 23, the muxing
    /// queue) that came later.
    fn base_args(input: &str, playlist: &str, segments: &str) -> Vec<String> {
        "-y -hide_banner -nostats -i INPUT -c:v libx264 -crf 23 -maxrate 8000k -bufsize 16000k \
         -vf SCALE -map 0:V:0? -map 0:a:0? -c:a aac -b:a 192k -ac 2 -max_muxing_queue_size 2048 \
         -force_key_frames:v expr:gte(t,n_forced*6) -f hls -hls_time 6 \
         -hls_list_size 0 -hls_flags independent_segments -hls_segment_type mpegts \
         -hls_segment_filename SEGMENTS -hls_playlist_type event -start_number 0 \
         -preset veryfast PLAYLIST"
            .split_whitespace()
            .map(|arg| match arg {
                "INPUT" => input,
                "SCALE" => SW_SCALE,
                "SEGMENTS" => segments,
                "PLAYLIST" => playlist,
                arg => arg,
            })
            .map(String::from)
            .collect()
    }

    /// Whether ffmpeg and ffprobe run. When they don't, says that `test` is skipped.
    fn have_ffmpeg(test: &str) -> bool {
        for tool in ["ffmpeg", "ffprobe"] {
            let found = std::process::Command::new(tool)
                .arg("-version")
                .output()
                .is_ok_and(|o| o.status.success());
            if !found {
                println!("{test}: skipped, {tool} is not on PATH");
                return false;
            }
        }
        true
    }

    /// Runs ffmpeg with `args`, quiet when `quiet`; panics with its errors when it fails.
    fn run_ffmpeg(args: &[String], quiet: bool) {
        let mut cmd = std::process::Command::new("ffmpeg");
        if quiet {
            cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
        }
        let out = cmd.args(args).output().unwrap();
        assert!(
            out.status.success(),
            "ffmpeg {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Transcodes `input` with the args of `profile` into a new directory `name` under `dir`;
    /// returns that directory.
    fn transcode_into(dir: &Path, name: &str, input: &Path, profile: &TranscodeProfile) -> PathBuf {
        let out = dir.join(name);
        std::fs::create_dir_all(&out).unwrap();
        let args = build_ffmpeg_args(
            input.to_str().unwrap(),
            out.join("main.m3u8").to_str().unwrap(),
            out.join("seg_%04d.ts").to_str().unwrap(),
            profile,
        );
        run_ffmpeg(&args, false);
        out
    }

    /// The segments in `dir` by name, with their bytes.
    fn segments_in(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut segments: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "ts"))
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                (name, std::fs::read(&path).unwrap())
            })
            .collect();
        segments.sort();
        segments
    }

    /// Makes a `seconds`-long test file at `path` with ffmpeg; `video` are the video options.
    fn make_source(path: &Path, seconds: u32, video: &str) {
        let make = format!(
            "-f lavfi -i testsrc2=size=320x240:rate=25 -f lavfi -i sine=frequency=440 -t {seconds} \
             {video} -c:a aac"
        );
        let mut args: Vec<String> = make.split_whitespace().map(String::from).collect();
        args.push(path.to_str().unwrap().to_string());
        run_ffmpeg(&args, true);
    }

    /// The durations of a playlist's segments.
    fn segment_durations(playlist: &Path) -> Vec<f64> {
        std::fs::read_to_string(playlist)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix("#EXTINF:"))
            .map(|rest| rest.trim_end_matches(',').parse().unwrap())
            .collect()
    }

    #[test]
    fn test_forced_key_frames_cut_the_first_segment_at_its_length() {
        if !have_ffmpeg("test_forced_key_frames_cut_the_first_segment_at_its_length") {
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3kf-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        // libx264's own key frames come every 250 frames, 10 s here: one 7 s segment without
        // the forced ones
        let input = dir.0.join("gop10.mp4");
        make_source(&input, 7, "-c:v libx264 -pix_fmt yuv420p");
        let profile = TranscodeProfile {
            segment_duration: 2,
            ..Default::default()
        };
        let out = transcode_into(&dir.0, "out", &input, &profile);
        let durations = segment_durations(&out.join("main.m3u8"));
        assert!(durations.len() >= 3, "{durations:?}");
        assert!((durations[0] - 2.0).abs() < 0.1, "{durations:?}");
    }

    #[test]
    fn test_copied_hevc_goes_out_in_fragmented_mp4() {
        if !have_ffmpeg("test_copied_hevc_goes_out_in_fragmented_mp4") {
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3hevc-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let input = dir.0.join("main10.mkv");
        make_source(
            &input,
            4,
            "-c:v libx265 -pix_fmt yuv420p10le -x265-params keyint=50:log-level=error",
        );
        let profile = TranscodeProfile {
            copy_video: true,
            fmp4: true,
            video_tag: Some("hvc1".to_string()),
            segment_duration: 2,
            ..Default::default()
        };
        let out = dir.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let playlist = out.join("main.m3u8");
        let args = build_ffmpeg_args(
            input.to_str().unwrap(),
            playlist.to_str().unwrap(),
            out.join("segment_%04d.mp4").to_str().unwrap(),
            &profile,
        );
        run_ffmpeg(&args, false);

        let text = std::fs::read_to_string(&playlist).unwrap();
        assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4\""), "{text}");
        assert!(text.contains("segment_0000.mp4"), "{text}");
        assert!(out.join("init.mp4").is_file());
        // The init segment and a media segment together play as 10-bit HEVC tagged hvc1
        let mut joined = std::fs::read(out.join("init.mp4")).unwrap();
        joined.extend(std::fs::read(out.join("segment_0000.mp4")).unwrap());
        let probe_in = dir.0.join("joined.mp4");
        std::fs::write(&probe_in, joined).unwrap();
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
            .arg("stream=codec_name,codec_tag_string,pix_fmt")
            .args(["-of", "csv=p=0"])
            .arg(&probe_in)
            .output()
            .unwrap();
        let probed = String::from_utf8_lossy(&probe.stdout);
        assert_eq!(probed.trim(), "hevc,hvc1,yuv420p10le", "{probed}");
    }

    #[test]
    fn test_build_ffmpeg_args_8bit_source_encodes_as_before() {
        // The args are the base's with the format step added
        let mut expected = base_args("/input.mkv", "/out/main.m3u8", "/out/seg_%04d.ts");
        let vf = position(&expected, "-vf") + 1;
        expected[vf].push_str(",format=yuv420p");
        assert_eq!(args_for(&TranscodeProfile::default()), expected);

        // For an 8-bit 4:2:0 source the step changes nothing: the segments are the same bytes
        if !have_ffmpeg("test_build_ffmpeg_args_8bit_source_encodes_as_before") {
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-b25-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let input = dir.0.join("8bit.mp4");
        let make = "-f lavfi -i testsrc2=size=320x240:rate=25 -f lavfi -i sine=frequency=440 -t 3 \
                    -c:v libx264 -pix_fmt yuv420p -c:a aac";
        let mut args: Vec<String> = make.split_whitespace().map(String::from).collect();
        args.push(input.to_str().unwrap().to_string());
        run_ffmpeg(&args, true);

        let after = transcode_into(&dir.0, "after", &input, &TranscodeProfile::default());
        let before = dir.0.join("before");
        std::fs::create_dir_all(&before).unwrap();
        run_ffmpeg(
            &base_args(
                input.to_str().unwrap(),
                before.join("main.m3u8").to_str().unwrap(),
                before.join("seg_%04d.ts").to_str().unwrap(),
            ),
            false,
        );
        let (after, before) = (segments_in(&after), segments_in(&before));
        assert!(!before.is_empty());
        let names = |segments: &[(String, Vec<u8>)]| -> Vec<String> {
            segments.iter().map(|(name, _)| name.clone()).collect()
        };
        assert_eq!(names(&after), names(&before));
        assert!(after == before, "the segments differ from the base's");
    }

    #[test]
    fn test_build_ffmpeg_args_deep_sources_encode_as_8bit_high() {
        if !have_ffmpeg("test_build_ffmpeg_args_deep_sources_encode_as_8bit_high") {
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-b25-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let srt = dir.0.join("sub.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nB25\n").unwrap();
        // 10-bit HEVC (x265), 10-bit H.264 (Hi10P anime), 4:2:2 and 4:4:4 H.264, each with a
        // text subtitle
        for (name, codec, pix_fmt) in [
            ("hevc10", "libx265", "yuv420p10le"),
            ("hi10", "libx264", "yuv420p10le"),
            ("422", "libx264", "yuv422p"),
            ("444", "libx264", "yuv444p"),
        ] {
            let input = dir.0.join(format!("{name}.mkv"));
            let make = "-f lavfi -i testsrc2=size=320x240:rate=25 -i SRT -t 2 -map 0:v -map 1:s \
                        -c:s srt -c:v CODEC -pix_fmt PIX_FMT";
            let mut args: Vec<String> = make
                .split_whitespace()
                .map(|arg| match arg {
                    "SRT" => srt.to_str().unwrap(),
                    "CODEC" => codec,
                    "PIX_FMT" => pix_fmt,
                    arg => arg,
                })
                .map(String::from)
                .collect();
            if codec == "libx265" {
                args.extend(["-x265-params".to_string(), "log-level=error".to_string()]);
            }
            args.push(input.to_str().unwrap().to_string());
            run_ffmpeg(&args, true);

            for burn in [false, true] {
                let profile = TranscodeProfile {
                    burn_subtitles: burn,
                    subtitle_stream_index: burn.then_some(0),
                    ..Default::default()
                };
                let out = transcode_into(&dir.0, &format!("{name}-{burn}"), &input, &profile);
                let probe = std::process::Command::new("ffprobe")
                    .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
                    .args(["stream=profile,pix_fmt", "-of", "compact=p=0:nk=1"])
                    .arg(out.join("seg_0000.ts"))
                    .output()
                    .unwrap();
                // MPEG-TS lists the stream again under its program: the first line is enough
                let found = String::from_utf8_lossy(&probe.stdout);
                let found = found.lines().next().unwrap_or_default();
                assert_eq!(found, "High|yuv420p", "{name}, burn-in {burn}");
            }
        }
    }

    #[tokio::test]
    async fn test_drain_stderr_survives_long_and_invalid_output() {
        let mut data = Vec::new();
        for i in 0..2000 {
            data.extend_from_slice(format!("frame={i} {}\n", "x".repeat(40)).as_bytes());
        }
        data.extend_from_slice(b"bad \xff\xfe bytes\n");
        data.extend_from_slice(&[b'y'; 1000]);
        data.extend_from_slice(b"\n\n\r\n");
        data.extend_from_slice(b"g4 last line");
        let tail = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));

        drain_stderr(&data[..], Uuid::new_v4(), tail.clone()).await;

        let tail = tail.lock().unwrap();
        assert_eq!(tail.len(), STDERR_TAIL_LINES);
        assert_eq!(tail.back().map(String::as_str), Some("g4 last line"));
        assert!(
            tail.iter()
                .any(|l| l.starts_with("bad ") && l.contains('\u{fffd}')),
            "{tail:?}"
        );
        assert!(
            tail.iter()
                .any(|l| l.len() == 300 && l.bytes().all(|b| b == b'y')),
            "{tail:?}"
        );
        assert!(!tail.iter().any(|l| l.trim().is_empty()), "{tail:?}");
    }

    // ── Sessions, run against fake ffmpeg scripts ──

    const SEGMENT: &str = "segment_0000.ts";

    /// Writes one segment and the playlist next to its last argument, then keeps running.
    const FAKE_GOOD: &str = r#"#!/bin/sh
for last; do :; done
dir=$(dirname "$last")
printf 'g4' > "$dir/segment_0000.ts"
printf '#EXTM3U\n#EXTINF:6.0,\nsegment_0000.ts\n' > "$last"
exec sleep 30
"#;

    /// Fails the way ffmpeg does on a file it cannot read.
    const FAKE_FAIL: &str = r#"#!/bin/sh
echo "fake ffmpeg starting" >&2
echo "/in.mkv: Invalid data found when processing input" >&2
exit 1
"#;

    /// Fails when asked to open a hardware device, else serves like `good`.
    const FAKE_HWFAIL: &str = r#"#!/bin/sh
for a; do
    case "$a" in -init_hw_device) echo "Device creation failed: -22." >&2; exit 1;; esac
done
for last; do :; done
dir=$(dirname "$last")
printf 'g4' > "$dir/segment_0000.ts"
printf '#EXTM3U\n#EXTINF:6.0,\nsegment_0000.ts\n' > "$last"
exec sleep 30
"#;

    /// A progressive transcode that fails when asked to open a hardware device, else pipes.
    const FAKE_HWFAIL_PIPED: &str = r#"#!/bin/sh
for a; do
    case "$a" in -init_hw_device) echo "Failed to initialise VAAPI connection" >&2; exit 1;; esac
done
printf 'r3gaps2'
exec sleep 30
"#;

    /// A progressive transcode that hangs on a hardware device without writing, else pipes.
    const FAKE_HWHANG_PIPED: &str = r#"#!/bin/sh
for a; do
    case "$a" in -init_hw_device) exec sleep 30;; esac
done
printf 'r3gaps2'
exec sleep 30
"#;

    /// Never writes a playlist.
    const FAKE_SLOW: &str = "#!/bin/sh\nexec sleep 30\n";

    /// A progressive transcode: writes to stdout, then runs on.
    const FAKE_PIPED: &str = "#!/bin/sh\nprintf 'r3gaps2'\nexec sleep 30\n";

    /// Writes its first segment slowly, a little every 0.3 s for 3 s, then the playlist: a
    /// transcode slower than real time that still makes progress.
    const FAKE_PROGRESSING: &str = r#"#!/bin/sh
for last; do :; done
dir=$(dirname "$last")
i=0
while [ "$i" -lt 10 ]; do
    printf 'chunk' >> "$dir/segment_0000.ts"
    sleep 0.3
    i=$((i + 1))
done
printf '#EXTM3U\n#EXTINF:6.0,\nsegment_0000.ts\n' > "$last"
exec sleep 30
"#;

    /// Starts a grandchild in its own process group, notes its pid, then serves like `good`.
    const FAKE_FAMILY: &str = r#"#!/bin/sh
for last; do :; done
dir=$(dirname "$last")
sleep 30 &
echo $! > "$dir/grandchild.pid"
printf 'g4' > "$dir/segment_0000.ts"
printf '#EXTM3U\n#EXTINF:6.0,\nsegment_0000.ts\n' > "$last"
wait
"#;

    /// Writes a little of its first segment, then nothing more.
    const FAKE_STALLED: &str = r#"#!/bin/sh
for last; do :; done
dir=$(dirname "$last")
printf 'chunk' > "$dir/segment_0000.ts"
exec sleep 30
"#;

    /// Writes about 260 KiB to stderr before its playlist; with stderr piped and never read it
    /// blocks for good once the 64 KiB pipe is full.
    const FAKE_NOISY: &str = r#"#!/bin/sh
for last; do :; done
dir=$(dirname "$last")
i=0
while [ "$i" -lt 3000 ]; do
    echo "frame=$i fps=25 q=28.0 size=1024kB time=00:00:01.00 bitrate=8000.0kbits/s speed=1x" >&2
    i=$((i + 1))
done
printf 'g4' > "$dir/segment_0000.ts"
printf '#EXTM3U\n' > "$last"
exec sleep 30
"#;

    /// Returns the path of the fake ffmpeg `name`. The scripts are written once per test
    /// process, before any test runs one, so no spawn meets a script still open for writing
    /// (ETXTBSY). They live in one fixed directory that later runs overwrite.
    #[cfg(unix)]
    fn fake_ffmpeg(name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        let dir = DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join("rustyfin-core-fake-ffmpeg");
            std::fs::create_dir_all(&dir).unwrap();
            for (name, script) in [
                ("good", FAKE_GOOD),
                ("fail", FAKE_FAIL),
                ("slow", FAKE_SLOW),
                ("noisy", FAKE_NOISY),
                ("progressing", FAKE_PROGRESSING),
                ("stalled", FAKE_STALLED),
                ("family", FAKE_FAMILY),
                ("piped", FAKE_PIPED),
                ("hwfail", FAKE_HWFAIL),
                ("hwfail-piped", FAKE_HWFAIL_PIPED),
                ("hwhang-piped", FAKE_HWHANG_PIPED),
            ] {
                // Rename into place: another test process may be running the old file.
                let tmp = dir.join(format!("{name}.{}", std::process::id()));
                std::fs::write(&tmp, script).unwrap();
                std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
                std::fs::rename(&tmp, dir.join(name)).unwrap();
            }
            dir
        });
        dir.join(name)
    }

    /// Removes a test's data dir when dropped.
    struct TestDir(PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A manager on a fresh data dir that runs the fake ffmpeg `script` and waits at most `wait`
    /// for a playlist.
    #[cfg(unix)]
    fn fake_manager(script: &str, wait: Duration) -> (TestDir, TranscodeManager) {
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-g4-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let mut mgr = TranscodeManager::new(&dir.0);
        mgr.ffmpeg = fake_ffmpeg(script);
        mgr.playlist_wait = wait;
        (dir, mgr)
    }

    async fn start(
        mgr: &TranscodeManager,
        item: Uuid,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<(PathBuf, Uuid), TranscodeError> {
        let profile = TranscodeProfile::default();
        mgr.start_transcode(item, "/in.mkv", &profile, play_session_id, device_id)
            .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_two_play_sessions_do_not_collide() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let (playlist_a, a) = start(&mgr, item, "psid-a", "dev").await.unwrap();
        let (playlist_b, b) = start(&mgr, item, "psid-b", "dev").await.unwrap();
        assert_ne!(a, b);
        assert_ne!(playlist_a.parent(), playlist_b.parent());
        assert!(playlist_a.exists() && playlist_b.exists());

        // The same play session, in any case, gets its own transcode back.
        let (_, again) = start(&mgr, item, "PSID-A", "dev").await.unwrap();
        assert_eq!(again, a);
        assert_eq!(mgr.sessions.len(), 2);

        assert!(mgr.segment_path(item, a, SEGMENT).is_some());
        assert!(mgr.segment_path(item, b, SEGMENT).is_some());
        // A playback id serves only its own item.
        assert!(mgr.segment_path(Uuid::new_v4(), b, SEGMENT).is_none());

        // Stopping one viewer leaves the other.
        assert_eq!(mgr.stop_play_session(Some(item), "psid-a", "").await, 1);
        assert!(mgr.segment_path(item, a, SEGMENT).is_none());
        assert!(
            !playlist_a.exists(),
            "a stopped session's files are deleted"
        );
        assert!(mgr.segment_path(item, b, SEGMENT).is_some());
        assert!(mgr.is_active(item).await);

        mgr.stop_all().await;
        assert!(!mgr.is_active(item).await);
        assert!(!playlist_b.exists());
    }

    /// Starts `item`'s transcode of `input` for `play_session_id`, with no device id, and returns
    /// the playback id.
    async fn start_on(
        mgr: &TranscodeManager,
        item: Uuid,
        input: &str,
        play_session_id: &str,
    ) -> Uuid {
        let profile = TranscodeProfile::default();
        let (_, playback_id) = mgr
            .start_transcode(item, input, &profile, play_session_id, "")
            .await
            .unwrap();
        playback_id
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_another_version_gets_its_own_session() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let (hd, sd) = ("/films/Film - 1080p.mkv", "/films/Film - 720p.mkv");
        let a = start_on(&mgr, item, hd, "psid").await;
        // The same play session asks for another version of the item: a transcode of that file
        let b = start_on(&mgr, item, sd, "psid").await;
        assert_ne!(
            a, b,
            "another version must not reuse the first version's transcode"
        );
        // Each version's own transcode is reused
        assert_eq!(start_on(&mgr, item, hd, "psid").await, a);
        assert_eq!(start_on(&mgr, item, sd, "psid").await, b);
        assert_eq!(mgr.sessions.len(), 2);

        // Requests without a play session or device id are told apart by their file too
        let c = start_on(&mgr, item, hd, "").await;
        let d = start_on(&mgr, item, sd, "").await;
        assert_ne!(c, d);
        assert_eq!(start_on(&mgr, item, hd, "").await, c);

        // Stopping the play session stops both of its transcodes and nothing else
        assert_eq!(mgr.stop_play_session(Some(item), "psid", "").await, 2);
        assert!(mgr.segment_path(item, a, SEGMENT).is_none());
        assert!(mgr.segment_path(item, b, SEGMENT).is_none());
        assert!(mgr.segment_path(item, c, SEGMENT).is_some());
        assert!(mgr.segment_path(item, d, SEGMENT).is_some());

        mgr.stop_all().await;
    }

    /// The playback id of a start of `/in.mkv` with `profile` on a play session, from device "d"
    async fn start_profile(
        mgr: &TranscodeManager,
        item: Uuid,
        profile: &TranscodeProfile,
        play_session_id: &str,
    ) -> Uuid {
        let (_, playback_id) = mgr
            .start_transcode(item, "/in.mkv", profile, play_session_id, "d")
            .await
            .unwrap();
        playback_id
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_another_profile_replaces_the_session() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(5));
        let item = Uuid::new_v4();
        let a = TranscodeProfile {
            audio_stream_index: Some(1),
            ..Default::default()
        };
        let b = TranscodeProfile {
            audio_stream_index: Some(2),
            ..Default::default()
        };
        let c = TranscodeProfile {
            start_time_ticks: 600_000_000,
            ..a.clone()
        };
        let id1 = start_profile(&mgr, item, &a, "p").await;
        // The same profile reuses the transcode
        assert_eq!(start_profile(&mgr, item, &a, "p").await, id1);
        // Other tracks replace it: the client of the play session has moved on from the old one
        let id2 = start_profile(&mgr, item, &b, "p").await;
        assert_ne!(id2, id1, "other tracks must not reuse the old transcode");
        assert_eq!(mgr.sessions.len(), 1);
        // Another start replaces it too
        let id3 = start_profile(&mgr, item, &c, "p").await;
        assert_ne!(id3, id2, "another start must not reuse the old transcode");
        assert_eq!(mgr.sessions.len(), 1);

        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_other_profiles_on_two_play_sessions_keep_both() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(5));
        let item = Uuid::new_v4();
        let a = TranscodeProfile {
            audio_stream_index: Some(1),
            ..Default::default()
        };
        let b = TranscodeProfile {
            audio_stream_index: Some(2),
            ..Default::default()
        };
        let on_p = start_profile(&mgr, item, &a, "p").await;
        let on_q = start_profile(&mgr, item, &b, "q").await;
        assert_ne!(on_p, on_q);
        assert_eq!(mgr.sessions.len(), 2);

        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_stop_by_device_and_keyless() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let other = Uuid::new_v4();
        let (_, d1) = start(&mgr, item, "", "dev-1").await.unwrap();
        let (_, d2) = start(&mgr, item, "", "dev-2").await.unwrap();
        let (_, p2) = start(&mgr, item, "psid-2", "dev-2").await.unwrap();
        let (_, keyless) = start(&mgr, item, "", "").await.unwrap();
        let (_, other_keyless) = start(&mgr, other, "", "").await.unwrap();
        // Without a PlaySessionId the DeviceId names the session; without either, the item's
        // unnamed session is reused.
        assert_eq!(start(&mgr, item, "", "DEV-1").await.unwrap().1, d1);
        assert_eq!(start(&mgr, item, "", "").await.unwrap().1, keyless);
        assert_eq!(mgr.sessions.len(), 5);

        // A DeviceId stops all of that device's sessions, with a PlaySessionId or without.
        assert_eq!(mgr.stop_play_session(None, "", "dev-2").await, 2);
        assert!(mgr.segment_path(item, d2, SEGMENT).is_none());
        assert!(mgr.segment_path(item, p2, SEGMENT).is_none());
        assert!(mgr.segment_path(item, d1, SEGMENT).is_some());

        // No names and no item stops nothing.
        assert_eq!(mgr.stop_play_session(None, "", "").await, 0);
        // No names with an item stops only that item's unnamed sessions.
        assert_eq!(mgr.stop_play_session(Some(item), "", "").await, 1);
        assert!(mgr.segment_path(item, keyless, SEGMENT).is_none());
        assert!(mgr.segment_path(item, d1, SEGMENT).is_some());
        assert!(mgr.segment_path(other, other_keyless, SEGMENT).is_some());

        // An item restricts a named stop to that item.
        assert_eq!(mgr.stop_play_session(Some(other), "", "dev-1").await, 0);
        assert!(mgr.segment_path(item, d1, SEGMENT).is_some());

        mgr.stop_all().await;
        assert_eq!(mgr.sessions.len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_a_stopped_play_session_does_not_start_again() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(5));
        let item = Uuid::new_v4();
        start(&mgr, item, "p", "").await.unwrap();
        assert_eq!(mgr.stop_play_session(None, "p", "").await, 1);
        // A request still on its way when the player closed must not start a transcode
        assert!(matches!(
            start(&mgr, item, "p", "").await,
            Err(TranscodeError::Stopped)
        ));
        assert_eq!(mgr.sessions.len(), 0);
        // Another play session still plays
        start(&mgr, item, "q", "").await.unwrap();

        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_a_stop_before_the_first_request_holds() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(5));
        let item = Uuid::new_v4();
        // The player closed before its first request arrived; the stop ignores case, as a
        // start's lookup does
        assert_eq!(mgr.stop_play_session(None, "P", "").await, 0);
        assert!(matches!(
            start(&mgr, item, "p", "").await,
            Err(TranscodeError::Stopped)
        ));
        assert_eq!(mgr.sessions.len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_a_device_stop_does_not_block_the_next_play() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(5));
        let item = Uuid::new_v4();
        start(&mgr, item, "", "d").await.unwrap();
        assert_eq!(mgr.stop_play_session(None, "", "d").await, 1);
        // A client without a PlaySessionId can play again at once
        start(&mgr, item, "", "d").await.unwrap();

        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_a_stop_is_forgotten_after_a_while() {
        let (_dir, mut mgr) = fake_manager("good", Duration::from_secs(5));
        mgr.stop_memory = Duration::from_millis(200);
        let item = Uuid::new_v4();
        start(&mgr, item, "p", "").await.unwrap();
        assert_eq!(mgr.stop_play_session(None, "p", "").await, 1);
        tokio::time::sleep(Duration::from_millis(400)).await;
        start(&mgr, item, "p", "").await.unwrap();

        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_ffmpeg_exit_error_is_returned() {
        let (dir, mgr) = fake_manager("fail", Duration::from_secs(20));
        let item = Uuid::new_v4();
        let started = std::time::Instant::now();
        let err = start(&mgr, item, "psid", "").await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "waited {:?} for an ffmpeg that had exited",
            started.elapsed()
        );
        match err {
            TranscodeError::FfmpegFailed(msg) => {
                assert!(
                    msg.contains("Invalid data found when processing input"),
                    "{msg}"
                )
            }
            other => panic!("expected FfmpegFailed, got {other:?}"),
        }
        // The failed session is gone, files and all.
        assert!(!mgr.is_active(item).await);
        let left: Vec<_> = std::fs::read_dir(dir.0.join("transcodes"))
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "{left:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_playlist_wait_is_bounded() {
        let (_dir, mgr) = fake_manager("slow", Duration::from_secs(1));
        let item = Uuid::new_v4();
        let started = std::time::Instant::now();
        let err = start(&mgr, item, "psid", "").await.unwrap_err();
        let waited = started.elapsed();
        assert!(matches!(err, TranscodeError::PlaylistTimeout), "{err:?}");
        assert!(
            waited >= Duration::from_secs(1) && waited < Duration::from_secs(5),
            "{waited:?}"
        );
        // ffmpeg keeps running, and a retry waits on the same transcode.
        assert!(mgr.is_active(item).await);
        let retry = start(&mgr, item, "psid", "").await;
        assert!(
            matches!(retry, Err(TranscodeError::PlaylistTimeout)),
            "{retry:?}"
        );
        assert_eq!(mgr.sessions.len(), 1);
        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_playlist_wait_runs_while_ffmpeg_makes_progress() {
        // Its first segment takes 3 s, three times the wait; it grows all the while
        let (_dir, mgr) = fake_manager("progressing", Duration::from_secs(1));
        let item = Uuid::new_v4();
        let started = std::time::Instant::now();
        let (playlist, _) = start(&mgr, item, "psid", "").await.unwrap();
        assert!(playlist.ends_with("main.m3u8"));
        assert!(
            started.elapsed() >= Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_shutdown_refuses_new_starts() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        start(&mgr, item, "psid-1", "").await.unwrap();
        mgr.shutdown().await;
        assert!(!mgr.is_active(item).await);
        let after = start(&mgr, item, "psid-2", "").await;
        assert!(
            matches!(after, Err(TranscodeError::ShuttingDown)),
            "{after:?}"
        );
        assert_eq!(mgr.sessions.len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_a_stop_kills_ffmpegs_whole_process_group() {
        let (_dir, mgr) = fake_manager("family", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let (playlist, _) = start(&mgr, item, "psid", "").await.unwrap();
        let pid_file = playlist.parent().unwrap().join("grandchild.pid");
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // A killed grandchild is reparented, and it stays a zombie until its new parent reaps it
        // (CI's init may never do), so a zombie counts as gone
        let zombie = |pid: i32| {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit_once(')')
                    .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
            })
        };
        // SAFETY: signal 0 only asks whether the process exists
        let alive = |pid| unsafe { libc::kill(pid, 0) } == 0 && !zombie(pid);
        assert!(alive(pid));
        mgr.stop_all().await;
        // The grandchild got SIGKILL with its group; give the kernel a moment to end it
        let mut gone = false;
        for _ in 0..50 {
            // Reap it if it became our zombie; otherwise this does nothing
            // SAFETY: waitpid on one pid with WNOHANG never blocks
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
            if !alive(pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone, "the grandchild {pid} outlived the stop");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_retrying_a_hung_ffmpeg_does_not_keep_it_from_cleanup() {
        let (_dir, mgr) = fake_manager("slow", Duration::from_millis(300));
        let item = Uuid::new_v4();
        assert!(start(&mgr, item, "psid", "").await.is_err());
        let first = mgr.sessions.iter().next().unwrap().last_access;
        assert!(start(&mgr, item, "psid", "").await.is_err());
        let after_retry = mgr.sessions.iter().next().unwrap().last_access;
        assert_eq!(first, after_retry, "a retry touched the hung session");
        let removed = mgr
            .cleanup_idle_sessions(Instant::now(), Duration::from_millis(500))
            .await;
        assert_eq!(removed, 1);
        assert!(!mgr.is_active(item).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_playlist_wait_ends_when_ffmpeg_stops_writing() {
        let (_dir, mgr) = fake_manager("stalled", Duration::from_secs(1));
        let item = Uuid::new_v4();
        let started = std::time::Instant::now();
        let err = start(&mgr, item, "psid", "").await.unwrap_err();
        let waited = started.elapsed();
        assert!(matches!(err, TranscodeError::PlaylistTimeout), "{err:?}");
        assert!(
            waited >= Duration::from_secs(1) && waited < Duration::from_secs(4),
            "{waited:?}"
        );
        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_noisy_stderr_does_not_stall() {
        let (_dir, mgr) = fake_manager("noisy", Duration::from_secs(20));
        let item = Uuid::new_v4();
        let (playlist, id) = start(&mgr, item, "psid", "")
            .await
            .expect("ffmpeg must not block on a full stderr pipe");
        assert!(playlist.exists());
        assert!(mgr.segment_path(item, id, SEGMENT).is_some());
        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cleanup_uses_last_access() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let (_, a) = start(&mgr, item, "a", "").await.unwrap();
        let (playlist_b, b) = start(&mgr, item, "b", "").await.unwrap();
        // Two hours on: A was requested a minute ago, B not since it started.
        let now = std::time::Instant::now() + Duration::from_secs(7200);
        mgr.sessions.get_mut(&a).unwrap().last_access = now - Duration::from_secs(60);

        assert_eq!(
            mgr.cleanup_idle_sessions(now, Duration::from_secs(600))
                .await,
            1
        );
        assert!(
            mgr.segment_path(item, a, SEGMENT).is_some(),
            "a session that is still requested stays, however old"
        );
        assert!(mgr.segment_path(item, b, SEGMENT).is_none());
        assert!(!playlist_b.exists());
        mgr.stop_all().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_requests_touch_session() {
        let (_dir, mgr) = fake_manager("good", Duration::from_secs(10));
        let item = Uuid::new_v4();
        let (_, a) = start(&mgr, item, "a", "").await.unwrap();
        let last_access = |mgr: &TranscodeManager| mgr.sessions.get(&a).unwrap().last_access;

        let before = last_access(&mgr);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(mgr.segment_path(item, a, SEGMENT).is_some());
        let after_segment = last_access(&mgr);
        assert!(
            after_segment > before,
            "a segment request touches the session"
        );

        tokio::time::sleep(Duration::from_millis(20)).await;
        start(&mgr, item, "a", "").await.unwrap();
        let after_playlist = last_access(&mgr);
        assert!(
            after_playlist > after_segment,
            "a playlist request touches the session"
        );

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(mgr.legacy_segment_path(item, SEGMENT).is_some());
        assert!(
            last_access(&mgr) > after_playlist,
            "a legacy segment request touches the session"
        );
        mgr.stop_all().await;
    }

    /// A progressive transcode writes to the pipe the caller reads; a stop by PlaySessionId
    /// kills it (the pipe ends), and the idle cleanup and an HLS start of the same play session
    /// leave it alone.
    #[tokio::test]
    async fn r3gaps2_progressive_pipes_and_stops_by_play_session() {
        use tokio::io::AsyncReadExt;
        let (_dir, mgr) = fake_manager("piped", Duration::from_secs(5));
        let item = Uuid::new_v4();
        let profile = TranscodeProfile {
            audio_only: true,
            ..TranscodeProfile::default()
        };
        let (playback, mut stdout) = mgr
            .start_progressive(item, "/in.flac", &profile, "mp3", "ps-1", "dev-1")
            .await
            .unwrap();
        let mut first = [0u8; 7];
        stdout.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"r3gaps2");
        assert!(mgr.is_active(item).await);
        // Idle for longer than the limit: still running, as its response is
        let later = Instant::now() + Duration::from_secs(3600);
        assert_eq!(mgr.cleanup_idle_sessions(later, IDLE_LIMIT).await, 0);
        assert_eq!(mgr.find_session(item, "/in.flac", "ps-1", "dev-1"), None);

        assert_eq!(mgr.stop_play_session(None, "ps-1", "").await, 1);
        let mut rest = Vec::new();
        let read =
            tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut rest)).await;
        assert!(matches!(read, Ok(Ok(0))), "the pipe ends with the stop");
        assert!(!mgr.is_active(item).await);
        assert!(!mgr.end_progressive(playback).await, "already gone");
    }

    /// A new progressive job of the same item and play session (else device) replaces the old
    /// one; other sessions keep theirs, and past the cap a start fails.
    #[tokio::test]
    async fn r3gaps2_progressive_jobs_are_replaced_and_capped() {
        let (_dir, mut mgr) = fake_manager("piped", Duration::from_secs(5));
        mgr.max_progressive = 2;
        let item = Uuid::new_v4();
        let profile = TranscodeProfile::default();
        let start = |ps: &'static str, dev: &'static str| {
            let mgr = mgr.clone();
            let profile = profile.clone();
            async move {
                mgr.start_progressive(item, "/in.flac", &profile, "mp3", ps, dev)
                    .await
                    .map(|(id, _)| id)
            }
        };
        let first = start("ps-a", "").await.unwrap();
        let second = start("ps-a", "").await.unwrap();
        assert!(!mgr.end_progressive(first).await, "the seek replaced it");
        let by_device = start("", "dev-b").await.unwrap();
        // Two run (ps-a's second and dev-b's): a third client is refused
        assert!(matches!(
            start("ps-c", "").await,
            Err(TranscodeError::TooMany(2))
        ));
        // dev-b asking again replaces its own job, within the cap
        let again = start("", "dev-b").await.unwrap();
        assert!(!mgr.end_progressive(by_device).await);
        assert!(mgr.end_progressive(second).await);
        assert!(mgr.end_progressive(again).await);
    }

    /// Ending a progressive transcode (the client went away) kills its ffmpeg.
    #[tokio::test]
    async fn r3gaps2_progressive_ends_on_request() {
        let (_dir, mgr) = fake_manager("piped", Duration::from_secs(5));
        let item = Uuid::new_v4();
        let (playback, _stdout) = mgr
            .start_progressive(
                item,
                "/in.flac",
                &TranscodeProfile::default(),
                "mpegts",
                "",
                "",
            )
            .await
            .unwrap();
        assert!(mgr.end_progressive(playback).await);
        assert!(!mgr.is_active(item).await);
    }

    /// The progressive arguments: the encode, then the muxer to stdout; no HLS options; the
    /// channels and sample rate asked for; no bitrate for a lossless encoder; fragmented MP4.
    #[test]
    fn r3gaps2_progressive_args() {
        let profile = TranscodeProfile {
            audio_only: true,
            audio_codec: "libmp3lame".into(),
            max_audio_bitrate_kbps: 320,
            audio_channels: Some(1),
            audio_sample_rate: Some(44100),
            ..TranscodeProfile::default()
        };
        let args = build_progressive_args("/in.flac", "mp3", &profile);
        assert!(has_pair(&args, "-c:a", "libmp3lame"));
        assert!(has_pair(&args, "-b:a", "320k"));
        assert!(has_pair(&args, "-ac", "1"));
        assert!(has_pair(&args, "-ar", "44100"));
        assert!(has_pair(&args, "-f", "mp3"));
        assert!(args.contains(&"-vn".to_string()));
        assert_eq!(args.last().unwrap(), "pipe:1");
        assert!(!args.iter().any(|a| a.starts_with("-hls")));

        let flac = TranscodeProfile {
            audio_codec: "flac".into(),
            ..profile.clone()
        };
        assert!(!build_progressive_args("/in.flac", "flac", &flac).contains(&"-b:a".to_string()));

        let video = TranscodeProfile::default();
        let args = build_progressive_args("/in.mkv", "mp4", &video);
        assert!(has_pair(
            &args,
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof"
        ));
        assert!(has_pair(&args, "-c:v", "libx264"));
        assert!(has_pair(&args, "-ac", "2"));
        assert!(!args.iter().any(|a| a.starts_with("-force_key_frames")));
        // HLS keeps stereo and no sample rate by default
        let hls = build_ffmpeg_args("/in.mkv", "/o/main.m3u8", "/o/s_%04d.ts", &video);
        assert!(has_pair(&hls, "-ac", "2") && !hls.contains(&"-ar".to_string()));
    }

    #[test]
    fn r3gaps2_progressive_formats_and_encoders() {
        assert_eq!(progressive_format("mp3", true), Some(("mp3", "audio/mpeg")));
        assert_eq!(progressive_format("AAC", true), Some(("adts", "audio/aac")));
        assert_eq!(
            progressive_format("ts", false),
            Some(("mpegts", "video/mp2t"))
        );
        assert_eq!(
            progressive_format("mkv", false),
            Some(("matroska", "video/x-matroska"))
        );
        assert_eq!(progressive_format("mp4", true), Some(("mp4", "audio/mp4")));
        assert_eq!(progressive_format("rm", false), None);
        assert_eq!(audio_encoder("MP3"), Some("libmp3lame"));
        assert_eq!(audio_encoder("dts"), None);
        assert_eq!(video_encoder("hevc"), Some("libx265"));
    }

    // ── r3transcoding: encoding options and hardware acceleration ──

    /// A profile with the server's hardware type and a source video of `codec`.
    fn hw_profile(
        accel: encoding::HardwareAccelerationType,
        codec: &str,
        edit: impl FnOnce(&mut EncodingSettings),
    ) -> TranscodeProfile {
        let mut encoding = EncodingSettings {
            hardware_acceleration: accel,
            ..Default::default()
        };
        edit(&mut encoding);
        TranscodeProfile {
            encoding,
            source_video_codec: Some(codec.to_string()),
            source_bit_depth: Some(8),
            ..Default::default()
        }
    }

    /// The args before the first input.
    fn input_args(args: &[String]) -> Vec<&str> {
        let input = position(args, "-i");
        args[3..input].iter().map(String::as_str).collect()
    }

    #[test]
    fn r3transcoding_vaapi_args_match_jellyfin() {
        use encoding::HardwareAccelerationType::Vaapi;
        let profile = hw_profile(Vaapi, "h264", |s| {
            s.vaapi_device = "/dev/dri/renderD129".into()
        });
        let args = args_for(&profile);
        assert_eq!(
            input_args(&args),
            [
                "-init_hw_device",
                "vaapi=va:/dev/dri/renderD129",
                "-filter_hw_device",
                "va",
                "-hwaccel",
                "vaapi",
                "-hwaccel_output_format",
                "vaapi",
                "-hwaccel_device",
                "va",
                "-noautorotate",
            ]
        );
        assert!(has_pair(&args, "-c:v", "h264_vaapi"), "{args:?}");
        assert!(has_pair(&args, "-b:v", "8000k"), "{args:?}");
        assert!(has_pair(&args, "-vf", HW_SCALES[0].2), "{args:?}");
        assert!(!args.contains(&"-preset".to_string()), "{args:?}");
        assert!(!args.contains(&"-crf".to_string()), "{args:?}");
    }

    #[test]
    fn r3transcoding_qsv_args_match_jellyfin() {
        use encoding::HardwareAccelerationType::Qsv;
        let args = args_for(&hw_profile(Qsv, "h264", |_| {}));
        assert_eq!(
            input_args(&args),
            [
                "-init_hw_device",
                "vaapi=va:/dev/dri/renderD128",
                "-init_hw_device",
                "qsv=qs@va",
                "-filter_hw_device",
                "qs",
                "-hwaccel",
                "qsv",
                "-hwaccel_output_format",
                "qsv",
                "-hwaccel_device",
                "qs",
                "-noautorotate",
            ]
        );
        assert!(has_pair(&args, "-c:v", "h264_qsv"), "{args:?}");
        assert!(has_pair(&args, "-vf", HW_SCALES[2].2), "{args:?}");
        assert_eq!(values(&args, "-preset"), ["veryfast"]);
        // QsvDevice names the render node
        let args = args_for(&hw_profile(Qsv, "h264", |s| {
            s.qsv_device = "/dev/dri/renderD129".into()
        }));
        assert!(has_pair(
            &args,
            "-init_hw_device",
            "vaapi=va:/dev/dri/renderD129"
        ));
    }

    #[test]
    fn r3transcoding_nvenc_args_match_jellyfin() {
        use encoding::HardwareAccelerationType::Nvenc;
        let args = args_for(&hw_profile(Nvenc, "h264", |s| {
            s.encoder_preset = "slow".into()
        }));
        assert_eq!(
            input_args(&args),
            [
                "-init_hw_device",
                "cuda=cu:0",
                "-filter_hw_device",
                "cu",
                "-hwaccel",
                "cuda",
                "-hwaccel_output_format",
                "cuda",
                "-hwaccel_device",
                "cu",
                "-noautorotate",
            ]
        );
        assert!(has_pair(&args, "-c:v", "h264_nvenc"), "{args:?}");
        assert!(has_pair(&args, "-vf", HW_SCALES[1].2), "{args:?}");
        assert_eq!(values(&args, "-preset"), ["p5"]);
    }

    #[test]
    fn r3transcoding_unlisted_codec_decodes_in_software_and_uploads() {
        use encoding::HardwareAccelerationType::{Nvenc, Qsv, Vaapi};
        let sw = format!("{SW_SCALE},format=yuv420p");
        for (accel, encoder, after) in [
            (Vaapi, "h264_vaapi", ",format=nv12,hwupload"),
            (Qsv, "h264_qsv", ",format=nv12"),
            (Nvenc, "h264_nvenc", ",format=nv12"),
        ] {
            // HEVC is not in the default HardwareDecodingCodecs (h264, vc1)
            let args = args_for(&hw_profile(accel, "hevc", |_| {}));
            assert!(
                !args.contains(&"-hwaccel".to_string()),
                "{accel:?}: {args:?}"
            );
            assert!(args.contains(&"-init_hw_device".to_string()), "{accel:?}");
            assert!(has_pair(&args, "-c:v", encoder), "{accel:?}: {args:?}");
            assert!(
                has_pair(&args, "-vf", &format!("{sw}{after}")),
                "{accel:?}: {args:?}"
            );
        }
    }

    #[test]
    fn r3transcoding_10bit_decode_follows_the_color_depth_options() {
        use encoding::HardwareAccelerationType::Vaapi;
        for (codec, on) in [
            ("hevc", true),
            ("hevc", false),
            ("vp9", true),
            ("vp9", false),
        ] {
            let mut profile = hw_profile(Vaapi, codec, |s| {
                s.hardware_decoding_codecs = vec!["hevc".into(), "vp9".into()];
                s.enable_decoding_color_depth_10_hevc = on;
                s.enable_decoding_color_depth_10_vp9 = on;
            });
            profile.source_bit_depth = Some(10);
            let args = args_for(&profile);
            assert_eq!(
                args.contains(&"-hwaccel".to_string()),
                on,
                "{codec} {on}: {args:?}"
            );
            // 8-bit decodes on the device either way
            profile.source_bit_depth = Some(8);
            assert!(args_for(&profile).contains(&"-hwaccel".to_string()));
        }
    }

    #[test]
    fn r3transcoding_hardware_encoding_off_downloads_for_x264() {
        use encoding::HardwareAccelerationType::Vaapi;
        let args = args_for(&hw_profile(Vaapi, "h264", |s| {
            s.enable_hardware_encoding = false
        }));
        assert!(args.contains(&"-hwaccel".to_string()), "{args:?}");
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(has_pair(&args, "-crf", "23"), "{args:?}");
        assert!(
            has_pair(
                &args,
                "-vf",
                &format!("{},hwdownload,format=nv12", HW_SCALES[0].2)
            ),
            "{args:?}"
        );
        assert_eq!(values(&args, "-preset"), ["veryfast"]);
        // Neither decoded nor encoded on the device: no device at all
        let args = args_for(&hw_profile(Vaapi, "hevc", |s| {
            s.enable_hardware_encoding = false
        }));
        assert!(!args.contains(&"-init_hw_device".to_string()), "{args:?}");
    }

    #[test]
    fn r3transcoding_stored_only_types_transcode_in_software() {
        use encoding::HardwareAccelerationType::*;
        let software = args_for(&hw_profile(None, "h264", |_| {}));
        for accel in [VideoToolbox, Amf, Rkmpp, V4l2m2m] {
            assert_eq!(
                args_for(&hw_profile(accel, "h264", |_| {})),
                software,
                "{accel:?}"
            );
        }
    }

    #[test]
    fn r3transcoding_options_reach_the_args() {
        let profile = TranscodeProfile {
            encoding: EncodingSettings {
                thread_count: 4,
                h264_crf: 18,
                encoder_preset: "slow".into(),
                max_muxing_queue_size: 4096,
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-threads", "4"), "{args:?}");
        assert!(has_pair(&args, "-crf", "18"), "{args:?}");
        assert!(!args.contains(&"-b:v".to_string()), "{args:?}");
        assert!(has_pair(&args, "-maxrate", "8000k"), "{args:?}");
        assert_eq!(values(&args, "-preset"), ["slow"]);
        assert!(
            has_pair(&args, "-max_muxing_queue_size", "4096"),
            "{args:?}"
        );
        // The default thread count lets ffmpeg choose
        let args = args_for(&TranscodeProfile::default());
        assert!(!args.contains(&"-threads".to_string()), "{args:?}");
        // Progressive output takes them too; audio-only has no muxing queue or preset
        let piped = build_progressive_args("/in.mkv", "matroska", &profile);
        assert!(has_pair(&piped, "-crf", "18") && has_pair(&piped, "-preset", "slow"));
        assert!(has_pair(&piped, "-max_muxing_queue_size", "4096"));
        let audio = TranscodeProfile {
            audio_only: true,
            ..profile.clone()
        };
        let piped = build_progressive_args("/in.mkv", "mp3", &audio);
        assert!(!piped.contains(&"-max_muxing_queue_size".to_string()));
        assert!(!piped.contains(&"-preset".to_string()));
    }

    #[test]
    fn r3transcoding_downmix_reaches_the_audio_args() {
        let mut profile = TranscodeProfile {
            source_audio_channels: Some(6),
            ..Default::default()
        };
        assert!(has_pair(&args_for(&profile), "-af", "volume=2"));
        profile.encoding.downmix_stereo_algorithm = "NightmodeDialogue".into();
        profile.encoding.downmix_audio_boost = 1.0;
        assert!(has_pair(
            &args_for(&profile),
            "-af",
            "pan=stereo|c0=c2+0.30*c0+0.30*c4|c1=c2+0.30*c1+0.30*c5"
        ));
        // No downmix: stereo source, six output channels, or a copied track
        for edit in [
            |p: &mut TranscodeProfile| p.source_audio_channels = Some(2),
            |p: &mut TranscodeProfile| p.audio_channels = Some(6),
            |p: &mut TranscodeProfile| p.copy_audio = true,
        ] {
            let mut p = profile.clone();
            edit(&mut p);
            assert!(!args_for(&p).contains(&"-af".to_string()));
        }
    }

    #[test]
    fn r3transcoding_hevc_encode_in_fmp4_gets_the_hvc1_tag() {
        let profile = TranscodeProfile {
            video_codec: "libx265".into(),
            fmp4: true,
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-c:v", "libx265"), "{args:?}");
        assert!(has_pair(&args, "-crf", "28"), "{args:?}");
        assert!(has_pair(&args, "-tag:v", "hvc1"), "{args:?}");
        // On the device the HEVC encoder is the device's
        let profile = TranscodeProfile {
            hw_accel: HwAccel::Nvenc,
            ..profile
        };
        assert!(has_pair(&args_for(&profile), "-c:v", "hevc_nvenc"));
    }

    #[test]
    fn r3transcoding_software_tonemap_args() {
        use encoding::HardwareAccelerationType::Vaapi;
        let tonemap = encoding::software_tonemap_filter(
            &EncodingSettings::default(),
            encoding::HdrTransfer::Pq,
        );
        let mut profile = hw_profile(Vaapi, "hevc", |s| {
            s.enable_tonemapping = true;
            s.hardware_decoding_codecs = vec!["hevc".into()];
        });
        // SDR: no tonemap, on the device
        let args = args_for(&profile);
        assert!(!args.iter().any(|a| a.contains("tonemap")), "{args:?}");
        assert!(args.contains(&"-hwaccel".to_string()));
        // HDR: tonemapped in software after the scale
        profile.source_hdr = Some(encoding::HdrTransfer::Pq);
        let args = args_for(&profile);
        assert!(
            has_pair(&args, "-vf", &format!("{SW_SCALE},{tonemap}")),
            "{args:?}"
        );
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(!args.contains(&"-hwaccel".to_string()), "{args:?}");
        // Tonemapping off: HDR goes through as before
        profile.encoding.enable_tonemapping = false;
        assert!(!args_for(&profile).iter().any(|a| a.contains("tonemap")));
    }

    /// The software tonemap runs in the installed ffmpeg on a PQ (HDR10) source.
    #[test]
    fn r3transcoding_software_tonemap_runs_in_ffmpeg() {
        if !have_ffmpeg("r3transcoding_software_tonemap_runs_in_ffmpeg") {
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3tm-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let input = dir.0.join("hdr.mkv");
        let make = "-f lavfi -i testsrc2=size=320x240:rate=25 -t 2 -c:v libx265 \
                    -x265-params log-level=error -pix_fmt yuv420p10le -color_primaries bt2020 \
                    -color_trc smpte2084 -colorspace bt2020nc";
        let mut args: Vec<String> = make.split_whitespace().map(String::from).collect();
        args.push(input.to_str().unwrap().to_string());
        run_ffmpeg(&args, true);
        let profile = TranscodeProfile {
            source_hdr: Some(encoding::HdrTransfer::Pq),
            encoding: EncodingSettings {
                enable_tonemapping: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = transcode_into(&dir.0, "sdr", &input, &profile);
        assert!(!segments_in(&out).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r3transcoding_failed_hardware_falls_back_to_software() {
        let (_dir, mgr) = fake_manager("hwfail", Duration::from_secs(10));
        // What the build offers is known here, so no probe runs the fake
        mgr.capabilities.set(FfmpegCapabilities::default()).unwrap();
        mgr.set_encoding_settings(EncodingSettings {
            hardware_acceleration: encoding::HardwareAccelerationType::Vaapi,
            ..Default::default()
        });
        let profile = TranscodeProfile::default();
        let hw = |input: &'static str| {
            let mgr = mgr.clone();
            let profile = profile.clone();
            async move { uses_hardware(&mgr.prepare(&profile, input).await) }
        };
        let item = Uuid::new_v4();
        let (_, id) = start(&mgr, item, "psid-a", "dev").await.unwrap();
        let used = mgr.sessions.get(&id).unwrap().profile.clone();
        assert!(!uses_hardware(&used), "{used:?}");
        // That file runs in software from then on; other files still try the device
        assert!(!hw("/in.mkv").await);
        assert!(hw("/other.mkv").await);
        // The device failing to open on three files in a row turns it off for every file
        for input in ["/b.mkv", "/c.mkv"] {
            mgr.start_transcode(Uuid::new_v4(), input, &profile, "", "")
                .await
                .unwrap();
        }
        assert!(mgr.hardware_health().off);
        assert!(!hw("/other.mkv").await);
        // Saving the options tries the hardware again
        mgr.set_encoding_settings(mgr.encoding_settings());
        assert!(hw("/in.mkv").await && hw("/other.mkv").await);
        mgr.stop_all().await;
    }

    /// A progressive hardware transcode that fails, or writes nothing in time, runs again in
    /// software, and that file stays in software; a software one that writes nothing isn't
    /// retried.
    #[cfg(unix)]
    #[tokio::test]
    async fn r3hwaccel_progressive_hardware_failure_retries_in_software() {
        for script in ["hwfail-piped", "hwhang-piped"] {
            let (_dir, mgr) = fake_manager(script, Duration::from_secs(10));
            mgr.capabilities.set(FfmpegCapabilities::default()).unwrap();
            mgr.set_encoding_settings(EncodingSettings {
                hardware_acceleration: encoding::HardwareAccelerationType::Vaapi,
                ..Default::default()
            });
            let profile = TranscodeProfile::default();
            let (id, _stdout, first) = mgr
                .start_progressive_first(
                    Uuid::new_v4(),
                    "/in.mkv",
                    &profile,
                    "matroska",
                    "ps-hw",
                    "dev",
                    Duration::from_secs(2),
                )
                .await
                .unwrap_or_else(|e| panic!("{script}: {e}"));
            assert_eq!(first, b"r3gaps2", "{script}");
            assert!(!mgr.progressive_uses_hardware(id), "{script}");
            assert!(mgr.hardware_health().failed_inputs.contains("/in.mkv"));
            mgr.stop_all().await;
        }
        // Software that writes nothing in time fails without a retry
        let (_dir, mgr) = fake_manager("slow", Duration::from_secs(10));
        let started = mgr
            .start_progressive_first(
                Uuid::new_v4(),
                "/in.mkv",
                &TranscodeProfile::default(),
                "matroska",
                "ps-sw",
                "dev",
                Duration::from_millis(500),
            )
            .await;
        assert!(matches!(started, Err(TranscodeError::PlaylistTimeout)));
        mgr.stop_all().await;
    }

    #[test]
    fn r3transcoding_one_file_or_session_limit_never_turns_hardware_off() {
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3h-{}", Uuid::new_v4())));
        let mgr = TranscodeManager::new(&dir.0);
        // The NVENC session limit and an undecodable file are not the device failing to open
        for i in 0..10 {
            mgr.note_hardware_result(
                &format!("/f{i}.mkv"),
                Some("OpenEncodeSessionEx failed: incompatible client key (21)"),
            );
            mgr.note_hardware_result("/bad.mkv", Some("Invalid data found when processing input"));
        }
        assert!(!mgr.hardware_health().off);
        assert_eq!(mgr.hardware_health().device_failures, 0);
        // Device failures count only in a row: a hardware start resets them
        let device = "Device creation failed: -22.";
        mgr.note_hardware_result("/a.mkv", Some(device));
        mgr.note_hardware_result("/b.mkv", Some(device));
        mgr.note_hardware_result("/ok.mkv", None);
        mgr.note_hardware_result("/c.mkv", Some(device));
        mgr.note_hardware_result("/d.mkv", Some(device));
        assert!(!mgr.hardware_health().off);
        mgr.note_hardware_result("/e.mkv", Some(device));
        assert!(mgr.hardware_health().off);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn r3transcoding_a_stop_while_starting_on_hardware_is_not_retried() {
        let (_dir, mgr) = fake_manager("slow", Duration::from_secs(10));
        mgr.capabilities.set(FfmpegCapabilities::default()).unwrap();
        mgr.set_encoding_settings(EncodingSettings {
            hardware_acceleration: encoding::HardwareAccelerationType::Vaapi,
            ..Default::default()
        });
        let item = Uuid::new_v4();
        // No PlaySessionId: the stop goes by device, and nothing refuses a later start
        let starting = {
            let mgr = mgr.clone();
            tokio::spawn(async move { start(&mgr, item, "", "dev-r3").await })
        };
        while mgr.sessions.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(mgr.stop_play_session(None, "", "dev-r3").await, 1);
        let result = starting.await.unwrap();
        assert!(matches!(result, Err(TranscodeError::Stopped)), "{result:?}");
        // No software transcode started after the stop, and the file isn't marked as failing
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(mgr.sessions.len(), 0);
        assert!(mgr.hardware_health().failed_inputs.is_empty());
    }

    #[tokio::test]
    async fn r3transcoding_missing_encoder_or_filter_falls_back() {
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3c-{}", Uuid::new_v4())));
        let mgr = TranscodeManager::new(&dir.0);
        mgr.capabilities
            .set(FfmpegCapabilities {
                hwaccels: Some(vec!["vaapi".into()]),
                encoders: Some(["libx264".to_string()].into()),
                filters: Some(["scale".to_string()].into()),
            })
            .unwrap();
        mgr.set_encoding_settings(EncodingSettings {
            hardware_acceleration: encoding::HardwareAccelerationType::Vaapi,
            enable_tonemapping: true,
            ..Default::default()
        });
        let p = mgr.prepare(&TranscodeProfile::default(), "/in.mkv").await;
        assert!(uses_hardware(&p) && !p.encoding.enable_hardware_encoding);
        let hdr = TranscodeProfile {
            source_hdr: Some(encoding::HdrTransfer::Hlg),
            ..Default::default()
        };
        assert!(
            !mgr.prepare(&hdr, "/in.mkv")
                .await
                .encoding
                .enable_tonemapping
        );
        // No vaapi hwaccel at all: software
        let mgr = TranscodeManager::new(&dir.0);
        mgr.capabilities
            .set(FfmpegCapabilities {
                hwaccels: Some(vec!["cuda".into()]),
                ..Default::default()
            })
            .unwrap();
        mgr.set_encoding_settings(EncodingSettings {
            hardware_acceleration: encoding::HardwareAccelerationType::Vaapi,
            ..Default::default()
        });
        assert!(!uses_hardware(
            &mgr.prepare(&TranscodeProfile::default(), "/in.mkv").await
        ));
    }

    #[test]
    fn r3transcoding_10bit_h264_decodes_in_software() {
        use encoding::HardwareAccelerationType::{Nvenc, Qsv, Vaapi};
        for accel in [Vaapi, Qsv, Nvenc] {
            let mut profile = hw_profile(accel, "h264", |_| {});
            profile.source_bit_depth = Some(10);
            let args = args_for(&profile);
            assert!(
                !args.contains(&"-hwaccel".to_string()),
                "{accel:?}: {args:?}"
            );
            // Still encoded on the device
            assert!(
                args.iter().any(|a| a.starts_with("h264_")),
                "{accel:?}: {args:?}"
            );
            profile.source_bit_depth = Some(8);
            assert!(args_for(&profile).contains(&"-hwaccel".to_string()));
        }
    }

    #[test]
    fn r3transcoding_tonemapping_never_turns_a_copy_off() {
        let profile = TranscodeProfile {
            copy_video: true,
            source_hdr: Some(encoding::HdrTransfer::Pq),
            encoding: EncodingSettings {
                enable_tonemapping: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-c:v", "copy"), "{args:?}");
        assert!(!args.iter().any(|a| a.contains("tonemap")), "{args:?}");
        // Encoded anyway (a burn-in), the HDR video is tonemapped
        let burned = TranscodeProfile {
            burn_subtitles: true,
            ..profile
        };
        let args = args_for(&burned);
        assert!(has_pair(&args, "-c:v", "libx264"), "{args:?}");
        assert!(
            args.iter().any(|a| a.contains("tonemap=tonemap")),
            "{args:?}"
        );
    }

    #[test]
    fn r3transcoding_svtav1_gets_no_maxrate() {
        let profile = TranscodeProfile {
            video_codec: "libsvtav1".into(),
            fmp4: true,
            ..Default::default()
        };
        let args = args_for(&profile);
        assert!(has_pair(&args, "-b:v", "8000k"), "{args:?}");
        assert!(has_pair(&args, "-bufsize", "16000k"), "{args:?}");
        assert!(!args.contains(&"-maxrate".to_string()), "{args:?}");
        assert_eq!(values(&args, "-preset"), ["8"]);
    }

    /// The SVT-AV1 arguments encode in the installed ffmpeg; with `-maxrate` equal to `-b:v`
    /// (CBR) libsvtav1 refuses to start.
    #[test]
    fn r3transcoding_svtav1_args_encode_in_ffmpeg() {
        if !have_ffmpeg("r3transcoding_svtav1_args_encode_in_ffmpeg") {
            return;
        }
        let encoders = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .unwrap();
        if !String::from_utf8_lossy(&encoders.stdout).contains("libsvtav1") {
            eprintln!("skipped: ffmpeg has no libsvtav1");
            return;
        }
        let dir = TestDir(std::env::temp_dir().join(format!("rustyfin-r3av1-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let input = dir.0.join("in.mp4");
        let make = "-f lavfi -i testsrc2=size=320x240:rate=25 -f lavfi -i sine=frequency=440 -t 2 \
                    -c:v libx264 -pix_fmt yuv420p -c:a aac";
        let mut args: Vec<String> = make.split_whitespace().map(String::from).collect();
        args.push(input.to_str().unwrap().to_string());
        run_ffmpeg(&args, true);
        let profile = TranscodeProfile {
            video_codec: "libsvtav1".into(),
            fmp4: true,
            max_video_bitrate_kbps: 1000,
            ..Default::default()
        };
        let out = transcode_into(&dir.0, "av1", &input, &profile);
        assert!(out.join("init.mp4").is_file());
    }

    #[test]
    fn r3transcoding_device_scale_keeps_the_aspect() {
        use encoding::HardwareAccelerationType::{Nvenc, Qsv, Vaapi};
        // A 2.4:1 4K source within 1920x1080 is 1920x800 on every device
        for (accel, scale) in [
            (Vaapi, "scale_vaapi=w=1920:h=800:format=nv12"),
            (Qsv, "scale_qsv=w=1920:h=800:format=nv12"),
            (Nvenc, "scale_cuda=w=1920:h=800:format=nv12"),
        ] {
            let mut profile = hw_profile(accel, "h264", |_| {});
            profile.source_size = Some((3840, 1600));
            let args = args_for(&profile);
            assert!(has_pair(&args, "-vf", scale), "{accel:?}: {args:?}");
        }
        // A small source is not scaled up
        let mut profile = hw_profile(Qsv, "h264", |_| {});
        profile.source_size = Some((1280, 534));
        assert!(has_pair(
            &args_for(&profile),
            "-vf",
            "scale_qsv=w=1280:h=534:format=nv12"
        ));
    }
}
