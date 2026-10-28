use chrono::Utc;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use crate::local_images;
use crate::probe;

// ---------------------------------------------------------------------------
// Multiple version detection
// ---------------------------------------------------------------------------

/// Quality/resolution tags that commonly appear in file names to distinguish
/// different versions of the same title.  Order matters: longer patterns first
/// so "2160p" matches before "1080p".
const VERSION_TAGS: &[&str] = &[
    "4k",
    "2160p",
    "uhd",
    "8k",
    "4320p",
    "1080p",
    "fhd",
    "720p",
    "480p",
    "sd",
    "hdr10plus",
    "hdr10",
    "dolbyvision",
    "hdr",
    "imax",
    "remaster",
    "extended",
    "uncut",
    "director'scut",
    "bluray",
    "blu-ray",
    "brrip",
    "webrip",
    "web-dl",
    "webdl",
    "hdtv",
    "dvdrip",
];

/// Normalize a file name by stripping quality/resolution tags so that different
/// versions of the same title produce the same normalized key.
///
/// Examples:
///   "Inception (2010) 4K HDR.mkv"       -> "inception"
///   "Inception (2010) 1080p BluRay.mkv" -> "inception"
///   "Breaking Bad S01E01 720p.mkv"      -> "breaking bad s01e01"
fn normalize_name(file_stem: &str) -> String {
    let lower = file_stem.to_lowercase();

    // Remove bracketed content first: [4K], (2010), {HDR}, etc.
    // This handles years, quality tags in brackets, etc.
    let mut cleaned = String::new();
    let mut depth = 0i32;
    for ch in lower.chars() {
        match ch {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    cleaned.push(ch);
                }
            }
        }
    }

    // Remove each tag (longest first to avoid partial matches)
    // Only match tags that are surrounded by whitespace or at boundaries
    let mut result = cleaned;
    for tag in VERSION_TAGS {
        // Replace tag when surrounded by spaces (case-insensitive, already lowered)
        let pattern = format!(" {} ", tag);
        result = result.replace(&pattern, " ");
        // Also match at start
        let start_pattern = format!("{} ", tag);
        result = result.replace(&start_pattern, "");
        // Also match at end
        let end_pattern = format!(" {}", tag);
        result = result.replace(&end_pattern, "");
    }

    // Strip standalone 4-digit years (1900-2100) that are not in brackets
    let words: Vec<&str> = result.split_whitespace().collect();
    result = words
        .into_iter()
        .filter(|w| {
            if let Ok(y) = w.parse::<i32>() {
                !(1900..=2100).contains(&y)
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Collapse multiple spaces and trim
    let mut final_result = String::new();
    let mut prev_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                final_result.push(' ');
            }
            prev_space = true;
        } else {
            final_result.push(ch);
            prev_space = false;
        }
    }

    final_result.trim().to_string()
}

/// Look up an existing media item by normalized name within the same parent
/// directory.  Returns the item ID if found, otherwise None.
async fn find_item_by_normalized_name(
    pool: &SqlitePool,
    normalized: &str,
    parent_dir: &str,
    media_type: MediaType,
) -> Result<Option<Uuid>, sqlx::Error> {
    // Query items whose path starts with the parent directory and match by
    // normalizing their file stem at the SQL level is expensive, so we fetch
    // candidates and filter in Rust.
    let media_type_str = media_type.item_kind().to_string();
    let candidates: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, path FROM media_items WHERE path LIKE ? AND media_type = ? AND runtime_ticks IS NOT NULL",
    )
    .bind(format!("{}%", parent_dir))
    .bind(&media_type_str)
    .fetch_all(pool)
    .await?;

    for (id, path) in &candidates {
        if let Some(stem) = Path::new(path).file_stem().and_then(|s| s.to_str())
            && normalize_name(stem) == normalized
        {
            return Ok(Some(Uuid::parse_str(id).unwrap_or_default()));
        }
    }

    Ok(None)
}

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
    tmdb_api_key: Option<&str>,
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
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(5));
        let mut handles = Vec::new();
        for new_id in new_item_ids {
            if let Ok(Some(item)) = rustyfin_db::repo_media::get_item_by_id(pool, new_id).await {
                let sem = semaphore.clone();
                let pool = pool.clone();
                let client = http_client.clone();
                let tmdb_key = tmdb_api_key.map(|s| s.to_string());
                let omdb_key = omdb_api_key.map(|s| s.to_string());
                let fanart_key = fanart_api_key.map(|s| s.to_string());
                let handle = tokio::spawn(async move {
                    let _perm = sem.acquire().await;
                    if let Err(e) = crate::metadata::refresh_item_metadata(
                        &pool,
                        &client,
                        &item,
                        tmdb_key.as_deref(),
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

/// Flat scan for Movies, Photos, Mixed libraries
async fn scan_flat_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
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

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // Async directory traversal — non-blocking on Tokio runtime
        let files = collect_files(path).await;

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

            // For video content, check if this is a version of an existing item
            let normalized = normalize_name(&file_name);
            let parent_dir = file_path_buf
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let is_version = if matches!(media_type, MediaType::Movie) {
                // Look for an existing item with the same normalized name in this directory
                if let Ok(Some(existing_id)) =
                    find_item_by_normalized_name(pool, &normalized, &parent_dir, media_type).await
                {
                    // This is a version of an existing item — add as a new media source
                    tracing::info!(
                        "detected version of item {}: {} -> {}",
                        existing_id,
                        file_path,
                        normalized
                    );
                    let file_path_clone = file_path.clone();
                    let pool_clone = pool.clone();
                    let item_id = existing_id;
                    probe::spawn_probe_and_add_source(pool_clone, item_id, file_path_clone);
                    count += 1;
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if is_version {
                continue;
            }

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

            new_items.push(item);
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

            // Spawn async probes for all newly inserted items (fire-and-forget).
            // The semaphore in probe module limits concurrency, so probes run in the background
            // without blocking the scan from returning.
            for item in &new_items {
                probe::spawn_probe_and_save(pool.clone(), item.id, item.path.clone());
            }
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
    Ok((count, new_item_ids))
}

/// Recursive scan for TV Shows: path/ShowName/Season XX/episode files
async fn scan_tv_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
    let mut count = 0u32;
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
        let files = collect_files(path).await;
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

            // Skip if already exists (in-memory check)
            if existing_paths.contains(&file_path) {
                existing_count += 1;
                continue;
            }

            // Determine hierarchy from path components relative to lib_path
            let rel = file_path_buf.strip_prefix(path).unwrap_or(&file_path_buf);
            let components: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();

            let (series_name, season_name, episode_name) = match components.len() {
                // path/ShowName/Season/episode.mkv
                3.. => {
                    let sn = components[0].to_string();
                    let sea = components[1].to_string();
                    let ep = file_path_buf
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    (sn, sea, ep)
                }
                // path/ShowName/episode.mkv (no season folder)
                2 => {
                    let sn = components[0].to_string();
                    let ep = file_path_buf
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    (sn, "Season 1".to_string(), ep)
                }
                // path/episode.mkv (flat)
                _ => {
                    let ep = file_path_buf
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    ("Unknown Series".to_string(), "Season 1".to_string(), ep)
                }
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

            // Check if this is a version of an existing episode
            let normalized = normalize_name(&episode_name);
            let is_version = if let Ok(Some(existing_id)) =
                find_item_by_normalized_name(pool, &normalized, &series_dir, MediaType::Episode)
                    .await
            {
                // Check that the found item belongs to the same season
                if let Ok(Some(existing_item)) =
                    rustyfin_db::repo_media::get_item_by_id(pool, existing_id).await
                {
                    if existing_item.parent_id == Some(season_id) {
                        tracing::info!(
                            "detected version of episode {}: {} -> {}",
                            existing_id,
                            file_path,
                            normalized
                        );
                        let file_path_clone = file_path.clone();
                        let pool_clone = pool.clone();
                        let item_id = existing_id;
                        probe::spawn_probe_and_add_source(pool_clone, item_id, file_path_clone);
                        count += 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if is_version {
                continue;
            }

            // Create episode item
            let episode_index = parse_episode_number(&episode_name);
            let season_index = parse_season_number(&season_name);
            let episode_item = MediaItem {
                id: Uuid::new_v4(),
                name: episode_name.clone(),
                sort_name: Some(episode_name.to_lowercase()),
                overview: None,
                media_type: MediaType::Episode,
                path: file_path.clone(),
                parent_id: Some(season_id),
                library_id: library.id,
                production_year: None,
                community_rating: None,
                official_rating: None,
                genres: vec![],
                date_created: Utc::now(),
                premiere_date: None,
                runtime_ticks: None,
                index_number: episode_index,
                parent_index_number: season_index,
                media_sources: Vec::new(),
                ..MediaItem::default_dto_fields()
            };

            new_items.push(episode_item);
        }

        tracing::info!(
            "TV library '{}': scanned {} files, skipped {} (non-video extension), created {} new items (series/seasons/episodes)",
            library.name,
            scanned_files,
            skipped_ext,
            new_items.len()
        );

        // Batch-insert all new items (series, seasons, episodes) in a single transaction
        if !new_items.is_empty() {
            if let Err(e) =
                rustyfin_db::repo_media::batch_upsert_media_items(pool, &new_items).await
            {
                tracing::error!("failed to batch insert TV items: {}", e);
            } else {
                // Find and save local images for series and seasons
                let mut all_images: Vec<rustyfin_model::image::ItemImage> = Vec::new();
                for item in &new_items {
                    if item.media_type == MediaType::Series || item.media_type == MediaType::Season
                    {
                        let item_path = std::path::Path::new(&item.path);
                        let images = local_images::find_local_images(item_path, item.id);
                        if !images.is_empty() {
                            tracing::debug!(
                                "found {} local images for '{}'",
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
                        tracing::info!("saved {} local images for TV library", all_images.len());
                    }
                }

                // Spawn async probes for episode items (fire-and-forget).
                // The semaphore in probe module limits concurrency.
                for item in &new_items {
                    if item.media_type == MediaType::Episode {
                        probe::spawn_probe_and_save(pool.clone(), item.id, item.path.clone());
                    }
                }
                count += new_items.len() as u32;
                new_item_ids = new_items.iter().map(|item| item.id).collect();
            }
        }
    }

    tracing::info!(
        "scanned TV library '{}': {} new items added ({} existing items unchanged)",
        library.name,
        count,
        existing_count
    );
    Ok((count, new_item_ids))
}

/// Recursive scan for Music: path/Artist/Album/track files
/// Builds Artist → Album → Track hierarchy using directory structure and audio tags.
async fn scan_music_library(
    pool: &SqlitePool,
    library: &Library,
) -> Result<(u32, Vec<Uuid>), sqlx::Error> {
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
        let files = collect_files(path).await;
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

fn parse_year(name: &str) -> Option<i32> {
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

/// Recursively collect all file paths under `dir` using async I/O.
/// Uses an iterative work-queue approach to avoid recursive async fn issues.
/// Returns a `Vec` of absolute paths, skipping directories that cannot be read.
async fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut work_queue = vec![root.to_path_buf()];

    while let Some(current) = work_queue.pop() {
        let mut entries = match fs::read_dir(&current).await {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!("cannot read directory {}: {}", current.display(), err);
                continue;
            }
        };

        let mut children = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
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
                children.push(path);
            } else {
                result.push(path);
            }
        }

        // Add subdirectories to the work queue
        work_queue.extend(children);
    }

    result
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

fn parse_episode_number(name: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    // Try S01E02 pattern
    if let Some(pos) = lower.find('e') {
        if pos > 0
            && lower
                .as_bytes()
                .get(pos - 1)
                .is_some_and(|b| b.is_ascii_digit())
        {
            // likely SxxExx
        }
        let rest = &lower[pos + 1..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<i32>() {
            return Some(n);
        }
    }
    // Try "Episode XX" or "E XX"
    for prefix in &["episode ", "ep ", "ep"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num_str.parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_name_strips_quality_tags() {
        // Movie with 4K HDR
        assert_eq!(normalize_name("Inception (2010) 4K HDR"), "inception");

        // Movie with 1080p BluRay
        assert_eq!(normalize_name("Inception (2010) 1080p BluRay"), "inception");

        // Both should match
        assert_eq!(
            normalize_name("Inception (2010) 4K HDR"),
            normalize_name("Inception (2010) 1080p BluRay")
        );

        // Episode with resolution tag
        assert_eq!(
            normalize_name("Breaking Bad S01E01 720p"),
            "breaking bad s01e01"
        );

        // Episode with different resolution
        assert_eq!(
            normalize_name("Breaking Bad S01E01 1080p"),
            "breaking bad s01e01"
        );

        // Both should match
        assert_eq!(
            normalize_name("Breaking Bad S01E01 720p"),
            normalize_name("Breaking Bad S01E01 1080p")
        );

        // Movie with brackets
        assert_eq!(
            normalize_name("The Matrix [2023] [4K] [HDR10]"),
            "the matrix"
        );

        // Movie with web-dl tag
        assert_eq!(normalize_name("Movie Name (2023) Web-DL HDR"), "movie name");

        // No tags — name preserved
        assert_eq!(normalize_name("Simple Movie"), "simple movie");

        // IMAX version
        assert_eq!(normalize_name("Oppenheimer 2023 IMAX 4K"), "oppenheimer");
    }
}
