use crate::query::Query;
use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rustyfin_db::repo_playback::StreamChoice;
use rustyfin_model::auth::Claims;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path as FilePath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

mod media_info;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(media_info::routes())
        .route("/Videos/{id}/stream", get(stream_video).head(head_video))
        .route(
            "/Videos/{id}/stream.{container}",
            get(stream_video).head(head_video),
        )
        .route(
            "/Videos/{id}/progressive.stream",
            get(stream_video_progressive),
        )
        .route("/Videos/{id}/PlaybackInfo", get(video_playback_info))
        .route("/Videos/{id}/Universal", get(video_universal))
        .route("/Videos/{id}/Chapters", get(get_chapters))
        .route("/Audio/{id}/stream", get(stream_audio).head(head_stream))
        .route(
            "/Audio/{id}/stream.{container}",
            get(stream_audio).head(head_stream),
        )
        .route("/Audio/{id}/universal", get(audio_universal))
        .route("/Audio/{id}/PlaybackInfo", get(audio_playback_info))
}

// ---------------------------------------------------------------------------
// Video versions
// ---------------------------------------------------------------------------

/// Which version of an item a playback request plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    /// No MediaSourceId, a blank one or the item's own id: the item's file.
    Primary,
    /// The source at this index of the item's sources.
    Source(usize),
}

/// The request's MediaSourceId (name matched in any ASCII case), trimmed. None when absent or
/// blank.
pub(crate) fn requested_source_id(params: &HashMap<String, String>) -> Option<&str> {
    params
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("MediaSourceId"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        // A client sends one name; should several match, the smallest value does not depend on
        // the map's order
        .min()
}

/// The version `requested` names among `sources` of item `item_id`. ApiError::NotFound when it
/// names none.
pub(crate) fn select_version(
    item_id: Uuid,
    sources: &[rustyfin_model::media_source::MediaSourceResponse],
    requested: Option<&str>,
) -> Result<Version, ApiError> {
    let Some(requested) = requested else {
        return Ok(Version::Primary);
    };
    let wanted = Uuid::parse_str(requested).ok();
    if wanted == Some(item_id) {
        return Ok(Version::Primary);
    }
    sources
        .iter()
        .position(|source| {
            source.id == requested
                || (wanted.is_some() && Uuid::parse_str(&source.id).ok() == wanted)
        })
        .map(Version::Source)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Media source {requested} not found for item {item_id}"
            ))
        })
}

/// The path of a video route. `stream.{container}` has a second parameter, which serde ignores.
#[derive(Deserialize)]
struct VideoPath {
    id: Uuid,
}

/// The file that a video request plays: the item's own file, or the file of the version that its
/// `MediaSourceId` names. The sources are read only when an id is given.
async fn video_file(
    state: &AppState,
    id: Uuid,
    params: &HashMap<String, String>,
) -> Result<String, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    let Some(requested) = requested_source_id(params) else {
        validate_path_against_library(state, &item.library_id, &item.path).await?;
        return Ok(item.path);
    };
    let sources = rustyfin_db::repo_media_source::get_media_sources(&state.db_ro, &id).await?;
    match select_version(id, &sources, Some(requested))? {
        Version::Primary => {
            validate_path_against_library(state, &item.library_id, &item.path).await?;
            Ok(item.path)
        }
        Version::Source(index) => {
            validate_path_against_library(state, &item.library_id, &sources[index].path).await?;
            Ok(sources[index].path.clone())
        }
    }
}

/// An absolute path with `.` components dropped, so symlinks are kept. `None` for a relative
/// path, or for one with a `..` component: the kernel resolves `..` after following a link, so
/// `lib/link/sub/../../x` can leave the library although it looks like it stays inside.
fn lexical_normalize(path: &FilePath) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if !path.is_absolute() {
        return None;
    }
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => return None,
            other => out.push(other),
        }
    }
    Some(out)
}

/// True when `file_path` lies under `lib_path`, either as written (a file reached through a
/// symlink inside the library, which Jellyfin also serves) or after resolving symlinks on both
/// sides. A path with a `..` component never passes as written.
fn path_is_under_library(file_path: &FilePath, canonical_file: &FilePath, lib_path: &str) -> bool {
    let lib = FilePath::new(lib_path);
    if let (Some(file_lex), Some(lib_lex)) = (lexical_normalize(file_path), lexical_normalize(lib))
        && file_lex.starts_with(&lib_lex)
    {
        return true;
    }
    lib.canonicalize()
        .is_ok_and(|canonical_lib| canonical_file.starts_with(canonical_lib))
}

/// Whether a stored path has a `..` component. Such a path is refused before anything is
/// resolved: the scanner never stores one, and `..` after a link can leave the library.
fn has_parent_dir(path: &FilePath) -> bool {
    path.components()
        .any(|c| c == std::path::Component::ParentDir)
}

/// The folders in the data dir that Rustyfin writes: `cache` (subtitles, images; server main.rs),
/// `logs`, `transcodes`, `trickplay` (main.rs), `plugins` (PluginManager), `metadata` and
/// `images` (routes/images.rs, the Jellyfin import), `backups` (routes/backup.rs) and
/// `jellyfin-import-copy` (jellyfin_import/source.rs). Anything else there, such as a media
/// folder, may be streamed.
const OWNED_DATA_DIRS: [&str; 9] = [
    "cache",
    "logs",
    "transcodes",
    "trickplay",
    "plugins",
    "metadata",
    "images",
    "backups",
    "jellyfin-import-copy",
];

/// The files at the top of the data dir that Rustyfin writes: `config.json` and its temp file
/// (config_file.rs, written then renamed), `server_id` and its temp file (main.rs), the wizard's
/// import queue and its temp file (wizard_import.rs), and the default database `rustyfin.db`
/// (main.rs). Password reset PIN files (`passwordreset-*.json`, rustyfin-auth) are matched by
/// their pattern.
const OWNED_DATA_FILES: [&str; 7] = [
    "config.json",
    "config.json.tmp",
    "server_id",
    "server_id.tmp",
    "jellyfin-import-queue.json",
    "jellyfin-import-queue.json.tmp",
    "rustyfin.db",
];

/// The files SQLite keeps beside a database file `x`: `x-wal`, `x-shm` and `x-journal`.
const DATABASE_SIBLINGS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// The paths Rustyfin owns, which a library link must not reach:
/// - in the data dir, any file under one of its own folders (`OWNED_DATA_DIRS`), and its own
///   top-level files (`OWNED_DATA_FILES`, PIN files, and their SQLite siblings), with names
///   compared without ASCII case;
/// - each owned folder resolved, so one linked out of the data dir (`transcodes -> /fast/...`)
///   is still owned;
/// - by full path: the database wherever `RUSTYFIN_DB` puts it, the TLS key and certificate
///   (`RUSTYFIN_TLS_KEY`, `RUSTYFIN_TLS_CERT`), and each data-dir file above, each with its
///   SQLite siblings (`DATABASE_SIBLINGS`).
///
/// Every path is compared resolved (a certbot `live/privkey.pem` link counts as the
/// `archive/privkey1.pem` it points to) and as configured, made absolute, so an owned path is
/// refused even when it can't be resolved.
struct OwnedPaths {
    data_dir: std::path::PathBuf,
    files: Vec<std::path::PathBuf>,
}

impl OwnedPaths {
    /// The two forms of `path`: resolved, when that works, and as configured, made absolute.
    fn forms(path: &FilePath) -> Vec<std::path::PathBuf> {
        path.canonicalize()
            .into_iter()
            .chain(std::path::absolute(path))
            .collect()
    }

    /// `path` with `suffix` added to its file name.
    fn with_suffix(path: &FilePath, suffix: &str) -> std::path::PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        name.into()
    }

    /// Whether `name` is `file` or one of its SQLite siblings, without ASCII case.
    fn is_file_or_sibling(name: &str, file: &str) -> bool {
        let (name, file) = (name.to_ascii_lowercase(), file.to_ascii_lowercase());
        name == file
            || name
                .strip_prefix(&file)
                .is_some_and(|rest| DATABASE_SIBLINGS.contains(&rest))
    }

    /// Whether `name`, a top-level name in the data dir, is a file Rustyfin owns there.
    fn is_owned_data_file(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        OWNED_DATA_FILES
            .iter()
            .any(|file| Self::is_file_or_sibling(name, file))
            || lower.starts_with("passwordreset-") && lower.ends_with(".json")
    }

    /// Whether the resolved `target` lies in the data dir form `data` as an owned file, counting
    /// the folders in `dirs` as owned.
    fn owned_by_name(data: &FilePath, target: &FilePath, dirs: &[&str]) -> bool {
        let Ok(rest) = target.strip_prefix(data) else {
            return false;
        };
        let mut parts = rest.components().map(|c| c.as_os_str().to_string_lossy());
        let Some(first) = parts.next() else {
            return false;
        };
        if parts.next().is_some() {
            dirs.iter().any(|dir| dir.eq_ignore_ascii_case(&first))
        } else {
            Self::is_owned_data_file(&first)
        }
    }

    /// Whether the resolved `target` is one of these owned paths.
    fn owns(&self, target: &FilePath) -> bool {
        self.owns_in(target, &OWNED_DATA_DIRS)
    }

    /// Whether the resolved `target` is one of these owned paths, counting only the data-dir
    /// folders in `dirs` as owned.
    fn owns_in(&self, target: &FilePath, dirs: &[&str]) -> bool {
        let data_forms = Self::forms(&self.data_dir);
        if data_forms
            .iter()
            .any(|data| Self::owned_by_name(data, target, dirs))
        {
            return true;
        }
        // Owned folders that are links out of the data dir
        let linked_dir = data_forms.iter().any(|data| {
            dirs.iter().any(|dir| {
                data.join(dir)
                    .canonicalize()
                    .is_ok_and(|real| target != real && target.starts_with(&real))
            })
        });
        if linked_dir {
            return true;
        }
        // Owned files by full path, each as configured and resolved, with its SQLite siblings
        let data_files = OWNED_DATA_FILES.iter().map(|name| self.data_dir.join(name));
        self.files.iter().cloned().chain(data_files).any(|file| {
            let mut candidates = Self::forms(&file);
            for suffix in DATABASE_SIBLINGS {
                candidates.extend(Self::forms(&Self::with_suffix(&file, suffix)));
                // SQLite keeps the siblings beside the file a link resolves to
                if let Ok(real) = file.canonicalize() {
                    candidates.push(Self::with_suffix(&real, suffix));
                }
            }
            candidates.iter().any(|candidate| candidate == target)
        })
    }
}

/// Whether `file_path` may be streamed from a library with `lib_paths`. It is refused when:
/// - it has a `..` component;
/// - resolving its links fails;
/// - it lies under none of `lib_paths`, as written or after resolving links (see
///   `path_is_under_library`);
/// - its resolved target is a path Rustyfin owns (see `OwnedPaths`). Other files inside the
///   data dir, such as media kept at `<data>/media`, may be streamed.
fn path_allowed(file_path: &str, lib_paths: &[String], owned: &OwnedPaths) -> bool {
    let raw = FilePath::new(file_path);
    if has_parent_dir(raw) {
        return false;
    }
    let Ok(canonical) = raw.canonicalize() else {
        return false;
    };
    if !lib_paths
        .iter()
        .any(|lib_path| path_is_under_library(raw, &canonical, lib_path))
    {
        return false;
    }
    !owned.owns(&canonical)
}

/// The file of the server's main database, as SQLite reports it; None for an in-memory one.
async fn database_file(pool: &sqlx::SqlitePool) -> Option<std::path::PathBuf> {
    sqlx::query_scalar::<_, String>("SELECT file FROM pragma_database_list WHERE name = 'main'")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .filter(|file| !file.is_empty())
        .map(std::path::PathBuf::from)
}

/// The paths this server owns: its data dir, its database file and its TLS files.
async fn owned_paths(state: &AppState) -> OwnedPaths {
    OwnedPaths {
        data_dir: state.data_dir.to_path_buf(),
        files: database_file(&state.db_ro)
            .await
            .into_iter()
            .chain(
                ["RUSTYFIN_TLS_KEY", "RUSTYFIN_TLS_CERT"]
                    .into_iter()
                    .filter_map(std::env::var_os)
                    .map(std::path::PathBuf::from),
            )
            .collect(),
    }
}

/// The data-dir folders that hold no pictures, so the image routes never read from them. The
/// other owned folders (`cache`, `metadata`, `images`, `trickplay`) keep the server's pictures.
const NON_IMAGE_DATA_DIRS: [&str; 5] = [
    "logs",
    "transcodes",
    "plugins",
    "backups",
    "jellyfin-import-copy",
];

/// Whether `head`, the first bytes of a file, start like a picture: JPEG, PNG, GIF, WebP, BMP,
/// ICO, TIFF, JPEG XL, an ISO media picture (AVIF, HEIF) or SVG.
fn looks_like_image(head: &[u8]) -> bool {
    const MAGIC: [&[u8]; 10] = [
        b"\xFF\xD8\xFF",
        b"\x89PNG\r\n\x1A\n",
        b"GIF87a",
        b"GIF89a",
        b"BM",
        b"\x00\x00\x01\x00",
        b"II*\x00",
        b"MM\x00*",
        b"\xFF\x0A",
        b"\x00\x00\x00\x0CJXL ",
    ];
    if MAGIC.iter().any(|magic| head.starts_with(magic)) {
        return true;
    }
    let webp = head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP".as_slice());
    let iso_media = head.get(4..8) == Some(b"ftyp".as_slice());
    if webp || iso_media {
        return true;
    }
    // SVG is text: an XML declaration or an <svg> tag first, and an <svg> tag in the head
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{FEFF}').trim_start();
    (text.starts_with("<?xml") || text.starts_with("<svg")) && text.contains("<svg")
}

/// Whether the image routes may read `file_path`. Pictures live in library folders, in the data
/// dir and in folders an import points at, so there is no library to hold them to. The file is
/// refused when:
/// - its path is relative or has a `..` component, or its links can't be resolved;
/// - its resolved target is a path this server owns, other than a picture folder (see
///   `NON_IMAGE_DATA_DIRS`): the database, config, TLS files, logs, backups and so on;
/// - it does not start like a picture (see `looks_like_image`).
fn image_path_allowed(file_path: &str, owned: &OwnedPaths) -> bool {
    use std::io::Read;

    let raw = FilePath::new(file_path);
    if !raw.is_absolute() || has_parent_dir(raw) {
        return false;
    }
    let Ok(canonical) = raw.canonicalize() else {
        return false;
    };
    if owned.owns_in(&canonical, &NON_IMAGE_DATA_DIRS) {
        return false;
    }
    let mut head = [0u8; 512];
    let mut read = 0;
    if let Ok(mut file) = std::fs::File::open(&canonical) {
        while read < head.len() {
            match file.read(&mut head[read..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => read += n,
            }
        }
    }
    looks_like_image(&head[..read])
}

/// Validates that a stored image path may be served (see `image_path_allowed`). The checks read
/// the file system, so they run on a blocking thread.
pub(crate) async fn validate_image_path(state: &AppState, file_path: &str) -> Result<(), ApiError> {
    let owned = owned_paths(state).await;
    let path = file_path.to_string();
    let allowed = tokio::task::spawn_blocking(move || image_path_allowed(&path, &owned))
        .await
        .unwrap_or(false);
    if allowed {
        return Ok(());
    }
    tracing::warn!("image path '{}' is refused", file_path);
    Err(ApiError::NotFound("image not found".to_string()))
}

/// Validates that `file_path` is inside one of the library's configured paths and not a link
/// into the server's own data (see `path_allowed`). This prevents path traversal if the
/// database is compromised.
pub(crate) async fn validate_path_against_library(
    state: &AppState,
    library_id: &uuid::Uuid,
    file_path: &str,
) -> Result<(), ApiError> {
    if has_parent_dir(FilePath::new(file_path)) {
        tracing::warn!("streaming path '{}' has a '..' component", file_path);
        return Err(ApiError::NotFound(
            "File is outside configured library paths".to_string(),
        ));
    }
    if let Ok(Some(library)) =
        rustyfin_db::repo_library::get_library_by_id(&state.db_ro, *library_id).await
    {
        let owned = owned_paths(state).await;
        if path_allowed(file_path, &library.paths, &owned) {
            return Ok(());
        }
        // Path not under any library path — log and reject
        tracing::warn!(
            "streaming path '{}' is outside library paths for library {}",
            file_path,
            library_id
        );
        return Err(ApiError::NotFound(
            "File is outside configured library paths".to_string(),
        ));
    }
    // If we can't find the library, we still allow the stream (backward compatibility)
    // but log a warning
    tracing::debug!(
        "could not find library {} for path validation, allowing stream",
        library_id
    );
    Ok(())
}

/// `GET /Videos/{id}/stream` and `stream.{container}`: the file of the requested version.
async fn stream_video(
    State(state): State<AppState>,
    Path(VideoPath { id }): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;
    serve_file(file_path, &headers).await
}

/// `HEAD` of `stream` and `stream.{container}`: the headers of the file that `GET` serves.
async fn head_video(
    State(state): State<AppState>,
    Path(VideoPath { id }): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;
    head_file(&file_path).await
}

async fn stream_audio(
    state: State<AppState>,
    path: Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    stream_file(state, path, headers).await
}

async fn head_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    validate_path_against_library(&state, &item.library_id, &item.path).await?;
    head_file(&item.path).await
}

/// The headers of a `HEAD` for the file at `file_path`: its type, its length, and that byte
/// ranges are accepted.
async fn head_file(file_path: &str) -> Result<Response, ApiError> {
    let metadata = tokio::fs::metadata(file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;

    let content_type = mime_from_path(file_path);
    let file_size = metadata.len();

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, file_size.to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
        ],
    )
        .into_response())
}

async fn stream_file(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    validate_path_against_library(&state, &item.library_id, &item.path).await?;
    serve_file(item.path, &headers).await
}

/// Serves the file at `file_path` for a request with `headers`: the whole file, or the byte range,
/// 304 or 416 that the headers ask for.
async fn serve_file(file_path: String, headers: &HeaderMap) -> Result<Response, ApiError> {
    let content_type = mime_from_path(&file_path);

    // Open file first, then get metadata from the file handle — avoids a separate stat() syscall
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read file metadata: {e}")))?;
    let file_size = metadata.len();

    // ETag support: use modification time + size as cache key
    let etag = if let Ok(mtime) = metadata.modified() {
        let mtime_secs = mtime
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("\"{}-{}\"", mtime_secs, file_size)
    } else {
        format!("\"{}\"", file_size)
    };

    // Check If-None-Match for conditional GET (304 Not Modified)
    if let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && let Ok(client_etag) = if_none_match.to_str()
        && client_etag == etag
    {
        return Ok((StatusCode::NOT_MODIFIED, ()).into_response());
    }

    let byte_range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map_or(ByteRange::Ignore, |s| parse_range(s, file_size));

    match byte_range {
        ByteRange::Partial(start, end) => {
            let length = end - start + 1;

            // Reopen for range request (need mutable handle for seek)
            let mut file = tokio::fs::File::open(&file_path)
                .await
                .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| ApiError::Internal(format!("Seek failed: {e}")))?;

            let reader = tokio::io::BufReader::new(file.take(length));
            let stream = tokio_util::io::ReaderStream::new(reader);
            let body = axum::body::Body::from_stream(stream);

            Ok((
                StatusCode::PARTIAL_CONTENT,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_LENGTH, length.to_string()),
                    (header::ACCEPT_RANGES, "bytes".to_string()),
                    (
                        header::CONTENT_RANGE,
                        format!("bytes {start}-{end}/{file_size}"),
                    ),
                    (header::ETAG, etag),
                ],
                body,
            )
                .into_response())
        }
        ByteRange::Unsatisfiable => Ok((
            StatusCode::RANGE_NOT_SATISFIABLE,
            [
                (header::CONTENT_RANGE, format!("bytes */{file_size}")),
                (header::ACCEPT_RANGES, "bytes".to_string()),
            ],
        )
            .into_response()),
        ByteRange::Ignore => {
            let file = tokio::fs::File::open(&file_path)
                .await
                .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

            let reader = tokio::io::BufReader::new(file);
            let stream = tokio_util::io::ReaderStream::new(reader);
            let body = axum::body::Body::from_stream(stream);

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CONTENT_LENGTH, file_size.to_string()),
                    (header::ACCEPT_RANGES, "bytes".to_string()),
                    (header::ETAG, etag),
                ],
                body,
            )
                .into_response())
        }
    }
}

/// How to answer a `Range` header (RFC 9110 §14.1.2).
#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    /// 206 with bytes `start..=end`.
    Partial(u64, u64),
    /// 416 with `Content-Range: bytes */<size>`.
    Unsatisfiable,
    /// Not a single byte range we serve: 200 with the whole file.
    Ignore,
}

/// Parse a `Range: bytes=START-END` header for a file of `file_size` bytes.
fn parse_range(range: &str, file_size: u64) -> ByteRange {
    let Some(spec) = range.strip_prefix("bytes=") else {
        return ByteRange::Ignore;
    };
    // Multi-range requests are not served; they get the whole file.
    if spec.contains(',') {
        return ByteRange::Ignore;
    }
    let Some((start_str, end_str)) = spec.split_once('-') else {
        return ByteRange::Ignore;
    };
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if start_str.is_empty() {
        // Suffix range: `bytes=-N` is the last N bytes.
        return match end_str.parse::<u64>() {
            Ok(0) => ByteRange::Unsatisfiable,
            Ok(n) if n > file_size => ByteRange::Ignore,
            Ok(n) => ByteRange::Partial(file_size - n, file_size - 1),
            Err(_) => ByteRange::Ignore,
        };
    }

    let Ok(start) = start_str.parse::<u64>() else {
        return ByteRange::Ignore;
    };
    // `A-` has no explicit end and runs to the last byte of the file.
    let end = if end_str.is_empty() {
        None
    } else {
        let Ok(end) = end_str.parse::<u64>() else {
            return ByteRange::Ignore;
        };
        if end < start {
            return ByteRange::Ignore;
        }
        Some(end)
    };
    if start >= file_size {
        return ByteRange::Unsatisfiable;
    }
    // start < file_size here, so file_size >= 1 and `file_size - 1` cannot underflow.
    let last = match end {
        Some(end) => end.min(file_size - 1),
        None => file_size - 1,
    };
    ByteRange::Partial(start, last)
}

fn mime_from_path(path: &str) -> String {
    let ext = FilePath::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // Video
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" | "m2ts" | "mts" | "m2t" => "video/mp2t",
        "mpg" | "mpeg" => "video/mpeg",
        "3gp" => "video/3gpp",
        // Audio
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "wma" => "audio/x-ms-wma",
        "aiff" | "aif" => "audio/aiff",
        "wv" => "audio/x-wavpack",
        "ape" => "audio/x-ape",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Audio Universal Endpoint
// ---------------------------------------------------------------------------

/// Query parameters for the universal audio endpoint
#[derive(Debug, Default)]
#[allow(dead_code)]
struct AudioUniversalQuery {
    max_audio_bitrate: Option<u32>,
    audio_stream_index: Option<u32>,
    start_time_ticks: Option<i64>,
    is_live: bool,
    media_source_id: Option<String>,
    device_id: Option<String>,
    audio_codec: Option<String>,
    enable_remote_media: bool,
    enable_direct_play: bool,
    enable_direct_stream: bool,
}

impl From<HashMap<String, String>> for AudioUniversalQuery {
    fn from(params: HashMap<String, String>) -> Self {
        Self {
            max_audio_bitrate: params.get("MaxAudioBitrate").and_then(|v| v.parse().ok()),
            audio_stream_index: params.get("AudioStreamIndex").and_then(|v| v.parse().ok()),
            start_time_ticks: params.get("StartTimeTicks").and_then(|v| v.parse().ok()),
            is_live: params
                .get("IsLive")
                .map(|v| v.parse().unwrap_or(false))
                .unwrap_or(false),
            media_source_id: params.get("MediaSourceId").cloned(),
            device_id: params.get("DeviceId").cloned(),
            audio_codec: params.get("AudioCodec").cloned(),
            enable_remote_media: params
                .get("EnableRemoteMedia")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_play: params
                .get("EnableDirectPlay")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_stream: params
                .get("EnableDirectStream")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
        }
    }
}

/// `GET /Audio/{id}/universal` — Primary music playback endpoint.
/// Returns a StreamingResponse with media sources and direct play/transcode URLs.
async fn audio_universal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let query = AudioUniversalQuery::from(params);

    // Single query: verify item exists and fetch media sources with streams
    let media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Audio item {id} not found")))?;

    // Select the appropriate media source
    let selected_source = if let Some(ref source_id) = query.media_source_id {
        media_sources
            .iter()
            .find(|s| s.id == *source_id)
            .cloned()
            .or_else(|| media_sources.first().cloned())
    } else {
        media_sources.first().cloned()
    };

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine if direct play is possible
    let can_direct_play = query.enable_direct_play
        && selected_source
            .as_ref()
            .is_some_and(|s| s.supports_direct_play);

    // Build the URL for direct play or transcoding
    let url = if can_direct_play {
        // Direct play URL
        Some(format!(
            "/Audio/{}/stream?MediaSourceId={}",
            id,
            selected_source
                .as_ref()
                .map_or("".to_string(), |s| s.id.clone())
        ))
    } else if query.enable_direct_stream {
        // Direct stream URL
        Some(format!("/Audio/{}/stream", id))
    } else {
        // Transcode URL (HLS)
        let mut url = format!(
            "/Videos/{}/main.m3u8?DeviceId={}",
            id,
            query.device_id.unwrap_or_default()
        );
        if let Some(bitrate) = query.max_audio_bitrate {
            url.push_str(&format!("&MaxAudioBitrate={}", bitrate));
        }
        if let Some(codec) = &query.audio_codec {
            url.push_str(&format!("&AudioCodec={}", codec));
        }
        Some(url)
    };

    // Update media sources with direct play flags based on client capabilities
    let mut response_sources = media_sources.clone();
    for source in &mut response_sources {
        source.supports_direct_play = query.enable_direct_play && source.supports_direct_play;
        source.supports_direct_stream = query.enable_direct_stream && source.supports_direct_stream;
        source.supports_transcoding = true;
    }

    let response = rustyfin_model::playback_info::StreamingResponse {
        media_sources: response_sources,
        url,
        analyze_duration_linear_percent: 100,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Audio/{id}/PlaybackInfo` — Returns playback info for an audio item.
async fn audio_playback_info(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(_params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Audio item {id} not found")))?;

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    let response = rustyfin_model::playback_info::PlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Video Progressive Stream Endpoint
// ---------------------------------------------------------------------------

/// `GET /Videos/{id}/progressive.stream` — Direct progressive download without Range support.
/// Clients use this for progressive playback where the entire file is streamed sequentially.
async fn stream_video_progressive(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let file_path = video_file(&state, id, &params).await?;

    let metadata = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| ApiError::NotFound(format!("File not accessible: {e}")))?;

    let file_size = metadata.len();
    let content_type = mime_from_path(&file_path);

    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot open file: {e}")))?;

    let reader = tokio::io::BufReader::new(file);
    let stream = tokio_util::io::ReaderStream::new(reader);
    let body = axum::body::Body::from_stream(stream);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_LENGTH, file_size.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "inline; filename=\"{}\"",
                    FilePath::new(&file_path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default()
                ),
            ),
        ],
        body,
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Video PlaybackInfo Endpoint
// ---------------------------------------------------------------------------

/// Query parameters for video PlaybackInfo
#[derive(Debug, Default)]
#[allow(dead_code)]
struct VideoPlaybackInfoQuery {
    max_video_bitrate: Option<u32>,
    max_audio_bitrate: Option<u32>,
    video_stream_index: Option<u32>,
    audio_stream_index: Option<u32>,
    subtitle_stream_index: Option<i32>,
    start_time_ticks: Option<i64>,
    is_live: bool,
    device_id: Option<String>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    transcode_to_subtitles: Option<String>,
    enable_remote_media: bool,
    enable_direct_play: bool,
    enable_direct_stream: bool,
    enable_video_stream_copy: bool,
    enable_audio_stream_copy: bool,
    width: Option<i32>,
    height: Option<i32>,
    max_streaming_bitrate: Option<u32>,
    segment_time: Option<u32>,
    profile: Option<String>,
    max_video_bit_depth: Option<u32>,
    h264_profile: Option<String>,
    h264_video_bit_depth: Option<String>,
}

impl From<HashMap<String, String>> for VideoPlaybackInfoQuery {
    fn from(params: HashMap<String, String>) -> Self {
        Self {
            max_video_bitrate: params.get("MaxVideoBitrate").and_then(|v| v.parse().ok()),
            max_audio_bitrate: params.get("MaxAudioBitrate").and_then(|v| v.parse().ok()),
            video_stream_index: params.get("VideoStreamIndex").and_then(|v| v.parse().ok()),
            audio_stream_index: params.get("AudioStreamIndex").and_then(|v| v.parse().ok()),
            subtitle_stream_index: params
                .get("SubtitleStreamIndex")
                .and_then(|v| v.parse().ok()),
            start_time_ticks: params.get("StartTimeTicks").and_then(|v| v.parse().ok()),
            is_live: params
                .get("IsLive")
                .map(|v| v.parse().unwrap_or(false))
                .unwrap_or(false),
            device_id: params.get("DeviceId").cloned(),
            // Support both singular (VideoCodec) and plural (VideoCodecs) parameter names
            video_codec: params
                .get("VideoCodecs")
                .cloned()
                .or_else(|| params.get("VideoCodec").cloned()),
            audio_codec: params
                .get("AudioCodecs")
                .cloned()
                .or_else(|| params.get("AudioCodec").cloned()),
            transcode_to_subtitles: params.get("TranscodeToSubtitles").cloned(),
            enable_remote_media: params
                .get("EnableRemoteMedia")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_play: params
                .get("EnableDirectPlay")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_direct_stream: params
                .get("EnableDirectStream")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_video_stream_copy: params
                .get("EnableVideoStreamCopy")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            enable_audio_stream_copy: params
                .get("EnableAudioStreamCopy")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            width: params.get("Width").and_then(|v| v.parse().ok()),
            height: params.get("Height").and_then(|v| v.parse().ok()),
            max_streaming_bitrate: params
                .get("MaxStreamingBitrate")
                .and_then(|v| v.parse().ok()),
            segment_time: params.get("SegmentTime").and_then(|v| v.parse().ok()),
            profile: params.get("Profile").cloned(),
            max_video_bit_depth: params.get("MaxVideoBitDepth").and_then(|v| v.parse().ok()),
            h264_profile: params.get("h264-profile").cloned(),
            h264_video_bit_depth: params.get("h264-videobitdepth").cloned(),
        }
    }
}

/// What a transcode must carry for a source, beyond what a direct play or stream would
#[derive(Debug, Default, PartialEq, Eq)]
struct TrackNeeds {
    /// The request names an audio stream other than the default one
    other_audio: bool,
    /// The request names a picture subtitle that the transcode burns in
    burn_in: bool,
}

/// The tracks that `query` names on `source` and that a direct play or stream would not carry
fn track_needs(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> TrackNeeds {
    let audio: Vec<_> = source
        .media_streams
        .iter()
        .filter(|s| is_stream_type(s, "Audio"))
        .collect();
    // The default audio is the first flagged one, else the first one
    let default_audio = audio
        .iter()
        .find(|s| s.is_default)
        .or_else(|| audio.first());
    let other_audio = match (query.audio_stream_index, default_audio) {
        (Some(wanted), Some(default)) => {
            let wanted = i64::from(wanted);
            audio.iter().any(|s| i64::from(s.index) == wanted) && i64::from(default.index) != wanted
        }
        _ => false,
    };
    // Picture subtitles are burned in by the transcode, text ones are drawn by the browser
    let burn_in = match query.subtitle_stream_index {
        Some(wanted) if wanted >= 0 => source.media_streams.iter().any(|s| {
            s.index == wanted
                && is_stream_type(s, "Subtitle")
                && s.codec
                    .as_deref()
                    .is_some_and(rustyfin_core::subtitle::is_image_subtitle)
        }),
        _ => false,
    };
    TrackNeeds {
        other_audio,
        burn_in,
    }
}

/// `GET /Videos/{id}/Universal` — Primary video playback endpoint.
/// Returns a StreamingResponse with media sources and direct play/transcode URLs.
async fn video_universal(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // The index of the version the request names; an id that names none is refused
    let index = match select_version(id, &media_sources, requested_source_id(&params))? {
        Version::Primary => 0,
        Version::Source(index) => index,
    };
    let query = VideoPlaybackInfoQuery::from(params);

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // The tracks the request names that a direct play or stream would not carry
    let needs = match media_sources.get(index) {
        Some(source) => track_needs(source, &query),
        None => TrackNeeds::default(),
    };

    // Determine direct play/stream/transcode capability
    let (supports_direct_play, supports_direct_stream, supports_transcoding) =
        if media_sources.is_empty() {
            // No media sources - fallback to transcoding if possible
            (false, false, true)
        } else {
            let source = &media_sources[index];

            let can_direct_play = query.enable_direct_play
                && !needs.other_audio
                && !needs.burn_in
                && check_direct_play(source, &query);
            let can_direct_stream = query.enable_direct_stream
                && !needs.other_audio
                && !needs.burn_in
                && check_direct_stream(source, &query);
            let can_transcode = true;

            (can_direct_play, can_direct_stream, can_transcode)
        };

    // Build the URL based on playback method
    let url = if supports_direct_play {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}&Static=true",
            id, media_sources[index].id
        ))
    } else if supports_direct_stream {
        Some(format!(
            "/Videos/{}/stream?MediaSourceId={}",
            id, media_sources[index].id
        ))
    } else if supports_transcoding {
        let mut url = format!(
            "/Videos/{}/main.m3u8?DeviceId={}",
            id,
            query.device_id.unwrap_or_default()
        );
        if let Some(source) = media_sources.get(index) {
            url.push_str(&format!("&MediaSourceId={}", source.id));
        }
        if let Some(bitrate) = query.max_video_bitrate {
            url.push_str(&format!("&VideoBitrate={}", bitrate));
        }
        if let Some(bitrate) = query.max_audio_bitrate {
            url.push_str(&format!("&AudioBitrate={}", bitrate));
        }
        if let Some(ref codec) = query.video_codec {
            url.push_str(&format!("&VideoCodec={}", codec));
        }
        if let Some(ref codec) = query.audio_codec {
            url.push_str(&format!("&AudioCodec={}", codec));
        }
        if let Some(width) = query.width {
            url.push_str(&format!("&MaxWidth={}", width));
        }
        if let Some(height) = query.height {
            url.push_str(&format!("&MaxHeight={}", height));
        }
        if let Some(ticks) = query.start_time_ticks {
            url.push_str(&format!("&StartTimeTicks={}", ticks));
        }
        if let Some(audio) = query.audio_stream_index {
            url.push_str(&format!("&AudioStreamIndex={}", audio));
        }
        if let Some(subtitle) = query.subtitle_stream_index.filter(|&i| i >= 0) {
            url.push_str(&format!("&SubtitleStreamIndex={}", subtitle));
        }
        if needs.burn_in {
            url.push_str("&SubtitleMethod=Encode");
        }
        if let Some(seg_time) = query.segment_time {
            url.push_str(&format!("&SegmentTime={}", seg_time));
        }
        url.push_str(&format!("&PlaySessionId={}", play_session_id));
        Some(url)
    } else {
        None
    };

    // Update media sources with direct play flags based on client capabilities
    for (i, source) in media_sources.iter_mut().enumerate() {
        source.supports_direct_play = i == index && supports_direct_play;
        source.supports_direct_stream = i == index && supports_direct_stream;
        source.supports_transcoding = i == index && supports_transcoding;
    }

    let response = rustyfin_model::playback_info::StreamingResponse {
        media_sources,
        url,
        analyze_duration_linear_percent: 100,
        play_session_id: Some(play_session_id),
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Videos/{id}/PlaybackInfo` — Returns playback info for a video item.
/// Includes direct play detection based on client codec support. Each source's default tracks
/// are the caller's remembered tracks, where the source has them.
async fn video_playback_info(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    // Single query: verify item exists and fetch media sources with streams
    let mut media_sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .ok_or_else(|| ApiError::NotFound(format!("Video item {id} not found")))?;

    // The subtitle files beside each video are listed after its streams
    for source in media_sources.iter_mut() {
        let external = crate::routes::subtitle::external_streams(source).await;
        source.media_streams.extend(external);
    }

    // The index of the version the request names; an id that names none is refused
    let index = match select_version(id, &media_sources, requested_source_id(&params))? {
        Version::Primary => 0,
        Version::Source(index) => index,
    };
    let query = VideoPlaybackInfoQuery::from(params);

    // Generate playback session ID
    let play_session_id = uuid::Uuid::new_v4().to_string();

    // Determine direct play/stream/transcode capability based on client codec support
    let (supports_direct_play, supports_direct_stream, supports_transcoding) = if media_sources
        .is_empty()
    {
        // No media sources - fallback to transcoding if possible
        (false, false, true)
    } else {
        let source = &media_sources[index];

        // Check if client can direct play
        let can_direct_play = query.enable_direct_play && check_direct_play(source, &query);

        // Check if client can direct stream (re-mux without transcoding)
        let can_direct_stream = query.enable_direct_stream && check_direct_stream(source, &query);

        // Transcoding is always possible if FFmpeg is available
        let can_transcode = true;

        (can_direct_play, can_direct_stream, can_transcode)
    };

    // Update media sources with direct play flags based on client capabilities
    for (i, source) in media_sources.iter_mut().enumerate() {
        source.supports_direct_play = i == index && supports_direct_play;
        source.supports_direct_stream = i == index && supports_direct_stream;
        source.supports_transcoding = i == index && supports_transcoding;
    }

    // The caller's remembered tracks become each source's defaults, where the source has them
    let (remembered_audio, remembered_subtitle) =
        rustyfin_db::repo_playback::get_stream_choice(&state.db_ro, claims.sub, id).await?;
    let media_sources: Vec<rustyfin_model::playback_info::PlaybackMediaSource> = media_sources
        .into_iter()
        .map(|source| {
            let default_audio_stream_index = remembered_audio
                .as_ref()
                .filter(|choice| has_remembered_stream(&source, "Audio", choice))
                .map(|choice| choice.index);
            let default_subtitle_stream_index = remembered_subtitle
                .as_ref()
                .filter(|choice| {
                    choice.index == -1 || has_remembered_stream(&source, "Subtitle", choice)
                })
                .map(|choice| choice.index);
            rustyfin_model::playback_info::PlaybackMediaSource {
                source,
                default_audio_stream_index,
                default_subtitle_stream_index,
            }
        })
        .collect();

    let response = rustyfin_model::playback_info::VideoPlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
        can_play: true,
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&response)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// `GET /Videos/{id}/Chapters` — Returns chapter markers for a video item.
async fn get_chapters(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let chapters = rustyfin_db::repo_chapter::get_chapters(&state.db_ro, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&chapters)
            .map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?,
    )
        .into_response())
}

/// Whether the first video and audio streams use a codec from the client's lists.
/// A list without entries falls back to the codecs a client is assumed to play.
fn codecs_playable(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // Comma-separated, e.g. "h264,hevc,vp9"
    let codec_list = |list: Option<&str>, fallback: &[&str]| -> Vec<String> {
        let entries: Vec<String> = list
            .unwrap_or_default()
            .split(',')
            .map(|c| c.trim().to_lowercase())
            .filter(|c| !c.is_empty())
            .collect();
        if entries.is_empty() {
            fallback.iter().map(|c| c.to_string()).collect()
        } else {
            entries
        }
    };
    let video_codecs = codec_list(query.video_codec.as_deref(), &["h264", "avc1"]);
    let audio_codecs = codec_list(query.audio_codec.as_deref(), &["aac", "mp4a", "mp3"]);

    // A stream without a codec passes
    let listed = |codec_type: &str, codecs: &[String]| {
        source
            .media_streams
            .iter()
            .find(|s| is_stream_type(s, codec_type))
            .and_then(|s| s.codec.as_ref())
            .is_none_or(|codec| {
                let codec = codec.to_lowercase();
                codecs.iter().any(|c| codec.contains(c.as_str()))
            })
    };
    listed("Video", &video_codecs) && listed("Audio", &audio_codecs)
}

/// Whether `stream` is of `kind` ("Video", "Audio"). The probe stores ffprobe's lowercase types.
fn is_stream_type(stream: &rustyfin_model::media_source::MediaStream, kind: &str) -> bool {
    stream.codec_type.eq_ignore_ascii_case(kind)
}

/// Whether the client takes the first video stream of `source` as it is, by the rule in
/// `transcode::client_takes_h264`. That is the stream `codecs_playable` checks; one that isn't
/// H.264, or no video at all, has nothing to refuse.
fn client_takes_h264_video(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    use super::transcode::{H264Request, client_takes_h264};

    let Some(video) = source
        .media_streams
        .iter()
        .find(|s| is_stream_type(s, "Video"))
    else {
        return true;
    };
    let is_h264 = video.codec.as_deref().is_some_and(|codec| {
        ["h264", "avc", "avc1"]
            .iter()
            .any(|name| codec.eq_ignore_ascii_case(name))
    });
    if !is_h264 {
        return true;
    }
    let request = H264Request {
        profile: query.profile.as_deref(),
        h264_profile: query.h264_profile.as_deref(),
        max_video_bit_depth: query.max_video_bit_depth,
        h264_video_bit_depth: query.h264_video_bit_depth.as_deref(),
    };
    client_takes_h264(request, video.profile.as_deref(), video.bit_depth)
}

/// Whether `source` has a stream of `kind` ("Audio", "Subtitle") at the choice's index whose key is
/// the one stored with the choice. A choice with no stored key never matches.
fn has_remembered_stream(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    kind: &str,
    choice: &StreamChoice,
) -> bool {
    let Some(key) = choice.key.as_deref() else {
        return false;
    };
    source
        .media_streams
        .iter()
        .any(|s| s.index == choice.index && is_stream_type(s, kind) && s.identity().key() == key)
}

/// Check if a media source can be directly played by the client.
/// Direct play requires the client to support all codecs without any transcoding.
fn check_direct_play(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // If no media streams are available, we cannot verify codec compatibility.
    // Reject direct play to avoid Format errors in the browser.
    let has_video_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Video"));
    let has_audio_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Audio"));
    if !has_video_stream && !has_audio_stream {
        // No stream info at all - cannot verify compatibility, reject direct play
        return false;
    }
    // If there's a video item but no video stream info, reject direct play
    if !has_video_stream && source.video_bitrate.is_some() {
        return false;
    }

    // Browsers can only play video in MP4/M4V/WebM containers natively.
    // MKV can sometimes be direct-streamed (remuxed) if codecs are compatible.
    let container = source.container.as_ref().map(|s| s.to_lowercase());
    let browser_compatible_containers = ["mp4", "m4v", "webm"];
    let remux_compatible_containers = ["mkv", "mka"];
    if let Some(ref cont) = container {
        if browser_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            // Native browser container - OK for direct play
        } else if remux_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            // MKV/MKA: allow direct stream (remux) but not direct play
            // This is handled by check_direct_stream separately
            return false;
        } else {
            return false;
        }
    } else {
        // If container is unknown, check the file extension
        let path_lower = source.path.to_lowercase();
        let browser_compatible_extensions = [".mp4", ".m4v", ".webm"];
        if !browser_compatible_extensions
            .iter()
            .any(|ext| path_lower.ends_with(ext))
        {
            return false;
        }
    }

    // Codecs must be in the client's lists, or in the fallback lists if it sent none
    if !codecs_playable(source, query) {
        return false;
    }
    // The client must take H.264 video as it is, even when it sent no list
    if !client_takes_h264_video(source, query) {
        return false;
    }
    // Without any list, the codec check is the whole answer
    let sent_list = [&query.video_codec, &query.audio_codec]
        .into_iter()
        .flatten()
        .any(|list| list.split(',').any(|c| !c.trim().is_empty()));
    if !sent_list {
        return true;
    }

    // Check bitrate constraints
    if let Some(max_bitrate) = query.max_video_bitrate
        && let Some(video_bitrate) = source.video_bitrate
        && video_bitrate > max_bitrate
    {
        return false;
    }

    if let Some(max_audio_bitrate) = query.max_audio_bitrate
        && let Some(audio_bitrate) = source.audio_bitrate
        && audio_bitrate > max_audio_bitrate
    {
        return false;
    }

    true
}

/// Check if a media source can be directly streamed (re-muxed without transcoding).
/// Direct stream allows container conversion but no codec transcoding.
fn check_direct_stream(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    query: &VideoPlaybackInfoQuery,
) -> bool {
    // If direct stream is not enabled, return false
    if !query.enable_direct_stream {
        return false;
    }

    // If no media streams are available, we cannot verify codec compatibility.
    // Reject direct stream to avoid Format errors in the browser.
    let has_video_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Video"));
    let has_audio_stream = source
        .media_streams
        .iter()
        .any(|s| is_stream_type(s, "Audio"));
    if !has_video_stream && !has_audio_stream {
        return false;
    }

    // Browsers can only play video in MP4/M4V containers natively.
    // MKV, AVI, TS, etc. require transcoding to a browser-compatible format.
    let container = source.container.as_ref().map(|s| s.to_lowercase());
    let browser_compatible_containers = ["mp4", "m4v", "webm"];
    if let Some(ref cont) = container {
        if !browser_compatible_containers
            .iter()
            .any(|c| cont.ends_with(c))
        {
            return false;
        }
    } else {
        // If container is unknown, check the file extension
        let path_lower = source.path.to_lowercase();
        let browser_compatible_extensions = [".mp4", ".m4v", ".webm"];
        if !browser_compatible_extensions
            .iter()
            .any(|ext| path_lower.ends_with(ext))
        {
            return false;
        }
    }

    // Codecs must be in the client's lists, or in the fallback lists if it sent none
    if !codecs_playable(source, query) {
        return false;
    }
    // The client must take H.264 video as it is
    if !client_takes_h264_video(source, query) {
        return false;
    }

    // If video stream copy is not enabled and there's a video stream, need transcoding
    if !query.enable_video_stream_copy && has_video_stream {
        return false;
    }

    // If audio stream copy is not enabled and there's an audio stream, need transcoding
    if !query.enable_audio_stream_copy && has_audio_stream {
        return false;
    }

    // Check bitrate constraints — direct stream may still need to respect bitrate limits
    // via container-level bitrate reduction, which is not always possible
    if let Some(max_bitrate) = query.max_streaming_bitrate
        && let Some(total_bitrate) = source.bitrate
        && total_bitrate > max_bitrate
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_range ──

    #[test]
    fn test_parse_range_full_range() {
        assert_eq!(parse_range("bytes=0-999", 1000), ByteRange::Partial(0, 999));
    }

    #[test]
    fn test_parse_range_partial() {
        assert_eq!(
            parse_range("bytes=100-199", 1000),
            ByteRange::Partial(100, 199)
        );
    }

    #[test]
    fn test_parse_range_open_ended() {
        assert_eq!(
            parse_range("bytes=500-", 1000),
            ByteRange::Partial(500, 999)
        );
    }

    #[test]
    fn test_parse_range_suffix() {
        assert_eq!(
            parse_range("bytes=-100", 1000),
            ByteRange::Partial(900, 999)
        );
    }

    #[test]
    fn test_parse_range_clamp_end_to_file_size() {
        assert_eq!(
            parse_range("bytes=0-9999", 1000),
            ByteRange::Partial(0, 999)
        );
    }

    #[test]
    fn test_parse_range_invalid_no_bytes_prefix() {
        assert_eq!(parse_range("0-999", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_start_beyond_file() {
        assert_eq!(
            parse_range("bytes=1000-1100", 1000),
            ByteRange::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_start_greater_than_end() {
        assert_eq!(parse_range("bytes=500-100", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_empty_string() {
        assert_eq!(parse_range("", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_invalid_numbers() {
        assert_eq!(parse_range("bytes=abc-def", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_suffix_larger_than_file() {
        assert_eq!(parse_range("bytes=-1001", 1000), ByteRange::Ignore);
    }

    // RFC 9110 §14.1.2: a suffix of length 0 and a first byte at or past the end are unsatisfiable (416).

    #[test]
    fn test_parse_range_zero_suffix_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_start_far_beyond_file_is_unsatisfiable() {
        assert_eq!(
            parse_range("bytes=999999999-", 1000),
            ByteRange::Unsatisfiable
        );
    }

    #[test]
    fn test_parse_range_start_at_file_size_is_unsatisfiable() {
        assert_eq!(parse_range("bytes=1000-", 1000), ByteRange::Unsatisfiable);
    }

    #[test]
    fn test_parse_range_huge_numbers_do_not_overflow() {
        assert_eq!(
            parse_range("bytes=18446744073709551615-", 1000),
            ByteRange::Unsatisfiable
        );
        assert_eq!(
            parse_range("bytes=0-18446744073709551615", 1000),
            ByteRange::Partial(0, 999)
        );
        assert_eq!(
            parse_range("bytes=-18446744073709551615", 1000),
            ByteRange::Ignore
        );
        assert_eq!(
            parse_range("bytes=18446744073709551616-", 1000),
            ByteRange::Ignore
        );
    }

    #[test]
    fn test_parse_range_empty_file() {
        assert_eq!(parse_range("bytes=0-", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 0), ByteRange::Unsatisfiable);
        assert_eq!(parse_range("bytes=-5", 0), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_single_bytes() {
        assert_eq!(parse_range("bytes=0-0", 1000), ByteRange::Partial(0, 0));
        assert_eq!(
            parse_range("bytes=999-", 1000),
            ByteRange::Partial(999, 999)
        );
        assert_eq!(parse_range("bytes=-1", 1000), ByteRange::Partial(999, 999));
    }

    #[test]
    fn test_parse_range_suffix_of_whole_file() {
        assert_eq!(parse_range("bytes=-1000", 1000), ByteRange::Partial(0, 999));
    }

    #[test]
    fn test_parse_range_multiple_ranges_are_ignored() {
        assert_eq!(parse_range("bytes=0-0,5-9", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=-0,0-9", 1000), ByteRange::Ignore);
    }

    #[test]
    fn test_parse_range_without_numbers_is_ignored() {
        assert_eq!(parse_range("bytes=-", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=", 1000), ByteRange::Ignore);
        assert_eq!(parse_range("bytes=12", 1000), ByteRange::Ignore);
    }

    // ── mime_from_path ──

    #[test]
    fn test_mime_mp4() {
        assert_eq!(mime_from_path("/path/to/video.mp4"), "video/mp4");
    }

    #[test]
    fn test_mime_mkv() {
        assert_eq!(mime_from_path("/path/to/video.mkv"), "video/x-matroska");
    }

    #[test]
    fn test_mime_avi() {
        assert_eq!(mime_from_path("/path/to/video.avi"), "video/x-msvideo");
    }

    #[test]
    fn test_mime_webm() {
        assert_eq!(mime_from_path("/path/to/video.webm"), "video/webm");
    }

    #[test]
    fn test_mime_mov() {
        assert_eq!(mime_from_path("/path/to/video.mov"), "video/quicktime");
    }

    #[test]
    fn test_mime_wmv() {
        assert_eq!(mime_from_path("/path/to/video.wmv"), "video/x-ms-wmv");
    }

    #[test]
    fn test_mime_ts() {
        assert_eq!(mime_from_path("/path/to/video.ts"), "video/mp2t");
    }

    /// A folder under the temp dir, removed when dropped.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("rustyfin-r1play-{}", Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn path(&self) -> &FilePath {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn r1play_mime_m2ts_and_mts() {
        assert_eq!(mime_from_path("/path/to/video.m2ts"), "video/mp2t");
        assert_eq!(mime_from_path("/path/to/VIDEO.MTS"), "video/mp2t");
        assert_eq!(mime_from_path("/path/to/video.m2t"), "video/mp2t");
    }

    #[test]
    fn r1play_symlinked_file_inside_library_is_allowed() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, lib.join("Movie")).unwrap();
        let file = lib.join("Movie/movie.mkv");
        let canonical = file.canonicalize().unwrap();
        assert!(path_is_under_library(
            &file,
            &canonical,
            lib.to_str().unwrap()
        ));
    }

    #[test]
    fn r1play_dotdot_escape_is_rejected() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(dir.path().join("secret.mkv"), b"x").unwrap();
        let file = lib.join("../secret.mkv");
        let canonical = file.canonicalize().unwrap();
        assert!(!path_is_under_library(
            &file,
            &canonical,
            lib.to_str().unwrap()
        ));
        assert_eq!(lexical_normalize(FilePath::new("/../x")), None);
        assert_eq!(lexical_normalize(FilePath::new("rel/x")), None);
    }

    #[test]
    fn r1play_dotdot_after_a_link_is_rejected() {
        // lib/link -> /dir/srv/a; lib/link/sub/../../x reads /dir/srv/x, outside the library,
        // although dropping the `..` pairs as written gives lib/x
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let target = dir.path().join("srv/a");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(target.join("sub")).unwrap();
        std::fs::write(dir.path().join("srv/x"), b"x").unwrap();
        std::os::unix::fs::symlink(&target, lib.join("link")).unwrap();
        let file = lib.join("link/sub/../../x");
        let canonical = file.canonicalize().unwrap();
        assert_eq!(canonical, dir.path().join("srv/x").canonicalize().unwrap());
        assert!(!path_is_under_library(
            &file,
            &canonical,
            lib.to_str().unwrap()
        ));
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let libs = [lib.to_str().unwrap().to_string()];
        assert!(!path_allowed(
            file.to_str().unwrap(),
            &libs,
            &owned(&data, &[])
        ));
        assert!(has_parent_dir(&file));
    }

    #[test]
    fn r1play_dotdot_and_unresolvable_paths_are_refused() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        std::fs::create_dir_all(lib.join("Movie")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(lib.join("movie.mkv"), b"x").unwrap();
        let libs = [lib.to_str().unwrap().to_string()];
        let path = |p: &str| lib.join(p).to_str().unwrap().to_string();
        assert!(path_allowed(&path("movie.mkv"), &libs, &owned(&data, &[])));
        // `..` that stays inside the library is refused too: it is never resolved
        assert!(!path_allowed(
            &path("Movie/../movie.mkv"),
            &libs,
            &owned(&data, &[]),
        ));
        // A path whose links can't be resolved is refused, not checked as written
        assert!(!path_allowed(
            &path("missing.mkv"),
            &libs,
            &owned(&data, &[])
        ));
        std::os::unix::fs::symlink(lib.join("loop.mkv"), lib.join("loop.mkv")).unwrap();
        assert!(!path_allowed(&path("loop.mkv"), &libs, &owned(&data, &[])));
    }

    /// The owned paths of a server with `data` and the separate owned files `files`.
    fn owned(data: &FilePath, files: &[&FilePath]) -> OwnedPaths {
        OwnedPaths {
            data_dir: data.to_path_buf(),
            files: files.iter().map(|f| f.to_path_buf()).collect(),
        }
    }

    #[test]
    fn r1play_link_into_the_data_dir_is_refused() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        let elsewhere = dir.path().join("elsewhere");
        for d in [&lib, &data, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(data.join("rustyfin.db"), b"x").unwrap();
        std::fs::write(elsewhere.join("movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(data.join("rustyfin.db"), lib.join("db.mkv")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, lib.join("Movie")).unwrap();
        let libs = [lib.to_str().unwrap().to_string()];
        let path = |p: &str| lib.join(p).to_str().unwrap().to_string();
        assert!(!path_allowed(&path("db.mkv"), &libs, &owned(&data, &[])));
        assert!(path_allowed(
            &path("Movie/movie.mkv"),
            &libs,
            &owned(&data, &[])
        ));
        // A library kept inside the data dir (Docker /data with /data/media) still plays
        let inner = data.join("media");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("clip.mkv"), b"x").unwrap();
        let inner_libs = [inner.to_str().unwrap().to_string()];
        let clip = inner.join("clip.mkv");
        assert!(path_allowed(
            clip.to_str().unwrap(),
            &inner_libs,
            &owned(&data, &[]),
        ));
    }

    const R3_JPEG: &[u8] = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00";
    const R3_PNG: &[u8] = b"\x89PNG\r\n\x1A\n\x00\x00\x00\x0DIHDR";

    #[test]
    fn r3playback_pictures_are_told_from_other_files() {
        for head in [
            R3_JPEG,
            R3_PNG,
            b"GIF89a\x01\x00".as_slice(),
            b"RIFF\x24\x00\x00\x00WEBPVP8 ".as_slice(),
            b"\x00\x00\x00\x1Cftypavif".as_slice(),
            b"BM\x36\x00".as_slice(),
            b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>".as_slice(),
            b"\xEF\xBB\xBF  <svg viewBox=\"0 0 1 1\"></svg>".as_slice(),
        ] {
            assert!(looks_like_image(head), "{head:?}");
        }
        for head in [
            b"SQLite format 3\x00".as_slice(),
            b"{\"ServerName\":\"x\"}".as_slice(),
            b"-----BEGIN PRIVATE KEY-----".as_slice(),
            b"root:x:0:0:root:/root:/bin/bash".as_slice(),
            b"<?xml version=\"1.0\"?><movie/>".as_slice(),
            b"\x1A\x45\xDF\xA3".as_slice(),
            b"".as_slice(),
            b"RIFF\x24\x00\x00\x00WAVEfmt ".as_slice(),
        ] {
            assert!(!looks_like_image(head), "{head:?}");
        }
    }

    #[test]
    fn r3playback_image_paths_refuse_owned_files_links_and_other_files() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        let import = dir.path().join("jellyfin").join("metadata");
        for d in [&lib, &data, &import] {
            std::fs::create_dir_all(d).unwrap();
        }
        for sub in ["metadata", "images", "cache", "logs", "backups", "plugins"] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
        }
        let tls = dir.path().join("key.pem");
        std::fs::write(&tls, b"-----BEGIN PRIVATE KEY-----").unwrap();
        std::fs::write(data.join("rustyfin.db"), b"SQLite format 3\x00").unwrap();
        std::fs::write(data.join("config.json"), b"{}").unwrap();
        let owned = owned(&data, &[&tls]);
        let s = |p: &std::path::Path| p.to_str().unwrap().to_string();

        // Pictures beside the media, in the server's picture folders and in an imported folder
        let poster = lib.join("poster.jpg");
        std::fs::write(&poster, R3_JPEG).unwrap();
        for folder in ["metadata", "images", "cache"] {
            let file = data.join(folder).join("a.png");
            std::fs::write(&file, R3_PNG).unwrap();
            assert!(image_path_allowed(&s(&file), &owned), "{folder}");
        }
        let imported = import.join("folder.jpg");
        std::fs::write(&imported, R3_JPEG).unwrap();
        assert!(image_path_allowed(&s(&poster), &owned));
        assert!(image_path_allowed(&s(&imported), &owned));
        // A link to a picture elsewhere still shows, as Jellyfin follows links too
        std::os::unix::fs::symlink(&imported, lib.join("fanart.jpg")).unwrap();
        assert!(image_path_allowed(&s(&lib.join("fanart.jpg")), &owned));

        // The database, config and TLS key, stored or reached by a link with a picture's name
        for (name, target) in [
            ("db.jpg", data.join("rustyfin.db")),
            ("config.jpg", data.join("config.json")),
            ("key.jpg", tls.clone()),
        ] {
            std::os::unix::fs::symlink(&target, lib.join(name)).unwrap();
            assert!(!image_path_allowed(&s(&lib.join(name)), &owned), "{name}");
            assert!(!image_path_allowed(&s(&target), &owned), "{name}");
        }
        // A picture in a folder that holds none (logs, backups, plugins) is refused all the same
        for folder in ["logs", "backups", "plugins"] {
            let file = data.join(folder).join("b.jpg");
            std::fs::write(&file, R3_JPEG).unwrap();
            assert!(!image_path_allowed(&s(&file), &owned), "{folder}");
        }
        // A file that is no picture, `..`, a relative path and a missing file are refused
        let text = lib.join("notes.jpg");
        std::fs::write(&text, b"root:x:0:0:root:/root:/bin/bash").unwrap();
        assert!(!image_path_allowed(&s(&text), &owned));
        let dotdot = format!("{}/../lib/poster.jpg", lib.display());
        assert!(!image_path_allowed(&dotdot, &owned));
        assert!(!image_path_allowed("poster.jpg", &owned));
        assert!(!image_path_allowed(&s(&lib.join("missing.jpg")), &owned));
        assert!(!image_path_allowed(&s(&lib), &owned));
    }

    #[test]
    fn r1play_data_dir_inside_a_library_is_refused() {
        // A library at /srv with RUSTYFIN_DATA=/srv/rustyfin
        let dir = TempDir::new();
        let srv = dir.path().join("srv");
        let data = srv.join("rustyfin");
        std::fs::create_dir_all(data.join("cache")).unwrap();
        std::fs::create_dir_all(srv.join("Movies")).unwrap();
        std::fs::write(data.join("rustyfin.db"), b"x").unwrap();
        std::fs::write(data.join("cache/x.mkv"), b"x").unwrap();
        std::fs::write(srv.join("Movies/movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(data.join("rustyfin.db"), srv.join("Movies/db.mkv")).unwrap();
        let libs = [srv.to_str().unwrap().to_string()];
        let path = |p: &FilePath| p.to_str().unwrap().to_string();
        assert!(path_allowed(
            &path(&srv.join("Movies/movie.mkv")),
            &libs,
            &owned(&data, &[]),
        ));
        for refused in [
            srv.join("Movies/db.mkv"),
            data.join("cache/x.mkv"),
            data.join("rustyfin.db"),
        ] {
            assert!(
                !path_allowed(&path(&refused), &libs, &owned(&data, &[])),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn r1play_database_file_outside_the_data_dir_is_refused() {
        // RUSTYFIN_DB=/dir/db/main.db, outside RUSTYFIN_DATA, with links to it and its siblings
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        let db_dir = dir.path().join("db");
        for d in [&lib, &data, &db_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let db = db_dir.join("main.db");
        let libs = [lib.to_str().unwrap().to_string()];
        for name in ["main.db", "main.db-wal", "main.db-shm", "other.db"] {
            std::fs::write(db_dir.join(name), b"x").unwrap();
            std::os::unix::fs::symlink(db_dir.join(name), lib.join(format!("{name}.mkv"))).unwrap();
        }
        let path = |name: &str| {
            lib.join(format!("{name}.mkv"))
                .to_str()
                .unwrap()
                .to_string()
        };
        for name in ["main.db", "main.db-wal", "main.db-shm"] {
            assert!(
                !path_allowed(&path(name), &libs, &owned(&data, &[&db])),
                "{name}"
            );
        }
        assert!(path_allowed(
            &path("other.db"),
            &libs,
            &owned(&data, &[&db])
        ));
        assert!(path_allowed(&path("main.db"), &libs, &owned(&data, &[])));
    }

    #[test]
    fn r1play_media_in_the_data_dir_plays_owned_files_do_not() {
        // RUSTYFIN_DATA=/data, a library at /data/media, the database at its default place
        let dir = TempDir::new();
        let data = dir.path().join("data");
        let media = data.join("media");
        std::fs::create_dir_all(media.join("Movie")).unwrap();
        std::fs::create_dir_all(data.join("cache/images")).unwrap();
        std::fs::write(media.join("Movie/movie.mkv"), b"x").unwrap();
        for owned_file in [
            "rustyfin.db",
            "rustyfin.db-wal",
            "config.json",
            "passwordreset-ab12.json",
            "cache/images/poster.jpg",
        ] {
            std::fs::write(data.join(owned_file), b"x").unwrap();
            let link = media.join(format!("{}.mkv", owned_file.replace('/', "_")));
            std::os::unix::fs::symlink(data.join(owned_file), &link).unwrap();
        }
        let db = data.join("rustyfin.db");
        let owned = owned(&data, &[&db]);
        let libs = [media.to_str().unwrap().to_string()];
        let lib_path = |p: &str| media.join(p).to_str().unwrap().to_string();
        assert!(path_allowed(&lib_path("Movie/movie.mkv"), &libs, &owned));
        for refused in [
            "rustyfin.db.mkv",
            "rustyfin.db-wal.mkv",
            "config.json.mkv",
            "passwordreset-ab12.json.mkv",
            "cache_images_poster.jpg.mkv",
        ] {
            assert!(
                !path_allowed(&lib_path(refused), &libs, &owned),
                "{refused}"
            );
        }
        // The same owned files straight from a library at the data dir itself
        let root = [data.to_str().unwrap().to_string()];
        let data_path = |p: &str| data.join(p).to_str().unwrap().to_string();
        assert!(path_allowed(
            &data_path("media/Movie/movie.mkv"),
            &root,
            &owned
        ));
        assert!(!path_allowed(&data_path("rustyfin.db"), &root, &owned));
        assert!(!path_allowed(
            &data_path("cache/images/poster.jpg"),
            &root,
            &owned
        ));
    }

    #[test]
    fn r1play_linked_tls_key_and_database_are_refused() {
        // certbot: RUSTYFIN_TLS_KEY=live/x/privkey.pem -> ../../archive/x/privkey1.pem, and
        // RUSTYFIN_DB=conf/main.db -> store/real.db, whose WAL SQLite keeps beside real.db
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        let archive = dir.path().join("letsencrypt/archive/x");
        let live = dir.path().join("letsencrypt/live/x");
        let store = dir.path().join("store");
        let conf = dir.path().join("conf");
        for d in [&lib, &data, &archive, &live, &store, &conf] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(archive.join("privkey1.pem"), b"x").unwrap();
        std::os::unix::fs::symlink("../../archive/x/privkey1.pem", live.join("privkey.pem"))
            .unwrap();
        for name in ["real.db", "real.db-wal", "real.db-shm", "other.db"] {
            std::fs::write(store.join(name), b"x").unwrap();
        }
        std::os::unix::fs::symlink(store.join("real.db"), conf.join("main.db")).unwrap();
        let links = [
            ("key.mkv", archive.join("privkey1.pem")),
            ("db.mkv", store.join("real.db")),
            ("wal.mkv", store.join("real.db-wal")),
            ("shm.mkv", store.join("real.db-shm")),
            ("other.mkv", store.join("other.db")),
        ];
        for (name, to) in &links {
            std::os::unix::fs::symlink(to, lib.join(name)).unwrap();
        }
        let key = live.join("privkey.pem");
        let db = conf.join("main.db");
        let owned = owned(&data, &[&key, &db]);
        let libs = [lib.to_str().unwrap().to_string()];
        let path = |p: &str| lib.join(p).to_str().unwrap().to_string();
        for refused in ["key.mkv", "db.mkv", "wal.mkv", "shm.mkv"] {
            assert!(!path_allowed(&path(refused), &libs, &owned), "{refused}");
        }
        assert!(path_allowed(&path("other.mkv"), &libs, &owned));
    }

    #[test]
    fn r1play_owned_folder_linked_out_of_the_data_dir_is_refused() {
        // /data/transcodes -> /fast/transcodes
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        let fast = dir.path().join("fast/transcodes/abc");
        for d in [&lib, &data, &fast] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(fast.join("segment_0000.ts"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("fast/transcodes"), data.join("transcodes"))
            .unwrap();
        std::os::unix::fs::symlink(&fast, lib.join("seg")).unwrap();
        let libs = [lib.to_str().unwrap().to_string()];
        let seg = lib.join("seg/segment_0000.ts");
        assert!(!path_allowed(
            seg.to_str().unwrap(),
            &libs,
            &owned(&data, &[])
        ));
    }

    #[test]
    fn r1play_owned_names_ignore_ascii_case_and_cover_temp_files() {
        let owned = owned(FilePath::new("/r1play-missing/data"), &[]);
        for name in [
            "CACHE/images/x.jpg",
            "Logs/server.log",
            "Config.JSON",
            "config.json.tmp",
            "RUSTYFIN.DB-WAL",
            "PasswordReset-ab12.JSON",
        ] {
            let target = FilePath::new("/r1play-missing/data").join(name);
            assert!(owned.owns(&target), "{name}");
        }
        for name in [
            "Media/cache/x.mkv",
            "cached/x.mkv",
            "config.json.bak",
            "Movie.mkv",
        ] {
            let target = FilePath::new("/r1play-missing/data").join(name);
            assert!(!owned.owns(&target), "{name}");
        }
    }

    #[test]
    fn r1play_owned_paths_need_no_resolving() {
        // A data dir and key file that can't be resolved are matched as configured
        let owned = owned(
            FilePath::new("/r1play-missing/data"),
            &[FilePath::new("/r1play-missing/tls/key.pem")],
        );
        assert!(owned.owns(FilePath::new("/r1play-missing/data/rustyfin.db-shm")));
        assert!(owned.owns(FilePath::new("/r1play-missing/data/logs/server.log")));
        assert!(owned.owns(FilePath::new("/r1play-missing/tls/key.pem")));
        assert!(!owned.owns(FilePath::new("/r1play-missing/data/media/a.mkv")));
        assert!(!owned.owns(FilePath::new("/r1play-missing/data/cache")));
    }

    #[tokio::test]
    async fn r1play_database_file_comes_from_sqlite() {
        let dir = TempDir::new();
        let db = dir.path().join("main.db");
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        let file = database_file(&pool).await.unwrap();
        assert_eq!(file.canonicalize().unwrap(), db.canonicalize().unwrap());
        pool.close().await;
    }

    #[test]
    fn test_mime_mpeg() {
        assert_eq!(mime_from_path("/path/to/video.mpeg"), "video/mpeg");
    }

    #[test]
    fn test_mime_mpg() {
        assert_eq!(mime_from_path("/path/to/video.mpg"), "video/mpeg");
    }

    #[test]
    fn test_mime_mp3() {
        assert_eq!(mime_from_path("/path/to/song.mp3"), "audio/mpeg");
    }

    #[test]
    fn test_mime_flac() {
        assert_eq!(mime_from_path("/path/to/song.flac"), "audio/flac");
    }

    #[test]
    fn test_mime_ogg() {
        assert_eq!(mime_from_path("/path/to/song.ogg"), "audio/ogg");
    }

    #[test]
    fn test_mime_opus() {
        assert_eq!(mime_from_path("/path/to/song.opus"), "audio/opus");
    }

    #[test]
    fn test_mime_wav() {
        assert_eq!(mime_from_path("/path/to/song.wav"), "audio/wav");
    }

    #[test]
    fn test_mime_aac() {
        assert_eq!(mime_from_path("/path/to/song.aac"), "audio/aac");
    }

    #[test]
    fn test_mime_m4a() {
        assert_eq!(mime_from_path("/path/to/song.m4a"), "audio/mp4");
    }

    #[test]
    fn test_mime_unknown_extension() {
        assert_eq!(
            mime_from_path("/path/to/file.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_mime_no_extension() {
        assert_eq!(mime_from_path("/path/to/file"), "application/octet-stream");
    }

    #[test]
    fn test_mime_case_insensitive_extension() {
        assert_eq!(mime_from_path("/path/to/video.MP4"), "video/mp4");
    }

    #[test]
    fn test_mime_uppercase_extension() {
        assert_eq!(mime_from_path("/path/to/video.MKV"), "video/x-matroska");
    }

    #[test]
    fn test_mime_m4v() {
        assert_eq!(mime_from_path("/path/to/video.m4v"), "video/mp4");
    }

    #[test]
    fn test_mime_wma() {
        assert_eq!(mime_from_path("/path/to/song.wma"), "audio/x-ms-wma");
    }

    #[test]
    fn test_mime_aiff() {
        assert_eq!(mime_from_path("/path/to/song.aiff"), "audio/aiff");
    }

    #[test]
    fn test_mime_3gp() {
        assert_eq!(mime_from_path("/path/to/video.3gp"), "video/3gpp");
    }

    // ── check_direct_play / check_direct_stream ──

    const LISTS: &str = "VideoCodecs=h264,vp8,vp9,av1&AudioCodecs=aac,mp3,opus,flac";

    /// A source in `container` with one video and one audio stream and a 4 Mbit/s video bitrate.
    fn source_with(
        container: &str,
        video: &str,
        audio: &str,
    ) -> rustyfin_model::media_source::MediaSourceResponse {
        serde_json::from_value(serde_json::json!({
            "Id": "source",
            "Uid": "source",
            "Container": container,
            "Protocol": "File",
            "Path": format!("/media/movie.{container}"),
            "VideoBitrate": 4_000_000,
            "MediaStreams": [
                { "Index": 0, "Codec": video, "CodecType": "Video" },
                { "Index": 1, "Codec": audio, "CodecType": "Audio" },
            ],
        }))
        .unwrap()
    }

    /// The query of a client that sends `params` (`Key=value&...`).
    fn query_from(params: &str) -> VideoPlaybackInfoQuery {
        let params: HashMap<String, String> = params
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        VideoPlaybackInfoQuery::from(params)
    }

    #[test]
    fn test_direct_play_and_stream_need_listed_codecs() {
        let query = query_from(LISTS);
        // The browser plays the video but not the AC3 audio, or the audio but not the HEVC video
        for (video, audio) in [("h264", "ac3"), ("hevc", "aac")] {
            let source = source_with("mp4", video, audio);
            assert!(!check_direct_play(&source, &query), "{video} + {audio}");
            assert!(!check_direct_stream(&source, &query), "{video} + {audio}");
        }
        let source = source_with("mp4", "h264", "aac");
        assert!(check_direct_play(&source, &query));
        assert!(check_direct_stream(&source, &query));
    }

    #[test]
    fn test_direct_play_without_lists_takes_h264_with_aac_or_mp3() {
        let query = query_from("");
        for (video, audio, playable) in [
            ("h264", "aac", true),
            ("h264", "mp3", true),
            ("h264", "ac3", false),
            ("h264", "eac3", false),
            ("h264", "opus", false),
            ("hevc", "aac", false),
            ("vp9", "aac", false),
        ] {
            let source = source_with("mp4", video, audio);
            assert_eq!(
                check_direct_play(&source, &query),
                playable,
                "{video} + {audio}"
            );
            assert_eq!(
                check_direct_stream(&source, &query),
                playable,
                "{video} + {audio}"
            );
        }
    }

    #[test]
    fn test_direct_play_falls_back_for_a_missing_or_empty_list() {
        // Only an audio list: the video must still be H.264
        let query = query_from("AudioCodecs=aac,ac3");
        assert!(!check_direct_play(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(!check_direct_stream(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(check_direct_play(
            &source_with("mp4", "h264", "ac3"),
            &query
        ));
        // Empty lists count as none
        let query = query_from("VideoCodecs=&AudioCodecs=");
        assert!(!check_direct_play(
            &source_with("mp4", "hevc", "aac"),
            &query
        ));
        assert!(!check_direct_play(
            &source_with("mp4", "h264", "ac3"),
            &query
        ));
        assert!(check_direct_play(
            &source_with("mp4", "h264", "aac"),
            &query
        ));
    }

    #[test]
    fn test_direct_play_keeps_container_and_bitrate_rules() {
        let query = query_from(LISTS);
        for container in ["mkv", "avi"] {
            let source = source_with(container, "h264", "aac");
            assert!(!check_direct_play(&source, &query), "{container}");
            assert!(!check_direct_stream(&source, &query), "{container}");
        }
        let query = query_from(&format!("{LISTS}&MaxVideoBitrate=1000000"));
        assert!(!check_direct_play(
            &source_with("mp4", "h264", "aac"),
            &query
        ));
    }

    #[test]
    fn test_probed_lowercase_stream_types_count() {
        // The probe stores ffprobe's stream types, "video" and "audio"
        let probed = |video: &str, audio: &str| {
            let mut source = source_with("mp4", video, audio);
            for stream in &mut source.media_streams {
                stream.codec_type = stream.codec_type.to_lowercase();
            }
            source
        };
        let query = query_from(LISTS);
        assert!(check_direct_play(&probed("h264", "aac"), &query));
        assert!(check_direct_stream(&probed("h264", "aac"), &query));
        for (video, audio) in [("h264", "ac3"), ("hevc", "aac")] {
            let source = probed(video, audio);
            assert!(!check_direct_play(&source, &query), "{video} + {audio}");
            assert!(!check_direct_stream(&source, &query), "{video} + {audio}");
        }
        for copy in ["EnableVideoStreamCopy", "EnableAudioStreamCopy"] {
            let query = query_from(&format!("{LISTS}&{copy}=false"));
            assert!(
                !check_direct_stream(&probed("h264", "aac"), &query),
                "{copy}"
            );
        }
    }

    // ── B25: 10-bit video ──

    /// An H.264 + AAC source in `container` whose video has `profile` and `bit_depth` (None: not
    /// stored).
    fn h264_source(
        container: &str,
        profile: Option<&str>,
        bit_depth: Option<u32>,
    ) -> rustyfin_model::media_source::MediaSourceResponse {
        let mut source = source_with(container, "h264", "aac");
        source.media_streams[0].profile = profile.map(Into::into);
        source.media_streams[0].bit_depth = bit_depth;
        source
    }

    /// Whether `source` is direct played and direct streamed for a client that sends `params`.
    fn direct(
        params: &str,
        source: &rustyfin_model::media_source::MediaSourceResponse,
    ) -> [bool; 2] {
        let query = query_from(params);
        [
            check_direct_play(source, &query),
            check_direct_stream(source, &query),
        ]
    }

    #[test]
    fn test_deep_h264_is_not_direct_for_a_client_that_states_nothing() {
        // The web client's lists, no lists, or only an audio list; the stored bit depth, else
        // the profile's
        for params in [LISTS, "", "AudioCodecs=aac"] {
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
                for container in ["mp4", "m4v", "mkv"] {
                    let source = h264_source(container, profile, bits);
                    assert_eq!(
                        direct(params, &source),
                        [false, false],
                        "{params} {container} {profile:?} {bits:?}"
                    );
                }
            }
        }
        // An empty profile list or an unparsable depth states nothing
        for params in [
            "Profile=",
            "h264-profile=,",
            "MaxVideoBitDepth=ten",
            "h264-videobitdepth=x",
        ] {
            let source = h264_source("mp4", Some("High 10"), None);
            assert_eq!(
                direct(&format!("{LISTS}&{params}"), &source),
                [false, false],
                "{params}"
            );
        }
    }

    #[test]
    fn test_8bit_h264_is_direct_as_before() {
        for params in [LISTS, "", "AudioCodecs=aac"] {
            for (profile, bits) in [
                (Some("High"), Some(8)),
                (Some("Main"), None),
                (Some("Baseline"), Some(8)),
                (Some("Constrained Baseline"), None),
                (None, None),
            ] {
                let source = h264_source("mp4", profile, bits);
                assert_eq!(
                    direct(params, &source),
                    [true, true],
                    "{params} {profile:?} {bits:?}"
                );
                // MKV never is
                let source = h264_source("mkv", profile, bits);
                assert_eq!(direct(params, &source), [false, false], "{params} mkv");
            }
        }
    }

    #[test]
    fn test_deep_h264_is_direct_for_a_client_that_states_it_takes_it() {
        for bits in [Some(10), None] {
            let hi10 = h264_source("mp4", Some("High 10"), bits);
            for params in [
                "MaxVideoBitDepth=10",
                "MaxVideoBitDepth=12",
                "h264-videobitdepth=10",
                "Profile=high,main,high10",
                "h264-profile=high|main|High 10",
                "Profile= High 10 ",
                "Profile=&h264-profile=high10",
            ] {
                let params = format!("{LISTS}&{params}");
                assert_eq!(direct(&params, &hi10), [true, true], "{params} {bits:?}");
            }
            for params in [
                "MaxVideoBitDepth=8",
                "h264-videobitdepth=8",
                "MaxVideoBitDepth=8&h264-videobitdepth=10",
                "Profile=high,main",
                "Profile=high&h264-profile=high10",
                "Profile=high,high10&MaxVideoBitDepth=8",
            ] {
                let params = format!("{LISTS}&{params}");
                assert_eq!(direct(&params, &hi10), [false, false], "{params} {bits:?}");
            }
            // A statement doesn't make MKV direct
            let source = h264_source("mkv", Some("High 10"), bits);
            let params = format!("{LISTS}&MaxVideoBitDepth=10");
            assert_eq!(direct(&params, &source), [false, false], "mkv {bits:?}");
        }
        // A profile not listed passes when it scores no higher than the first one listed, and
        // one without a score never does
        let high = h264_source("mp4", Some("High"), Some(8));
        assert_eq!(direct("Profile=high10", &high), [true, true]);
        assert_eq!(direct("Profile=main", &high), [false, false]);
        let constrained = h264_source("mp4", Some("Constrained Baseline"), Some(8));
        assert_eq!(direct("Profile=baseline", &constrained), [true, true]);
        for profile in ["High 10 Intra", "High 4:2:2", "High 4:4:4 Predictive"] {
            let source = h264_source("mp4", Some(profile), Some(8));
            assert_eq!(
                direct("Profile=high10", &source),
                [false, false],
                "{profile}"
            );
        }
        // A depth limit alone takes 4:2:2 within it, and what is not known passes a statement
        let source = h264_source("mp4", Some("High 4:2:2"), Some(8));
        assert_eq!(direct("MaxVideoBitDepth=10", &source), [true, true]);
        let source = h264_source("mp4", None, None);
        assert_eq!(direct("MaxVideoBitDepth=8", &source), [true, true]);
        let source = h264_source("mp4", None, Some(8));
        assert_eq!(direct("Profile=high", &source), [true, true]);
        // Other codecs are unchanged: the rule is for H.264
        let mut source = source_with("mp4", "vp9", "opus");
        source.media_streams[0].profile = Some("Profile 2".into());
        source.media_streams[0].bit_depth = Some(10);
        assert_eq!(direct(LISTS, &source), [true, true]);
    }

    #[test]
    fn test_deep_h264_is_direct_when_it_is_not_the_first_video_stream() {
        // The rule reads the first video stream, as the codec lists do: here HEVC Main 10, which
        // the client lists
        let mut source = source_with("mp4", "hevc", "aac");
        source.media_streams[0].profile = Some("Main 10".into());
        source.media_streams[0].bit_depth = Some(10);
        source.media_streams[1].index = 2;
        source.media_streams.insert(
            1,
            rustyfin_model::media_source::MediaStream {
                index: 1,
                codec: Some("h264".into()),
                codec_type: "Video".into(),
                profile: Some("High 10".into()),
                bit_depth: Some(10),
                ..Default::default()
            },
        );
        let params = "VideoCodecs=hevc,h264&AudioCodecs=aac";
        assert_eq!(direct(params, &source), [true, true]);
    }

    // ── B16: version selection ──

    /// One H.264 + AAC MP4 source per id, in this order.
    fn sources_with_ids(ids: &[&str]) -> Vec<rustyfin_model::media_source::MediaSourceResponse> {
        ids.iter()
            .map(|id| {
                let mut source = source_with("mp4", "h264", "aac");
                source.id = id.to_string();
                source.path = format!("/media/{id}.mp4");
                source
            })
            .collect()
    }

    fn params_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn test_requested_source_id() {
        assert_eq!(requested_source_id(&params_of(&[])), None);
        assert_eq!(requested_source_id(&params_of(&[("Static", "true")])), None);
        // A blank id is no id
        for blank in ["", "  "] {
            let params = params_of(&[("MediaSourceId", blank)]);
            assert_eq!(requested_source_id(&params), None, "{blank:?}");
        }
        // Clients send the name in either case
        for name in ["MediaSourceId", "mediaSourceId", "MEDIASOURCEID"] {
            let params = params_of(&[(name, "abc")]);
            assert_eq!(requested_source_id(&params), Some("abc"), "{name}");
        }
        let params = params_of(&[("MediaSourceId", " abc ")]);
        assert_eq!(requested_source_id(&params), Some("abc"));
    }

    #[test]
    fn test_select_version() {
        let item = Uuid::new_v4();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let sources = sources_with_ids(&[&a.to_string(), &b.to_string()]);
        let select = |requested: Option<&str>| select_version(item, &sources, requested);

        // No id, or the item's own id in any form: the primary
        for requested in [
            None,
            Some(item.to_string()),
            Some(item.simple().to_string()),
            Some(item.to_string().to_uppercase()),
        ] {
            let version = select(requested.as_deref()).unwrap();
            assert_eq!(version, Version::Primary, "{requested:?}");
        }
        // A version's id, in any form
        for (id, index) in [(a, 0), (b, 1)] {
            for requested in [
                id.to_string(),
                id.simple().to_string(),
                id.to_string().to_uppercase(),
            ] {
                let version = select(Some(&requested)).unwrap();
                assert_eq!(version, Version::Source(index), "{requested}");
            }
        }
        // Anything else names no version of this item
        for requested in [Uuid::new_v4().to_string(), "nope".to_string()] {
            let selected = select(Some(&requested));
            assert!(
                matches!(selected, Err(ApiError::NotFound(_))),
                "{requested}: {selected:?}"
            );
        }

        // A movie that was never probed has no version but itself
        let version = select_version(item, &[], None).unwrap();
        assert_eq!(version, Version::Primary);
        let version = select_version(item, &[], Some(&item.to_string())).unwrap();
        assert_eq!(version, Version::Primary);
        let selected = select_version(item, &[], Some(&a.to_string()));
        assert!(
            matches!(selected, Err(ApiError::NotFound(_))),
            "{selected:?}"
        );

        // A source id that is not a UUID matches as it is
        let odd = sources_with_ids(&["odd-source"]);
        let version = select_version(item, &odd, Some("odd-source")).unwrap();
        assert_eq!(version, Version::Source(0));
    }
}
