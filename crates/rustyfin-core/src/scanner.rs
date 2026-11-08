use chrono::{DateTime, Utc};
use rustyfin_db::repo_media::PathScope;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use crate::local_images;
use crate::naming;
use crate::probe;
use crate::scan_progress::ScanTicket;
use crate::scan_sync;
use crate::tmdb_client::TmdbApi;

pub(crate) const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg",
];
pub(crate) const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "wav", "aac", "wma", "m4a", "opus"];
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

pub async fn scan_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    scan_library_with_metadata(
        pool,
        library,
        &reqwest::Client::new(),
        None,
        None,
        None,
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

/// How many items a scan added, removed and read again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanChanges {
    /// New items of every type, series, seasons, artists and albums included.
    pub added: u32,
    /// Items that the scan no longer collected, and deleted.
    pub removed: u32,
    /// Existing items that got probe steps again, because their files changed.
    pub reread: u32,
}

impl ScanChanges {
    /// Whether the scan added, removed or read again at least one item.
    pub fn changed(&self) -> bool {
        self.added > 0 || self.removed > 0 || self.reread > 0
    }
}

/// Scans the library, then refreshes the metadata of its new items. Returns how many items were
/// added. `ticket` is the caller's share of the library's scan, which the registry lists until the
/// share and the probes end.
pub async fn scan_library_with_metadata(
    pool: &SqlitePool,
    library: &Library,
    http_client: &reqwest::Client,
    tmdb: Option<&TmdbApi>,
    omdb_api_key: Option<&str>,
    fanart_api_key: Option<&str>,
    ticket: &ScanTicket,
) -> Result<u32, sqlx::Error> {
    scan_with_metadata(
        pool,
        library,
        &ScanScope::Library,
        http_client,
        tmdb,
        omdb_api_key,
        fanart_api_key,
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
    tmdb: Option<&TmdbApi>,
    omdb_api_key: Option<&str>,
    fanart_api_key: Option<&str>,
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
    // The refresh shows as progress only when a provider is configured to make API calls.
    let counted = tmdb.is_some() || omdb_api_key.is_some() || fanart_api_key.is_some();
    let (changes, new_item_ids) = match library.content_type {
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
        let mut handles = Vec::new();
        for new_id in new_item_ids {
            if let Ok(Some(item)) = rustyfin_db::repo_media::get_item_by_id(pool, new_id).await {
                let sem = semaphore.clone();
                let pool = pool.clone();
                let client = http_client.clone();
                let tmdb = tmdb.cloned();
                let omdb_key = omdb_api_key.map(|s| s.to_string());
                let fanart_key = fanart_api_key.map(|s| s.to_string());
                let handle = tokio::spawn(async move {
                    let _perm = sem.acquire().await;
                    if let Err(e) = crate::metadata::refresh_item_metadata(
                        &pool,
                        &client,
                        &item,
                        tmdb.as_ref(),
                        omdb_key.as_deref(),
                        fanart_key.as_deref(),
                    )
                    .await
                    {
                        tracing::warn!("metadata refresh failed for '{}': {}", item.name, e);
                    }
                });
                handles.push(handle);
            } else if counted {
                // An item that can't be loaded has nothing to refresh, but it still counts.
                ticket.advance(1);
            }
        }
        for handle in handles {
            let _ = handle.await;
            if counted {
                ticket.advance(1);
            }
        }
    }

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
    let outcomes =
        scan_sync::reconcile_movie_groups(pool, library.id, &filter, &movie_groups, &gone).await?;
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
    };
    Ok((changes, new_item_ids))
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
    // The episode files of each season, by season id: the id of its series, the season's name, then its
    // files in walk order. The episodes are reconciled once every path has been walked.
    let mut episode_files: BTreeMap<Uuid, (Uuid, String, Vec<PathBuf>)> = BTreeMap::new();

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
            let ext = file_path_buf
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            scanned_files += 1;
            if !VIDEO_EXTENSIONS.contains(&ext.as_str()) {
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
                let series_item = MediaItem {
                    id,
                    name: series_name.clone(),
                    sort_name: Some(series_name.to_lowercase()),
                    overview: None,
                    media_type: MediaType::Series,
                    path: series_dir.clone(),
                    parent_id: None,
                    library_id: library.id,
                    production_year: None,
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
            let season_id = if let Some(id) = season_map.get(&season_path) {
                *id
            } else if let Some(id) =
                lookup_item(pool, library.id, scope, "Season", &season_path).await?
            {
                season_map.insert(season_path.clone(), id);
                id
            } else {
                let id = Uuid::new_v4();
                let season_index = parse_season_number(&season_name);
                let season_item = MediaItem {
                    id,
                    name: season_name.clone(),
                    sort_name: Some(format!("season {:04}", season_index.unwrap_or(1))),
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
                    index_number: season_index,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push(season_item);
                season_map.insert(season_path, id);
                id
            };

            episode_files
                .entry(season_id)
                .or_insert_with(|| (series_id, season_name, Vec::new()))
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

    // Group the files of each season into episodes, then reconcile the episodes with the items.
    // The numbers of an episode come from its primary file, the first of its versions.
    let mut episode_groups: Vec<scan_sync::EpisodeGroup> = Vec::new();
    for (&season_id, (series_id, season_name, season_files)) in &episode_files {
        for versions in naming::group_episode_files(season_files, parse_season_number(season_name))
        {
            let stem = versions[0]
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown");
            let parsed = naming::parse_episode(stem);
            episode_groups.push(scan_sync::EpisodeGroup {
                files: versions,
                season_id,
                series_id: *series_id,
                index_number: parsed.map(|n| n.episode),
                parent_index_number: parse_season_number(season_name)
                    .or_else(|| parsed.and_then(|n| n.season)),
            });
        }
    }
    // The items whose file is gone are read first: a group with no owner may take one over.
    let gone =
        scan_sync::gone_items(pool, library, &filter, &collected, &unreadable, &loops).await?;
    let outcomes =
        scan_sync::reconcile_episode_groups(pool, library.id, &filter, &episode_groups, &gone)
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
    let _guard = SCAN_LOCK.lock().await;
    ticket.start();
    let filter = path_scope(scope);
    let mut count = 0u32;
    let mut existing_count = 0u32;
    let mut reread = 0u32;
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id, &filter)
            .await?
            .into_iter()
            .collect();

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
                // A track whose file changed since it was stamped is probed again, under the same item.
                // A track that was never stamped is stamped now, without a probe.
                if let Some(&(item_id, size, modified)) = known_tracks.get(&file_path) {
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
    let changes = ScanChanges {
        added: count,
        removed,
        reread,
    };
    Ok((changes, new_item_ids))
}

fn detect_media_type(ext: &str, content_type: &LibraryContentType) -> Option<MediaType> {
    match content_type {
        LibraryContentType::Movies if VIDEO_EXTENSIONS.contains(&ext) => Some(MediaType::Movie),
        LibraryContentType::TvShows if VIDEO_EXTENSIONS.contains(&ext) => Some(MediaType::Episode),
        LibraryContentType::Music if AUDIO_EXTENSIONS.contains(&ext) => Some(MediaType::Audio),
        LibraryContentType::Photos if PHOTO_EXTENSIONS.contains(&ext) => Some(MediaType::Photo),
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
/// a file link is dated by its final target and a folder link by its own times. Now, if they
/// can't be read.
#[cfg(target_os = "linux")]
async fn date_added(path: &Path) -> DateTime<Utc> {
    use std::os::unix::fs::MetadataExt;
    let Some(meta) = date_metadata(path).await else {
        return Utc::now();
    };
    let modified = (meta.mtime(), meta.mtime_nsec());
    let changed = (meta.ctime(), meta.ctime_nsec());
    let (secs, nsecs) = modified.min(changed);
    DateTime::<Utc>::from_timestamp(secs, nsecs as u32).unwrap_or_else(Utc::now)
}

/// When the item at `path` was added, as Jellyfin dates it: its creation time, else its
/// modification time, from `date_metadata`, so a file link is dated by its final target and a
/// folder link by its own times. Now, if they can't be read.
#[cfg(not(target_os = "linux"))]
async fn date_added(path: &Path) -> DateTime<Utc> {
    match date_metadata(path).await {
        Some(meta) => meta
            .created()
            .or_else(|_| meta.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now()),
        None => Utc::now(),
    }
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

fn parse_season_number(name: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    // "Season 02", "Season 2", "S02", "Staffel 2"
    for prefix in &["season ", "s", "staffel "] {
        if let Some(rest) = lower.strip_prefix(prefix)
            && let Ok(n) = rest.trim().parse::<i32>()
        {
            return Some(n);
        }
    }
    None
}
