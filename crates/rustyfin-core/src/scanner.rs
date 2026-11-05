use chrono::Utc;
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
use crate::scan_sync;
use crate::tmdb_client::TmdbApi;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg",
];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "wav", "aac", "wma", "m4a", "opus"];
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

pub async fn scan_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    scan_library_with_metadata(pool, library, &reqwest::Client::new(), None, None, None).await
}

pub async fn scan_library_with_metadata(
    pool: &SqlitePool,
    library: &Library,
    http_client: &reqwest::Client,
    tmdb: Option<&TmdbApi>,
    omdb_api_key: Option<&str>,
    fanart_api_key: Option<&str>,
) -> Result<u32, sqlx::Error> {
    tracing::info!(
        "scanning library '{}' (content_type={:?}, paths={:?})",
        library.name,
        library.content_type,
        library.paths
    );
    let (count, new_item_ids) = match library.content_type {
        LibraryContentType::TvShows => scan_tv_library(pool, library).await?,
        LibraryContentType::Music => scan_music_library(pool, library).await?,
        _ => scan_flat_library(pool, library).await?,
    };

    // Refresh metadata only for newly added items, not all items in the library.
    // This avoids unnecessary API calls on incremental scans.
    if !new_item_ids.is_empty() {
        tracing::info!(
            "refreshing metadata for {} newly added items in library '{}'",
            new_item_ids.len(),
            library.name
        );
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
            }
        }
        for handle in handles {
            let _ = handle.await;
        }
    }

    // Rebuild materialized filter indexes after scan completes
    // This keeps the filter_genre_index, filter_year_index, filter_rating_index up-to-date
    if let Err(e) = rustyfin_db::repo_media::rebuild_filter_indexes(pool).await {
        tracing::warn!("failed to rebuild filter indexes: {}", e);
    }

    Ok(count)
}

/// Serializes the scans that write items and queue probes. A scan takes it first and holds it
/// until the probes it queued have finished (see `spawn_plans_under_lock`).
static SCAN_LOCK: Mutex<()> = Mutex::const_new(());

/// Waits until no scan is running and the probes of the last scan have finished.
pub async fn wait_for_idle() {
    drop(SCAN_LOCK.lock().await);
}

/// Runs the probe plans of a scan in the background. One task owns `guard` until every plan has
/// finished, so the next scan waits for them.
fn spawn_plans_under_lock(
    pool: SqlitePool,
    guard: MutexGuard<'static, ()>,
    library_name: String,
    plans: Vec<(Uuid, Vec<probe::ProbeStep>)>,
) {
    tokio::spawn(async move {
        let total_steps: usize = plans.iter().map(|(_, steps)| steps.len()).sum();
        let handles: Vec<_> = plans
            .into_iter()
            .map(|(item_id, steps)| probe::spawn_probe_plan(pool.clone(), item_id, steps))
            .collect();
        for handle in handles {
            let _ = handle.await;
        }
        tracing::info!(
            "probes finished for library '{}': {} files",
            library_name,
            total_steps
        );
        drop(guard);
    });
}

/// Flat scan for Movies, Photos, Mixed libraries
async fn scan_flat_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
    let guard = SCAN_LOCK.lock().await;
    let mut count = 0u32;
    let mut existing_count = 0u32;
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    // Avoids firing one DB query per file during scanning.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id)
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

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // Async directory traversal — non-blocking on Tokio runtime
        let (files, unreadable_dirs, loop_dirs) = collect_files(path).await;
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
                date_created: Utc::now(),
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
    let outcomes = scan_sync::reconcile_movie_groups(pool, library.id, &movie_groups).await?;
    for (group, outcome) in movie_groups.iter().zip(outcomes) {
        match outcome {
            scan_sync::GroupOutcome::Existing { item_id, steps } => {
                if !steps.is_empty() {
                    plans.push((item_id, steps));
                }
            }
            scan_sync::GroupOutcome::New { path, steps } => {
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
                    date_created: Utc::now(),
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
    match scan_sync::remove_not_collected(pool, library, &collected, &unreadable, &loops).await {
        Ok(removed) => tracing::info!(
            "removed {removed} items no longer collected from library '{}'",
            library.name
        ),
        Err(e) => tracing::warn!(
            "removing items no longer collected from library '{}' failed: {}",
            library.name,
            e
        ),
    }
    spawn_plans_under_lock(pool.clone(), guard, library.name.clone(), plans);
    Ok((count, new_item_ids))
}

/// Recursive scan for TV Shows: path/ShowName/Season XX/episode files
async fn scan_tv_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
    let guard = SCAN_LOCK.lock().await;
    let mut count = 0u32;
    let mut plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    let mut existing_count = 0u32;
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id)
            .await?
            .into_iter()
            .collect();

    // Pre-load all existing series and season paths into HashMaps for O(1) ID lookup.
    // Eliminates per-series/season database queries during directory traversal.
    let existing_series: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "Series").await?;
    let existing_seasons: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "Season").await?;
    // The episode files of each season, by season id: the season's name, then its files in walk
    // order. The episodes are reconciled once every path has been walked.
    let mut episode_files: BTreeMap<Uuid, (String, Vec<PathBuf>)> = BTreeMap::new();

    // Every file that the walks returned, every folder they could not list, and every symlink
    // loop they skipped. The removal at the end of the scan needs all three.
    let mut collected: HashSet<String> = HashSet::new();
    let mut unreadable: Vec<PathBuf> = Vec::new();
    let mut loops: Vec<PathBuf> = Vec::new();

    for lib_path in &library.paths {
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
        let (files, unreadable_dirs, loop_dirs) = collect_files(path).await;
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
                    date_created: Utc::now(),
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
                    date_created: Utc::now(),
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
                .or_insert_with(|| (season_name, Vec::new()))
                .1
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
    for (&season_id, (season_name, season_files)) in &episode_files {
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
                index_number: parsed.map(|n| n.episode),
                parent_index_number: parse_season_number(season_name)
                    .or_else(|| parsed.and_then(|n| n.season)),
            });
        }
    }
    let outcomes = scan_sync::reconcile_episode_groups(pool, library.id, &episode_groups).await?;
    let mut new_episodes: Vec<MediaItem> = Vec::new();
    let mut new_episode_plans: Vec<(Uuid, Vec<probe::ProbeStep>)> = Vec::new();
    for (group, outcome) in episode_groups.iter().zip(outcomes) {
        match outcome {
            scan_sync::GroupOutcome::Existing { item_id, steps } => {
                if !steps.is_empty() {
                    plans.push((item_id, steps));
                }
            }
            scan_sync::GroupOutcome::New { path, steps } => {
                let name = group.files[0]
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
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
                    date_created: Utc::now(),
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
    match scan_sync::remove_not_collected(pool, library, &collected, &unreadable, &loops).await {
        Ok(removed) => tracing::info!(
            "removed {removed} items no longer collected from library '{}'",
            library.name
        ),
        Err(e) => tracing::warn!(
            "removing items no longer collected from library '{}' failed: {}",
            library.name,
            e
        ),
    }
    spawn_plans_under_lock(pool.clone(), guard, library.name.clone(), plans);
    Ok((count, new_item_ids))
}

/// Recursive scan for Music: path/Artist/Album/track files
/// Builds Artist → Album → Track hierarchy using directory structure and audio tags.
async fn scan_music_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
    let _guard = SCAN_LOCK.lock().await;
    let mut count = 0u32;
    let mut existing_count = 0u32;
    let mut new_item_ids: Vec<Uuid> = Vec::new();

    // Load all existing paths into a HashSet for O(1) existence checks.
    let existing_paths: HashSet<String> =
        rustyfin_db::repo_media::get_library_paths(pool, library.id)
            .await?
            .into_iter()
            .collect();

    // Pre-load all existing music album paths into HashMaps for O(1) ID lookup.
    // Artists and albums both use MediaType::MusicAlbum in the DB.
    let existing_albums: HashMap<String, Uuid> =
        rustyfin_db::repo_media::get_library_items_by_type(pool, library.id, "MusicAlbum").await?;

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // album_path -> album_id (start with pre-loaded DB data)
        let mut album_map: HashMap<String, Uuid> = existing_albums.clone();
        // artist_key -> artist_id (built during scan; artists share MusicAlbum type)
        let mut artist_map: HashMap<String, Uuid> = HashMap::new();

        // Collect new items for batch insert in a single transaction
        let mut new_items: Vec<MediaItem> = Vec::new();

        // Async directory traversal — non-blocking on Tokio runtime
        let (files, _, _) = collect_files(path).await;
        tracing::info!(
            "music library '{}': found {} files in path {}",
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
            if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                skipped_ext += 1;
                continue;
            }

            let file_path = file_path_buf.to_string_lossy().to_string();

            // Skip if already exists (in-memory check)
            if existing_paths.contains(&file_path) {
                existing_count += 1;
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
                        date_created: Utc::now(),
                        premiere_date: None,
                        runtime_ticks: None,
                        index_number: None,
                        parent_index_number: None,
                        media_sources: Vec::new(),
                        ..MediaItem::default_dto_fields()
                    };
                    new_items.push(artist_item);
                    artist_map.insert(artist_key, id);
                    Some(id)
                }
            } else {
                None
            };

            // Resolve album with parent_id pointing to artist — use pre-loaded HashMap first
            let album_id = if let Some(id) = album_map.get(&album_dir) {
                *id
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
                    date_created: Utc::now(),
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push(album_item);
                album_map.insert(album_dir, id);
                id
            };

            let file_name = file_path_buf
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            // Use tag title if available, otherwise file stem
            let item_name = title_from_tag.clone().unwrap_or(file_name.clone());

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
                date_created: Utc::now(),
                premiere_date: None,
                runtime_ticks: None,
                index_number: track_number,
                parent_index_number: None,
                media_sources: Vec::new(),
                ..MediaItem::default_dto_fields()
            };

            new_items.push(item);
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
                new_item_ids = new_items.iter().map(|item| item.id).collect();
            }
        }
    }

    tracing::info!(
        "scanned music library '{}': {} new items added ({} existing items unchanged)",
        library.name,
        count,
        existing_count
    );
    Ok((count, new_item_ids))
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
type DirId = (u64, u64);

/// Identity of a directory, for symlink loop detection: its canonical path off unix.
#[cfg(not(unix))]
type DirId = PathBuf;

/// The identity of the directory at `path`. `fs::metadata` follows symlinks, so a link and its
/// target have the same identity.
#[cfg(unix)]
async fn dir_id(path: &Path) -> std::io::Result<DirId> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).await?;
    Ok((meta.dev(), meta.ino()))
}

/// The identity of the directory at `path`: its canonical path, since there are no inodes off unix.
#[cfg(not(unix))]
async fn dir_id(path: &Path) -> std::io::Result<DirId> {
    fs::canonicalize(path).await
}

/// Recursively collects the file paths under `root`, using async I/O and an iterative work queue.
///
/// Returns three lists. The first is the files, sorted by path. The second is the directories that
/// could not be listed completely, from a failed `read_dir` or an error part way through one. The
/// third is the subdirectories skipped as symlink loops: a subdirectory whose identity is already
/// among its ancestors. Entries that `naming` ignores are skipped. Two links to one folder are both
/// followed. The root's own name is never checked.
async fn collect_files(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut result = Vec::new();
    let mut unreadable = Vec::new();
    let mut loops = Vec::new();

    let root_id = match dir_id(root).await {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("cannot read directory {}: {}", root.display(), err);
            return (result, vec![root.to_path_buf()], Vec::new());
        }
    };
    // Each queued directory carries the identities of itself and its ancestors, root included.
    let mut work_queue = vec![(root.to_path_buf(), vec![root_id])];

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
        loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    let path = entry.path();
                    let file_name = entry.file_name();
                    let name = file_name.to_string_lossy();
                    let is_dir = if let Ok(ft) = entry.file_type().await {
                        if ft.is_dir() {
                            true
                        } else if ft.is_symlink() {
                            fs::metadata(&path)
                                .await
                                .map(|m| m.is_dir())
                                .unwrap_or(false)
                        } else {
                            false
                        }
                    } else {
                        path.is_dir()
                    };

                    if is_dir {
                        if !naming::is_ignored_dir(&name) {
                            children.push(path);
                        }
                    } else if !naming::is_ignored_file(&name) {
                        result.push(path);
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!("cannot read directory {}: {}", current.display(), err);
                    unreadable.push(current);
                    break;
                }
            }
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
