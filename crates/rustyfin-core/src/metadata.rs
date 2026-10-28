use rustyfin_model::image::{ImageType, ItemImage};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::path::Path;
use uuid::Uuid;

use crate::local_images;
use crate::nfo_parser;
use crate::provider_tmdb::PersonPhoto;

/// Refresh metadata for a single item: local images, NFO, then optionally remote providers
pub async fn refresh_item_metadata(
    pool: &SqlitePool,
    client: &reqwest::Client,
    item: &MediaItem,
    tmdb_api_key: Option<&str>,
    omdb_api_key: Option<&str>,
    fanart_api_key: Option<&str>,
) -> Result<(), String> {
    let media_path = Path::new(&item.path);

    // 1. Scan for local images
    let local_images = local_images::find_local_images(media_path, item.id);
    if !local_images.is_empty() {
        rustyfin_db::repo_image::batch_upsert_images(pool, &local_images)
            .await
            .map_err(|e| e.to_string())?;
    }
    tracing::debug!(
        "found {} local images for '{}'",
        local_images.len(),
        item.name
    );

    // 2. Parse NFO file if present
    if let Some(nfo_path) = nfo_parser::find_nfo_for_media(media_path)
        && let Some(meta) = nfo_parser::parse_nfo(&nfo_path).await
    {
        // Update media item metadata from NFO
        apply_metadata_to_item(pool, item, &meta).await?;

        // Batch-store image URLs from NFO, skipping types that already exist
        store_images_batch(pool, item.id, &meta.image_urls, "nfo").await?;
    }

    // 3. TMDb provider - use cleaned filename for better search results
    if let Some(api_key) = tmdb_api_key {
        // Clean the filename for better TMDB search matching
        let search_name = clean_filename_for_search(&item.path);

        let meta = match item.media_type {
            MediaType::Movie => {
                crate::provider_tmdb::search_movie(
                    client,
                    api_key,
                    &search_name,
                    item.production_year,
                )
                .await
            }
            MediaType::Series => {
                crate::provider_tmdb::search_tv(client, api_key, &search_name).await
            }
            _ => None,
        };

        if !search_name.is_empty() && search_name != item.name {
            tracing::debug!(
                "searching TMDB for '{}' (cleaned from '{}')",
                search_name,
                item.name
            );
        }

        if let Some(meta) = meta {
            apply_metadata_to_item(pool, item, &meta).await?;
            store_images_batch(pool, item.id, &meta.image_urls, "tmdb").await?;

            // Fetch and store cast/crew photos from TMDb
            if let Some(tmdb_id) = meta.tmdb_id {
                let people = match item.media_type {
                    MediaType::Movie => {
                        crate::provider_tmdb::get_movie_people(client, api_key, tmdb_id).await
                    }
                    MediaType::Series | MediaType::Episode => {
                        let season = item.parent_index_number.unwrap_or(1);
                        crate::provider_tmdb::get_tv_people(client, api_key, tmdb_id, season).await
                    }
                    _ => Vec::new(),
                };
                store_people(pool, item.id, &people).await?;
            }
        }
    }

    // 4. OMDb provider
    if let Some(api_key) = omdb_api_key
        && matches!(item.media_type, MediaType::Movie | MediaType::Series)
        && let Some(meta) =
            crate::provider_omdb::search(api_key, &item.name, item.production_year).await
    {
        apply_metadata_to_item(pool, item, &meta).await?;
        store_images_batch(pool, item.id, &meta.image_urls, "omdb").await?;
    }

    // 5. Fanart.tv provider - enriches images from TMDb/TheTVDB
    if let Some(api_key) = fanart_api_key {
        match item.media_type {
            MediaType::Movie => {
                if let Some(tmdb_id) = item.tmdb_id {
                    let images = crate::provider_fanart::get_movie_images(api_key, tmdb_id).await;
                    if !images.is_empty() {
                        store_images_batch(pool, item.id, &images, "fanart").await?;
                    }
                }
            }
            MediaType::Series => {
                if let Some(tvdb_id) = item.tvdb_id {
                    let images = crate::provider_fanart::get_tv_show_images(api_key, tvdb_id).await;
                    if !images.is_empty() {
                        store_images_batch(pool, item.id, &images, "fanart").await?;
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn apply_metadata_to_item(
    pool: &SqlitePool,
    item: &MediaItem,
    meta: &rustyfin_model::metadata::ExternalMetadata,
) -> Result<(), String> {
    // Update fields from external metadata
    let mut updated = item.clone();
    let mut changed = false;

    // Update name from TMDB if it looks like a filename (contains dots, underscores, or release tags)
    if let Some(ref tmdb_name) = meta.name {
        let looks_like_filename = updated.name.contains('.')
            || updated.name.contains('_')
            || updated.name.contains("-ENCOUNTERS")
            || updated.name.contains("-NIMA4K")
            || updated.name.contains("-eisbaer")
            || updated.name.contains("-WATCHABLE")
            || updated.name.contains("1080p")
            || updated.name.contains("2160p")
            || updated.name.contains("BluRay")
            || updated.name.contains("WEB-DL")
            || updated.name.contains("HDTV");

        if looks_like_filename || updated.name.len() > 80 {
            // The current name looks like a filename, replace with TMDB name
            if updated.name != *tmdb_name {
                updated.name = tmdb_name.clone();
                updated.sort_name = Some(tmdb_name.to_lowercase());
                changed = true;
                tracing::info!(
                    "updated name from filename '{}' to TMDB name '{}'",
                    item.name,
                    tmdb_name
                );
            }
        }
    }

    if updated.overview.is_none()
        && let Some(ref v) = meta.overview
    {
        updated.overview = Some(v.clone());
        changed = true;
    }
    if updated.production_year.is_none()
        && let Some(v) = meta.production_year
    {
        updated.production_year = Some(v);
        changed = true;
    }
    if updated.community_rating.is_none()
        && let Some(v) = meta.community_rating
    {
        updated.community_rating = Some(v);
        changed = true;
    }
    if updated.official_rating.is_none()
        && let Some(ref v) = meta.official_rating
    {
        updated.official_rating = Some(v.clone());
        changed = true;
    }
    if updated.genres.is_empty()
        && let Some(ref v) = meta.genres
        && !v.is_empty()
    {
        updated.genres = v.clone();
        changed = true;
    }
    if updated.runtime_ticks.is_none()
        && let Some(v) = meta.runtime_ticks
    {
        updated.runtime_ticks = Some(v);
        changed = true;
    }
    if updated.tmdb_id.is_none()
        && let Some(v) = meta.tmdb_id
    {
        updated.tmdb_id = Some(v);
        changed = true;
    }
    if updated.tvdb_id.is_none()
        && let Some(v) = meta.tvdb_id
    {
        updated.tvdb_id = Some(v);
        changed = true;
    }
    if updated.status.is_none()
        && let Some(ref v) = meta.status
    {
        updated.status = Some(v.clone());
        changed = true;
    }
    if updated.original_language.is_none()
        && let Some(ref v) = meta.original_language
    {
        updated.original_language = Some(v.clone());
        changed = true;
    }
    if updated.taglines.is_empty()
        && let Some(ref v) = meta.tagline
        && !v.is_empty()
    {
        updated.taglines = vec![v.clone()];
        changed = true;
    }
    if updated.production_locations.is_empty()
        && let Some(ref v) = meta.production_countries
        && !v.is_empty()
    {
        updated.production_locations = v.clone();
        changed = true;
    }
    if updated.end_date.is_none()
        && let Some(ref v) = meta.end_date
    {
        // Parse end_date string to DateTime<Utc>
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d")
            && let dt_utc = dt.and_time(chrono::NaiveTime::MIN).and_utc()
        {
            updated.end_date = Some(dt_utc);
            changed = true;
        }
    }
    if updated.budget.is_none()
        && let Some(v) = meta.budget
    {
        updated.budget = Some(v);
        changed = true;
    }
    if updated.revenue.is_none()
        && let Some(v) = meta.revenue
    {
        updated.revenue = Some(v);
        changed = true;
    }
    if updated.homepage.is_none()
        && let Some(ref v) = meta.homepage
        && !v.is_empty()
    {
        updated.homepage = Some(v.clone());
        changed = true;
    }
    if updated.vote_count.is_none()
        && let Some(v) = meta.vote_count
    {
        updated.vote_count = Some(v);
        changed = true;
    }
    if updated.popularity.is_none()
        && let Some(v) = meta.popularity
    {
        updated.popularity = Some(v);
        changed = true;
    }

    if changed {
        rustyfin_db::repo_media::upsert_media_item(pool, &updated)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Store studios from metadata
    if let Some(ref studios) = meta.studios
        && !studios.is_empty()
    {
        store_studios(pool, item.id, studios).await?;
    }

    Ok(())
}

/// Clean a filename to make it suitable for TMDB search.
/// Removes file extensions, release tags, quality indicators, and normalizes separators.
/// Example: "Zombieland.2009.German.DL.1080p.BluRay.x264-SCENE.mkv" -> "Zombieland 2009"
pub fn clean_filename_for_search(path: &str) -> String {
    use std::path::Path;

    let p = Path::new(path);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    // Replace common separators with spaces
    let mut cleaned = stem
        .replace(['.', '_', '-'], " ")
        .replace("  ", " ")
        .trim()
        .to_string();

    // Remove common release groups and quality tags
    let tags_to_remove = [
        "GERMAN",
        "DL",
        "DC",
        "Dubbed",
        "PROPER",
        "1080p",
        "720p",
        "2160p",
        "4K",
        "UHD",
        "HDR",
        "Dolby",
        "BluRay",
        "BLURAY",
        "WEB-DL",
        "WEBDL",
        "HDTV",
        "BDRip",
        "WEBRip",
        "x264",
        "x265",
        "HEVC",
        "H264",
        "AVC",
        "MKV",
        "MP4",
        "AVI",
        "M4V",
        "EAC3",
        "DD51",
        "DDP",
        "DTSHD",
        "TrueHD",
        "GERMAN.DL",
        "ENGLISH",
    ];

    for tag in &tags_to_remove {
        let pattern = format!(" {} ", tag);
        if cleaned.contains(&pattern) {
            cleaned = cleaned.replace(&pattern, " ");
        }
        // Also check at start/end
        let start_pattern = format!("{} ", tag);
        if let Some(stripped) = cleaned.strip_prefix(&start_pattern) {
            cleaned = stripped.to_string();
        }
        let end_pattern = format!(" {}", tag);
        if let Some(stripped) = cleaned.strip_suffix(&end_pattern) {
            cleaned = stripped.to_string();
        }
    }

    // Remove encoder groups (e.g., "-SCENE", "-NIMA4K", "-eisbaer", "-WATCHABLE")
    if let Some(pos) = cleaned.rfind('-')
        && pos > 10
    {
        cleaned = cleaned[..pos].to_string();
    }

    // Clean up extra spaces
    cleaned = cleaned.split_whitespace().collect::<Vec<&str>>().join(" ");

    // Extract year if present (4-digit year at the end or in parentheses)
    // Keep the year for better search results
    cleaned = cleaned.trim().to_string();

    cleaned
}

/// Batch-store provider images, merging all sources.
/// Each provider's images get their own image_index slot so multiple providers
/// can contribute images of the same type (e.g. multiple backdrops from tmdb, fanart, etc.).
async fn store_images_batch(
    pool: &SqlitePool,
    item_id: Uuid,
    images: &[rustyfin_model::metadata::MetadataImage],
    provider: &str,
) -> Result<(), String> {
    // Group images by base type to assign proper indices
    // Parse image_type strings like "Primary", "Primary_1", "Backdrop", "Backdrop_2"
    let mut type_counts: std::collections::HashMap<
        String,
        Vec<(usize, &rustyfin_model::metadata::MetadataImage)>,
    > = std::collections::HashMap::new();
    for img_meta in images {
        // Extract base type: "Primary_2" -> "Primary", "Backdrop" -> "Backdrop"
        let base_type = img_meta
            .image_type
            .split('_')
            .next()
            .unwrap_or(&img_meta.image_type);
        let entry = type_counts.entry(base_type.to_string()).or_default();
        entry.push((entry.len(), img_meta));
    }

    // Fetch existing images to determine next available index
    let existing_images = rustyfin_db::repo_image::get_images_for_item(pool, item_id)
        .await
        .map_err(|e| e.to_string())?;

    // Build map of base_type -> max_index for THIS provider only
    // For the first provider image of each type, always use index 0
    // so the client can find it (client defaults to index 0)
    let mut provider_max_indices: std::collections::HashMap<String, i32> =
        std::collections::HashMap::new();
    for img in &existing_images {
        if img.provider == provider {
            let base = img.image_type.as_str().to_string();
            let current = provider_max_indices.entry(base).or_insert(-1);
            if img.image_index > *current {
                *current = img.image_index;
            }
        }
    }

    // Build list of images to insert
    let mut to_insert = Vec::new();
    for (base_type, img_group) in type_counts {
        if let Some(image_type_base) = ImageType::from_str(&base_type) {
            // First provider image of each type goes to index 0
            // Subsequent images from the same provider follow after the max index
            let start_index = match provider_max_indices.get(&base_type) {
                Some(&max_idx) if max_idx >= 0 => max_idx + 1,
                _ => 0,
            };

            for (idx, img_meta) in img_group {
                let item_image_type = if idx == 0 {
                    image_type_base
                } else {
                    // Parse the specific variant (e.g. "Primary_1" -> Primary with index)
                    ImageType::from_str(&img_meta.image_type).unwrap_or(image_type_base)
                };
                to_insert.push(ItemImage {
                    item_id,
                    image_type: item_image_type,
                    image_index: start_index + idx as i32,
                    path: img_meta.url.clone(),
                    provider: provider.to_string(),
                    width: None,
                    height: None,
                });
            }
        }
    }

    // Batch-insert all images in a single transaction
    if !to_insert.is_empty() {
        rustyfin_db::repo_image::batch_upsert_images(pool, &to_insert)
            .await
            .map_err(|e| e.to_string())?;
        tracing::debug!(
            "stored {} images from '{}' for item {}",
            to_insert.len(),
            provider,
            item_id
        );
    }
    Ok(())
}

/// Store people (cast/crew) from TMDb into the people and item_people tables
async fn store_people(
    pool: &SqlitePool,
    item_id: Uuid,
    people: &[PersonPhoto],
) -> Result<(), String> {
    let item_id_str = item_id.to_string();
    for person in people {
        // Upsert person into people table with photo URL as image_hash
        sqlx::query(
            "INSERT INTO people (id, name, type, image_hash)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET image_hash = COALESCE(excluded.image_hash, people.image_hash)",
        )
        .bind(&person.person_id)
        .bind(&person.name)
        .bind("Actor")
        .bind(&person.photo_url)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

        // Link person to item
        sqlx::query(
            "INSERT OR IGNORE INTO item_people (item_id, person_id, type)
             VALUES (?1, ?2, ?3)",
        )
        .bind(&item_id_str)
        .bind(&person.person_id)
        .bind("Actor")
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }
    tracing::debug!("stored {} people for item {}", people.len(), item_id);
    Ok(())
}

/// Store studios from TMDb into the studios and item_studios tables
async fn store_studios(
    pool: &SqlitePool,
    item_id: Uuid,
    studio_names: &[String],
) -> Result<(), String> {
    let item_id_str = item_id.to_string();
    for studio_name in studio_names {
        // Generate a stable ID from the studio name
        let studio_id = format!("studio-{}", studio_name.to_lowercase().replace(' ', "-"));
        // Upsert studio
        sqlx::query(
            "INSERT INTO studios (id, name)
             VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        )
        .bind(&studio_id)
        .bind(studio_name)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        // Link studio to item
        sqlx::query(
            "INSERT OR IGNORE INTO item_studios (item_id, studio_id)
             VALUES (?1, ?2)",
        )
        .bind(&item_id_str)
        .bind(&studio_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }
    tracing::debug!("stored {} studios for item {}", studio_names.len(), item_id);
    Ok(())
}
