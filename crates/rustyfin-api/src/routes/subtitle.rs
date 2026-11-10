use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt};
use uuid::Uuid;

use rustyfin_core::subtitle::{
    OpenSubtitlesConfig, discover_external_subtitles, download_subtitle, is_image_subtitle,
    shift_vtt, srt_to_vtt, strip_ass_blocks,
};
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

use crate::error::ApiError;
use crate::routes::streaming::{Version, select_version};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/Subtitles/{index}", get(get_subtitle))
        .route(
            "/Videos/{id}/Subtitles/{index}/Stream",
            get(get_subtitle_stream),
        )
        .route(
            "/Videos/{id}/{media_source_id}/Subtitles/{index}/Stream.{format}",
            get(source_subtitle_vtt),
        )
        .route(
            "/Videos/{id}/{media_source_id}/Subtitles/{index}/{start_ticks}/Stream.{format}",
            get(source_subtitle_vtt_from),
        )
}

/// Administrator-only subtitle routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Items/{id}/RemoteSearch/Subtitles",
            get(search_remote_subtitles),
        )
        .route(
            "/Videos/{id}/Subtitles/{index}/Download",
            post(download_remote_subtitle),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubtitleQuery {
    /// If set to "vtt", SRT subtitles will be converted to WebVTT.
    pub format: Option<String>,
}

async fn get_subtitle(
    State(state): State<AppState>,
    Path((id, index)): Path<(Uuid, usize)>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let media_path = PathBuf::from(&item.path);
    let subs = discover_external_subtitles(&media_path);

    let sub = subs
        .into_iter()
        .find(|s| s.index == index)
        .ok_or_else(|| ApiError::NotFound(format!("Subtitle index {index} not found")))?;

    let content = tokio::fs::read_to_string(&sub.path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read subtitle: {e}")))?;

    let want_vtt = query
        .format
        .as_deref()
        .map(|f| f.eq_ignore_ascii_case("vtt"))
        .unwrap_or(false);

    if want_vtt && sub.format == "srt" {
        let vtt = srt_to_vtt(&content);
        Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
            vtt,
        )
            .into_response())
    } else {
        let ct = match sub.format.as_str() {
            "vtt" => "text/vtt; charset=utf-8",
            "srt" => "application/x-subrip; charset=utf-8",
            "ass" | "ssa" => "text/x-ssa; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
        Ok((StatusCode::OK, [(header::CONTENT_TYPE, ct)], content).into_response())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct RemoteSubtitleResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub format: String,
    pub download_count: Option<i64>,
}

/// GET /Videos/{id}/Subtitles/{index}/Stream – Stream subtitle file directly
async fn get_subtitle_stream(
    State(state): State<AppState>,
    Path((id, index)): Path<(Uuid, usize)>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let media_path = PathBuf::from(&item.path);
    let subs = discover_external_subtitles(&media_path);

    let sub = subs
        .into_iter()
        .find(|s| s.index == index)
        .ok_or_else(|| ApiError::NotFound(format!("Subtitle index {index} not found")))?;

    let file = tokio::fs::File::open(&sub.path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read subtitle: {e}")))?;
    let content = read_capped(file, MAX_SUBTITLE_BYTES)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read subtitle: {e}")))?
        .ok_or_else(|| {
            ApiError::PayloadTooLarge("The subtitle file is larger than 32 MiB".to_string())
        })?;

    let ct = match sub.format.as_str() {
        "vtt" => "text/vtt; charset=utf-8",
        "srt" => "application/x-subrip; charset=utf-8",
        "ass" | "ssa" => "text/x-ssa; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    };

    let filename = sub
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let disposition = format!("attachment; filename=\"{}\"", filename);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct),
            (header::CONTENT_DISPOSITION, &disposition),
        ],
        content,
    )
        .into_response())
}

/// GET /Videos/{id}/{media_source_id}/Subtitles/{index}/Stream.{format} – A subtitle as WebVTT
async fn source_subtitle_vtt(
    State(state): State<AppState>,
    Path((id, media_source_id, index, format)): Path<(Uuid, String, i32, String)>,
) -> Result<Response, ApiError> {
    subtitle_vtt(&state, id, &media_source_id, index, 0, &format).await
}

/// GET /Videos/{id}/{media_source_id}/Subtitles/{index}/{start_ticks}/Stream.{format} – The same
/// WebVTT, its cues moved back by `start_ticks`
async fn source_subtitle_vtt_from(
    State(state): State<AppState>,
    Path((id, media_source_id, index, start_ticks, format)): Path<(Uuid, String, i32, i64, String)>,
) -> Result<Response, ApiError> {
    subtitle_vtt(&state, id, &media_source_id, index, start_ticks, &format).await
}

/// The external subtitle files of `source` that its streams do not list, as subtitle streams
/// numbered after the highest stream index, in file-name order. The directory is read on a
/// blocking thread, so the async workers do not wait on it; a failed task gives no files.
pub(crate) async fn external_streams(source: &MediaSourceResponse) -> Vec<MediaStream> {
    let listed: Vec<PathBuf> = source
        .media_streams
        .iter()
        .filter_map(|stream| stream.path.as_deref().map(PathBuf::from))
        .collect();
    let first = source
        .media_streams
        .iter()
        .map(|stream| stream.index)
        .max()
        .unwrap_or(-1)
        + 1;
    let media_path = PathBuf::from(&source.path);
    let found = tokio::task::spawn_blocking(move || discover_external_subtitles(&media_path))
        .await
        .unwrap_or_default();
    found
        .into_iter()
        .filter(|file| !listed.contains(&file.path))
        .enumerate()
        .map(|(k, file)| MediaStream {
            index: first + k as i32,
            codec: Some(file.format),
            codec_type: "subtitle".to_string(),
            language: file.language,
            is_external: true,
            path: Some(file.path.to_string_lossy().into_owned()),
            r#type: Some("External".to_string()),
            ..Default::default()
        })
        .collect()
}

/// The WebVTT of subtitle stream `index` of media source `media_source_id` of item `id`, from
/// `start_ticks` on. An embedded stream is converted from the source's file, an external one from
/// its own file. The cache keeps the whole text; the cues move back after it is read.
async fn subtitle_vtt(
    state: &AppState,
    id: Uuid,
    media_source_id: &str,
    index: i32,
    start_ticks: i64,
    format: &str,
) -> Result<Response, ApiError> {
    if !format.eq_ignore_ascii_case("vtt") {
        return Err(ApiError::BadRequest(format!(
            "Subtitles are served as vtt, not {format}"
        )));
    }

    let sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    let source = match select_version(id, &sources, Some(media_source_id))? {
        Version::Primary => sources.first(),
        Version::Source(i) => sources.get(i),
    }
    .ok_or_else(|| {
        ApiError::NotFound(format!(
            "Media source {media_source_id} not found for item {id}"
        ))
    })?;

    // The stream: an embedded one, or an external one, which must name its file
    let external = external_streams(source).await;
    let stream = source
        .media_streams
        .iter()
        .chain(&external)
        .find(|s| s.index == index && s.codec_type.eq_ignore_ascii_case("subtitle"))
        .filter(|s| !s.is_external || s.path.is_some())
        .ok_or_else(|| ApiError::NotFound(format!("Subtitle stream {index} not found")))?;

    if is_image_subtitle(stream.codec.as_deref().unwrap_or("")) {
        return Err(ApiError::BadRequest(format!(
            "Subtitle stream {index} is a picture subtitle, which has no text to convert"
        )));
    }

    // The input: the external file, or the source's file with the stream picked by its index
    let (input, map) = if stream.is_external {
        let path = stream.path.as_deref().unwrap_or_default();
        (PathBuf::from(path), "0:s:0".to_string())
    } else {
        (PathBuf::from(&source.path), format!("0:{index}"))
    };
    let meta = tokio::fs::metadata(&input).await.map_err(|e| {
        tracing::warn!("Cannot read the subtitle source {}: {e}", input.display());
        ApiError::Internal("Cannot read the subtitle source".to_string())
    })?;
    // An external file is read whole, so one over the cap is refused before any conversion
    if stream.is_external && meta.len() > MAX_SUBTITLE_BYTES as u64 {
        return Err(ApiError::PayloadTooLarge(
            "The subtitle file is larger than 32 MiB".to_string(),
        ));
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let source_key: String = source
        .id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cache_dir = state.data_dir.join("cache").join("subtitle-vtt");
    // One cache file per stream; its stamp names the input it was made from
    let cache_file = cache_dir.join(format!("v1-{source_key}-{index}.vtt"));
    let input_hash = fnv1a(input.as_os_str().as_encoded_bytes());
    let stamp = format!("{input_hash:016x}-{}-{mtime}", meta.len());

    // The WebVTT text: from the cache, or from one ffmpeg conversion per cache file at a time
    let text = match read_cached(&cache_file, &stamp).await {
        Some(text) => text,
        None => {
            VTT_JOBS
                .run(&cache_file, async {
                    // A request that waited for this file finds the cache its predecessor wrote
                    if let Some(text) = read_cached(&cache_file, &stamp).await {
                        return Ok(text);
                    }
                    let cp1252 = stream.is_external && is_cp1252(&input).await;
                    let demuxer = if stream.is_external {
                        subtitle_demuxer(&input)
                    } else {
                        None
                    };
                    let text = ffmpeg_vtt(&input, &map, demuxer, cp1252).await?;
                    // ffmpeg keeps the ASS override and comment blocks as text; no ASS renderer
                    // shows them
                    let ass = stream.codec.as_deref().is_some_and(|codec| {
                        codec.eq_ignore_ascii_case("ass") || codec.eq_ignore_ascii_case("ssa")
                    });
                    let text = if ass { strip_ass_blocks(&text) } else { text };
                    match write_cache(&cache_dir, &cache_file, &stamp, &text).await {
                        Ok(()) => sweep_vtt_cache_daily(&cache_dir),
                        Err(e) => {
                            tracing::warn!(
                                "Cannot cache the subtitle at {}: {e}",
                                cache_file.display()
                            );
                        }
                    }
                    Ok::<_, ApiError>(text)
                })
                .await?
        }
    };

    let vtt = if start_ticks > 0 {
        shift_vtt(&text, start_ticks)
    } else {
        text
    };
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
        vtt,
    )
        .into_response())
}

/// The ffmpeg demuxer of the external subtitle file `path`, by its extension in lower case; None
/// for an extension that names no format, so that ffmpeg probes the file.
fn subtitle_demuxer(path: &std::path::Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "srt" => Some("srt"),
        "ass" | "ssa" => Some("ass"),
        "vtt" => Some("webvtt"),
        _ => None,
    }
}

/// The ffmpeg arguments that write subtitle stream `map` of `input` as WebVTT on stdout. Input
/// options go before `-i`: `-sub_charenc CP1252` when `cp1252`, and `-f {demuxer}` when a demuxer
/// is given. The forced format keeps ffmpeg from reading an external file by its content (a
/// playlist or concat list named `.srt` would open other files or URLs), and only local files
/// open.
fn vtt_args(
    input: &std::path::Path,
    map: &str,
    demuxer: Option<&str>,
    cp1252: bool,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-nostdin", "-v", "error", "-protocol_whitelist", "file"]
        .map(OsString::from)
        .into();
    if cp1252 {
        args.extend(["-sub_charenc", "CP1252"].map(OsString::from));
    }
    if let Some(demuxer) = demuxer {
        args.extend(["-f", demuxer].map(OsString::from));
    }
    args.push("-i".into());
    args.push(input.into());
    args.extend(["-map", map, "-f", "webvtt", "-"].map(OsString::from));
    args
}

/// Runs ffmpeg with `vtt_args` to write subtitle stream `map` of `input` as WebVTT, reading the
/// file in `demuxer`'s format when one is given and as Windows-1252 when `cp1252`. A failure is
/// logged with ffmpeg's stderr; the 500 does not name the file.
async fn ffmpeg_vtt(
    input: &std::path::Path,
    map: &str,
    demuxer: Option<&str>,
    cp1252: bool,
) -> Result<String, ApiError> {
    let mut command = tokio::process::Command::new("ffmpeg");
    command.args(vtt_args(input, map, demuxer, cp1252));
    let stderr = match run_capped(&mut command, MAX_SUBTITLE_BYTES, VTT_TIME_LIMIT).await {
        Ok(RunEnd::Exited { status, stdout, .. }) if status.success() => {
            match String::from_utf8(stdout) {
                Ok(vtt) => return Ok(vtt),
                Err(_) => "ffmpeg wrote text that is not UTF-8".to_string(),
            }
        }
        Ok(RunEnd::Exited { stderr, .. }) => stderr.trim().to_string(),
        Ok(RunEnd::TooLarge) => {
            return Err(ApiError::PayloadTooLarge(
                "The subtitle is larger than 32 MiB as WebVTT".to_string(),
            ));
        }
        Ok(RunEnd::TimedOut) => {
            tracing::warn!("Converting {} as WebVTT took too long", input.display());
            return Err(ApiError::Internal(
                "Converting the subtitle took too long".to_string(),
            ));
        }
        Err(e) => e.to_string(),
    };
    tracing::warn!("Cannot write {} as WebVTT: {stderr}", input.display());
    Err(ApiError::Internal(
        "Cannot convert the subtitle to WebVTT".to_string(),
    ))
}

/// Whether an external subtitle file is Windows-1252: its bytes are neither UTF-8 nor UTF-16,
/// which a byte order mark starts. A file that cannot be read, or is over the cap, is not.
async fn is_cp1252(file: &std::path::Path) -> bool {
    let Ok(handle) = tokio::fs::File::open(file).await else {
        return false;
    };
    let Ok(Some(bytes)) = read_capped(handle, MAX_SUBTITLE_BYTES).await else {
        return false;
    };
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    !utf16 && std::str::from_utf8(&bytes).is_err()
}

/// The most bytes of an external subtitle file, and of the WebVTT that ffmpeg writes for one
const MAX_SUBTITLE_BYTES: usize = 32 << 20;
/// The conversions to WebVTT that run at once, across all subtitle files
const MAX_VTT_JOBS: usize = 2;
/// The time one conversion may take: an embedded stream reads all of its source file
const VTT_TIME_LIMIT: Duration = Duration::from_secs(10 * 60);
/// The start of a conversion's stderr that is kept for its log line
const STDERR_KEEP: usize = 16 << 10;

/// Reads `reader` to its end, unless it holds more than `cap` bytes: then None.
async fn read_capped(
    reader: impl AsyncRead + Unpin,
    cap: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader.take(cap as u64 + 1).read_to_end(&mut bytes).await?;
    Ok((bytes.len() <= cap).then_some(bytes))
}

/// How a conversion's process ended
enum RunEnd {
    /// It exited on its own: with its whole stdout, and the start of its stderr
    Exited {
        status: ExitStatus,
        stdout: Vec<u8>,
        stderr: String,
    },
    /// Its stdout passed the cap, so it was killed
    TooLarge,
    /// It ran past the time limit, so it was killed
    TimedOut,
}

/// Runs `command` with no stdin, and reads its stdout to at most `cap` bytes, all within `limit`.
/// The process is killed when it passes either bound, or when this future is dropped.
async fn run_capped(
    command: &mut tokio::process::Command,
    cap: usize,
    limit: Duration,
) -> std::io::Result<RunEnd> {
    let started = Instant::now();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(std::io::Error::other("the process output is not piped"));
    };
    // Drained to its end, so that the process never blocks on a full stderr pipe
    let errors = tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break kept,
                Ok(n) => {
                    let room = STDERR_KEEP.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    });
    let bytes = match tokio::time::timeout(limit, read_capped(&mut stdout, cap)).await {
        Err(_elapsed) => {
            let _ = child.kill().await;
            return Ok(RunEnd::TimedOut);
        }
        Ok(read) => read?,
    };
    let Some(bytes) = bytes else {
        let _ = child.kill().await;
        return Ok(RunEnd::TooLarge);
    };
    // The output ended in time; the exit has what is left of the limit
    match tokio::time::timeout(limit.saturating_sub(started.elapsed()), child.wait()).await {
        Err(_elapsed) => {
            let _ = child.kill().await;
            Ok(RunEnd::TimedOut)
        }
        Ok(status) => {
            let stderr = errors.await.unwrap_or_default();
            Ok(RunEnd::Exited {
                status: status?,
                stdout: bytes,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        }
    }
}

/// The WebVTT conversions: one per cache file at a time, and `permits` in all. A later request
/// for a file waits for the one before it, and then finds the cache that one wrote.
struct VttJobs {
    permits: tokio::sync::Semaphore,
    files: std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl VttJobs {
    fn new(permits: usize) -> Self {
        Self {
            permits: tokio::sync::Semaphore::new(permits),
            files: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Runs `job` once no other job holds `file` and a permit is free.
    async fn run<T>(&self, file: &std::path::Path, job: impl std::future::Future<Output = T>) -> T {
        let turn = Turn::new(self, file);
        let _file_lock = turn.entry.lock().await;
        let _permit = self
            .permits
            .acquire()
            .await
            .expect("the semaphore is never closed");
        job.await
    }
}

/// A request's turn for one cache file. Dropping it removes the file's entry from the map when no
/// other request holds the entry, which also covers a request dropped while it waited.
struct Turn<'a> {
    jobs: &'a VttJobs,
    file: PathBuf,
    entry: Arc<tokio::sync::Mutex<()>>,
}

impl<'a> Turn<'a> {
    fn new(jobs: &'a VttJobs, file: &std::path::Path) -> Self {
        let entry = jobs
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(file.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        Self {
            jobs,
            file: file.to_path_buf(),
            entry,
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut files = self
            .jobs
            .files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Only this turn and the map hold the entry: no other request waits for the file
        if Arc::strong_count(&self.entry) == 2 {
            files.remove(&self.file);
        }
    }
}

/// The WebVTT conversions of all the subtitle routes
static VTT_JOBS: LazyLock<VttJobs> = LazyLock::new(|| VttJobs::new(MAX_VTT_JOBS));

/// FNV-1a with 64 bits: the hash of an input's path in a cache stamp.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The WebVTT text in cache file `file` when its first line is `stamp`, the stamp of the input it
/// was made from; None otherwise. A hit sets the file's mtime to now, the time of its last read, on
/// a blocking thread that is not waited for.
async fn read_cached(file: &std::path::Path, stamp: &str) -> Option<String> {
    let cached = tokio::fs::read_to_string(file).await.ok()?;
    let (found, text) = cached.split_once('\n')?;
    if found != stamp {
        return None;
    }
    let file = file.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::File::open(&file).and_then(|f| f.set_modified(SystemTime::now()));
    });
    Some(text.to_string())
}

/// Writes a cache file as `stamp` on its first line and then `text`, through a temporary file
/// beside it and a rename, so that a reader never sees a part-written file.
async fn write_cache(
    dir: &std::path::Path,
    file: &std::path::Path,
    stamp: &str,
    text: &str,
) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    // A name of its own beside the file, so that concurrent writers do not collide
    let tmp = file.with_extension(format!("{}.tmp", Uuid::new_v4().simple()));
    let written = match tokio::fs::write(&tmp, format!("{stamp}\n{text}")).await {
        Ok(()) => tokio::fs::rename(&tmp, file).await,
        Err(e) => Err(e),
    };
    if written.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    written
}

/// Removes stale files from the WebVTT cache `dir` as of `now` and returns how many it removed.
/// Only regular files directly in `dir` count. A `.tmp` file over an hour old is left by a failed
/// write, a `.vtt` file not named `v1-*` is an older format, and a `v1-*.vtt` file unread for 30
/// days is stale (its mtime is its last read). Then the least recently read `v1-*.vtt` files go
/// while the rest total over `max_bytes`. Other files stay.
fn sweep_vtt_cache(dir: &std::path::Path, now: SystemTime, max_bytes: u64) -> usize {
    const HOUR: Duration = Duration::from_secs(3600);
    const THIRTY_DAYS: Duration = Duration::from_secs(30 * 86_400);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    // The `v1-*.vtt` files that stay, as (last read, length, path)
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(read) = meta.modified() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let age = now.duration_since(read).unwrap_or_default();
        let stale = if name.ends_with(".tmp") {
            age > HOUR
        } else if name.ends_with(".vtt") {
            !name.starts_with("v1-") || age > THIRTY_DAYS
        } else {
            false
        };
        if stale {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        } else if name.starts_with("v1-") && name.ends_with(".vtt") {
            kept.push((read, meta.len(), path));
        }
    }
    // Then the least recently read go while the rest total over the cap
    kept.sort_by_key(|(read, _, _)| *read);
    let mut total: u64 = kept.iter().map(|(_, len, _)| *len).sum();
    for (_, len, path) in kept {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    removed
}

/// The most the WebVTT cache may hold, in bytes: the sweep removes the least recently read past it.
const MAX_VTT_CACHE: u64 = 512 << 20;

/// The Unix time in seconds of the last sweep of the WebVTT cache; 0 before the first one.
static LAST_SWEEP: AtomicU64 = AtomicU64::new(0);

/// Sweeps the WebVTT cache `dir` on a blocking thread, at most once a day: the first write of a day
/// claims the day in `LAST_SWEEP`, and the writes after it do not sweep.
fn sweep_vtt_cache_daily(dir: &std::path::Path) {
    let now = SystemTime::now();
    let secs = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if secs < last + 86_400 {
        return;
    }
    if LAST_SWEEP
        .compare_exchange(last, secs, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || sweep_vtt_cache(&dir, now, MAX_VTT_CACHE));
    }
}

async fn search_remote_subtitles(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<RemoteSubtitleQuery>,
) -> Result<Response, ApiError> {
    // Search for subtitles using OpenSubtitles or local cache
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let results = rustyfin_core::subtitle::search_remote_subtitles(
        &state.http_client,
        &state.db_ro,
        &item.name,
        query.language.as_deref(),
        item.production_year,
    )
    .await?;

    Ok((StatusCode::OK, axum::Json(results)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteSubtitleQuery {
    #[serde(default)]
    pub language: Option<String>,
}

/// Request body for downloading a remote subtitle
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DownloadSubtitleRequest {
    /// OpenSubtitles subtitle ID
    pub subtitle_id: String,
}

/// POST /Videos/{id}/Subtitles/{index}/Download – Download and cache a remote subtitle
async fn download_remote_subtitle(
    State(state): State<AppState>,
    Path((_id, _index)): Path<(Uuid, usize)>,
    Json(req): Json<DownloadSubtitleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cache_dir = state.data_dir.join("cache").join("subtitles");

    let config = OpenSubtitlesConfig::default();
    match download_subtitle(&state.http_client, &config, &req.subtitle_id, &cache_dir).await {
        Ok(path) => {
            tracing::info!("Subtitle downloaded to {}", path.display());
            Ok(Json(json!({
                "status": "ok",
                "path": path.to_string_lossy().to_string()
            })))
        }
        Err(e) => {
            tracing::warn!("Subtitle download failed: {}", e);
            Err(ApiError::Internal(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, SystemTime};

    /// A folder under the temp dir with empty files, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn with(names: &[&str]) -> Folder {
            let dir =
                std::env::temp_dir().join(format!("rustyfin-b22-{}", Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap();
            for name in names {
                std::fs::write(dir.join(name), "").unwrap();
            }
            Folder(dir)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A source for `Film.mkv` in `folder` with `streams` (MediaStream JSON).
    fn source(folder: &Folder, streams: serde_json::Value) -> MediaSourceResponse {
        serde_json::from_value(json!({
            "Id": "b22-source",
            "Uid": "b22-source",
            "Protocol": "File",
            "Path": folder.path("Film.mkv"),
            "MediaStreams": streams,
        }))
        .unwrap()
    }

    /// A stream's index, codec, language and path.
    type Summary<'a> = (i32, Option<&'a str>, Option<&'a str>, Option<&'a str>);

    fn summary(streams: &[MediaStream]) -> Vec<Summary<'_>> {
        streams
            .iter()
            .map(|s| {
                (
                    s.index,
                    s.codec.as_deref(),
                    s.language.as_deref(),
                    s.path.as_deref(),
                )
            })
            .collect()
    }

    const FILES: [&str; 5] = [
        "Film.mkv",
        "Film.eng.srt",
        "Film.ger.ass",
        "Film.srt",
        "Other.srt",
    ];

    #[tokio::test]
    async fn test_external_streams_follow_the_highest_index() {
        let folder = Folder::with(&FILES);
        let source = source(
            &folder,
            json!([
                { "Index": 0, "CodecType": "video", "Codec": "h264" },
                { "Index": 2, "CodecType": "subtitle", "Codec": "subrip" },
                { "Index": 1, "CodecType": "audio", "Codec": "aac" },
            ]),
        );
        let streams = external_streams(&source).await;
        let (eng, ger, plain) = (
            folder.path("Film.eng.srt"),
            folder.path("Film.ger.ass"),
            folder.path("Film.srt"),
        );
        assert_eq!(
            summary(&streams),
            vec![
                (3, Some("srt"), Some("eng"), Some(eng.as_str())),
                (4, Some("ass"), Some("ger"), Some(ger.as_str())),
                (5, Some("srt"), None, Some(plain.as_str())),
            ]
        );
        for stream in &streams {
            assert_eq!(stream.codec_type, "subtitle");
            assert_eq!(stream.r#type.as_deref(), Some("External"));
            assert!(stream.is_external);
            assert!(!stream.is_default && !stream.is_forced);
            assert_eq!(stream.title, None);
        }
    }

    #[tokio::test]
    async fn test_external_streams_skip_files_the_source_lists() {
        let folder = Folder::with(&FILES);
        let source = source(
            &folder,
            json!([
                { "Index": 0, "CodecType": "video", "Codec": "h264" },
                {
                    "Index": 7,
                    "CodecType": "subtitle",
                    "Codec": "srt",
                    "IsExternal": true,
                    "Path": folder.path("Film.eng.srt"),
                },
            ]),
        );
        let (ger, plain) = (folder.path("Film.ger.ass"), folder.path("Film.srt"));
        assert_eq!(
            summary(&external_streams(&source).await),
            vec![
                (8, Some("ass"), Some("ger"), Some(ger.as_str())),
                (9, Some("srt"), None, Some(plain.as_str())),
            ]
        );
    }

    #[tokio::test]
    async fn test_external_streams_of_a_source_without_streams_start_at_zero() {
        let folder = Folder::with(&FILES);
        let indexes: Vec<i32> = external_streams(&source(&folder, json!([])))
            .await
            .iter()
            .map(|s| s.index)
            .collect();
        assert_eq!(indexes, vec![0, 1, 2]);

        // No subtitle files: nothing
        let bare = Folder::with(&["Film.mkv"]);
        assert!(external_streams(&source(&bare, json!([]))).await.is_empty());
    }

    #[test]
    fn test_subtitle_demuxer_follows_the_extension() {
        assert_eq!(subtitle_demuxer(Path::new("film.srt")), Some("srt"));
        assert_eq!(subtitle_demuxer(Path::new("film.ASS")), Some("ass"));
        assert_eq!(subtitle_demuxer(Path::new("film.ssa")), Some("ass"));
        assert_eq!(subtitle_demuxer(Path::new("film.vtt")), Some("webvtt"));
        assert_eq!(subtitle_demuxer(Path::new("film.txt")), None);
    }

    #[test]
    fn test_vtt_args_force_the_external_format() {
        let args: Vec<String> = vtt_args(Path::new("/media/film.srt"), "0:s:0", Some("srt"), false)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        let before = &args[..input];
        assert!(before.windows(2).any(|p| p[0] == "-f" && p[1] == "srt"));
        assert!(
            before
                .windows(2)
                .any(|p| p[0] == "-protocol_whitelist" && p[1] == "file")
        );
        assert_eq!(args[input + 1], "/media/film.srt");
        assert_eq!(args[input + 2..], ["-map", "0:s:0", "-f", "webvtt", "-"]);
    }

    #[test]
    fn test_vtt_args_let_ffmpeg_probe_an_embedded_stream() {
        let args: Vec<String> = vtt_args(Path::new("/media/film.mkv"), "0:3", None, false)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert!(!args[..input].iter().any(|arg| arg == "-f"));
        assert_eq!(args[input + 2..], ["-map", "0:3", "-f", "webvtt", "-"]);
    }

    #[test]
    fn test_vtt_args_read_cp1252() {
        let args: Vec<String> = vtt_args(Path::new("/media/film.srt"), "0:s:0", Some("srt"), true)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        let before = &args[..input];
        assert!(
            before
                .windows(2)
                .any(|p| p[0] == "-sub_charenc" && p[1] == "CP1252")
        );
    }

    /// Writes `name` in `dir` as `len` bytes, last modified `age` before `now`.
    fn aged_file(dir: &Path, name: &str, len: usize, now: SystemTime, age: Duration) {
        let path = dir.join(name);
        std::fs::write(&path, vec![b'x'; len]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - age)
            .unwrap();
    }

    #[test]
    fn test_sweep_vtt_cache_removes_stale_files() {
        let folder = Folder::with(&[]);
        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        let day = Duration::from_secs(86_400);
        aged_file(&folder.0, "v1-a-3.vtt", 10, now, Duration::ZERO);
        aged_file(&folder.0, "v1-d-3.5678.tmp", 10, now, Duration::ZERO);
        aged_file(&folder.0, "notes.txt", 10, now, day * 60);
        aged_file(&folder.0, "v1-b-3.vtt", 10, now, day * 31);
        aged_file(&folder.0, "v1-c-3.1234.tmp", 10, now, hour * 2);
        aged_file(&folder.0, "a-3-10-20.vtt", 10, now, Duration::ZERO);

        assert_eq!(sweep_vtt_cache(&folder.0, now, u64::MAX), 3);
        let mut names: Vec<String> = std::fs::read_dir(&folder.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["notes.txt", "v1-a-3.vtt", "v1-d-3.5678.tmp"]);
    }

    #[test]
    fn test_sweep_vtt_cache_keeps_its_size() {
        let folder = Folder::with(&[]);
        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        aged_file(&folder.0, "v1-src-0.vtt", 10, now, hour * 3);
        aged_file(&folder.0, "v1-src-1.vtt", 10, now, hour * 2);
        aged_file(&folder.0, "v1-src-2.vtt", 10, now, hour);

        // 30 bytes against 25: the one read longest ago goes, which leaves 20
        assert_eq!(sweep_vtt_cache(&folder.0, now, 25), 1);
        assert!(!Path::new(&folder.path("v1-src-0.vtt")).exists());
        assert!(Path::new(&folder.path("v1-src-1.vtt")).exists());
        assert!(Path::new(&folder.path("v1-src-2.vtt")).exists());
    }

    #[tokio::test]
    async fn test_read_capped_keeps_input_at_the_cap() {
        assert_eq!(
            read_capped(&b"abcd"[..], 4).await.unwrap(),
            Some(b"abcd".to_vec())
        );
    }

    #[tokio::test]
    async fn test_read_capped_refuses_input_over_the_cap() {
        assert_eq!(read_capped(&b"abcde"[..], 4).await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_run_capped_returns_the_output() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "printf abc; printf oops >&2; exit 3"]);
        let RunEnd::Exited {
            status,
            stdout,
            stderr,
        } = run_capped(&mut command, 1000, Duration::from_secs(10))
            .await
            .unwrap()
        else {
            panic!("the child did not exit");
        };
        assert_eq!(status.code(), Some(3));
        assert_eq!(stdout, b"abc".to_vec());
        assert_eq!(stderr, "oops");
    }

    #[tokio::test]
    async fn test_run_capped_stops_at_the_cap() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "head -c 100000 /dev/zero; exec sleep 30"]);
        let started = std::time::Instant::now();
        let end = run_capped(&mut command, 1000, Duration::from_secs(20))
            .await
            .unwrap();
        assert!(matches!(end, RunEnd::TooLarge));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_run_capped_stops_at_the_time_limit() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let started = std::time::Instant::now();
        let end = run_capped(&mut command, 1000, Duration::from_millis(300))
            .await
            .unwrap();
        assert!(matches!(end, RunEnd::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_run_capped_drains_a_long_stderr() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "head -c 300000 /dev/zero >&2; printf ok"]);
        let RunEnd::Exited { stdout, .. } = run_capped(&mut command, 10, Duration::from_secs(10))
            .await
            .unwrap()
        else {
            panic!("the child did not exit");
        };
        assert_eq!(stdout, b"ok".to_vec());
    }

    #[tokio::test]
    async fn test_vtt_jobs_run_at_most_their_permits() {
        let jobs = VttJobs::new(2);
        let (running, peak, runs) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        let work = (0..6).map(|n| {
            let file = PathBuf::from(format!("/cache/{n}.vtt"));
            let (jobs, running, peak, runs) = (&jobs, &running, &peak, &runs);
            async move {
                jobs.run(&file, async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    runs.fetch_add(1, Ordering::SeqCst);
                })
                .await
            }
        });
        futures_util::future::join_all(work).await;
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(runs.load(Ordering::SeqCst), 6);
    }

    #[tokio::test]
    async fn test_vtt_jobs_run_one_job_per_file() {
        let jobs = VttJobs::new(4);
        let file = PathBuf::from("/cache/one.vtt");
        let (running, peak, runs) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        let work = (0..3).map(|_| {
            let (jobs, file, running, peak, runs) = (&jobs, &file, &running, &peak, &runs);
            async move {
                jobs.run(file, async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    runs.fetch_add(1, Ordering::SeqCst);
                })
                .await
            }
        });
        futures_util::future::join_all(work).await;
        assert_eq!(peak.load(Ordering::SeqCst), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        assert!(jobs.files.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_vtt_jobs_forget_a_dropped_wait() {
        let jobs = VttJobs::new(1);
        let file = PathBuf::from("/cache/x.vtt");
        // B waits for the file that A holds, and is dropped while it waits
        let (_, waited) = tokio::join!(
            jobs.run(&file, tokio::time::sleep(Duration::from_millis(200))),
            tokio::time::timeout(Duration::from_millis(50), jobs.run(&file, async {})),
        );
        assert!(waited.is_err());
        assert!(jobs.files.lock().unwrap().is_empty());
    }
}
