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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// the video copy and hardware acceleration off.
    pub image_subtitle: Option<ImageSubtitle>,
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
    /// The PlaySessionIds stopped lately (in ASCII lower case) and when each was stopped
    stopped: Arc<std::sync::Mutex<HashMap<String, Instant>>>,
    stop_memory: Duration, // 60 s; unit tests shorten it
}

impl TranscodeManager {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            cache_root: data_dir.join("transcodes"),
            ffmpeg: PathBuf::from("ffmpeg"),
            playlist_wait: Duration::from_secs(30),
            start_lock: Arc::new(tokio::sync::Mutex::new(())),
            stopped: Arc::new(std::sync::Mutex::new(HashMap::new())),
            stop_memory: Duration::from_secs(60),
        }
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
    /// exits first, or when no playlist appears within `playlist_wait`; ffmpeg then keeps running.
    /// A play session stopped less than `stop_memory` ago fails at once with `Stopped`.
    pub async fn start_transcode(
        &self,
        item_id: Uuid,
        input_path: &str,
        profile: &TranscodeProfile,
        play_session_id: &str,
        device_id: &str,
    ) -> Result<(PathBuf, Uuid), TranscodeError> {
        let playback_id = {
            let _lock = self.start_lock.lock().await;
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

        // Poll without the lock, so other requests can start their transcodes meanwhile
        let deadline = std::time::Instant::now() + self.playlist_wait;
        loop {
            match self.poll_session(playback_id) {
                StartPoll::Ready(playlist) => return Ok((playlist, playback_id)),
                StartPoll::Exited(status) => {
                    return Err(self.ffmpeg_exited(playback_id, status).await);
                }
                StartPoll::Gone => {
                    return Err(TranscodeError::FfmpegFailed("transcode stopped".into()));
                }
                StartPoll::Running => {}
            }
            if std::time::Instant::now() >= deadline {
                return Err(TranscodeError::PlaylistTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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
                session.item_id == item_id
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

    /// Touch a session that can still serve (its playlist exists or its ffmpeg runs) and
    /// return true. Returns false for any other session.
    fn touch_live_session(&self, playback_id: Uuid) -> bool {
        let Some(mut session) = self.sessions.get_mut(&playback_id) else {
            return false;
        };
        let running = session
            .child
            .as_mut()
            .is_some_and(|child| !matches!(child.try_wait(), Ok(Some(_))));
        let live = running || session.output_dir.join("main.m3u8").exists();
        if live {
            session.last_access = Instant::now();
        }
        live
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

        let spawned = Command::new(&self.ffmpeg)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
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
            },
        );
        Ok(playback_id)
    }

    /// ffmpeg exited before writing a playlist: drop the session and return its last stderr lines.
    async fn ffmpeg_exited(&self, playback_id: Uuid, status: ExitStatus) -> TranscodeError {
        let Some((stderr_task, tail)) = self
            .sessions
            .get_mut(&playback_id)
            .map(|mut s| (s.stderr_task.take(), s.stderr_tail.clone()))
        else {
            return TranscodeError::FfmpegFailed("transcode stopped".into());
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
    pub fn legacy_segment_path(&self, item_id: Uuid, segment_name: &str) -> Option<PathBuf> {
        let latest = self
            .sessions
            .iter()
            .filter(|e| e.value().item_id == item_id)
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
            .filter(|e| now.saturating_duration_since(e.value().last_access) > max_idle)
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

fn build_ffmpeg_args(
    input: &str,
    playlist_output: &str,
    segment_pattern: &str,
    profile: &TranscodeProfile,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    // Overwrite output without asking; keep the banner and progress lines out of stderr
    args.push("-y".to_string());
    args.extend(["-hide_banner".to_string(), "-nostats".to_string()]);

    // An image subtitle is overlaid in software, so it turns hardware acceleration off
    let image_subtitle = profile.image_subtitle.filter(|_| !profile.audio_only);
    let hw_accel = if image_subtitle.is_some() {
        HwAccel::None
    } else {
        profile.hw_accel
    };

    // Burn-in and the overlay need a re-encode, so they turn the video copy off
    let copy_video = profile.copy_video
        && !profile.audio_only
        && !profile.burn_subtitles
        && image_subtitle.is_none();

    // Hardware acceleration input flags (skip for audio-only and copied video)
    if !profile.audio_only && !copy_video {
        match hw_accel {
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

    // Resume or seek point: an input seek, so output time 0 is the start point
    if profile.start_time_ticks > 0 {
        args.extend(["-ss".to_string(), seconds_text(profile.start_time_ticks)]);
    }

    // Input
    args.extend(["-i".to_string(), input.to_string()]);

    if profile.audio_only {
        // Audio-only transcoding: drop the video only. -an would drop the audio too.
        args.push("-vn".to_string());
    } else if copy_video {
        args.extend(["-c:v".to_string(), "copy".to_string()]);
    } else {
        // Video codec
        let video_codec = match hw_accel {
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
        let scale_filter = match hw_accel {
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

        // Subtitle burn-in: an image subtitle is overlaid, and it wins over a text burn-in
        if let Some(sub) = image_subtitle {
            // The subtitle is scaled to the video's size first when that is known, laid over the
            // video, and the result is scaled like any video. hw_accel is None here, so
            // scale_filter is the software scale.
            let graph = match sub.video_size {
                Some((w, h)) => format!(
                    "[0:{}]scale={w}:{h}[sub];[0:V:0][sub]overlay=eof_action=pass:repeatlast=0,{scale_filter}[v]",
                    sub.stream_index
                ),
                None => format!(
                    "[0:V:0][0:{}]overlay=eof_action=pass:repeatlast=0,{scale_filter}[v]",
                    sub.stream_index
                ),
            };
            args.extend(["-filter_complex".to_string(), graph]);
        } else if profile.burn_subtitles {
            let sub_idx = profile.subtitle_stream_index.unwrap_or(0);
            let burn = subtitles_filter(input, sub_idx, profile.start_time_ticks);
            if hw_accel == HwAccel::None {
                // Software: combine scale + subtitles in one filter chain
                let sw_scale = format!(
                    "scale='min({},iw)':'min({},ih)':force_original_aspect_ratio=decrease",
                    profile.max_width, profile.max_height
                );
                args.extend(["-vf".to_string(), format!("{sw_scale},{burn}")]);
            } else {
                // HW accel: need to download → burn subs → upload (complex, use sw fallback for subs)
                args.extend([
                    "-vf".to_string(),
                    format!("{scale_filter},hwdownload,format=nv12,{burn}"),
                ]);
            }
        } else {
            args.extend(["-vf".to_string(), scale_filter]);
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
        args.extend([
            "-c:a".to_string(),
            profile.audio_codec.clone(),
            "-b:a".to_string(),
            format!("{}k", profile.max_audio_bitrate_kbps),
            "-ac".to_string(),
            "2".to_string(),
        ]);
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
        "-hls_segment_type".to_string(),
        "mpegts".to_string(),
        "-hls_segment_filename".to_string(),
        segment_pattern.to_string(),
        "-hls_playlist_type".to_string(),
        "event".to_string(),
        "-start_number".to_string(),
        "0".to_string(),
    ]);

    // Preset for speed (skip for HW accel, audio-only and copied video)
    if hw_accel == HwAccel::None && !profile.audio_only && !copy_video {
        args.extend(["-preset".to_string(), "veryfast".to_string()]);
    }

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

    #[error("the play session was stopped")]
    Stopped,
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

    #[test]
    fn test_build_ffmpeg_args_image_subtitle_is_overlaid() {
        let sw_scale = "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease";
        // Hardware acceleration and a video copy give way to the software overlay
        for hw_accel in [HwAccel::None, HwAccel::Vaapi, HwAccel::Nvenc, HwAccel::Qsv] {
            let profile = TranscodeProfile {
                image_subtitle: Some(ImageSubtitle {
                    stream_index: 5,
                    video_size: Some((1440, 1080)),
                }),
                audio_stream_index: Some(1),
                copy_video: true,
                hw_accel,
                ..Default::default()
            };
            let args = args_for(&profile);
            // The subtitle is scaled to the source video's size, laid over it, then the
            // result is scaled like any encoded video
            assert!(
                has_pair(
                    &args,
                    "-filter_complex",
                    &format!(
                        "[0:5]scale=1440:1080[sub];[0:V:0][sub]overlay=eof_action=pass:repeatlast=0,{sw_scale}[v]"
                    )
                ),
                "{hw_accel:?}: {args:?}"
            );
            assert_eq!(maps(&args), ["[v]", "0:1"], "{hw_accel:?}");
            assert!(has_pair(&args, "-c:v", "libx264"), "{hw_accel:?}: {args:?}");
            for flag in ["-vf", "-hwaccel", "-vaapi_device"] {
                assert!(!args.contains(&flag.to_string()), "{hw_accel:?}: {args:?}");
            }
        }
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
                "[0:V:0][0:3]overlay=eof_action=pass:repeatlast=0,scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease[v]"
            ),
            "{args:?}"
        );
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

    /// Never writes a playlist.
    const FAKE_SLOW: &str = "#!/bin/sh\nexec sleep 30\n";

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
}
