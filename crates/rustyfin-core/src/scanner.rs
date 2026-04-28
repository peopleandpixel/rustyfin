use chrono::Utc;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;
use walkdir::WalkDir;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "mpg", "mpeg",
];
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "wav", "aac", "wma", "m4a", "opus"];
const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

pub async fn scan_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    scan_library_with_metadata(pool, library, None, None).await
}

pub async fn scan_library_with_metadata(
    pool: &SqlitePool,
    library: &Library,
    tmdb_api_key: Option<&str>,
    omdb_api_key: Option<&str>,
) -> Result<u32, sqlx::Error> {
    let count = match library.content_type {
        LibraryContentType::TvShows => scan_tv_library(pool, library).await?,
        LibraryContentType::Music => scan_music_library(pool, library).await?,
        _ => scan_flat_library(pool, library).await?,
    };

    // Automatically fetch metadata for all items in this library
    if let Ok(items) = rustyfin_db::repo_media::get_items_by_library(pool, library.id).await {
        for item in &items {
            if let Err(e) = crate::metadata::refresh_item_metadata(pool, item, tmdb_api_key, omdb_api_key).await {
                tracing::warn!("metadata refresh failed for '{}': {}", item.name, e);
            }
        }
    }

    Ok(count)
}

/// Flat scan for Movies, Photos, Mixed libraries
async fn scan_flat_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    let mut count = 0u32;

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let media_type = detect_media_type(&ext, &library.content_type);
            let media_type = match media_type {
                Some(mt) => mt,
                None => continue,
            };

            let file_path = entry.path().to_string_lossy().to_string();

            // Skip if already exists
            if rustyfin_db::repo_media::get_item_by_path(pool, &file_path)
                .await?
                .is_some()
            {
                continue;
            }

            let file_name = entry
                .path()
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
                path: file_path,
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
            };

            if let Err(e) = rustyfin_db::repo_media::upsert_media_item(pool, &item).await {
                tracing::error!("failed to insert media item: {}", e);
            } else {
                count += 1;
            }
        }
    }

    tracing::info!("scanned {} items for library '{}'", count, library.name);
    Ok(count)
}

/// Recursive scan for TV Shows: path/ShowName/Season XX/episode files
async fn scan_tv_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    let mut count = 0u32;

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        // series_path -> series_id
        let mut series_map: HashMap<String, Uuid> = HashMap::new();
        // (series_path, season_name) -> season_id
        let mut season_map: HashMap<(String, String), Uuid> = HashMap::new();

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if !VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }

            let file_path = entry.path().to_string_lossy().to_string();

            // Skip if already exists
            if rustyfin_db::repo_media::get_item_by_path(pool, &file_path)
                .await?
                .is_some()
            {
                continue;
            }

            // Determine hierarchy from path components relative to lib_path
            let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
            let components: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();

            let (series_name, season_name, episode_name) = match components.len() {
                // path/ShowName/Season/episode.mkv
                3.. => {
                    let sn = components[0].to_string();
                    let sea = components[1].to_string();
                    let ep = entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    (sn, sea, ep)
                }
                // path/ShowName/episode.mkv (no season folder)
                2 => {
                    let sn = components[0].to_string();
                    let ep = entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    (sn, "Season 1".to_string(), ep)
                }
                // path/episode.mkv (flat)
                _ => {
                    let ep = entry
                        .path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    ("Unknown Series".to_string(), "Season 1".to_string(), ep)
                }
            };

            let series_dir = path.join(&series_name).to_string_lossy().to_string();

            // Ensure series item exists
            let series_id = if let Some(id) = series_map.get(&series_dir) {
                *id
            } else {
                // Check DB first
                let existing = rustyfin_db::repo_media::get_item_by_path(pool, &series_dir).await?;
                let id = if let Some(item) = existing {
                    item.id
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
                    };
                    rustyfin_db::repo_media::upsert_media_item(pool, &series_item).await?;
                    count += 1;
                    id
                };
                series_map.insert(series_dir.clone(), id);
                id
            };

            // Ensure season item exists
            let season_key = (series_dir.clone(), season_name.clone());
            let season_id = if let Some(id) = season_map.get(&season_key) {
                *id
            } else {
                let season_path = format!("{}/{}", series_dir, season_name);
                let existing =
                    rustyfin_db::repo_media::get_item_by_path(pool, &season_path).await?;
                let id = if let Some(item) = existing {
                    item.id
                } else {
                    let id = Uuid::new_v4();
                    let season_index = parse_season_number(&season_name);
                    let season_item = MediaItem {
                        id,
                        name: season_name.clone(),
                        sort_name: Some(format!("season {:04}", season_index.unwrap_or(1))),
                        overview: None,
                        media_type: MediaType::Season,
                        path: season_path,
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
                    };
                    rustyfin_db::repo_media::upsert_media_item(pool, &season_item).await?;
                    count += 1;
                    id
                };
                season_map.insert(season_key, id);
                id
            };

            // Create episode item
            let episode_index = parse_episode_number(&episode_name);
            let season_index = parse_season_number(&season_name);
            let episode_item = MediaItem {
                id: Uuid::new_v4(),
                name: episode_name.clone(),
                sort_name: Some(episode_name.to_lowercase()),
                overview: None,
                media_type: MediaType::Episode,
                path: file_path,
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
            };

            if let Err(e) = rustyfin_db::repo_media::upsert_media_item(pool, &episode_item).await {
                tracing::error!("failed to insert episode: {}", e);
            } else {
                count += 1;
            }
        }
    }

    tracing::info!(
        "scanned {} items for TV library '{}'",
        count,
        library.name
    );
    Ok(count)
}

/// Recursive scan for Music: path/Artist/Album/track files
async fn scan_music_library(pool: &SqlitePool, library: &Library) -> Result<u32, sqlx::Error> {
    let mut count = 0u32;

    for lib_path in &library.paths {
        let path = Path::new(lib_path);
        if !path.exists() {
            tracing::warn!("library path does not exist: {}", lib_path);
            continue;
        }

        let mut album_map: HashMap<String, Uuid> = HashMap::new();

        for entry in WalkDir::new(path)
            .min_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }

            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }

            let file_path = entry.path().to_string_lossy().to_string();

            if rustyfin_db::repo_media::get_item_by_path(pool, &file_path)
                .await?
                .is_some()
            {
                continue;
            }

            let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
            let components: Vec<&str> = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();

            let parent_id = if components.len() >= 2 {
                // Use parent directory as album
                let album_dir = entry
                    .path()
                    .parent()
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let album_name = components[components.len() - 2].to_string();

                let album_id = if let Some(id) = album_map.get(&album_dir) {
                    *id
                } else {
                    let existing =
                        rustyfin_db::repo_media::get_item_by_path(pool, &album_dir).await?;
                    let id = if let Some(item) = existing {
                        item.id
                    } else {
                        let id = Uuid::new_v4();
                        let album_item = MediaItem {
                            id,
                            name: album_name.clone(),
                            sort_name: Some(album_name.to_lowercase()),
                            overview: None,
                            media_type: MediaType::MusicAlbum,
                            path: album_dir.clone(),
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
                        };
                        rustyfin_db::repo_media::upsert_media_item(pool, &album_item).await?;
                        count += 1;
                        id
                    };
                    album_map.insert(album_dir, id);
                    id
                };
                Some(album_id)
            } else {
                None
            };

            let file_name = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            let item = MediaItem {
                id: Uuid::new_v4(),
                name: file_name.clone(),
                sort_name: Some(file_name.to_lowercase()),
                overview: None,
                media_type: MediaType::Audio,
                path: file_path,
                parent_id,
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
            };

            if let Err(e) = rustyfin_db::repo_media::upsert_media_item(pool, &item).await {
                tracing::error!("failed to insert audio item: {}", e);
            } else {
                count += 1;
            }
        }
    }

    tracing::info!(
        "scanned {} items for music library '{}'",
        count,
        library.name
    );
    Ok(count)
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
    if let Some(start) = name.rfind('(') {
        if let Some(end) = name[start..].find(')') {
            let candidate = &name[start + 1..start + end];
            if let Ok(year) = candidate.parse::<i32>() {
                if (1900..=2100).contains(&year) {
                    return Some(year);
                }
            }
        }
    }
    None
}

fn parse_season_number(name: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    // "Season 02", "Season 2", "S02", "Staffel 2"
    for prefix in &["season ", "s", "staffel "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Ok(n) = rest.trim().parse::<i32>() {
                return Some(n);
            }
        }
    }
    None
}

fn parse_episode_number(name: &str) -> Option<i32> {
    let lower = name.to_lowercase();
    // Try S01E02 pattern
    if let Some(pos) = lower.find('e') {
        if pos > 0 && lower.as_bytes().get(pos - 1).map_or(false, |b| b.is_ascii_digit()) {
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
