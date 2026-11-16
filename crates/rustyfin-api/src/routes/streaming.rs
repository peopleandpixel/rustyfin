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
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{AppState, LibraryRoot, OwnedSet};

mod media_info;
mod progressive;
mod universal_audio;

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
        .route("/Audio/{id}/PlaybackInfo", get(audio_playback_info))
        .merge(universal_audio::routes())
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
/// The PlaySessionId a request sends, trimmed; None when it sends none or a blank one. Jellyfin's
/// Universal routes keep the client's id, so `DELETE /Videos/ActiveEncodings?PlaySessionId=` with
/// that id stops the transcode.
fn requested_play_session_id(params: &HashMap<String, String>) -> Option<String> {
    params
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("PlaySessionId"))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .min()
        .map(str::to_string)
}

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

/// The path of a video route: the item, and the container of `stream.{container}`.
#[derive(Deserialize)]
struct VideoPath {
    id: Uuid,
    container: Option<String>,
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
        return validate_path_against_library(state, &item.path).await;
    };
    let sources = rustyfin_db::repo_media_source::get_media_sources(&state.db_ro, &id).await?;
    match select_version(id, &sources, Some(requested))? {
        Version::Primary => validate_path_against_library(state, &item.path).await,
        Version::Source(index) => validate_path_against_library(state, &sources[index].path).await,
    }
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
    /// A resolve that fails for another reason than a missing file clears `clean`.
    fn forms(path: &FilePath, clean: &std::cell::Cell<bool>) -> Vec<std::path::PathBuf> {
        Self::resolved(path, clean)
            .into_iter()
            .chain(std::path::absolute(path))
            .collect()
    }

    /// `path` resolved. None when it can't be; a failure other than a missing file (a share
    /// that didn't answer, a permission) clears `clean`.
    fn resolved(path: &FilePath, clean: &std::cell::Cell<bool>) -> Option<std::path::PathBuf> {
        match path.canonicalize() {
            Ok(real) => Some(real),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    clean.set(false);
                }
                None
            }
        }
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

    /// The set `resolve_checked` makes, for tests.
    #[cfg(test)]
    fn resolve(&self) -> OwnedSet {
        self.resolve_checked().0
    }

    /// Resolves every owned path, which reads the file system: the data dir's forms, each owned
    /// folder that resolves, and each owned file in each form with its SQLite siblings. Checks
    /// then compare paths only (`OwnedSet::owns_in`). Also says whether every path resolved or
    /// was missing: a set with a path that failed otherwise lacks that path's resolved form, so
    /// it isn't kept (see `owned_set`).
    fn resolve_checked(&self) -> (OwnedSet, bool) {
        let clean = std::cell::Cell::new(true);
        let data_forms = Self::forms(&self.data_dir, &clean);
        let linked_dirs = data_forms
            .iter()
            .flat_map(|data| {
                let clean = &clean;
                OWNED_DATA_DIRS.iter().filter_map(move |dir| {
                    let real = Self::resolved(&data.join(dir), clean)?;
                    Some((dir.to_string(), real))
                })
            })
            .collect();
        let mut files = std::collections::HashSet::new();
        let data_files = OWNED_DATA_FILES.iter().map(|name| self.data_dir.join(name));
        for file in self.files.iter().cloned().chain(data_files) {
            files.extend(Self::forms(&file, &clean));
            let real = Self::resolved(&file, &clean);
            for suffix in DATABASE_SIBLINGS {
                files.extend(Self::forms(&Self::with_suffix(&file, suffix), &clean));
                // SQLite keeps the siblings beside the file a link resolves to
                if let Some(real) = &real {
                    files.insert(Self::with_suffix(real, suffix));
                }
            }
        }
        let set = OwnedSet {
            data_forms,
            linked_dirs,
            files,
        };
        (set, clean.get())
    }

    /// Whether the resolved `target` is one of these owned paths (resolving them first).
    #[cfg(test)]
    fn owns(&self, target: &FilePath) -> bool {
        self.resolve().owns(target)
    }
}

/// The owned set to use and whether to keep it: a `fresh` set that resolved cleanly replaces
/// the kept one; one with a failed path is used only when nothing is kept, and not kept, so the
/// next request resolves again. The previous set keeps the folders the failed resolve lost.
fn choose_owned_set(
    kept: Option<Arc<OwnedSet>>,
    fresh: OwnedSet,
    clean: bool,
) -> (Arc<OwnedSet>, bool) {
    match kept {
        Some(kept) if !clean => (kept, false),
        _ => (Arc::new(fresh), clean),
    }
}

impl OwnedSet {
    /// Whether the resolved `target` is one of these owned paths. No file system read.
    fn owns(&self, target: &FilePath) -> bool {
        self.owns_in(target, &OWNED_DATA_DIRS)
    }

    /// Whether the resolved `target` is one of these owned paths, counting only the data-dir
    /// folders in `dirs` as owned. No file system read.
    fn owns_in(&self, target: &FilePath, dirs: &[&str]) -> bool {
        if self
            .data_forms
            .iter()
            .any(|data| OwnedPaths::owned_by_name(data, target, dirs))
        {
            return true;
        }
        // Owned folders that are links out of the data dir
        let linked_dir = self.linked_dirs.iter().any(|(name, real)| {
            dirs.iter().any(|dir| dir == name) && target != real && target.starts_with(real)
        });
        // Owned files by full path, each as configured and resolved, with its SQLite siblings
        linked_dir || self.files.contains(target)
    }

    /// Whether the resolved `target` lies in the data dir.
    fn in_data_dir(&self, target: &FilePath) -> bool {
        self.data_forms.iter().any(|data| target.starts_with(data))
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

/// The resolved path of `file_path` when it may be streamed from libraries at `roots`; None when
/// it is refused:
/// - it has a `..` component;
/// - resolving its links fails;
/// - it lies under none of `roots`, as written or resolved. With `strict` (the server's
///   `RefuseLinksOutOfLibraries`) the resolved target must lie under one: a link inside a library
///   that points out of every library is refused, one into another library plays. Without it, a
///   path under a library as written is enough, as Jellyfin follows such links too;
/// - its resolved target is a path Rustyfin owns (see `OwnedPaths`). Other files inside the
///   data dir, such as media kept at `<data>/media`, may be streamed.
///
/// Callers open the returned path, not `file_path`, so a link changed after the check can't
/// lead elsewhere.
fn checked_path(
    file_path: &str,
    roots: &[LibraryRoot],
    owned: &OwnedSet,
    strict: bool,
) -> Option<std::path::PathBuf> {
    let raw = FilePath::new(file_path);
    if has_parent_dir(raw) {
        return None;
    }
    let canonical = raw.canonicalize().ok()?;
    if !in_roots(raw, &canonical, roots, strict) || owned.owns(&canonical) {
        return None;
    }
    Some(canonical)
}

/// Whether a file at `raw`, resolved to `canonical`, lies in one of the libraries at `roots`:
/// resolved, or, without `strict`, as written.
fn in_roots(raw: &FilePath, canonical: &FilePath, roots: &[LibraryRoot], strict: bool) -> bool {
    let resolved_inside = roots
        .iter()
        .filter_map(|root| root.resolved.as_ref())
        .any(|root| canonical.starts_with(root));
    let written_inside = !strict
        && lexical_normalize(raw).is_some_and(|written| {
            roots
                .iter()
                .filter_map(|root| root.written.as_ref())
                .any(|root| written.starts_with(root))
        });
    resolved_inside || written_inside
}

/// Whether `file_path` may be streamed from libraries at `lib_paths` by default (see
/// `checked_path`).
#[cfg(test)]
fn path_allowed(file_path: &str, lib_paths: &[String], owned: &OwnedPaths) -> bool {
    checked_path(
        file_path,
        &library_roots_of(lib_paths),
        &owned.resolve(),
        false,
    )
    .is_some()
}

/// Whether `file_path` may be streamed with `RefuseLinksOutOfLibraries` on.
#[cfg(test)]
fn path_allowed_strict(file_path: &str, lib_paths: &[String], owned: &OwnedPaths) -> bool {
    checked_path(
        file_path,
        &library_roots_of(lib_paths),
        &owned.resolve(),
        true,
    )
    .is_some()
}

/// `paths` as written and resolved.
fn library_roots_of(paths: &[String]) -> Vec<LibraryRoot> {
    paths
        .iter()
        .map(|path| LibraryRoot {
            written: lexical_normalize(FilePath::new(path)),
            resolved: FilePath::new(path).canonicalize().ok(),
        })
        .collect()
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

/// The paths this server owns: its data dir, its database file and its TLS files. The files are
/// read once and kept (see `FileGuardCache`).
async fn owned_paths(state: &AppState) -> OwnedPaths {
    let files = state
        .file_guard
        .owned_files
        .get_or_init(|| async {
            database_file(&state.db_ro)
                .await
                .into_iter()
                .chain(
                    ["RUSTYFIN_TLS_KEY", "RUSTYFIN_TLS_CERT"]
                        .into_iter()
                        .filter_map(std::env::var_os)
                        .map(std::path::PathBuf::from),
                )
                .collect()
        })
        .await
        .clone();
    OwnedPaths {
        data_dir: state.data_dir.to_path_buf(),
        files,
    }
}

/// How long a resolved `OwnedSet` is used before the owned paths are resolved again, so an owned
/// folder linked elsewhere while the server runs counts within this time.
const OWNED_SET_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// The owned paths, resolved: the kept set while it is younger than `OWNED_SET_MAX_AGE`, else
/// resolved again on a blocking thread and kept (see `FileGuardCache`). None when resolving
/// failed, which refuses the path.
async fn owned_set(state: &AppState) -> Option<Arc<OwnedSet>> {
    if let Some((at, set)) = state.file_guard.owned_set.read().unwrap().clone()
        && at.elapsed() < OWNED_SET_MAX_AGE
    {
        return Some(set);
    }
    let owned = owned_paths(state).await;
    let (fresh, clean) = tokio::task::spawn_blocking(move || owned.resolve_checked())
        .await
        .ok()?;
    let kept = state
        .file_guard
        .owned_set
        .read()
        .unwrap()
        .as_ref()
        .map(|(_, set)| set.clone());
    let (set, keep) = choose_owned_set(kept, fresh, clean);
    if keep {
        *state.file_guard.owned_set.write().unwrap() =
            Some((std::time::Instant::now(), set.clone()));
    } else {
        tracing::warn!("an owned path could not be resolved; the owned paths are resolved again");
    }
    Some(set)
}

/// Every library's paths, resolved: the kept ones, or read from the database when there are
/// none yet or `refresh` is set (see `FileGuardCache`).
async fn library_roots(state: &AppState, refresh: bool) -> Arc<Vec<LibraryRoot>> {
    if !refresh && let Some(roots) = state.file_guard.library_roots.read().unwrap().clone() {
        return roots;
    }
    let paths: Vec<String> = rustyfin_db::repo_library::get_all_libraries(&state.db_ro)
        .await
        .unwrap_or_default()
        .into_iter()
        .flat_map(|library| library.paths)
        .collect();
    let roots = Arc::new(
        tokio::task::spawn_blocking(move || library_roots_of(&paths))
            .await
            .unwrap_or_default(),
    );
    *state.file_guard.library_roots.write().unwrap() = Some(roots.clone());
    *state.file_guard.library_roots_read_at.write().unwrap() = Some(std::time::Instant::now());
    roots
}

/// Drops the kept library paths, so the next check reads them again. The library routes call it
/// after a library is added, removed or given other folders.
pub(crate) fn forget_library_roots(state: &AppState) {
    *state.file_guard.library_roots.write().unwrap() = None;
}

/// How often refused pictures may read the library paths again: a page of missing or
/// out-of-library pictures then costs one database read, not one per picture.
const IMAGE_ROOTS_REFRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a picture refused as outside the libraries may read the library paths again: they
/// were read more than `IMAGE_ROOTS_REFRESH` ago.
fn image_roots_may_refresh(state: &AppState) -> bool {
    state
        .file_guard
        .library_roots_read_at
        .read()
        .unwrap()
        .is_none_or(|at| at.elapsed() >= IMAGE_ROOTS_REFRESH)
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
/// ICO, TIFF, JPEG XL, an ISO media picture (AVIF, HEIF by their brands) or SVG.
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
    // An ISO media file is a picture only with a picture brand: MP4, MOV and M4A are not
    const IMAGE_BRANDS: [&[u8]; 6] = [b"avif", b"avis", b"heic", b"heix", b"mif1", b"msf1"];
    let iso_media = head.get(4..8) == Some(b"ftyp".as_slice())
        && head
            .get(8..12)
            .is_some_and(|brand| IMAGE_BRANDS.contains(&brand));
    if webp || iso_media {
        return true;
    }
    // SVG is text: an XML declaration or an <svg> tag first, and an <svg> tag in the head
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{FEFF}').trim_start();
    (text.starts_with("<?xml") || text.starts_with("<svg")) && text.contains("<svg")
}

/// Why the image routes refuse a stored picture path.
#[derive(Debug, PartialEq, Eq)]
enum ImageRefusal {
    /// It lies in no library and not in the data dir; libraries read again may hold it.
    OutsideRoots,
    /// Anything else (see `checked_image_path`).
    Refused,
}

/// Whether the image routes may read `file_path`. Pictures live in library folders (a poster
/// beside a film) and in the data dir (downloaded, extracted, uploaded and imported pictures).
/// The file is refused when:
/// - its path is relative or has a `..` component, or its links can't be resolved;
/// - it lies in none of the libraries at `roots` (as streams count them, `strict` being the
///   server's `RefuseLinksOutOfLibraries`) and not in the data dir;
/// - its resolved target is a path this server owns, other than a picture folder (see
///   `NON_IMAGE_DATA_DIRS`): the database, config, TLS files, logs, backups and so on;
/// - it does not start like a picture (see `looks_like_image`).
///
/// Returns the resolved path, which the caller opens.
fn checked_image_path(
    file_path: &str,
    owned: &OwnedSet,
    roots: &[LibraryRoot],
    strict: bool,
) -> Result<std::path::PathBuf, ImageRefusal> {
    use std::io::Read;

    let raw = FilePath::new(file_path);
    if !raw.is_absolute() || has_parent_dir(raw) {
        return Err(ImageRefusal::Refused);
    }
    let canonical = raw.canonicalize().map_err(|_| ImageRefusal::Refused)?;
    if owned.owns_in(&canonical, &NON_IMAGE_DATA_DIRS) {
        return Err(ImageRefusal::Refused);
    }
    if !owned.in_data_dir(&canonical) && !in_roots(raw, &canonical, roots, strict) {
        return Err(ImageRefusal::OutsideRoots);
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
    if looks_like_image(&head[..read]) {
        Ok(canonical)
    } else {
        Err(ImageRefusal::Refused)
    }
}

/// Whether the image routes may read `file_path` from libraries at `lib_paths` (see
/// `checked_image_path`).
#[cfg(test)]
fn image_path_allowed(file_path: &str, lib_paths: &[String], owned: &OwnedPaths) -> bool {
    checked_image_path(
        file_path,
        &owned.resolve(),
        &library_roots_of(lib_paths),
        false,
    )
    .is_ok()
}

/// The resolved path of a stored image that may be served (see `checked_image_path`), which the
/// caller opens. The checks read the file system, so they run on a blocking thread. The library
/// paths are kept between requests; a picture outside them reads them again before it is refused
/// only when they are older than `IMAGE_ROOTS_REFRESH`.
pub(crate) async fn validate_image_path(
    state: &AppState,
    file_path: &str,
) -> Result<String, ApiError> {
    let refused = || {
        tracing::warn!("image path '{}' is refused", file_path);
        ApiError::NotFound("image not found".to_string())
    };
    let owned = owned_set(state).await.ok_or_else(refused)?;
    let strict = state.config.read().await.refuse_links_out_of_libraries;
    for refresh in [false, true] {
        let roots = library_roots(state, refresh).await;
        let (path, owned) = (file_path.to_string(), owned.clone());
        let checked =
            tokio::task::spawn_blocking(move || checked_image_path(&path, &owned, &roots, strict))
                .await
                .unwrap_or(Err(ImageRefusal::Refused));
        match checked {
            Ok(path) => return Ok(path.to_string_lossy().into_owned()),
            Err(ImageRefusal::OutsideRoots) if !refresh && image_roots_may_refresh(state) => {
                continue;
            }
            Err(_) => break,
        }
    }
    Err(refused())
}

/// The resolved path of `file_path` when it lies in a library and is none of the server's own
/// files (see `checked_path`, strict when the server's `RefuseLinksOutOfLibraries` is on); the
/// caller opens that path. This prevents path traversal if the
/// database is compromised. The library paths are kept between requests and read again once
/// before a path is refused, so a library added since counts.
pub(crate) async fn validate_path_against_library(
    state: &AppState,
    file_path: &str,
) -> Result<String, ApiError> {
    let refused = || {
        tracing::warn!(
            "streaming path '{}' is outside the library paths",
            file_path
        );
        ApiError::NotFound("File is outside configured library paths".to_string())
    };
    if has_parent_dir(FilePath::new(file_path)) {
        return Err(refused());
    }
    let owned = owned_set(state).await.ok_or_else(refused)?;
    let strict = state.config.read().await.refuse_links_out_of_libraries;
    for refresh in [false, true] {
        let roots = library_roots(state, refresh).await;
        let (path, owned) = (file_path.to_string(), owned.clone());
        let checked =
            tokio::task::spawn_blocking(move || checked_path(&path, &roots, &owned, strict))
                .await
                .ok()
                .flatten();
        if let Some(path) = checked {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    Err(refused())
}

/// What `/Videos/{id}/stream` and `stream.{container}` serve. `static=true` gets the file, and
/// so does a request with no `static` that names no `VideoCodec` and no `AudioCodec` (Rustyfin's
/// own clients ask so for the file). `static=false`, or codecs without `static`, ask for a
/// progressive transcode, as Jellyfin makes it: into the URL's container, else `Container`,
/// else the source's (see `progressive_video_profile` for what is copied).
async fn video_stream_answer(
    state: &AppState,
    caller: &progressive::Caller,
    VideoPath { id, container }: VideoPath,
    params: &HashMap<String, String>,
    uri: &axum::http::Uri,
) -> Result<Served, ApiError> {
    let param = |name: &str| progressive::param(params, name);
    let transcode = match param("static").map(|v| v.eq_ignore_ascii_case("true")) {
        Some(is_static) => !is_static,
        None => param("VideoCodec").is_some() || param("AudioCodec").is_some(),
    };
    if !transcode {
        return Ok(Served::File(video_file(state, id, params).await?));
    }
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    let sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .unwrap_or_default();
    let (input, source) = match select_version(id, &sources, requested_source_id(params))? {
        Version::Primary => (item.path.as_str(), sources.first()),
        Version::Source(index) => (sources[index].path.as_str(), sources.get(index)),
    };
    let path = validate_path_against_library(state, input).await?;
    let query = Query::<super::transcode::TranscodeQuery>::try_from_uri(uri)
        .map_err(|e| ApiError::BadRequest(format!("Invalid transcode query: {e}")))?
        .0;
    let mut query = super::transcode::to_file_numbering(query, source).await;
    // The remote bitrate limit, before the copy decision
    query.video_bitrate = caller.bitrate(query.video_bitrate);
    let mut profile = super::transcode::progressive_video_profile(
        query,
        params,
        source,
        &state.transcode_manager.encoding_settings(),
    );
    // The channels: never more than the client's limit, the played track's or the encoder's
    let track = profile
        .audio_stream_index
        .and_then(|index| {
            source?
                .media_streams
                .iter()
                .find(|s| !s.is_external && u32::try_from(s.index) == Ok(index))
        })
        .or_else(|| {
            source?
                .media_streams
                .iter()
                .find(|s| s.codec_type.eq_ignore_ascii_case("audio"))
        });
    profile.audio_channels = progressive::output_channels(
        progressive::channel_limit(params),
        track.and_then(|s| s.channels),
        &profile.audio_codec,
    );
    let container = container
        .or_else(|| param("Container").map(str::to_string))
        .or_else(|| source.and_then(|s| s.container.clone()))
        .unwrap_or_else(|| "mkv".to_string());
    // A probed container may be a list ("mov,mp4,m4a"): its first name
    let container = container
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    Ok(Served::Progressive {
        path,
        profile: Box::new(profile),
        container,
        item_id: id,
        play_session_id: requested_play_session_id(params).unwrap_or_default(),
        device_id: param("DeviceId").unwrap_or_default().to_string(),
    })
}

/// `GET /Videos/{id}/stream` and `stream.{container}`: the file of the requested version, or its
/// progressive transcode (see `video_stream_answer`).
async fn stream_video(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    local: Option<Extension<crate::middleware::ClientIsLocal>>,
    Path(path): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let caller = progressive::Caller::of(&state, claims.as_deref(), local.as_deref()).await?;
    let served = video_stream_answer(&state, &caller, path, &params, &uri).await?;
    served.respond(&state, &caller, &headers, false).await
}

/// `HEAD` of `stream` and `stream.{container}`: the headers that `GET` sends; it starts no
/// transcode.
async fn head_video(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    local: Option<Extension<crate::middleware::ClientIsLocal>>,
    Path(path): Path<VideoPath>,
    Query(params): Query<HashMap<String, String>>,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let caller = progressive::Caller::of(&state, claims.as_deref(), local.as_deref()).await?;
    let served = video_stream_answer(&state, &caller, path, &params, &uri).await?;
    served.respond(&state, &caller, &headers, true).await
}

/// The path of an audio stream route: the item, and the container of `stream.{container}`.
#[derive(Deserialize)]
struct AudioPath {
    id: Uuid,
    container: Option<String>,
}

/// The query parameters that limit an audio transcode; a request with one asks for a transcode
/// even of the track's own codec, as Jellyfin's progressive job copies or re-encodes it.
const AUDIO_LIMITS: [&str; 6] = [
    "AudioBitrate",
    "MaxAudioBitrate",
    "AudioSampleRate",
    "MaxAudioSampleRate",
    "AudioChannels",
    "MaxAudioChannels",
];

/// What `/Audio/{id}/stream` and `stream.{container}` serve: the track's file, or a progressive
/// transcode. `static=true`, or a request that names no codec and no container, gets the file;
/// so does one whose container (else `AudioCodec` list) the track's container or codec is among
/// without limits (`AUDIO_LIMITS`). Anything else is transcoded progressively, as Jellyfin
/// does: into the URL's container, else `Container`, else the track's own container when its
/// codec is listed, else the first listed codec's.
async fn audio_stream_answer(
    state: &AppState,
    caller: &progressive::Caller,
    AudioPath { id, container }: AudioPath,
    params: &HashMap<String, String>,
) -> Result<Served, ApiError> {
    // A remote caller's bitrate limit counts as one the request sets
    let params = &caller.clamp_audio(params);
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    // The resolved path, which the file is opened by
    let path = validate_path_against_library(state, &item.path).await?;

    let param = |name: &str| progressive::param(params, name);
    if param("static").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return Ok(Served::File(path));
    }
    let listed: Vec<String> = param("AudioCodec")
        .unwrap_or_default()
        .split(',')
        .map(|codec| codec.trim().to_ascii_lowercase())
        .filter(|codec| !codec.is_empty())
        .collect();
    let container = container
        .as_deref()
        .or(param("Container"))
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty());
    if listed.is_empty() && container.is_none() {
        return Ok(Served::File(path));
    }
    let source =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .unwrap_or_default()
            .into_iter()
            .next();
    let Some(mut source) = source else {
        // A track that was never probed plays as it is
        return Ok(Served::File(path));
    };
    // The numbers a client sends (AudioStreamIndex) are PlaybackInfo's
    crate::routes::subtitle::number_streams(&mut source);
    let codec = source
        .media_streams
        .iter()
        .find(|s| s.codec_type.eq_ignore_ascii_case("audio"))
        .and_then(|s| s.codec.as_deref())
        .map(str::to_ascii_lowercase);
    let source_container = source.container.as_deref().map(str::to_ascii_lowercase);
    let is = |a: &Option<String>, b: &str| a.as_deref() == Some(b);
    let container_takes = container
        .as_deref()
        .is_none_or(|c| is(&source_container, c) || is(&codec, c));
    let codecs_take = listed.is_empty() || codec.as_ref().is_some_and(|c| listed.contains(c));
    let limited = AUDIO_LIMITS.iter().any(|name| param(name).is_some());
    if container_takes && codecs_take && !limited {
        return Ok(Served::File(path));
    }

    let container = match container {
        Some(container) => container,
        None if codecs_take => source_container
            .clone()
            .unwrap_or_else(|| listed[0].clone()),
        None => listed[0].clone(),
    };
    let profile = progressive::audio_profile(params, Some(&source), &container)?;
    Ok(Served::Progressive {
        path,
        profile: Box::new(profile),
        container,
        item_id: id,
        play_session_id: requested_play_session_id(params).unwrap_or_default(),
        device_id: param("DeviceId").unwrap_or_default().to_string(),
    })
}

/// What a stream route serves.
enum Served {
    /// The file at this path, with byte ranges.
    File(String),
    /// A progressive transcode.
    Progressive {
        path: String,
        profile: Box<rustyfin_core::transcode::TranscodeProfile>,
        container: String,
        item_id: Uuid,
        play_session_id: String,
        device_id: String,
    },
}

impl Served {
    /// The response: the file (or its headers for HEAD), or the transcode.
    async fn respond(
        self,
        state: &AppState,
        caller: &progressive::Caller,
        headers: &HeaderMap,
        head: bool,
    ) -> Result<Response, ApiError> {
        match self {
            Served::File(path) if head => head_file(&path).await,
            Served::File(path) => serve_file(path, headers).await,
            Served::Progressive {
                path,
                profile,
                container,
                item_id,
                play_session_id,
                device_id,
            } => {
                let job = progressive::Job {
                    item_id,
                    input_path: &path,
                    profile: *profile,
                    container: &container,
                    play_session_id,
                    device_id,
                };
                progressive::respond(state, caller, job, head).await
            }
        }
    }
}

/// `GET /Audio/{id}/stream` and `stream.{container}`: the track's file or its progressive
/// transcode (see `audio_stream_answer`).
async fn stream_audio(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    local: Option<Extension<crate::middleware::ClientIsLocal>>,
    Path(path): Path<AudioPath>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let caller = progressive::Caller::of(&state, claims.as_deref(), local.as_deref()).await?;
    let served = audio_stream_answer(&state, &caller, path, &params).await?;
    served.respond(&state, &caller, &headers, false).await
}

/// `HEAD` of the audio stream routes: the headers that `GET` sends; it starts no transcode.
async fn head_stream(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
    local: Option<Extension<crate::middleware::ClientIsLocal>>,
    Path(path): Path<AudioPath>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let caller = progressive::Caller::of(&state, claims.as_deref(), local.as_deref()).await?;
    let served = audio_stream_answer(&state, &caller, path, &params).await?;
    served.respond(&state, &caller, &headers, true).await
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

/// Serves the file at `file_path` for a request with `headers`: the whole file, or the byte range,
/// 304 or 416 that the headers ask for.
pub(crate) async fn serve_file(
    file_path: String,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
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
    /// Jellyfin's SubtitleMethod: "Encode" asks for the subtitle to be burned in, text ones too
    subtitle_method: Option<String>,
    segment_time: Option<u32>,
    profile: Option<String>,
    max_video_bit_depth: Option<u32>,
    h264_profile: Option<String>,
    h264_video_bit_depth: Option<String>,
    hevc_profile: Option<String>,
    hevc_video_bit_depth: Option<String>,
    hevc_level: Option<String>,
    hevc_range_type: Option<String>,
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
            subtitle_method: params.get("SubtitleMethod").cloned(),
            segment_time: params.get("SegmentTime").and_then(|v| v.parse().ok()),
            profile: params.get("Profile").cloned(),
            max_video_bit_depth: params.get("MaxVideoBitDepth").and_then(|v| v.parse().ok()),
            h264_profile: params.get("h264-profile").cloned(),
            h264_video_bit_depth: params.get("h264-videobitdepth").cloned(),
            hevc_profile: params.get("hevc-profile").cloned(),
            hevc_video_bit_depth: params.get("hevc-videobitdepth").cloned(),
            hevc_level: params.get("hevc-level").cloned(),
            hevc_range_type: params.get("hevc-rangetype").cloned(),
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
    // Picture subtitles are burned in by the transcode, text ones are drawn by the browser unless
    // the request asks for them burned in too (SubtitleMethod=Encode)
    let encode = query
        .subtitle_method
        .as_deref()
        .is_some_and(|method| method.eq_ignore_ascii_case("encode"));
    let burn_in = match query.subtitle_stream_index {
        Some(wanted) if wanted >= 0 => source.media_streams.iter().any(|s| {
            s.index == wanted
                && is_stream_type(s, "Subtitle")
                && (encode
                    || s.codec
                        .as_deref()
                        .is_some_and(rustyfin_core::subtitle::is_image_subtitle))
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
    // The client's play session id, else a new one: the client stops its transcode by it
    let play_session_id =
        requested_play_session_id(&params).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let query = VideoPlaybackInfoQuery::from(params);

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
        // The transcode's VideoBitrate is in kbps; Jellyfin's MaxVideoBitrate is in bits/s. With
        // both limits the lower one wins.
        let max_video_kbps = query.max_video_bitrate.map(|bps| (bps / 1000).max(1));
        let streaming_kbps = streaming_video_kbps(
            query.max_streaming_bitrate,
            media_sources.get(index).and_then(|source| source.bitrate),
        );
        if let Some(bitrate) = [max_video_kbps, streaming_kbps].into_iter().flatten().min() {
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
        // What the client states about the video it takes, which decides a stream copy
        if let Some(depth) = query.max_video_bit_depth {
            url.push_str(&format!("&MaxVideoBitDepth={depth}"));
        }
        for (name, value) in [
            ("hevc-profile", &query.hevc_profile),
            ("hevc-videobitdepth", &query.hevc_video_bit_depth),
            ("hevc-level", &query.hevc_level),
            ("hevc-rangetype", &query.hevc_range_type),
        ] {
            if let Some(value) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                let encoded: String = form_urlencoded::byte_serialize(value.as_bytes()).collect();
                url.push_str(&format!("&{name}={encoded}"));
            }
        }
        let encoded: String = form_urlencoded::byte_serialize(play_session_id.as_bytes()).collect();
        url.push_str(&format!("&PlaySessionId={encoded}"));
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

    // The subtitle files beside each video, numbered before its streams as Jellyfin numbers them
    for source in media_sources.iter_mut() {
        crate::routes::subtitle::with_external_streams(source).await;
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

    // The caller's policy, as in /Items/{id}/PlaybackInfo: no remux without
    // EnablePlaybackRemuxing, no transcode without EnableVideoPlaybackTranscoding, and no direct
    // play of a remote source with ForceRemoteSourceTranscoding
    let access = crate::parental::caller_access(&state, &claims).await?;
    for source in media_sources.iter_mut() {
        source.supports_direct_play &=
            !(access.force_remote_source_transcoding && source.is_remote);
        source.supports_direct_stream &= access.enable_playback_remuxing;
        source.supports_transcoding &= access.enable_video_playback_transcoding;
    }

    // The caller's remembered tracks become each source's defaults, where the source has them
    let (remembered_audio, remembered_subtitle) =
        remembered_choices(&state, claims.sub, id).await?;
    let media_sources: Vec<rustyfin_model::playback_info::PlaybackMediaSource> = media_sources
        .into_iter()
        .map(|source| {
            let default_audio_stream_index = remembered_audio
                .as_ref()
                .and_then(|choice| remembered_stream(&source, "Audio", choice));
            let default_subtitle_stream_index = remembered_subtitle.as_ref().and_then(|choice| {
                if choice.index == -1 {
                    Some(-1)
                } else {
                    remembered_stream(&source, "Subtitle", choice)
                }
            });
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

/// The audio and subtitle choices `user` last made for `item`, each where the user keeps
/// Jellyfin's RememberAudioSelections and RememberSubtitleSelections on.
pub(crate) async fn remembered_choices(
    state: &AppState,
    user: Uuid,
    item: Uuid,
) -> Result<(Option<StreamChoice>, Option<StreamChoice>), ApiError> {
    let (audio, subtitle) =
        rustyfin_db::repo_playback::get_stream_choice(&state.db_ro, user, item).await?;
    let config = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user)
        .await?
        .map(|user| rustyfin_model::user::jellyfin_configuration(&user.configuration))
        .unwrap_or_default();
    let remembers = |key: &str| config.get(key).and_then(|v| v.as_bool()) != Some(false);
    Ok((
        audio.filter(|_| remembers("RememberAudioSelections")),
        subtitle.filter(|_| remembers("RememberSubtitleSelections")),
    ))
}

/// The user's settings that pick a video's default tracks (`PlayDefaultAudioTrack`,
/// `AudioLanguagePreference`, `SubtitleMode`, `SubtitleLanguagePreference`), with Jellyfin's
/// defaults for the ones not stored and the languages normalized as Jellyfin does.
pub(crate) async fn track_preferences(
    state: &AppState,
    user: Uuid,
) -> Result<rustyfin_core::stream_builder::TrackPreferences, ApiError> {
    use rustyfin_core::stream_builder::{SubtitleMode, TrackPreferences};
    let config = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user)
        .await?
        .map(|user| rustyfin_model::user::jellyfin_configuration(&user.configuration))
        .unwrap_or_default();
    let text = |key: &str| config.get(key).and_then(|v| v.as_str()).unwrap_or_default();
    let audio = text("AudioLanguagePreference");
    let languages = crate::routes::localization::normalize_language;
    Ok(TrackPreferences {
        play_default_audio_track: config
            .get("PlayDefaultAudioTrack")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        original_language_audio: audio.eq_ignore_ascii_case("OriginalLanguage"),
        audio_languages: languages(audio),
        subtitle_mode: SubtitleMode::parse(text("SubtitleMode")),
        subtitle_languages: languages(text("SubtitleLanguagePreference")),
    })
}

/// The index of the stream of `kind` ("Audio", "Subtitle") in `source` that a remembered choice
/// names: the stream at the choice's index whose key is the one stored with it, else the one
/// stream with that key (the numbering moved: Jellyfin numbers external subtitle files first,
/// and a choice stored before Rustyfin did names an embedded stream by its index in the file).
/// A choice with no stored key, or a key several streams share, names none.
pub(crate) fn remembered_stream(
    source: &rustyfin_model::media_source::MediaSourceResponse,
    kind: &str,
    choice: &StreamChoice,
) -> Option<i32> {
    let key = choice.key.as_deref()?;
    let mut keyed = source
        .media_streams
        .iter()
        .filter(|s| is_stream_type(s, kind) && s.identity().key() == key);
    let first = keyed.next()?;
    if first.index == choice.index || keyed.clone().any(|s| s.index == choice.index) {
        return Some(choice.index);
    }
    keyed.next().is_none().then_some(first.index)
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

    // The client's quality limit holds for direct play too, as in Jellyfin
    if let Some(max_bitrate) = query.max_streaming_bitrate
        && let Some(total_bitrate) = source.bitrate
        && total_bitrate > max_bitrate
    {
        return false;
    }

    true
}

/// The video bitrate in kbps (the HLS transcode's `VideoBitrate` unit) for a transcode under the client's
/// `MaxStreamingBitrate` in bits per second: the limit less the transcoded audio. As in Jellyfin, a source
/// over the limit is encoded up to the limit (so the video is never copied above it), and a limit below the
/// transcode's default bitrate lowers it. A source that fits a high limit, or no limit, keeps the default.
fn streaming_video_kbps(
    max_streaming_bitrate: Option<u32>,
    source_bitrate: Option<u32>,
) -> Option<u32> {
    let default = rustyfin_core::transcode::TranscodeProfile::default();
    let limit = max_streaming_bitrate?;
    let kbps = (limit / 1000)
        .saturating_sub(default.max_audio_bitrate_kbps)
        .max(64);
    let over_limit = source_bitrate.is_some_and(|bitrate| bitrate > limit);
    (over_limit || kbps < default.max_video_bitrate_kbps).then_some(kbps)
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
        let data = dir.path().join("data");
        for d in [&lib, &outside, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(outside.join("movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, lib.join("Movie")).unwrap();
        let file = lib.join("Movie/movie.mkv");
        let libs = [lib.to_str().unwrap().to_string()];
        assert!(path_allowed(
            file.to_str().unwrap(),
            &libs,
            &owned(&data, &[])
        ));
    }

    #[test]
    fn r3playback_links_out_of_every_library_follow_the_setting() {
        // A link that sits inside a library but points out of every library plays by default,
        // and is refused with RefuseLinksOutOfLibraries; one into another library plays either
        // way. The checked path is the resolved one, which the caller opens.
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let outside = dir.path().join("elsewhere");
        let data = dir.path().join("data");
        for d in [&lib, &outside, &data] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(outside.join("movie.mkv"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, lib.join("Movie")).unwrap();
        std::os::unix::fs::symlink(outside.join("movie.mkv"), lib.join("film.mkv")).unwrap();
        let s = |p: std::path::PathBuf| p.to_str().unwrap().to_string();
        let one = [s(lib.clone())];
        let both = [s(lib.clone()), s(outside.clone())];
        let target = Some(outside.join("movie.mkv").canonicalize().unwrap());
        for file in [lib.join("Movie/movie.mkv"), lib.join("film.mkv")] {
            let file = s(file);
            for strict in [false, true] {
                let checked = checked_path(
                    &file,
                    &library_roots_of(&one),
                    &owned(&data, &[]).resolve(),
                    strict,
                );
                assert_eq!(
                    checked,
                    if strict { None } else { target.clone() },
                    "{file} {strict}"
                );
                let checked = checked_path(
                    &file,
                    &library_roots_of(&both),
                    &owned(&data, &[]).resolve(),
                    strict,
                );
                assert_eq!(checked, target, "{file} {strict}");
            }
        }
        // A link to the server's own files and `..` are refused either way
        std::fs::write(data.join("rustyfin.db"), b"x").unwrap();
        std::os::unix::fs::symlink(data.join("rustyfin.db"), lib.join("db.mkv")).unwrap();
        for file in [
            s(lib.join("db.mkv")),
            format!("{}/../elsewhere/movie.mkv", lib.display()),
        ] {
            assert!(!path_allowed(&file, &both, &owned(&data, &[])), "{file}");
            assert!(
                !path_allowed_strict(&file, &both, &owned(&data, &[])),
                "{file}"
            );
        }
    }

    #[test]
    fn r1play_dotdot_escape_is_rejected() {
        let dir = TempDir::new();
        let lib = dir.path().join("lib");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(dir.path().join("secret.mkv"), b"x").unwrap();
        std::fs::write(lib.join("in.mkv"), b"x").unwrap();
        let libs = [lib.to_str().unwrap().to_string()];
        // Even `..` that stays inside the library is refused before anything resolves
        for file in [lib.join("../secret.mkv"), lib.join("../lib/in.mkv")] {
            assert!(!path_allowed(
                file.to_str().unwrap(),
                &libs,
                &owned(&data, &[])
            ));
        }
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
        // With RefuseLinksOutOfLibraries the link out of every library is refused
        assert!(!path_allowed_strict(
            &path("Movie/movie.mkv"),
            &libs,
            &owned(&data, &[])
        ));
        assert!(!path_allowed_strict(
            &path("db.mkv"),
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
            b"\x00\x00\x00\x18ftypheic".as_slice(),
            b"\x00\x00\x00\x18ftypmif1".as_slice(),
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
            // MP4, MOV and M4A share the ISO box but are not pictures
            b"\x00\x00\x00\x20ftypisom".as_slice(),
            b"\x00\x00\x00\x14ftypqt  ".as_slice(),
            b"\x00\x00\x00\x20ftypM4A ".as_slice(),
            b"\x00\x00\x00\x08ftyp".as_slice(),
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
        let libs = [s(&lib)];
        let image_path_allowed =
            |file: &str, owned: &OwnedPaths| image_path_allowed(file, &libs, owned);

        // Pictures beside the media and in the server's picture folders
        let poster = lib.join("poster.jpg");
        std::fs::write(&poster, R3_JPEG).unwrap();
        for folder in ["metadata", "images", "cache"] {
            let file = data.join(folder).join("a.png");
            std::fs::write(&file, R3_PNG).unwrap();
            assert!(image_path_allowed(&s(&file), &owned), "{folder}");
        }
        assert!(image_path_allowed(&s(&poster), &owned));
        // A picture in no library and outside the data dir is refused (an import copies its
        // pictures into the data dir)
        let imported = import.join("folder.jpg");
        std::fs::write(&imported, R3_JPEG).unwrap();
        assert!(!image_path_allowed(&s(&imported), &owned));
        // A link in a library to a picture elsewhere still shows, as Jellyfin follows links too
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
        // Another file there plays; with RefuseLinksOutOfLibraries only when its folder is a
        // library too
        let both = [libs[0].clone(), db_dir.to_str().unwrap().to_string()];
        assert!(path_allowed(
            &path("other.db"),
            &libs,
            &owned(&data, &[&db])
        ));
        assert!(!path_allowed_strict(
            &path("other.db"),
            &libs,
            &owned(&data, &[&db])
        ));
        assert!(path_allowed(
            &path("other.db"),
            &both,
            &owned(&data, &[&db])
        ));
        for name in ["main.db", "main.db-wal", "main.db-shm"] {
            assert!(
                !path_allowed(&path(name), &both, &owned(&data, &[&db])),
                "{name}"
            );
        }
        assert!(path_allowed(&path("main.db"), &both, &owned(&data, &[])));
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
        // Another file there plays; with RefuseLinksOutOfLibraries only when its folder is a
        // library too
        let both = [libs[0].clone(), store.to_str().unwrap().to_string()];
        for refused in ["key.mkv", "db.mkv", "wal.mkv", "shm.mkv"] {
            assert!(!path_allowed(&path(refused), &both, &owned), "{refused}");
        }
        assert!(path_allowed(&path("other.mkv"), &libs, &owned));
        assert!(!path_allowed_strict(&path("other.mkv"), &libs, &owned));
        assert!(path_allowed_strict(&path("other.mkv"), &both, &owned));
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

    /// A data folder whose resolve fails for another reason than a missing file (here a link
    /// into a folder that may not be entered) makes the set unclean; missing folders don't.
    #[test]
    fn r3gaps3_a_failed_resolve_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let data = dir.path().join("data");
        let locked = dir.path().join("locked");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(locked.join("inner")).unwrap();
        assert!(
            owned(&data, &[]).resolve_checked().1,
            "missing folders are fine"
        );

        std::os::unix::fs::symlink(locked.join("inner"), data.join("transcodes")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let entered = std::fs::read_dir(&locked).is_ok();
        let (set, clean) = owned(&data, &[]).resolve_checked();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if entered {
            // Run as root, which enters any folder: nothing fails to resolve
            return;
        }
        assert!(!clean);
        assert!(!set.linked_dirs.iter().any(|(name, _)| name == "transcodes"));
    }

    /// An unclean resolve keeps the previous set and isn't kept itself; with nothing kept it is
    /// used once. A clean one replaces the kept set.
    #[test]
    fn r3gaps3_an_unclean_owned_set_is_not_kept() {
        let previous = Arc::new(OwnedSet {
            linked_dirs: vec![("transcodes".to_string(), "/fast/transcodes".into())],
            ..Default::default()
        });
        let fresh = OwnedSet::default();
        let (set, keep) = choose_owned_set(Some(previous.clone()), fresh.clone(), false);
        assert!(Arc::ptr_eq(&set, &previous) && !keep);
        let (set, keep) = choose_owned_set(None, fresh.clone(), false);
        assert!(*set == fresh && !keep);
        let (set, keep) = choose_owned_set(Some(previous), fresh.clone(), true);
        assert!(*set == fresh && keep);
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
