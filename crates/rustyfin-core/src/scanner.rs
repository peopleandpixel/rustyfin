use chrono::{DateTime, Utc};
use rustyfin_db::repo_media::PathScope;
use rustyfin_model::image::ImageType;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{LocationType, MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use crate::local_images;
use crate::naming;
use crate::nfo_parser;
use crate::plugin_manager::PluginManager;
use crate::probe;
use crate::scan_progress::ScanTicket;
use crate::scan_sync;

/// Jellyfin's video extensions (`NamingOptions.cs:24-79`) without `001`, `asx`, `bin`, `ifo`, `img`, `iso`,
/// `nrg` and `strm`: split parts, playlists, disc images and links, which ffmpeg can't play as one title.
/// `bin` counts in TV libraries only when it is named as an episode (`is_episode_file`).
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg", "3g2", "3gp",
    "amv", "asf", "bivx", "divx", "dv", "dvr-ms", "f4v", "fli", "m2t", "m2ts", "m2v", "mk3d",
    "mpe", "mts", "mxf", "nsv", "nuv", "ogm", "ogv", "pva", "qt", "rec", "rm", "rmvb", "svq3",
    "tp", "ty", "viv", "vob", "vp3", "wtv", "xvid",
];

/// Whether `path` is an episode file in a TV library. Jellyfin lists every `.bin` (`NamingOptions.cs:24-79`);
/// most are disc images or data, so a `.bin` counts only when its stem parses as an episode.
pub(crate) fn is_episode_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        return true;
    }
    ext == "bin"
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| naming::parse_episode(stem).is_some())
}

pub(crate) const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "wav", "aac", "wma", "m4a", "opus"];
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

pub async fn scan_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    let pm = PluginManager::new(&std::path::PathBuf::from("."));
    scan_library_with_metadata(
        pool,
        library,
        &reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new()),
        &pm,
        &ScanTicket::untracked(),
    )
    .await
}

/// What a scan covers in a library: all of it, one folder of a library path with its subfolders,
/// or the files directly in one library path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScanScope {
    /// Every library path, with all of its folders.
    Library,
    /// A folder under a library path, with its subfolders.
    Folder(PathBuf),
    /// The files directly in a library path, not the ones in its folders.
    RootFiles(PathBuf),
}

/// How many items a scan added, removed and read again, and how many images changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanChanges {
    /// New items of every type, series, seasons, artists and albums included.
    pub added: u32,
    /// Items that the scan no longer collected, and deleted.
    pub removed: u32,
    /// Existing items that got probe steps again, because their files changed or an older probe
    /// read them.
    pub reread: u32,
    /// Images whose file changed since it was stored.
    pub images: u32,
}

impl ScanChanges {
    /// Whether the scan added, removed or read again at least one item, or changed an image.
    pub fn changed(&self) -> bool {
        self.added > 0 || self.removed > 0 || self.reread > 0 || self.images > 0
    }
}

/// Scans the library, then refreshes the metadata of its new items. Returns how many items were
/// added. `ticket` is the caller's share of the library's scan, which the registry lists until the
/// share and the probes end.
pub async fn scan_library_with_metadata(
    pool: &SqlitePool,
    library: &Library,
    http_client: &reqwest::Client,
    plugin_manager: &PluginManager,
    ticket: &ScanTicket,
) -> Result<u32, sqlx::Error> {
    scan_with_metadata(
        pool,
        library,
        &ScanScope::Library,
        http_client,
        plugin_manager,
        ticket,
    )
    .await
    .map(|changes| changes.added)
}

/// Scans the part of the library that `scope` names, then refreshes the metadata of its new items.
/// `ticket` is the caller's share of the library's scan, which the registry lists until the share
/// and the probes end.
#[allow(clippy::too_many_arguments)]
pub async fn scan_with_metadata(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    http_client: &reqwest::Client,
    plugin_manager: &PluginManager,
    ticket: &ScanTicket,
) -> Result<ScanChanges, sqlx::Error> {
    if *scope == ScanScope::Library {
        tracing::info!(
            "scanning library '{}' (content_type={:?}, paths={:?})",
            library.name,
            library.content_type,
            library.paths
        );
    } else {
        tracing::info!("scanning {scope:?} of library '{}'", library.name);
    }
    let scope = &effective_scope(library, scope).await;
    // No library path holds this scope, so no walk covers it, and the removal would delete its items.
    if *scope != ScanScope::Library && scope_roots(library, scope).is_empty() {
        tracing::warn!(
            "{scope:?} is in no library path of library '{}', so nothing is scanned",
            library.name
        );
        return Ok(ScanChanges::default());
    }
    // The refresh shows as progress only when metadata providers are registered.
    let providers = plugin_manager.metadata_providers().await;
    let counted = !providers.is_empty();
    let (mut changes, new_item_ids) = match library.content_type {
        LibraryContentType::TvShows => scan_tv_library(pool, library, scope, ticket).await?,
        LibraryContentType::Music => {
            scan_music_library(pool, library, scope, ticket, counted).await?
        }
        _ => scan_flat_library(pool, library, scope, ticket).await?,
    };

    // Refresh metadata only for newly added items, not all items in the library.
    // This avoids unnecessary API calls on incremental scans.
    if !new_item_ids.is_empty() {
        tracing::info!(
            "refreshing metadata for {} newly added items in library '{}'",
            new_item_ids.len(),
            library.name
        );
        // A music scan reserved the units of its new items as it created them.
        if counted && library.content_type != LibraryContentType::Music {
            ticket.add_total(new_item_ids.len());
        }
        // Increased from 5 to 10 — TMDb rate limits are typically 20-40 req/s, so 10 is safe.
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
        // Seasons and episodes are looked up through their series' provider id, so the series
        // (and everything else) are refreshed first, then the seasons and episodes.
        let mut first = Vec::new();
        let mut later = Vec::new();
        for new_id in new_item_ids {
            match rustyfin_db::repo_media::get_item_by_id(pool, new_id).await {
                Ok(Some(item))
                    if matches!(
                        item.media_type,
                        rustyfin_model::media::MediaType::Season
                            | rustyfin_model::media::MediaType::Episode
                    ) =>
                {
                    later.push(item)
                }
                Ok(Some(item)) => first.push(item),
                _ if counted => {
                    // An item that can't be loaded has nothing to refresh, but it still counts.
                    ticket.advance(1);
                }
                _ => {}
            }
        }
        for wave in [first, later] {
            let mut handles = Vec::new();
            for item in wave {
                let sem = semaphore.clone();
                let pool = pool.clone();
                let client = http_client.clone();
                let pm = plugin_manager.clone();
                let handle = tokio::spawn(async move {
                    let _perm = sem.acquire().await;
                    if let Err(e) = crate::metadata::refresh_item_metadata_with(
                        &pool,
                        &client,
                        &item,
                        &pm,
                        crate::metadata::RefreshOptions::SCAN,
                    )
                    .await
                    {
                        tracing::warn!("metadata refresh failed for '{}': {}", item.name, e);
                    }
                });
                handles.push(handle);
            }
            for handle in handles {
                let _ = handle.await;
                if counted {
                    ticket.advance(1);
                }
            }
        }
    }

    // The file reads below share one guard, so a scan gives up waiting on at most a few files.
    let mut reads = FileReads::new(FILE_READ_WAIT);

    // Photos get their size, EXIF fields and Primary image, as in Jellyfin.
    if matches!(
        library.content_type,
        LibraryContentType::Photos | LibraryContentType::Mixed
    ) {
        read_photos(
            pool,
            library,
            scope,
            crate::photo_info::read_photo,
            &mut reads,
        )
        .await;
    }

    // A local image replaced in place gets its new stamp, so its tag and copies change.
    changes.images = restamp_local_images(
        pool,
        library,
        scope,
        local_images::local_image_version,
        &mut reads,
    )
    .await;
    // New and changed pictures get their grid copies in the background
    crate::image_warmer::kick();

    // Rebuild materialized filter indexes after scan completes
    // This keeps the filter_genre_index, filter_year_index, filter_rating_index up-to-date.
    // A scoped rescan that changed nothing leaves them as they are.
    if (*scope == ScanScope::Library || changes.changed())
        && let Err(e) = rustyfin_db::repo_media::rebuild_filter_indexes(pool).await
    {
        tracing::warn!("failed to rebuild filter indexes: {}", e);
    }

    Ok(changes)
}

/// Reads the photos of `scope` that were never read or whose file changed since, and stores what each
/// says (`photo_info::read_photo`: the header size and the EXIF block, nothing decoded). A photo with
/// no Primary image and a readable header gets its own file as one, as Jellyfin serves a photo's
/// picture as its Primary. A failure is logged and skips its photo. Each photo is read through
/// `reads` with `read` (`photo_info::read_photo`, or a stand-in in tests), and what a chunk of
/// `PHOTO_CHUNK` photos gave is stored before the next is read. Returns how many photos' metadata
/// was stored.
async fn read_photos(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    read: PhotoReader,
    reads: &mut FileReads,
) -> u32 {
    let photos =
        match rustyfin_db::repo_photo::library_photos(pool, library.id, &path_scope(scope)).await {
            Ok(photos) => photos,
            Err(e) => {
                tracing::warn!(
                    "failed to list the photos of library '{}': {}",
                    library.name,
                    e
                );
                return 0;
            }
        };
    let mut count = 0;
    for chunk in photos.chunks(PHOTO_CHUNK) {
        let mut done = Vec::with_capacity(chunk.len());
        for photo in chunk {
            if reads.given_up(&library.name) {
                return count + store_photo_reads(pool, done).await;
            }
            let task = photo.clone();
            if let Some(Some(result)) = reads
                .run(&photo.path, move || read_one_photo(&task, read))
                .await
            {
                done.push((photo.clone(), result));
            }
        }
        count += store_photo_reads(pool, done).await;
    }
    count
}

/// How many photos are read before what they gave is stored, so an interrupted scan keeps it.
const PHOTO_CHUNK: usize = 200;

/// How long a scan waits for one file's stat and reads. A read can't be stopped: one that takes
/// longer finishes on its own blocking thread, and the scan goes on without it.
const FILE_READ_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// How many reads may be stopped waiting for: per scan, and still running across the process.
/// While this many abandoned reads are still running, scans skip their file reads, as the storage
/// likely isn't answering, so scans never tie up more blocking threads than this between them.
const MAX_ABANDONED_READS: usize = 3;

/// The paths of the reads a scan stopped waiting for that are still running. Scans of this process
/// share one (`HUNG_READS`); a test makes its own.
#[derive(Default)]
struct HungReads {
    paths: std::sync::Mutex<HashSet<String>>,
}

impl HungReads {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.paths.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn contains(&self, path: &str) -> bool {
        self.lock().contains(path)
    }

    fn len(&self) -> usize {
        self.lock().len()
    }
}

/// The process's hung reads, shared by every scan.
static HUNG_READS: std::sync::LazyLock<std::sync::Arc<HungReads>> =
    std::sync::LazyLock::new(Default::default);

/// One read's state, shared by the scan and the read's task, so the path leaves `HungReads` exactly
/// when an abandoned read ends.
#[derive(Default)]
struct ReadState {
    done: bool,
    abandoned: bool,
}

/// Marks a read done when its task ends, even by a panic, and takes an abandoned read's path out
/// of `HungReads`.
struct ReadEnd {
    state: std::sync::Arc<std::sync::Mutex<ReadState>>,
    hung: std::sync::Arc<HungReads>,
    path: String,
}

impl Drop for ReadEnd {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.done = true;
        if state.abandoned {
            self.hung.lock().remove(&self.path);
        }
    }
}

/// Runs a scan's blocking file reads, each on its own blocking task, waiting at most `wait` for
/// each. A read it stops waiting for goes on running and its path stays in `hung` until it ends;
/// later scans skip that path meanwhile. A scan skips the rest of its reads after it stopped
/// waiting for `MAX_ABANDONED_READS`, or while that many abandoned reads are still running.
struct FileReads {
    wait: std::time::Duration,
    hung: std::sync::Arc<HungReads>,
    abandoned: usize,
    warned: bool,
}

impl FileReads {
    fn new(wait: std::time::Duration) -> Self {
        Self::with_hung(wait, HUNG_READS.clone())
    }

    fn with_hung(wait: std::time::Duration, hung: std::sync::Arc<HungReads>) -> Self {
        Self {
            wait,
            hung,
            abandoned: 0,
            warned: false,
        }
    }

    /// Whether the scan skips the rest of its reads; warns once.
    fn given_up(&mut self, library: &str) -> bool {
        let running = self.hung.len();
        let given_up = self.abandoned >= MAX_ABANDONED_READS || running >= MAX_ABANDONED_READS;
        if given_up && !self.warned {
            self.warned = true;
            tracing::warn!(
                "library '{}': {} file reads took over {:?} in this scan and {} abandoned reads are \
                 still running; the rest are left for a later scan",
                library,
                self.abandoned,
                self.wait,
                running
            );
        }
        given_up
    }

    /// Runs `read` for the file at `path`; `None` when the scan stopped waiting for it, its task
    /// failed, or an earlier abandoned read of the same path is still running.
    async fn run<R: Send + 'static>(
        &mut self,
        path: &str,
        read: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        if self.hung.contains(path) {
            tracing::warn!(
                "an earlier read of '{}' is still running; skipped for this scan",
                path
            );
            return None;
        }
        let state = std::sync::Arc::new(std::sync::Mutex::new(ReadState::default()));
        let end = ReadEnd {
            state: state.clone(),
            hung: self.hung.clone(),
            path: path.to_string(),
        };
        let task = tokio::task::spawn_blocking(move || {
            let _end = end;
            read()
        });
        match tokio::time::timeout(self.wait, task).await {
            Ok(Ok(result)) => Some(result),
            Ok(Err(e)) => {
                tracing::warn!("failed to read '{}': {}", path, e);
                None
            }
            Err(_) => {
                self.abandoned += 1;
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                if !state.done {
                    state.abandoned = true;
                    self.hung.lock().insert(path.to_string());
                }
                tracing::warn!(
                    "reading '{}' took over {:?}; the scan goes on without it",
                    path,
                    self.wait
                );
                None
            }
        }
    }
}

/// Reads a photo's file: its stamp and what it says (`photo_info::read_photo`, or a stand-in in
/// tests).
type PhotoReader = fn(&Path) -> (Option<i64>, rustyfin_model::photo::PhotoInfo);

/// What one photo's read gave: the file's stamp and what it says. `changed` when the metadata is to
/// be stored again; else only the size was read, for a missing Primary image.
struct PhotoRead {
    stamp: Option<i64>,
    info: rustyfin_model::photo::PhotoInfo,
    changed: bool,
}

/// Stats and reads one photo; `None` when there's nothing to store: the file is gone, isn't a
/// regular file, or is unchanged with a Primary image. A FIFO, socket or device named like a photo
/// is never opened, as an open could block. A read that panics is logged; a changed file then gets
/// an empty row at its current stamp, so it isn't read again until it changes.
fn read_one_photo(
    photo: &rustyfin_db::repo_photo::StoredPhoto,
    read: PhotoReader,
) -> Option<PhotoRead> {
    let path = Path::new(&photo.path);
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        tracing::warn!("photo '{}' is not a regular file; skipped", photo.path);
        return None;
    }
    let current = crate::image_proxy::file_stamp(&meta);
    if photo.stamp == Some(Some(current)) {
        if photo.has_primary {
            return None;
        }
        let read = std::panic::catch_unwind(|| crate::photo_info::read_photo_size(path));
        let Ok((stamp, width, height)) = read else {
            tracing::warn!(
                "reading the size of photo '{}' panicked; skipped",
                photo.path
            );
            return None;
        };
        let info = rustyfin_model::photo::PhotoInfo {
            width,
            height,
            ..Default::default()
        };
        return Some(PhotoRead {
            stamp,
            info,
            changed: false,
        });
    }
    Some(match std::panic::catch_unwind(|| read(path)) {
        Ok((stamp, info)) => PhotoRead {
            stamp,
            info,
            changed: true,
        },
        Err(_) => {
            tracing::warn!(
                "reading photo '{}' panicked; it is skipped until the file changes",
                photo.path
            );
            PhotoRead {
                stamp: Some(current),
                info: rustyfin_model::photo::PhotoInfo::default(),
                changed: true,
            }
        }
    })
}

/// Stores what a chunk of photos gave: the metadata of each changed one, and a Primary image for
/// each with none and a readable header. Returns how many metadata rows were stored.
async fn store_photo_reads(
    pool: &SqlitePool,
    reads: Vec<(rustyfin_db::repo_photo::StoredPhoto, PhotoRead)>,
) -> u32 {
    let mut count = 0;
    for (photo, read) in reads {
        let PhotoRead {
            stamp,
            info,
            changed,
        } = read;
        if changed {
            if let Err(e) =
                rustyfin_db::repo_photo::save_photo_info(pool, photo.item_id, stamp, &info).await
            {
                tracing::warn!("failed to store the metadata of '{}': {}", photo.path, e);
                continue;
            }
            count += 1;
        }
        // A file whose header gives no size is no picture to serve.
        if photo.has_primary || stamp.is_none() || info.width.is_none() {
            continue;
        }
        let image = rustyfin_model::image::ItemImage {
            item_id: photo.item_id,
            image_type: ImageType::Primary,
            image_index: 0,
            path: photo.path.clone(),
            provider: "local".to_string(),
            width: info.width,
            height: info.height,
            blurhash: None,
            stamp,
        };
        if let Err(e) = rustyfin_db::repo_image::upsert_image(pool, &image).await {
            tracing::warn!("failed to store the image of '{}': {}", photo.path, e);
        }
    }
    count
}

/// Reads a local image's file: its stamp and size (`local_images::local_image_version`, or a
/// stand-in in tests).
type ImageReader = fn(&Path) -> (Option<i64>, Option<i32>, Option<i32>);

/// Stats one local image and, when its file changed, reads its stamp and size; `None` when there's
/// nothing to store. A FIFO, socket or device is never opened, as an open could block. An image
/// that is its item's own file (a photo's Primary) has its size read from the header alone
/// (`photo_info::read_photo_size`), as the photo pass does; other images use `read`. A read that
/// panics is logged and skips the image.
fn restamp_one(
    mut img: rustyfin_model::image::ItemImage,
    own_file: bool,
    read: ImageReader,
) -> Option<(rustyfin_model::image::ItemImage, bool)> {
    let meta = std::fs::metadata(&img.path).ok()?;
    if !meta.is_file() {
        tracing::warn!("image '{}' is not a regular file; skipped", img.path);
        return None;
    }
    if img.stamp == Some(crate::image_proxy::file_stamp(&meta)) {
        return None;
    }
    let path = Path::new(&img.path);
    let version = std::panic::catch_unwind(|| {
        if own_file {
            crate::photo_info::read_photo_size(path)
        } else {
            read(path)
        }
    });
    let Ok((stamp, width, height)) = version else {
        tracing::warn!("reading image '{}' panicked; skipped", img.path);
        return None;
    };
    let counted = img.stamp.is_some();
    img.stamp = Some(stamp?);
    if let (Some(w), Some(h)) = (width, height) {
        img.width = Some(w);
        img.height = Some(h);
    }
    Some((img, counted))
}

/// Restamps the local images of `scope` whose files changed since they were stored, and returns how
/// many had a stored stamp that changed. An image with no stamp yet is stamped without being counted.
/// A failed stat or read skips its image, and a database error skips it too, so it counts nothing.
/// A header that can't be read leaves the stored size. Each image is stat'ed and read through
/// `reads` (see `restamp_one`).
async fn restamp_local_images(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    read: ImageReader,
    reads: &mut FileReads,
) -> u32 {
    let images = match rustyfin_db::repo_image::get_local_images_in_scope(
        pool,
        library.id,
        &path_scope(scope),
    )
    .await
    {
        Ok(images) => images,
        Err(e) => {
            tracing::warn!(
                "failed to read the images of library '{}': {}",
                library.name,
                e
            );
            return 0;
        }
    };
    // One stat per image. Only a file whose stamp differs is read again, for its size.
    let mut changed = Vec::new();
    for (img, own_file) in images {
        if reads.given_up(&library.name) {
            break;
        }
        let path = img.path.clone();
        if let Some(Some(result)) = reads
            .run(&path, move || restamp_one(img, own_file, read))
            .await
        {
            changed.push(result);
        }
    }

    // The blurhashes are computed first, so the stores below run as one batch. Each runs through
    // `reads`, with a panic caught; one that doesn't finish keeps the image's old blurhash.
    let mut counted_images = 0;
    let mut batch = Vec::with_capacity(changed.len());
    for (mut img, counted) in changed {
        if counted {
            if !reads.given_up(&library.name) {
                let path = img.path.clone();
                let hash = reads
                    .run(&img.path, move || {
                        std::panic::catch_unwind(|| {
                            crate::image_proxy::local_blurhash(Path::new(&path))
                        })
                    })
                    .await;
                match hash {
                    Some(Ok(hash)) => img.blurhash = hash,
                    Some(Err(_)) => {
                        tracing::warn!("the blurhash of image '{}' panicked; kept", img.path)
                    }
                    None => {}
                }
            }
            counted_images += 1;
        }
        batch.push(img);
    }
    match rustyfin_db::repo_image::update_image_versions(pool, &batch).await {
        Ok(()) => counted_images,
        Err(e) => {
            tracing::warn!(
                "failed to store the versions of the images of library '{}': {}",
                library.name,
                e
            );
            0
        }
    }
}

/// Serializes the scans that write items. Held until the probes they queued have finished.
pub static SCAN_LOCK: Mutex<()> = Mutex::const_new(());

/// Waits until no scan is running and the probes of the last scan have finished.
pub async fn wait_for_idle() {
    drop(SCAN_LOCK.lock().await);
}

/// Runs the probe plans of a scan in the background. One task owns `guard` until every plan has
/// finished, so the next scan waits for them. The plans are the work of `ticket`'s scan.
fn spawn_plans_under_lock(
    pool: SqlitePool,
    guard: MutexGuard<'static, ()>,
    library_name: String,
    plans: Vec<(Uuid, Vec<probe::ProbeStep>)>,
    ticket: ScanTicket,
) {
    ticket.add_total(plans.len());
    tokio::spawn(async move {
        let total_steps: usize = plans.iter().map(|(_, steps)| steps.len()).sum();
        let handles: Vec<_> = plans
            .into_iter()
            .map(|(item_id, steps)| probe::spawn_probe_plan(pool.clone(), item_id, steps))
            .collect();
        for handle in handles {
            let _ = handle.await;
            ticket.advance(1);
        }
        tracing::info!(
            "probes finished for library '{}': {} files",
            library_name,
            total_steps
        );
        // The ticket goes first, so this scan's share has ended once the next scan holds the lock.
        drop(ticket);
        drop(guard);
    });
}

/// The scope that a scan of `scope` reads. A `Folder` that names a file is read as the folder that
/// holds the file: `RootFiles` when that folder is a library path, else `Folder`.
async fn effective_scope(library: &Library, scope: &ScanScope) -> ScanScope {
    let ScanScope::Folder(folder) = scope else {
        return scope.clone();
    };
    let is_file = fs::metadata(folder).await.is_ok_and(|meta| !meta.is_dir());
    let Some(parent) = folder.parent().filter(|_| is_file) else {
        return scope.clone();
    };
    if library.paths.iter().any(|root| Path::new(root) == parent) {
        ScanScope::RootFiles(parent.to_path_buf())
    } else {
        ScanScope::Folder(parent.to_path_buf())
    }
}

/// The path filter of a scan of `scope`. Its folders have no trailing `/`, as no path of an item has
/// one.
fn path_scope(scope: &ScanScope) -> PathScope {
    match scope {
        ScanScope::Library => PathScope::All,
        ScanScope::Folder(folder) => {
            PathScope::Under(folder.to_string_lossy().trim_end_matches('/').to_string())
        }
        ScanScope::RootFiles(dir) => {
            PathScope::DirectlyIn(dir.to_string_lossy().trim_end_matches('/').to_string())
        }
    }
}

/// The library paths that a scan of `scope` walks, in library order. A folder belongs to the longest
/// library path that holds it, so no other path is walked for it.
fn scope_roots<'a>(library: &'a Library, scope: &ScanScope) -> Vec<&'a str> {
    let paths = library.paths.iter().map(String::as_str);
    match scope {
        ScanScope::Library => paths.collect(),
        ScanScope::Folder(folder) => paths
            .filter(|root| folder.starts_with(root))
            .max_by_key(|root| Path::new(root).components().count())
            .into_iter()
            .collect(),
        ScanScope::RootFiles(dir) => paths.filter(|root| Path::new(root) == dir).collect(),
    }
}

/// The item of `media_type` at `path`, for a miss in a preload of a scan. A full scan preloads
/// every item of the library, so it looks up nothing.
async fn lookup_item(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &ScanScope,
    media_type: &str,
    path: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if *scope == ScanScope::Library {
        return Ok(None);
    }
    rustyfin_db::repo_media::get_library_item_id(pool, library_id, media_type, path).await
}

/// The artist at `path`, for a miss in the artist preload of a scan. As in `lookup_item`, a full
/// scan looks up nothing. Nor does a name-keyed artist, with the empty path: the preload has all of
/// them, in every scope.
async fn lookup_artist(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &ScanScope,
    path: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    if *scope == ScanScope::Library || path.is_empty() {
        return Ok(None);
    }
    rustyfin_db::repo_media::get_library_artist_id(pool, library_id, path).await
}

/// Removes the items of `filter` that the scan no longer collected, and returns how many. A failure
/// is logged, and counts as none.
async fn remove_uncollected(
    pool: &SqlitePool,
    library: &Library,
    filter: &PathScope,
    collected: &HashSet<String>,
    unreadable: &[PathBuf],
    loops: &[PathBuf],
) -> u32 {
    match scan_sync::remove_not_collected(pool, library, filter, collected, unreadable, loops).await
    {
        Ok(removed) => {
            tracing::info!(
                "removed {removed} items no longer collected from library '{}'",
                library.name
            );
            removed
        }
        Err(e) => {
            tracing::warn!(
                "removing items no longer collected from library '{}' failed: {}",
                library.name,
                e
            );
            0
        }
    }
}

/// Flat scan for Movies, Photos, Mixed libraries
async fn scan_flat_library(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    ticket: &ScanTicket,
) -> Result<(ScanChanges, Vec<Uuid>), sqlx::Error> {
    let guard = SCAN_LOCK.lock().await;
    ticket.start();
    let filter = path_scope(scope);
    let mut count = 0u32;
    let mut existing_count = 0u32;
    let mut reread = 0u32;
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    // Avoids firing one DB query per file during scanning.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id, &filter)
            .await?
            .into_iter()
            .collect();

    // The audio sources that an older probe wrote, by path. Their files are read again, once.
    let outdated = scan_sync::outdated_audio_sources(pool, library.id, &filter).await?;
    let rereads = scan_sync::RereadGate::new();

    // Collect new items for batch insert in a single transaction
    let mut new_items: Vec<MediaItem> = Vec::new();
    // Probe plans. The plans of new items wait in `new_plans` until their rows are inserted.
    let mut plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    let mut new_plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    // The video files of each folder. They are grouped into movies after the walk.
    let mut movie_files: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();

    // Every file that the walks returned, every folder they could not list, and every symlink
    // loop they skipped. The removal at the end of the scan needs all three.
    let mut collected: HashSet<String> = HashSet::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    let mut loops: Vec<PathBuf> = Vec::new();

    for lib_path in scope_roots(library, scope) {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // Async directory traversal — non-blocking on Tokio runtime
        let (files, unreadable_dirs, loop_dirs) = collect_in_scope(path, scope).await;
        collected.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
        unreadable.extend(unreadable_dirs);
        loops.extend(loop_dirs);

        for file_path_buf in files {
            let ext = file_path_buf
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let media_type = detect_media_type(&ext, &library.content_type);
            let media_type = match media_type {
                Some(mt) => mt,
                None => continue,
            };

            let file_path = file_path_buf.to_string_lossy().to_string();

            // Movies are grouped per folder after the walk. Existing files are grouped too.
            if matches!(media_type, MediaType::Movie) {
                if existing_paths.contains(&file_path) {
                    existing_count += 1;
                }
                let folder = file_path_buf
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                movie_files.entry(folder).or_default().push(file_path_buf);
                continue;
            }

            // Skip if this exact file path already exists (in-memory check)
            if existing_paths.contains(&file_path) {
                existing_count += 1;
                // An audio file that an older probe wrote is read again, once, when
                // `rereads` opens.
                if let Some(&item_id) = outdated.get(&file_path)
                    && rereads.open().await
                {
                    reread += 1;
                    plans.push((item_id, vec![probe::ProbeStep::Primary(file_path.clone())]));
                }
                continue;
            }

            let file_name = file_path_buf
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            let item = MediaItem {
                id: Uuid::new_v4(),
                name: file_name.clone(),
                sort_name: Some(file_name.to_lowercase()),
                overview: None,
                media_type,
                path: file_path.clone(),
                parent_id: None,
                library_id: library.id,
                production_year: parse_year(&file_name),
                community_rating: None,
                official_rating: None,
                genres: vec![],
                date_created: date_added(&file_path_buf).await,
                premiere_date: None,
                runtime_ticks: None,
                index_number: None,
                parent_index_number: None,
                media_sources: Vec::new(),
                ..MediaItem::default_dto_fields()
            };

            new_plans.push((item.id, vec![probe::ProbeStep::Primary(item.path.clone())]));
            new_items.push(item);
        }
    }

    // Group the movie files of each folder into movies, then reconcile them with the items
    let mut movie_groups: Vec<naming::VersionGroup> = Vec::new();
    for (folder, files) in &movie_files {
        let is_library_root = library
            .paths
            .iter()
            .any(|p| Path::new(p) == folder.as_path());
        movie_groups.extend(naming::group_movie_files(folder, files, is_library_root));
    }
    // The items whose file is gone are read first: a group with no owner may take one over.
    let gone =
        scan_sync::gone_items(pool, library, &filter, &collected, &unreadable, &loops).await?;
    let outcomes = scan_sync::reconcile_movie_groups(
        pool,
        library.id,
        &filter,
        &movie_groups,
        &gone,
        &rereads,
    )
    .await?;
    for (group, outcome) in movie_groups.iter().zip(outcomes) {
        match outcome {
            scan_sync::GroupOutcome::Existing { item_id, steps } => {
                if !steps.is_empty() {
                    reread += 1;
                    plans.push((item_id, steps));
                }
            }
            scan_sync::GroupOutcome::New { path, steps } => {
                let date_created = date_added(Path::new(&path)).await;
                let item = MediaItem {
                    id: Uuid::new_v4(),
                    name: group.title.clone(),
                    sort_name: Some(group.title.to_lowercase()),
                    overview: None,
                    media_type: MediaType::Movie,
                    path,
                    parent_id: None,
                    library_id: library.id,
                    production_year: group.year,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_plans.push((item.id, steps));
                new_items.push(item);
            }
        }
    }

    // Batch-insert all new items in a single transaction
    if !new_items.is_empty() {
        if let Err(e) = rustyfin_db::repo_media::batch_upsert_media_items(pool, &new_items).await {
            tracing::error!("failed to batch insert media items: {}", e);
        } else {
            // Find and save local images for all newly inserted items
            let mut all_images: Vec<rustyfin_model::image::ItemImage> = Vec::new();
            for item in &new_items {
                let item_path = std::path::Path::new(&item.path);
                let images = local_images::find_local_images(item_path, item.id);
                if !images.is_empty() {
                    tracing::debug!("found {} local images for '{}'", images.len(), item.name);
                }
                all_images.extend(images);
            }
            if !all_images.is_empty() {
                if let Err(e) =
                    rustyfin_db::repo_image::batch_upsert_images(pool, &all_images).await
                {
                    tracing::error!("failed to batch insert local images: {}", e);
                } else {
                    tracing::info!("saved {} local images", all_images.len());
                }
            }

            // The plans of the new items run with the other plans, when the scan has finished.
            plans.append(&mut new_plans);
            count += new_items.len() as u32;
            // Collect IDs for metadata refresh
            new_item_ids = new_items.iter().map(|item| item.id).collect();
        }
    }

    tracing::info!(
        "scanned library '{}': {} new items added ({} existing items unchanged)",
        library.name,
        count,
        existing_count
    );

    // Remove the items that this scan no longer collects. A failure is logged, not returned.
    let removed = remove_uncollected(pool, library, &filter, &collected, &unreadable, &loops).await;
    spawn_plans_under_lock(
        pool.clone(),
        guard,
        library.name.clone(),
        plans,
        ticket.clone(),
    );
    let changes = ScanChanges {
        added: count,
        removed,
        reread,
        images: 0,
    };
    Ok((changes, new_item_ids))
}

/// A series folder that a TV scan walked.
struct SeriesWalk {
    /// The folder's path.
    dir: String,
    /// The folder's name.
    folder: String,
    /// Its seasons, by id: the name of each one's folder, or the one made for its loose files, and its number.
    seasons: BTreeMap<Uuid, (String, Option<i32>)>,
}

/// Recursive scan for TV Shows: path/ShowName/Season XX/episode files
async fn scan_tv_library(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    ticket: &ScanTicket,
) -> Result<(ScanChanges, Vec<Uuid>), sqlx::Error> {
    let guard = SCAN_LOCK.lock().await;
    ticket.start();
    let filter = path_scope(scope);
    let mut count = 0u32;
    let mut plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    let mut existing_count = 0u32;
    let mut reread = 0u32;
    let rereads = scan_sync::RereadGate::new();
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id, &filter)
            .await?
            .into_iter()
            .collect();

    // Pre-load all existing series and season paths into HashMaps for O(1) ID lookup.
    // Eliminates per-series/season database queries during directory traversal. A scoped scan
    // loads the ones in its scope, and looks up the others when it meets them.
    let existing_series: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "Series", &filter)
            .await?;
    let existing_seasons: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "Season", &filter)
            .await?;
    // The episode files of each season, by season id: the id of its series, the season's number, then its
    // files in walk order. The episodes are reconciled once every path has been walked.
    let mut episode_files: BTreeMap<Uuid, (Uuid, Option<i32>, Vec<PathBuf>)> = BTreeMap::new();
    // Each series that the walks met, by id: its folder's path and name, and its seasons with their
    // folder's name and number. Series and seasons stored under older rules are settled from these.
    let mut series_folders: BTreeMap<Uuid, SeriesWalk> = BTreeMap::new();

    // Every file that the walks returned, every folder they could not list, and every symlink
    // loop they skipped. The removal at the end of the scan needs all three.
    let mut collected: HashSet<String> = HashSet::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    let mut loops: Vec<PathBuf> = Vec::new();

    for lib_path in scope_roots(library, scope) {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // series_path -> series_id (start with pre-loaded DB data)
        let mut series_map: HashMap<String, Uuid> = existing_series.clone();
        // season_path -> season_id (start with pre-loaded DB data)
        let mut season_map: HashMap<String, Uuid> = existing_seasons.clone();

        // Collect new items for batch insert in a single transaction
        let mut new_items: Vec<MediaItem> = Vec::new();

        // Async directory traversal — non-blocking on Tokio runtime
        let (files, unreadable_dirs, loop_dirs) = collect_in_scope(path, scope).await;
        collected.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
        unreadable.extend(unreadable_dirs);
        loops.extend(loop_dirs);
        tracing::info!(
            "TV library '{}': found {} files in path {}",
            library.name,
            files.len(),
            lib_path
        );

        let mut scanned_files = 0u32;
        let mut skipped_ext = 0u32;
        for file_path_buf in files {
            scanned_files += 1;
            if !is_episode_file(&file_path_buf) {
                skipped_ext += 1;
                continue;
            }

            let file_path = file_path_buf.to_string_lossy().to_string();

            // An existing file is counted, then placed like a new one.
            if existing_paths.contains(&file_path) {
                existing_count += 1;
            }

            // Determine hierarchy from path components relative to lib_path
            let rel = file_path_buf.strip_prefix(path).unwrap_or(&file_path_buf);
            let components: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();

            let (series_name, season_name) = match components.len() {
                // path/ShowName/Season/episode.mkv
                3.. => {
                    let sn = components[0].to_string();
                    let sea = components[1].to_string();
                    (sn, sea)
                }
                // path/ShowName/episode.mkv (no season folder)
                2 => {
                    let sn = components[0].to_string();
                    let ep = file_path_buf
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let season = naming::parse_episode(&ep).and_then(|n| n.season);
                    (sn, format!("Season {}", season.unwrap_or(1)))
                }
                // path/episode.mkv (flat)
                _ => ("Unknown Series".to_string(), "Season 1".to_string()),
            };

            let series_dir = path.join(&series_name).to_string_lossy().to_string();

            // Ensure series item exists — use pre-loaded HashMap first, then create new
            let series_id = if let Some(id) = series_map.get(&series_dir) {
                *id
            } else if let Some(id) =
                lookup_item(pool, library.id, scope, "Series", &series_dir).await?
            {
                series_map.insert(series_dir.clone(), id);
                id
            } else {
                let id = Uuid::new_v4();
                let (name, year) = match naming::series_title_and_year(&series_name) {
                    Some((title, year)) => (title.to_string(), Some(year)),
                    None => (series_name.clone(), None),
                };
                let series_item = MediaItem {
                    id,
                    sort_name: Some(name.to_lowercase()),
                    name,
                    overview: None,
                    media_type: MediaType::Series,
                    path: series_dir.clone(),
                    parent_id: None,
                    library_id: library.id,
                    production_year: year,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created: date_added(Path::new(&series_dir)).await,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push(series_item);
                series_map.insert(series_dir.clone(), id);
                id
            };

            // Ensure season item exists — use pre-loaded HashMap first, then create new
            let season_path = format!("{}/{}", series_dir, season_name);
            let season_number = parse_season_number(&season_name, &series_name);
            let season_id = if let Some(id) = season_map.get(&season_path) {
                *id
            } else if let Some(id) =
                lookup_item(pool, library.id, scope, "Season", &season_path).await?
            {
                season_map.insert(season_path.clone(), id);
                id
            } else {
                let id = Uuid::new_v4();
                let season_item = MediaItem {
                    id,
                    name: season_name.clone(),
                    sort_name: Some(season_sort_name(season_number)),
                    overview: None,
                    media_type: MediaType::Season,
                    path: season_path.clone(),
                    parent_id: Some(series_id),
                    library_id: library.id,
                    production_year: None,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created: date_added(Path::new(&season_path)).await,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: season_number,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    // A new season with no stat result is a folder; the next scan settles it.
                    location_type: season_location(&season_path).await.unwrap_or_default(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push(season_item);
                season_map.insert(season_path, id);
                id
            };

            series_folders
                .entry(series_id)
                .or_insert_with(|| SeriesWalk {
                    dir: series_dir.clone(),
                    folder: series_name.clone(),
                    seasons: BTreeMap::new(),
                })
                .seasons
                .entry(season_id)
                .or_insert_with(|| (season_name.clone(), season_number));
            episode_files
                .entry(season_id)
                .or_insert_with(|| (series_id, season_number, Vec::new()))
                .2
                .push(file_path_buf);
        }

        tracing::info!(
            "TV library '{}': scanned {} files, skipped {} (non-video extension), created {} new series and seasons",
            library.name,
            scanned_files,
            skipped_ext,
            new_items.len()
        );

        // Batch-insert the new series and seasons in a single transaction. The episodes of the
        // library need their seasons, so this runs before any episode is reconciled.
        if !new_items.is_empty() {
            rustyfin_db::repo_media::batch_upsert_media_items(pool, &new_items).await?;
            // Find and save local images for series and seasons
            let mut all_images: Vec<rustyfin_model::image::ItemImage> = Vec::new();
            for item in &new_items {
                if item.media_type == MediaType::Series || item.media_type == MediaType::Season {
                    let item_path = std::path::Path::new(&item.path);
                    let images = local_images::find_local_images(item_path, item.id);
                    if !images.is_empty() {
                        tracing::debug!("found {} local images for '{}'", images.len(), item.name);
                    }
                    all_images.extend(images);
                }
            }
            if !all_images.is_empty() {
                if let Err(e) =
                    rustyfin_db::repo_image::batch_upsert_images(pool, &all_images).await
                {
                    tracing::error!("failed to batch insert local images: {}", e);
                } else {
                    tracing::info!("saved {} local images for TV library", all_images.len());
                }
            }
            count += new_items.len() as u32;
            new_item_ids.extend(new_items.iter().map(|item| item.id));
        }
    }

    // A season stored under an older rule gets the number that its folder gives now.
    let season_numbers: Vec<(Uuid, Option<i32>)> = episode_files
        .iter()
        .map(|(&season_id, &(_, season_number, _))| (season_id, season_number))
        .collect();
    let settled = scan_sync::settle_season_numbers(pool, library.id, &season_numbers).await?;
    if settled > 0 {
        tracing::info!(
            "TV library '{}': set the number of {} stored seasons",
            library.name,
            settled
        );
    }
    // A season stored under an older rule loses the Path of a folder that it doesn't have, and a season whose
    // folder has appeared since gets its Path. A season whose stat fails is left out, so it keeps its location.
    let mut season_folders: Vec<(Uuid, LocationType)> = Vec::new();
    for walk in series_folders.values() {
        for (&id, (name, _)) in &walk.seasons {
            if let Some(location) = season_location(&format!("{}/{}", walk.dir, name)).await {
                season_folders.push((id, location));
            }
        }
    }
    let settled = scan_sync::settle_season_folders(pool, library.id, &season_folders).await?;
    if settled > 0 {
        tracing::info!(
            "TV library '{}': set whether {} stored seasons have a folder",
            library.name,
            settled
        );
    }
    // A series stored under an older rule gets the name and year that its folder gives now, and what its
    // own tvshow.nfo gives; its seasons and the episode files directly in its folder lose what they took
    // from that file, and the images that it gave them.
    let mut folders = Vec::with_capacity(series_folders.len());
    for (&id, walk) in &series_folders {
        let nfo = match nfo_parser::find_series_nfo(Path::new(&walk.dir)) {
            Some(nfo_path) => nfo_parser::parse_nfo(&nfo_path).await,
            None => None,
        };
        // Under the older rule, a season and a loose episode read the series' tvshow.nfo when
        // `find_nfo_for_media` found no other `.nfo` for them.
        let (nfo_seasons, nfo_episodes) = match nfo {
            Some(_) => {
                let series_dir = Path::new(&walk.dir);
                let seasons = walk
                    .seasons
                    .iter()
                    .filter(|(_, (name, _))| {
                        nfo_parser::find_nfo_for_media(&series_dir.join(name)).is_none()
                    })
                    .map(|(&season_id, (name, number))| (season_id, name.clone(), *number))
                    .collect();
                let episodes = episode_files
                    .values()
                    .filter(|(series_id, _, _)| *series_id == id)
                    .flat_map(|(_, _, files)| files)
                    .filter(|file| file.parent() == Some(series_dir))
                    .filter(|file| nfo_parser::find_nfo_for_media(file).is_none())
                    .map(|file| file.to_string_lossy().into_owned())
                    .collect();
                (seasons, episodes)
            }
            None => (Vec::new(), Vec::new()),
        };
        folders.push(scan_sync::SeriesFolder {
            id,
            folder: walk.folder.clone(),
            nfo,
            nfo_seasons,
            nfo_episodes,
        });
    }
    let settled = scan_sync::settle_series(pool, library.id, &folders).await?;
    if settled > 0 {
        tracing::info!(
            "TV library '{}': settled the names and metadata of {} stored series and seasons",
            library.name,
            settled
        );
    }

    // Group the files of each season into episodes, then reconcile the episodes with the items.
    // The numbers of an episode come from its primary file, the first of its versions. The last
    // episode of a file that holds several is Jellyfin's IndexNumberEnd (12.4
    // `Library/LibraryManager.cs:3391-3398`).
    let mut episode_groups: Vec<scan_sync::EpisodeGroup> = Vec::new();
    for (&season_id, &(series_id, season_number, ref season_files)) in &episode_files {
        for versions in naming::group_episode_files(season_files, season_number) {
            let stem = versions[0]
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown");
            let parsed = naming::parse_episode(stem);
            episode_groups.push(scan_sync::EpisodeGroup {
                files: versions,
                season_id,
                series_id,
                index_number: parsed.map(|n| n.episode),
                index_number_end: parsed.and_then(|n| n.ending),
                // In a specials folder the file name's season wins (Jellyfin 12.4 `LibraryManager.cs:3400-3423`).
                parent_index_number: if season_number == Some(0) {
                    parsed.and_then(|n| n.season).or(season_number)
                } else {
                    season_number.or_else(|| parsed.and_then(|n| n.season))
                },
            });
        }
    }
    // The items whose file is gone are read first: a group with no owner may take one over.
    let gone =
        scan_sync::gone_items(pool, library, &filter, &collected, &unreadable, &loops).await?;
    let outcomes = scan_sync::reconcile_episode_groups(
        pool,
        library.id,
        &filter,
        &episode_groups,
        &gone,
        &rereads,
    )
    .await?;
    let mut new_episodes: Vec<MediaItem> = Vec::new();
    let mut new_episode_plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    for (group, outcome) in episode_groups.iter().zip(outcomes) {
        match outcome {
            scan_sync::GroupOutcome::Existing { item_id, steps } => {
                if !steps.is_empty() {
                    reread += 1;
                    plans.push((item_id, steps));
                }
            }
            scan_sync::GroupOutcome::New { path, steps } => {
                let name = group.files[0]
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let date_created = date_added(Path::new(&path)).await;
                let item = MediaItem {
                    id: Uuid::new_v4(),
                    name: name.clone(),
                    sort_name: Some(name.to_lowercase()),
                    overview: None,
                    media_type: MediaType::Episode,
                    path,
                    parent_id: Some(group.season_id),
                    library_id: library.id,
                    production_year: None,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: group.index_number,
                    parent_index_number: group.parent_index_number,
                    index_number_end: group.index_number_end,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_episode_plans.push((item.id, steps));
                new_episodes.push(item);
            }
        }
    }

    // Batch-insert the new episodes in a single transaction. As in the movie scan, their plans
    // are queued only when the insert succeeds.
    if !new_episodes.is_empty() {
        if let Err(e) = rustyfin_db::repo_media::batch_upsert_media_items(pool, &new_episodes).await
        {
            tracing::error!("failed to batch insert episodes: {}", e);
        } else {
            plans.append(&mut new_episode_plans);
            count += new_episodes.len() as u32;
            new_item_ids.extend(new_episodes.iter().map(|item| item.id));
        }
    }

    tracing::info!(
        "scanned TV library '{}': {} new items added ({} existing items unchanged)",
        library.name,
        count,
        existing_count
    );

    // Remove the items that this scan no longer collects. A failure is logged, not returned.
    let removed = remove_uncollected(pool, library, &filter, &collected, &unreadable, &loops).await;
    spawn_plans_under_lock(
        pool.clone(),
        guard,
        library.name.clone(),
        plans,
        ticket.clone(),
    );
    let changes = ScanChanges {
        added: count,
        removed,
        reread,
        images: 0,
    };
    Ok((changes, new_item_ids))
}

/// Recursive scan for Music: path/Artist/Album/track files
/// Builds Artist → Album → Track hierarchy using directory structure and audio tags.
/// With `counted`, each new artist, album and track reserves one unit of the ticket's total.
async fn scan_music_library(
    pool: &SqlitePool,
    library: &Library,
    scope: &ScanScope,
    ticket: &ScanTicket,
    counted: bool,
) -> Result<(ScanChanges, Vec<Uuid>), sqlx::Error> {
    let guard = SCAN_LOCK.lock().await;
    ticket.start();
    let filter = path_scope(scope);
    let mut count = 0u32;
    let mut existing_count = 0u32;
    let mut reread = 0u32;
    let rereads = scan_sync::RereadGate::new();
    let mut new_item_ids: Vec<Uuid> = Vec::new();
    // Probe plans of the old tracks, read again under the scan lock, see the end of the scan.
    let mut plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id, &filter)
            .await?
            .into_iter()
            .collect();

    // The audio sources that an older probe wrote, by path. Their tracks are read again, once.
    let outdated = scan_sync::outdated_audio_sources(pool, library.id, &filter).await?;

    // The source of each known track, by path: its item, and the stamp that it stores.
    let known_tracks: HashMap<String, (Uuid, Option<i64>, Option<i64>)> =
        rustyfin_db::repo_media::get_library_audio_sources(pool, library.id, &filter)
            .await?
            .into_iter()
            .map(|(id, path, size, modified)| (path, (id, size, modified)))
            .collect();

    // Pre-load the music album rows once for the whole scan, for O(1) ID lookup.
    // Artists and albums both use MediaType::MusicAlbum in the DB, so artist paths are here too.
    // A scoped scan loads the rows in its scope, and looks up the others when it meets them.
    // album_path -> album_id
    let mut album_map: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "MusicAlbum", &filter)
            .await?;
    // artist_key -> artist_id, for the artist rows of earlier scans, keyed as the loop keys them
    let mut artist_map: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_artists(pool, library.id, &filter)
            .await?
            .into_iter()
            .map(|(id, path, name)| {
                let key = if path.is_empty() {
                    name.to_lowercase()
                } else {
                    path
                };
                (key, id)
            })
            .collect();

    // Walk every root before the first file, so that the total covers all of them. The walks also
    // give the removal at the end what it needs: every file seen, the unlisted folders and the loops.
    let mut roots: Vec<(&Path, Vec<PathBuf>)> = Vec::new();
    let mut collected: HashSet<String> = HashSet::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    let mut loops: Vec<PathBuf> = Vec::new();
    for lib_path in scope_roots(library, scope) {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // Async directory traversal — non-blocking on Tokio runtime
        let (files, unreadable_dirs, loop_dirs) = collect_in_scope(path, scope).await;
        tracing::info!(
            "music library '{}': found {} files in path {}",
            library.name,
            files.len(),
            lib_path
        );
        collected.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
        unreadable.extend(unreadable_dirs);
        loops.extend(loop_dirs);
        roots.push((path, files));
    }
    ticket.add_total(roots.iter().map(|(_, files)| files.len()).sum());
    // The tracks whose file is gone are read before the new files settle: a new file may take one over.
    let mut gone_tracks =
        scan_sync::gone_tracks(pool, library, &filter, &collected, &unreadable, &loops).await?;

    for (path, files) in roots {
        // Collect new items for batch insert in a single transaction
        let mut new_items: Vec<MediaItem> = Vec::new();

        let mut scanned_files = 0u32;
        let mut skipped_ext = 0u32;
        for file_path_buf in files {
            // Every file the loop visits counts, skipped ones included.
            ticket.advance(1);
            let ext = file_path_buf
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            scanned_files += 1;
            if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                skipped_ext += 1;
                continue;
            }

            let file_path = file_path_buf.to_string_lossy().to_string();

            // Skip if already exists (in-memory check)
            if existing_paths.contains(&file_path) {
                existing_count += 1;
                // A track whose source an older probe wrote is read again by this scan's probe
                // plans, which hold the scan lock until they end, when `rereads` opens. A track
                // whose file changed since it was stamped is probed again, under the same item. A
                // track that was never stamped is stamped now, without a probe.
                if let Some(&item_id) = outdated.get(&file_path)
                    && rereads.open().await
                {
                    reread += 1;
                    plans.push((item_id, vec![probe::ProbeStep::Primary(file_path.clone())]));
                } else if let Some(&(item_id, size, modified)) = known_tracks.get(&file_path) {
                    if modified.is_none() {
                        let stamp = scan_sync::file_stamp(&file_path);
                        scan_sync::store_stamp(pool, &item_id.to_string(), &file_path, stamp)
                            .await?;
                    } else if scan_sync::source_changed(&file_path, size, modified) {
                        reread += 1;
                        probe::spawn_probe_and_save(pool.clone(), item_id, file_path.clone());
                    }
                }
                continue;
            }

            let rel = file_path_buf.strip_prefix(path).unwrap_or(&file_path_buf);
            let components: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();

            // Determine artist and album from directory structure
            // Expected: MusicLibrary/Artist/Album/track.mp3
            // Fallback: MusicLibrary/Album/track.mp3 (no artist)
            let (artist_name_from_dir, album_name_from_dir, album_dir, artist_dir) =
                if components.len() >= 3 {
                    // Artist/Album/track.mp3
                    let artist_dir = path.join(components[components.len() - 3]);
                    let album_dir = file_path_buf.parent().unwrap_or(path);
                    (
                        Some(components[components.len() - 3].to_string()),
                        components[components.len() - 2].to_string(),
                        album_dir.to_string_lossy().to_string(),
                        artist_dir.to_string_lossy().to_string(),
                    )
                } else if components.len() >= 2 {
                    // Album/track.mp3 (no artist directory)
                    let album_dir = file_path_buf.parent().unwrap_or(path);
                    (
                        None,
                        components[components.len() - 2].to_string(),
                        album_dir.to_string_lossy().to_string(),
                        String::new(),
                    )
                } else {
                    // track.mp3 at root
                    (
                        None,
                        "Unknown Album".to_string(),
                        path.to_string_lossy().to_string(),
                        String::new(),
                    )
                };

            // First, probe the file synchronously to get audio tags for metadata
            let probe_result = probe::probe_file(&file_path).await;
            let (
                _has_audio_tags,
                artist_name_from_tag,
                album_name_from_tag,
                genre,
                year,
                track_number,
                title_from_tag,
            ) = if let Ok(result) = &probe_result {
                if let Some(ref tags) = result.audio_tags {
                    (
                        true,
                        tags.artist.clone().or_else(|| tags.album_artist.clone()),
                        tags.album.clone(),
                        tags.genre.clone(),
                        tags.year.as_ref().and_then(|y| y.parse::<i32>().ok()),
                        tags.track_number.map(|n| n as i32),
                        tags.title.clone(),
                    )
                } else {
                    (false, None, None, None, None, None, None)
                }
            } else {
                (false, None, None, None, None, None, None)
            };

            // Use tag album name if available, otherwise directory name
            let effective_album_name = album_name_from_tag
                .clone()
                .unwrap_or(album_name_from_dir.clone());

            // Resolve artist
            let effective_artist_name = artist_name_from_tag
                .clone()
                .or_else(|| artist_name_from_dir.clone());

            // Resolve artist ID if we have an artist name
            let artist_id = if let Some(ref artist_name) = effective_artist_name {
                let artist_key = if !artist_dir.is_empty() {
                    artist_dir.clone()
                } else {
                    artist_name.to_lowercase()
                };
                if let Some(id) = artist_map.get(&artist_key) {
                    Some(*id)
                } else if let Some(id) = lookup_artist(pool, library.id, scope, &artist_dir).await?
                {
                    artist_map.insert(artist_key, id);
                    Some(id)
                } else {
                    let id = Uuid::new_v4();
                    let artist_item = MediaItem {
                        id,
                        name: artist_name.clone(),
                        sort_name: Some(artist_name.to_lowercase()),
                        overview: None,
                        media_type: MediaType::MusicAlbum, // Artists use same type as albums in DB
                        path: artist_dir.clone(),
                        parent_id: None,
                        library_id: library.id,
                        production_year: None,
                        community_rating: None,
                        official_rating: None,
                        genres: vec![],
                        date_created: date_added(Path::new(&artist_dir)).await,
                        premiere_date: None,
                        runtime_ticks: None,
                        index_number: None,
                        parent_index_number: None,
                        media_sources: Vec::new(),
                        ..MediaItem::default_dto_fields()
                    };
                    new_items.push(artist_item);
                    artist_map.insert(artist_key, id);
                    if counted {
                        ticket.add_total(1);
                    }
                    Some(id)
                }
            } else {
                None
            };

            // Resolve album with parent_id pointing to artist — use pre-loaded HashMap first
            let album_id = if let Some(id) = album_map.get(&album_dir) {
                *id
            } else if let Some(id) =
                lookup_item(pool, library.id, scope, "MusicAlbum", &album_dir).await?
            {
                album_map.insert(album_dir.clone(), id);
                id
            } else {
                let id = Uuid::new_v4();
                let album_item = MediaItem {
                    id,
                    name: effective_album_name.clone(),
                    sort_name: Some(effective_album_name.to_lowercase()),
                    overview: None,
                    media_type: MediaType::MusicAlbum,
                    path: album_dir.clone(),
                    parent_id: artist_id,
                    library_id: library.id,
                    production_year: year,
                    community_rating: None,
                    official_rating: None,
                    genres: genre.clone().map_or(vec![], |g| vec![g]),
                    date_created: date_added(Path::new(&album_dir)).await,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push(album_item);
                album_map.insert(album_dir, id);
                if counted {
                    ticket.add_total(1);
                }
                id
            };

            let file_name = file_path_buf
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            // Use tag title if available, otherwise file stem
            let item_name = title_from_tag.clone().unwrap_or(file_name.clone());

            // A new file in the folder of a missing track, with that track's number and title, takes the
            // track over: the track keeps its id and user data, and it is probed again instead of a new
            // item being made.
            if let Some(index) =
                scan_sync::heir_track(&gone_tracks, &file_path_buf, track_number, &item_name)
            {
                let track = gone_tracks.swap_remove(index);
                scan_sync::take_over_track(pool, &track, &file_path, &item_name).await?;
                tracing::info!(
                    "Audio {} takes the new file {} of {}",
                    track.id,
                    file_path,
                    track.path
                );
                reread += 1;
                probe::spawn_probe_and_save(pool.clone(), track.id, file_path.clone());
                continue;
            }

            let item = MediaItem {
                id: Uuid::new_v4(),
                name: item_name,
                sort_name: Some(file_name.to_lowercase()),
                overview: None,
                media_type: MediaType::Audio,
                path: file_path_buf.to_string_lossy().to_string(),
                parent_id: Some(album_id),
                library_id: library.id,
                production_year: year,
                community_rating: None,
                official_rating: None,
                genres: genre.clone().map_or(vec![], |g| vec![g]),
                date_created: date_added(&file_path_buf).await,
                premiere_date: None,
                runtime_ticks: None,
                index_number: track_number,
                parent_index_number: None,
                media_sources: Vec::new(),
                ..MediaItem::default_dto_fields()
            };

            new_items.push(item);
            if counted {
                ticket.add_total(1);
            }
        }

        tracing::info!(
            "music library '{}': scanned {} files, skipped {} (non-audio extension), created {} new items",
            library.name,
            scanned_files,
            skipped_ext,
            new_items.len()
        );

        // Batch-insert all new items (artists, albums, tracks) in a single transaction
        if !new_items.is_empty() {
            if let Err(e) =
                rustyfin_db::repo_media::batch_upsert_media_items(pool, &new_items).await
            {
                tracing::error!("failed to batch insert music items: {}", e);
                // The batch was not stored, so a later root must not find its artists or albums.
                // Its units will never be refreshed, so they count as done now.
                let unstored: HashSet<Uuid> = new_items.iter().map(|item| item.id).collect();
                artist_map.retain(|_, id| !unstored.contains(id));
                album_map.retain(|_, id| !unstored.contains(id));
                if counted {
                    ticket.advance(new_items.len());
                }
            } else {
                // Find and save local images for albums (cover art)
                let mut all_images: Vec<rustyfin_model::image::ItemImage> = Vec::new();
                for item in &new_items {
                    if item.media_type == MediaType::MusicAlbum {
                        // For albums, look for images in the album directory
                        let album_dir = std::path::Path::new(&item.path);
                        let images = local_images::find_local_images(album_dir, item.id);
                        if !images.is_empty() {
                            tracing::debug!(
                                "found {} local images for album '{}'",
                                images.len(),
                                item.name
                            );
                        }
                        all_images.extend(images);
                    }
                }
                if !all_images.is_empty() {
                    if let Err(e) =
                        rustyfin_db::repo_image::batch_upsert_images(pool, &all_images).await
                    {
                        tracing::error!("failed to batch insert local images: {}", e);
                    } else {
                        tracing::info!("saved {} local images for music library", all_images.len());
                    }
                }

                // Spawn async probes for audio items (fire-and-forget).
                // The semaphore in probe module limits concurrency.
                for item in &new_items {
                    if item.media_type == MediaType::Audio {
                        probe::spawn_probe_and_save(pool.clone(), item.id, item.path.clone());
                    }
                }
                count += new_items.len() as u32;
                new_item_ids.extend(new_items.iter().map(|item| item.id));
            }
        }
    }

    // Remove the items that this scan no longer collects. A failure is logged, not returned.
    let removed = remove_uncollected(pool, library, &filter, &collected, &unreadable, &loops).await;
    tracing::info!(
        "scanned music library '{}': {} new items added ({} existing items unchanged)",
        library.name,
        count,
        existing_count
    );
    spawn_plans_under_lock(
        pool.clone(),
        guard,
        library.name.clone(),
        plans,
        ticket.clone(),
    );
    let changes = ScanChanges {
        added: count,
        removed,
        reread,
        images: 0,
    };
    Ok((changes, new_item_ids))
}

fn detect_media_type(ext: &str, content_type: &LibraryContentType) -> Option<MediaType> {
    match content_type {
        LibraryContentType::Movies if VIDEO_EXTENSIONS.contains(&ext) => Some(MediaType::Movie),
        LibraryContentType::TvShows if VIDEO_EXTENSIONS.contains(&ext) => Some(MediaType::Episode),
        LibraryContentType::Music if AUDIO_EXTENSIONS.contains(&ext) => Some(MediaType::Audio),
        // A home videos library holds photos and home videos, as Jellyfin's does.
        LibraryContentType::Photos if PHOTO_EXTENSIONS.contains(&ext) => Some(MediaType::Photo),
        LibraryContentType::Photos if VIDEO_EXTENSIONS.contains(&ext) => Some(MediaType::Video),
        LibraryContentType::Mixed => {
            if VIDEO_EXTENSIONS.contains(&ext) {
                Some(MediaType::Movie)
            } else if AUDIO_EXTENSIONS.contains(&ext) {
                Some(MediaType::Audio)
            } else if PHOTO_EXTENSIONS.contains(&ext) {
                Some(MediaType::Photo)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(crate) fn parse_year(name: &str) -> Option<i32> {
    // Try to find a 4-digit year in parentheses like "Movie Name (2023)"
    if let Some(start) = name.rfind('(')
        && let Some(end) = name[start..].find(')')
    {
        let candidate = &name[start + 1..start + end];
        if let Ok(year) = candidate.parse::<i32>()
            && (1900..=2100).contains(&year)
        {
            return Some(year);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Async directory traversal
// ---------------------------------------------------------------------------

/// Identity of a directory, for symlink loop detection: its (device, inode) on unix.
#[cfg(unix)]
pub(crate) type DirId = (u64, u64);

/// Identity of a directory, for symlink loop detection: its canonical path off unix.
#[cfg(not(unix))]
pub(crate) type DirId = PathBuf;

/// The identity of the directory at `path`. `fs::metadata` follows symlinks, so a link and its
/// target have the same identity.
#[cfg(unix)]
pub(crate) async fn dir_id(path: &Path) -> std::io::Result<DirId> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).await?;
    Ok((meta.dev(), meta.ino()))
}

/// The identity of the directory at `path`: its canonical path, since there are no inodes off unix.
#[cfg(not(unix))]
pub(crate) async fn dir_id(path: &Path) -> std::io::Result<DirId> {
    fs::canonicalize(path).await
}

/// The metadata `date_added` reads for `path`. A file link gives its final target's, every hop
/// followed, as Jellyfin resolves one; anything else, a folder link, and a link whose target can't be
/// read give the entry's own, from `lstat`. None if even that can't be read.
async fn date_metadata(path: &Path) -> Option<std::fs::Metadata> {
    let own = fs::symlink_metadata(path).await.ok()?;
    if own.file_type().is_symlink()
        && let Ok(target) = fs::metadata(path).await
        && target.is_file()
    {
        return Some(target);
    }
    Some(own)
}

/// When the item at `path` was added, as Jellyfin dates it. On Linux .NET has no birth time, so its
/// creation time is the older of the modification and status-change times, from `date_metadata`, so
/// a file link is dated by its final target and a folder link by its own times. None if they can't
/// be read, or the time is out of range.
#[cfg(target_os = "linux")]
pub(crate) async fn path_date(path: &Path) -> Option<DateTime<Utc>> {
    use std::os::unix::fs::MetadataExt;
    let meta = date_metadata(path).await?;
    let modified = (meta.mtime(), meta.mtime_nsec());
    let changed = (meta.ctime(), meta.ctime_nsec());
    let (secs, nsecs) = modified.min(changed);
    DateTime::<Utc>::from_timestamp(secs, nsecs as u32)
}

/// When the item at `path` was added, as Jellyfin dates it: its creation time, else its
/// modification time, from `date_metadata`, so a file link is dated by its final target and a
/// folder link by its own times. None if they can't be read.
#[cfg(not(target_os = "linux"))]
pub(crate) async fn path_date(path: &Path) -> Option<DateTime<Utc>> {
    let meta = date_metadata(path).await?;
    meta.created()
        .or_else(|_| meta.modified())
        .map(DateTime::<Utc>::from)
        .ok()
}

/// When the item at `path` was added, as `path_date` gives it. Now, if that is None.
async fn date_added(path: &Path) -> DateTime<Utc> {
    path_date(path).await.unwrap_or_else(Utc::now)
}

/// What a scan makes of one entry of a folder.
enum EntryKind {
    /// A folder, which a walk descends into unless its name is ignored.
    Dir,
    /// A file, which a scan collects unless its name is ignored.
    File,
    /// A dangling symlink, whose target is missing. It is unreadable, not gone, so its item is kept.
    Dangling,
}

/// Classifies `entry`, an entry of a folder that a scan lists. A symlink takes the kind of its
/// target, since `fs::metadata` follows it.
async fn entry_kind(entry: &fs::DirEntry) -> EntryKind {
    let path = entry.path();
    match entry.file_type().await {
        Ok(ft) if ft.is_dir() => EntryKind::Dir,
        Ok(ft) if ft.is_symlink() => match fs::metadata(&path).await {
            Ok(meta) if meta.is_dir() => EntryKind::Dir,
            Ok(_) => EntryKind::File,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => EntryKind::Dangling,
            Err(_) => EntryKind::File,
        },
        Ok(_) => EntryKind::File,
        Err(_) if path.is_dir() => EntryKind::Dir,
        Err(_) => EntryKind::File,
    }
}

/// Walks the folder `start` and the folders under it, as `collect_files` walks a root. `chain` holds
/// the identities of `start` and its ancestors, and each subfolder is checked against it.
async fn walk_from(
    start: PathBuf,
    chain: Vec<DirId>,
) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut result = Vec::new();
    let mut unreadable = Vec::new();
    let mut loops = Vec::new();

    // Each queued directory carries the identities of itself and its ancestors, the start included.
    let mut work_queue = vec![(start, chain)];

    while let Some((current, chain)) = work_queue.pop() {
        let mut entries = match fs::read_dir(&current).await {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!("cannot read directory {}: {}", current.display(), err);
                unreadable.push(current);
                continue;
            }
        };

        let mut children = Vec::new();
        let mut dangling = 0;
        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    let path = entry.path();
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    match entry_kind(&entry).await {
                        EntryKind::Dir => {
                            if !naming::is_ignored_dir(&name) {
                                children.push(path);
                            }
                        }
                        EntryKind::File => {
                            if !naming::is_ignored_file(&name) {
                                result.push(path);
                            }
                        }
                        EntryKind::Dangling => {
                            dangling += 1;
                            unreadable.push(path);
                        }
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!("cannot read directory {}: {}", current.display(), err);
                    unreadable.push(current.clone());
                    break;
                }
            }
        }

        if dangling > 0 {
            tracing::warn!(
                "{} dangling symlinks in {}: their targets are missing, so their items are kept",
                dangling,
                current.display()
            );
        }

        // Queue the subdirectories, skipping any that loop back to an ancestor.
        for child in children {
            match dir_id(&child).await {
                Ok(id) if chain.contains(&id) => {
                    tracing::warn!("skipping symlink loop at {}", child.display());
                    loops.push(child);
                }
                Ok(id) => {
                    let mut child_chain = chain.clone();
                    child_chain.push(id);
                    work_queue.push((child, child_chain));
                }
                Err(err) => {
                    tracing::warn!("cannot read directory {}: {}", child.display(), err);
                    unreadable.push(child);
                }
            }
        }
    }

    result.sort();
    (result, unreadable, loops)
}

/// Recursively collects the file paths under `root`, using async I/O and an iterative work queue.
///
/// Returns three lists. The first is the files, sorted by path. The second is the paths that could
/// not be read: the directories that could not be listed completely, from a failed `read_dir` or an
/// error part way through one, and the dangling symlinks, whose targets are missing. A dangling
/// symlink is unreadable, not gone, so its item is kept. The third is the subdirectories skipped as
/// symlink loops: a subdirectory whose identity is already among its ancestors. Entries that `naming`
/// ignores are skipped. Two links to one folder are both followed. The root's own name is never
/// checked.
async fn collect_files(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    match dir_id(root).await {
        Ok(root_id) => walk_from(root.to_path_buf(), vec![root_id]).await,
        Err(err) => {
            tracing::warn!("cannot read directory {}: {}", root.display(), err);
            (Vec::new(), vec![root.to_path_buf()], Vec::new())
        }
    }
}

/// Collects the files that a scan of `scope` reads under `root`, a library path, as `collect_files`
/// does for a full scan.
async fn collect_in_scope(
    root: &Path,
    scope: &ScanScope,
) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    match scope {
        ScanScope::Library => collect_files(root).await,
        ScanScope::Folder(folder) => collect_folder(root, folder).await,
        ScanScope::RootFiles(_) => list_root_files(root).await,
    }
}

/// Collects the files under `folder`, a folder of the library path `root`. The walk starts with the
/// identities of `root` and `folder` in its chain, so a link back to `root` is a loop.
async fn collect_folder(root: &Path, folder: &Path) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    // The library path itself is walked as a root is.
    if folder == root {
        return collect_files(root).await;
    }
    let name = folder.file_name().map(|name| name.to_string_lossy());
    if name.is_some_and(|name| naming::is_ignored_dir(&name)) {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let root_id = match dir_id(root).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("cannot read directory {}: {}", root.display(), err);
            return (Vec::new(), vec![root.to_path_buf()], Vec::new());
        }
    };
    let folder_id = match dir_id(folder).await {
        Ok(id) => id,
        // A folder that is itself a dangling link is unreadable, so its items are kept. A folder that
        // is gone collects nothing, and its items are removed.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if fs::symlink_metadata(folder).await.is_err() {
                return (Vec::new(), Vec::new(), Vec::new());
            }
            tracing::warn!(
                "dangling symlink {}: its target is missing, so its items are kept",
                folder.display()
            );
            return (Vec::new(), vec![folder.to_path_buf()], Vec::new());
        }
        Err(err) => {
            tracing::warn!("cannot read directory {}: {}", folder.display(), err);
            return (Vec::new(), vec![folder.to_path_buf()], Vec::new());
        }
    };
    if folder_id == root_id {
        tracing::warn!("skipping symlink loop at {}", folder.display());
        return (Vec::new(), Vec::new(), vec![folder.to_path_buf()]);
    }
    walk_from(folder.to_path_buf(), vec![root_id, folder_id]).await
}

/// Lists the files directly in `root`, a library path, as `collect_files` keeps them: the same ignore
/// rules, and no folders. A dangling link is unreadable, not gone, so it is listed as unreadable and
/// its item is kept. A `read_dir` error, even part way, makes `root` unreadable.
async fn list_root_files(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut result = Vec::new();
    let mut unreadable = Vec::new();
    let mut dangling = 0;
    match fs::read_dir(root).await {
        Ok(mut entries) => loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    match entry_kind(&entry).await {
                        EntryKind::File if !naming::is_ignored_file(&name) => {
                            result.push(entry.path());
                        }
                        EntryKind::Dangling => {
                            dangling += 1;
                            unreadable.push(entry.path());
                        }
                        _ => {}
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!("cannot read directory {}: {}", root.display(), err);
                    unreadable.push(root.to_path_buf());
                    break;
                }
            }
        },
        Err(err) => {
            tracing::warn!("cannot read directory {}: {}", root.display(), err);
            unreadable.push(root.to_path_buf());
        }
    }
    if dangling > 0 {
        tracing::warn!(
            "{} dangling symlinks in {}: their targets are missing, so their items are kept",
            dangling,
            root.display()
        );
    }
    result.sort();
    (result, unreadable, Vec::new())
}

/// The location type of the season stored at `path`: Virtual where that folder doesn't exist, as for a season
/// made for the episode files in its series' folder, and FileSystem where it does. Jellyfin 12.4 gives such a
/// season no Path (`Resolvers/TV/SeasonResolver.cs:63`), which makes it Virtual (`Entities/BaseItem.cs:348-365`).
/// None when the stat fails for another reason, so the caller keeps the stored location.
async fn season_location(path: &str) -> Option<LocationType> {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_dir() => Some(LocationType::FileSystem),
        Ok(_) => Some(LocationType::Virtual),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Some(LocationType::Virtual)
        }
        Err(_) => None,
    }
}

/// The number of the season folder called `name` in the series folder called `series_folder`: the number
/// after `Season `, `S` or `Staffel `, or 0 for a specials folder (`naming::is_specials_folder`).
fn parse_season_number(name: &str, series_folder: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    // "Season 02", "Season 2", "S02", "Staffel 2"
    for prefix in &["season ", "s", "staffel "] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Ok(n) = rest.trim().parse::<i32>()
        {
            return Some(n);
        }
    }
    naming::is_specials_folder(name, series_folder).then_some(0)
}

/// The sort name that the scan gives a season numbered `number`.
pub(crate) fn season_sort_name(number: Option<i32>) -> String {
    format!("season {:04}", number.unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn b32_f6_a_failed_stat_keeps_a_seasons_stored_location() {
        let dir = std::env::temp_dir().join(format!("b32-f6-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Season 1")).unwrap();
        std::fs::write(dir.join("file"), b"").unwrap();
        std::os::unix::fs::symlink("loop", dir.join("loop")).unwrap();
        let at = |rel: &str| dir.join(rel).to_string_lossy().into_owned();

        assert_eq!(
            season_location(&at("Season 1")).await,
            Some(LocationType::FileSystem)
        );
        // No folder: nothing there, a file, or a path under a file.
        for rel in ["Season 2", "file", "file/Season 1"] {
            assert_eq!(
                season_location(&at(rel)).await,
                Some(LocationType::Virtual),
                "{rel}"
            );
        }
        // Any other error, here a symlink loop, says nothing about the folder.
        assert_eq!(season_location(&at("loop")).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A temp dir, removed when the test ends, whether it passed or not.
    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A database and a Photos library on `<dir>/photos` with one Photo item per file name, which
    /// is written as a small JPEG unless the caller makes it. Returns the pool, the library and the
    /// folder.
    async fn r2photos_world(dir: &Path, names: &[&str]) -> (SqlitePool, Library, PathBuf) {
        let folder = dir.join("photos");
        std::fs::create_dir_all(&folder).unwrap();
        let pool = rustyfin_db::init_db(dir.join("test.db").to_str().unwrap())
            .await
            .unwrap();
        let lib = Library {
            id: Uuid::new_v4(),
            name: "R2 photos".to_string(),
            content_type: LibraryContentType::Photos,
            paths: vec![folder.to_string_lossy().into_owned()],
        };
        rustyfin_db::repo_library::create_library(&pool, &lib)
            .await
            .unwrap();
        for name in names {
            sqlx::query(
                "INSERT INTO media_items (id, library_id, name, media_type, path) \
                 VALUES (?, ?, ?, 'Photo', ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(lib.id.to_string())
            .bind(*name)
            .bind(folder.join(name).to_string_lossy().into_owned())
            .execute(&pool)
            .await
            .unwrap();
        }
        (pool, lib, folder)
    }

    fn r2photos_jpeg(path: &Path) {
        image::RgbImage::from_pixel(8, 6, image::Rgb([1, 2, 3]))
            .save(path)
            .unwrap();
    }

    /// Each stored photo's file name and width.
    async fn r2photos_rows(pool: &SqlitePool) -> Vec<(String, Option<i32>)> {
        let mut rows: Vec<(String, Option<i32>)> = sqlx::query_as(
            "SELECT m.name, p.width FROM photo_metadata p JOIN media_items m ON m.id = p.item_id",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        rows.sort();
        rows
    }

    /// The file name of `path`, for the readers' logs.
    fn r2photos_name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// Runs a test's body on its own runtime with a deadline of 60 s, and doesn't wait for blocking
    /// tasks still running at the end, so a test whose read blocks fails instead of hanging.
    fn r2photos_block_on(body: impl std::future::Future<Output = ()>) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let deadline = std::time::Duration::from_secs(60);
        let done = rt.block_on(async { tokio::time::timeout(deadline, body).await });
        rt.shutdown_timeout(std::time::Duration::from_millis(100));
        done.expect("the test timed out: a read blocked");
    }

    /// Waits, up to 30 s, until `cond` holds.
    async fn r2photos_until(what: &str, cond: impl Fn() -> bool) {
        for _ in 0..3000 {
            if cond() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out waiting until {what}");
    }

    /// Blocks until `release` is set.
    fn r2photos_wait_for(release: &std::sync::atomic::AtomicBool) {
        while !release.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The files `r2photos_reader` was called for, in order.
    static R2PHOTOS_READ: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    /// Lets `r2photos_reader`'s reads of `hang.jpg` end.
    static R2PHOTOS_RELEASE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// `photo_info::read_photo`, except that `panic.jpg` panics and `hang.jpg` blocks until
    /// `R2PHOTOS_RELEASE` is set. Each call is logged in `R2PHOTOS_READ` first.
    fn r2photos_reader(path: &Path) -> (Option<i64>, rustyfin_model::photo::PhotoInfo) {
        R2PHOTOS_READ.lock().unwrap().push(r2photos_name(path));
        match path.file_name().and_then(|n| n.to_str()) {
            Some("panic.jpg") => panic!("r2photos: a reader that panics"),
            Some("hang.jpg") => r2photos_wait_for(&R2PHOTOS_RELEASE),
            _ => {}
        }
        crate::photo_info::read_photo(path)
    }

    #[cfg(unix)]
    #[test]
    fn r2photos_a_fifo_named_like_a_photo_does_not_block_the_pass() {
        r2photos_block_on(async {
            let temp =
                TempDir(std::env::temp_dir().join(format!("r2photos-fifo-{}", Uuid::new_v4())));
            let (pool, lib, folder) = r2photos_world(&temp.0, &["fifo.jpg", "good.jpg"]).await;
            r2photos_jpeg(&folder.join("good.jpg"));
            let fifo = std::ffi::CString::new(folder.join("fifo.jpg").to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

            let hung = std::sync::Arc::new(HungReads::default());
            let mut reads = FileReads::with_hung(std::time::Duration::from_secs(5), hung);
            let read = crate::photo_info::read_photo;
            let count = read_photos(&pool, &lib, &ScanScope::Library, read, &mut reads).await;
            // The FIFO was never opened: no read was given up on.
            assert_eq!(reads.abandoned, 0);
            assert_eq!(count, 1);
            assert_eq!(r2photos_rows(&pool).await, [("good.jpg".into(), Some(8))]);
        });
    }

    #[test]
    fn r2photos_a_photo_that_fails_panics_or_hangs_is_skipped_and_the_rest_are_saved() {
        r2photos_block_on(r2photos_bad_photos());
    }

    async fn r2photos_bad_photos() {
        let temp = TempDir(std::env::temp_dir().join(format!("r2photos-bad-{}", Uuid::new_v4())));
        let names = ["bad.jpg", "good.jpg", "hang.jpg", "panic.jpg"];
        let (pool, lib, folder) = r2photos_world(&temp.0, &names).await;
        for name in ["good.jpg", "panic.jpg", "hang.jpg"] {
            r2photos_jpeg(&folder.join(name));
        }
        std::fs::write(
            folder.join("bad.jpg"),
            b"\xFF\xD8\xFF\xE1\xFF\xFFnot a jpeg",
        )
        .unwrap();
        let hang = folder.join("hang.jpg").to_string_lossy().into_owned();

        // The hanging read blocks until released, so it times out whatever the machine's load.
        let wait = std::time::Duration::from_secs(5);
        let hung = std::sync::Arc::new(HungReads::default());
        let scope = ScanScope::Library;
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_reader, &mut reads).await;
        // The parser's failure gives an empty row, the panic one too (so it isn't read again until
        // the file changes), and the hanging photo is left for a later scan.
        assert_eq!(count, 3);
        assert!(hung.contains(&hang));
        let mut first = std::mem::take(&mut *R2PHOTOS_READ.lock().unwrap());
        first.sort();
        assert_eq!(first, ["bad.jpg", "good.jpg", "hang.jpg", "panic.jpg"]);
        assert_eq!(
            r2photos_rows(&pool).await,
            [
                ("bad.jpg".into(), None),
                ("good.jpg".into(), Some(8)),
                ("panic.jpg".into(), None),
            ]
        );
        let primaries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM item_images WHERE image_type = 'Primary'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(primaries, 1);

        // A second scan skips the photo whose read still runs, and stores nothing. The unchanged
        // photos with no Primary get only their header read for one: panic.jpg is a whole JPEG, so
        // it gets its Primary now; bad.jpg has no size, so none.
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_reader, &mut reads).await;
        assert_eq!(count, 0);
        assert!(R2PHOTOS_READ.lock().unwrap().is_empty());
        assert_eq!(reads.abandoned, 0);
        let primaries: Vec<String> = sqlx::query_scalar(
            "SELECT m.name FROM item_images i JOIN media_items m ON m.id = i.item_id \
             WHERE i.image_type = 'Primary' ORDER BY m.name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(primaries, ["good.jpg", "panic.jpg"]);

        // Once the hanging read ends, its path is free, and the next scan reads that photo alone.
        R2PHOTOS_RELEASE.store(true, std::sync::atomic::Ordering::SeqCst);
        r2photos_until("the hanging read ended", || hung.len() == 0).await;
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_reader, &mut reads).await;
        assert_eq!(count, 1);
        assert_eq!(*R2PHOTOS_READ.lock().unwrap(), ["hang.jpg"]);
    }

    /// The files `r2photos_hanging_reader` was called for.
    static R2PHOTOS_HUNG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    /// Lets `r2photos_hanging_reader`'s reads end.
    static R2PHOTOS_RELEASE_ALL: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// A reader that logs its call in `R2PHOTOS_HUNG`, then blocks until `R2PHOTOS_RELEASE_ALL` is
    /// set.
    fn r2photos_hanging_reader(path: &Path) -> (Option<i64>, rustyfin_model::photo::PhotoInfo) {
        R2PHOTOS_HUNG.lock().unwrap().push(r2photos_name(path));
        r2photos_wait_for(&R2PHOTOS_RELEASE_ALL);
        crate::photo_info::read_photo(path)
    }

    #[test]
    fn r2photos_abandoned_reads_are_capped_within_and_across_scans() {
        r2photos_block_on(r2photos_cap());
    }

    async fn r2photos_cap() {
        let temp = TempDir(std::env::temp_dir().join(format!("r2photos-cap-{}", Uuid::new_v4())));
        let names = ["1.jpg", "2.jpg", "3.jpg", "4.jpg", "5.jpg"];
        let (pool, lib, folder) = r2photos_world(&temp.0, &names).await;
        for name in names {
            r2photos_jpeg(&folder.join(name));
        }
        // Every read blocks until released, so each one times out whatever the machine's load.
        let wait = std::time::Duration::from_millis(200);
        let hung = std::sync::Arc::new(HungReads::default());
        let scope = ScanScope::Library;
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_hanging_reader, &mut reads).await;
        assert_eq!(count, 0);
        // Three reads were abandoned and still run; the other two were never started.
        assert_eq!(reads.abandoned, 3);
        assert_eq!(hung.len(), 3);
        assert!(reads.given_up(&lib.name));
        r2photos_until("the three reads started", || {
            R2PHOTOS_HUNG.lock().unwrap().len() >= 3
        })
        .await;

        // The image restamp in the same scan reads nothing either.
        let image = rustyfin_model::image::ItemImage {
            item_id: r2photos_item(&pool, "1.jpg").await,
            image_type: ImageType::Backdrop,
            image_index: 0,
            path: folder.join("5.jpg").to_string_lossy().into_owned(),
            provider: "local".to_string(),
            width: None,
            height: None,
            blurhash: None,
            stamp: None,
        };
        rustyfin_db::repo_image::upsert_image(&pool, &image)
            .await
            .unwrap();
        restamp_local_images(&pool, &lib, &scope, r2photos_image_reader, &mut reads).await;
        assert_eq!(r2photos_stamps(&pool).await, [("5.jpg".into(), None)]);

        // A later scan, while those three reads still run, reads nothing at all.
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_hanging_reader, &mut reads).await;
        assert_eq!(count, 0);
        restamp_local_images(&pool, &lib, &scope, r2photos_image_reader, &mut reads).await;
        assert_eq!(reads.abandoned, 0);
        assert_eq!(R2PHOTOS_HUNG.lock().unwrap().len(), 3);
        assert_eq!(r2photos_stamps(&pool).await, [("5.jpg".into(), None)]);

        // When they end, the next scan reads every photo, and the restamp runs.
        R2PHOTOS_RELEASE_ALL.store(true, std::sync::atomic::Ordering::SeqCst);
        r2photos_until("the hung reads ended", || hung.len() == 0).await;
        let mut reads = FileReads::with_hung(wait, hung.clone());
        let count = read_photos(&pool, &lib, &scope, r2photos_hanging_reader, &mut reads).await;
        assert_eq!(count, 5);
        assert_eq!(R2PHOTOS_HUNG.lock().unwrap().len(), 8);
    }

    /// The id of the item named `name`.
    async fn r2photos_item(pool: &SqlitePool, name: &str) -> Uuid {
        let id: String = sqlx::query_scalar("SELECT id FROM media_items WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap();
        Uuid::parse_str(&id).unwrap()
    }

    /// Each stored image's file name and stamp, by file name.
    async fn r2photos_stamps(pool: &SqlitePool) -> Vec<(String, Option<i64>)> {
        let rows: Vec<(String, Option<i64>)> =
            sqlx::query_as("SELECT path, stamp FROM item_images ORDER BY path")
                .fetch_all(pool)
                .await
                .unwrap();
        rows.into_iter()
            .map(|(path, stamp)| (r2photos_name(Path::new(&path)), stamp))
            .collect()
    }

    /// The files `r2photos_image_reader` was called for.
    static R2PHOTOS_IMAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    /// `local_images::local_image_version`, except that `panic.jpg` panics. Each call is logged in
    /// `R2PHOTOS_IMAGES` first.
    fn r2photos_image_reader(path: &Path) -> (Option<i64>, Option<i32>, Option<i32>) {
        R2PHOTOS_IMAGES.lock().unwrap().push(r2photos_name(path));
        if path.ends_with("panic.jpg") {
            panic!("r2photos: an image reader that panics");
        }
        local_images::local_image_version(path)
    }

    #[cfg(unix)]
    #[test]
    fn r2photos_the_image_restamp_skips_fifos_and_panics_and_reads_a_photo_header_only() {
        r2photos_block_on(r2photos_restamp());
    }

    #[cfg(unix)]
    async fn r2photos_restamp() {
        let temp = TempDir(std::env::temp_dir().join(format!("r2photos-stamp-{}", Uuid::new_v4())));
        let (pool, lib, folder) = r2photos_world(&temp.0, &["photo.jpg"]).await;
        let item_id = r2photos_item(&pool, "photo.jpg").await;
        r2photos_jpeg(&folder.join("photo.jpg"));
        r2photos_jpeg(&folder.join("good.jpg"));
        r2photos_jpeg(&folder.join("panic.jpg"));
        std::fs::write(folder.join("garbage.jpg"), b"\xFF\xD8 not a picture").unwrap();
        let fifo = std::ffi::CString::new(folder.join("fifo.jpg").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let images = [
            (ImageType::Primary, 0, "photo.jpg"),
            (ImageType::Backdrop, 0, "fifo.jpg"),
            (ImageType::Backdrop, 1, "panic.jpg"),
            (ImageType::Backdrop, 2, "garbage.jpg"),
            (ImageType::Thumb, 0, "good.jpg"),
        ];
        for (image_type, image_index, name) in images {
            // good.jpg and garbage.jpg were stamped before and changed since, so they get a new
            // blurhash; the rest are stamped for the first time.
            let changed = matches!(name, "good.jpg" | "garbage.jpg");
            let image = rustyfin_model::image::ItemImage {
                item_id,
                image_type,
                image_index,
                path: folder.join(name).to_string_lossy().into_owned(),
                provider: "local".to_string(),
                width: None,
                height: None,
                blurhash: changed.then(|| "old".to_string()),
                stamp: changed.then_some(1),
            };
            rustyfin_db::repo_image::upsert_image(&pool, &image)
                .await
                .unwrap();
        }

        let hung = std::sync::Arc::new(HungReads::default());
        let mut reads = FileReads::with_hung(std::time::Duration::from_secs(5), hung);
        let scope = ScanScope::Library;
        let counted =
            restamp_local_images(&pool, &lib, &scope, r2photos_image_reader, &mut reads).await;
        // The FIFO was never opened: no read was given up on.
        assert_eq!(reads.abandoned, 0);
        assert_eq!(counted, 2);
        let hashes: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT path, blurhash FROM item_images WHERE path GLOB '*good.jpg' \
             OR path GLOB '*garbage.jpg' ORDER BY path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        // The changed picture gets its new blurhash; garbage that can't be decoded gets none.
        assert_eq!(hashes[0].1, None, "{hashes:?}");
        assert!(
            hashes[1].1.as_deref().is_some_and(|h| h != "old"),
            "{hashes:?}"
        );

        let stamped: Vec<(String, bool)> = r2photos_stamps(&pool)
            .await
            .into_iter()
            .map(|(name, stamp)| (name, stamp.is_some()))
            .collect();
        assert_eq!(
            stamped,
            [
                ("fifo.jpg".into(), false),
                ("garbage.jpg".into(), true),
                ("good.jpg".into(), true),
                ("panic.jpg".into(), false),
                ("photo.jpg".into(), true),
            ]
        );
        // The FIFO is never opened, and the photo's own file is read by its header alone.
        let mut called = R2PHOTOS_IMAGES.lock().unwrap().clone();
        called.sort();
        assert_eq!(called, ["garbage.jpg", "good.jpg", "panic.jpg"]);
    }
}
