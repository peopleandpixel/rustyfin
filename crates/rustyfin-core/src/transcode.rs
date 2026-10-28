use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

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

#[derive(Debug, Clone)]
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
    /// Audio stream index to transcode (for multi-audio support)
    pub audio_stream_index: Option<u32>,
    /// Whether to transcode audio only (no video)
    pub audio_only: bool,
    /// Enable adaptive bitrate transcoding with multiple quality levels
    pub adaptive_bitrate: bool,
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
        }
    }
}

// ---------------------------------------------------------------------------
// Transcode session
// ---------------------------------------------------------------------------

struct TranscodeSession {
    _item_id: Uuid,
    playback_id: Uuid,
    output_dir: PathBuf,
    child: Option<Child>,
    started_at: std::time::Instant,
    _profile: TranscodeProfile,
}

// ---------------------------------------------------------------------------
// Transcode manager
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TranscodeManager {
    sessions: Arc<RwLock<HashMap<Uuid, TranscodeSession>>>,
    cache_root: PathBuf,
}

impl TranscodeManager {
    pub fn new(data_dir: &Path) -> Self {
        let cache_root = data_dir.join("transcodes");
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            cache_root,
        }
    }

    /// Start an HLS transcode for the given media file.
    /// Returns the path to the main.m3u8 playlist.
    pub async fn start_transcode(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
    ) -> Result<(PathBuf, Uuid), TranscodeError> {
        // Generate a unique playback ID for this session
        let playback_id = Uuid::new_v4();

        // Check if session already exists
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(&item_id) {
                let playlist = session.output_dir.join("main.m3u8");
                if playlist.exists() {
                    return Ok((playlist, session.playback_id));
                }
            }
        }

        // Stop any existing session for this item
        self.stop_transcode(item_id).await;

        let output_dir = self.cache_root.join(item_id.to_string());
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| TranscodeError::Io(format!("Cannot create output dir: {e}")))?;

        let playlist_path = output_dir.join("main.m3u8");
        // Use Jellyfin-compatible segment naming: segment_{id}.ts
        let segment_pattern = output_dir.join("segment_%04d.ts");

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

        let child = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| TranscodeError::FfmpegNotFound(e.to_string()))?;

        let session = TranscodeSession {
            _item_id: item_id,
            playback_id,
            output_dir: output_dir.clone(),
            child: Some(child),
            started_at: std::time::Instant::now(),
            _profile: profile.clone(),
        };

        self.sessions.write().await.insert(item_id, session);

        // Wait briefly for FFmpeg to create the playlist
        for _ in 0..30 {
            if playlist_path.exists() {
                return Ok((playlist_path, playback_id));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        Ok((playlist_path, playback_id))
    }

    /// Stop a running transcode session and clean up.
    pub async fn stop_transcode(&self, item_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(mut session) = sessions.remove(&item_id)
            && let Some(mut child) = session.child.take()
        {
            let _ = child.kill().await;
            debug!(item_id = %item_id, "Killed FFmpeg process");
        }
    }

    /// Stop all running transcodes.
    pub async fn stop_all(&self) {
        let mut sessions = self.sessions.write().await;
        for (id, mut session) in sessions.drain() {
            if let Some(mut child) = session.child.take() {
                let _ = child.kill().await;
                debug!(item_id = %id, "Killed FFmpeg process");
            }
        }
    }

    /// Get the path to a specific HLS segment file.
    pub fn get_segment_path(&self, item_id: Uuid, segment_name: &str) -> PathBuf {
        self.cache_root.join(item_id.to_string()).join(segment_name)
    }

    /// Get the playback ID for an active transcode session.
    pub async fn get_playback_id(&self, item_id: Uuid) -> Option<Uuid> {
        self.sessions
            .read()
            .await
            .get(&item_id)
            .map(|s| s.playback_id)
    }

    /// Get the path to the master playlist.
    pub fn get_playlist_path(&self, item_id: Uuid) -> PathBuf {
        self.cache_root.join(item_id.to_string()).join("main.m3u8")
    }

    /// Check if a transcode session is active for the given item.
    pub async fn is_active(&self, item_id: Uuid) -> bool {
        self.sessions.read().await.contains_key(&item_id)
    }

    /// Clean up transcode cache: remove directories for sessions that are no longer active.
    pub async fn cleanup_cache(&self) -> Result<(), TranscodeError> {
        let active_ids: Vec<Uuid> = { self.sessions.read().await.keys().copied().collect() };

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
                info!(item_id = %id, "Cleaning up transcode cache");
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }

        Ok(())
    }

    /// Clean up stale sessions (running longer than max_duration).
    pub async fn cleanup_stale_sessions(&self, max_duration: Duration) {
        let stale: Vec<Uuid> = {
            let sessions = self.sessions.read().await;
            sessions
                .iter()
                .filter(|(_, s)| s.started_at.elapsed() > max_duration)
                .map(|(id, _)| *id)
                .collect()
        };

        for id in stale {
            warn!(item_id = %id, "Stopping stale transcode session");
            self.stop_transcode(id).await;
        }
    }
}

// ---------------------------------------------------------------------------
// FFmpeg argument builder
// ---------------------------------------------------------------------------

fn build_ffmpeg_args(
    input: &str,
    playlist_output: &str,
    segment_pattern: &str,
    profile: &TranscodeProfile,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    // Overwrite output without asking
    args.push("-y".to_string());

    // Hardware acceleration input flags (skip for audio-only)
    if !profile.audio_only {
        match profile.hw_accel {
            HwAccel::Vaapi => {
                args.extend([
                    "-hwaccel".to_string(),
                    "vaapi".to_string(),
                    "-hwaccel_output_format".to_string(),
                    "vaapi".to_string(),
                    "-vaapi_device".to_string(),
                    "/dev/dri/renderD128".to_string(),
                ]);
            }
            HwAccel::Nvenc => {
                args.extend([
                    "-hwaccel".to_string(),
                    "cuda".to_string(),
                    "-hwaccel_output_format".to_string(),
                    "cuda".to_string(),
                ]);
            }
            HwAccel::Qsv => {
                args.extend([
                    "-hwaccel".to_string(),
                    "qsv".to_string(),
                    "-hwaccel_output_format".to_string(),
                    "qsv".to_string(),
                ]);
            }
            HwAccel::None => {}
        }
    }

    // Input
    args.extend(["-i".to_string(), input.to_string()]);

    if profile.audio_only {
        // Audio-only transcoding: no video stream
        args.extend([
            "-an".to_string(), // no video (redundant with -vn, but explicit)
            "-vn".to_string(), // disable video
        ]);
    } else {
        // Video codec
        let video_codec = match profile.hw_accel {
            HwAccel::Vaapi => "h264_vaapi".to_string(),
            HwAccel::Nvenc => "h264_nvenc".to_string(),
            HwAccel::Qsv => "h264_qsv".to_string(),
            HwAccel::None => profile.video_codec.clone(),
        };
        args.extend(["-c:v".to_string(), video_codec]);

        // Video bitrate & resolution
        args.extend([
            "-b:v".to_string(),
            format!("{}k", profile.max_video_bitrate_kbps),
            "-maxrate".to_string(),
            format!("{}k", profile.max_video_bitrate_kbps),
            "-bufsize".to_string(),
            format!("{}k", profile.max_video_bitrate_kbps * 2),
        ]);

        // Scale filter
        let scale_filter = match profile.hw_accel {
            HwAccel::Vaapi => format!(
                "scale_vaapi=w='min({},iw)':h='min({},ih)':force_original_aspect_ratio=decrease",
                profile.max_width, profile.max_height
            ),
            HwAccel::Nvenc => format!(
                "scale_cuda=w='min({},iw)':h='min({},ih)':force_original_aspect_ratio=decrease",
                profile.max_width, profile.max_height
            ),
            HwAccel::Qsv => format!(
                "scale_qsv=w='min({},iw)':h='min({},ih)'",
                profile.max_width, profile.max_height
            ),
            HwAccel::None => format!(
                "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
                profile.max_width, profile.max_height
            ),
        };

        // Subtitle burn-in
        if profile.burn_subtitles {
            let sub_idx = profile.subtitle_stream_index.unwrap_or(0);
            if profile.hw_accel == HwAccel::None {
                // Software: combine scale + subtitles in one filter chain
                let sw_scale = format!(
                    "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
                    profile.max_width, profile.max_height
                );
                args.extend([
                    "-vf".to_string(),
                    format!("{sw_scale},subtitles='{input}':si={sub_idx}"),
                ]);
            } else {
                // HW accel: need to download → burn subs → upload (complex, use sw fallback for subs)
                args.extend([
                    "-vf".to_string(),
                    format!(
                        "{scale_filter},hwdownload,format=nv12,subtitles='{input}':si={sub_idx}",
                    ),
                ]);
            }
        } else {
            args.extend(["-vf".to_string(), scale_filter]);
        }
    }

    // Audio stream selection and encoding
    if let Some(audio_idx) = profile.audio_stream_index {
        // Select specific audio stream for multi-audio support
        args.extend(["-map".to_string(), format!("0:a:{}", audio_idx)]);
    }

    args.extend([
        "-c:a".to_string(),
        profile.audio_codec.clone(),
        "-b:a".to_string(),
        format!("{}k", profile.max_audio_bitrate_kbps),
        "-ac".to_string(),
        "2".to_string(),
    ]);

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
        "-hls_segment_type".to_string(),
        "mpegts".to_string(),
        "-hls_segment_filename".to_string(),
        segment_pattern.to_string(),
        "-hls_playlist_type".to_string(),
        "event".to_string(),
        "-start_number".to_string(),
        "0".to_string(),
    ]);

    // Preset for speed (skip for HW accel)
    if profile.hw_accel == HwAccel::None && !profile.audio_only {
        args.extend(["-preset".to_string(), "veryfast".to_string()]);
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
