use rustyfin_model::image::{ImageType, ItemImage};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::Path;
use uuid::Uuid;

use crate::local_images;
use crate::nfo_parser;
use crate::provider_tmdb::PersonPhoto;

/// Refresh metadata for a single item: local images, NFO, then optionally remote providers
pub async fn refresh_item_metadata(
    pool: &SqlitePool,
    item: &MediaItem,
    tmdb_api_key: Option<&str>,
    omdb_api_key: Option<&str>,
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

    // 3. TMDb provider
    if let Some(api_key) = tmdb_api_key {
        let meta = match item.media_type {
            MediaType::Movie => {
                crate::provider_tmdb::search_movie(api_key, &item.name, item.production_year).await
            }
            MediaType::Series => crate::provider_tmdb::search_tv(api_key, &item.name).await,
            _ => None,
        };

        if let Some(meta) = meta {
            apply_metadata_to_item(pool, item, &meta).await?;
            store_images_batch(pool, item.id, &meta.image_urls, "tmdb").await?;

            // Fetch and store cast/crew photos from TMDb
            if let Some(tmdb_id) = meta.tmdb_id {
                let people = match item.media_type {
                    MediaType::Movie => {
                        crate::provider_tmdb::get_movie_people(api_key, tmdb_id).await
                    }
                    MediaType::Series | MediaType::Episode => {
                        let season = item.parent_index_number.unwrap_or(1);
                        crate::provider_tmdb::get_tv_people(api_key, tmdb_id, season).await
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

    Ok(())
}

async fn apply_metadata_to_item(
    pool: &SqlitePool,
    item: &MediaItem,
    meta: &rustyfin_model::metadata::ExternalMetadata,
) -> Result<(), String> {
    // Update fields that are currently empty
    let mut updated = item.clone();
    let mut changed = false;

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

    if changed {
        rustyfin_db::repo_media::upsert_media_item(pool, &updated)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Batch-store provider images, skipping types that already exist.
/// Fetches all existing image types in one query, then batch-inserts the missing ones.
async fn store_images_batch(
    pool: &SqlitePool,
    item_id: Uuid,
    images: &[rustyfin_model::metadata::MetadataImage],
    provider: &str,
) -> Result<(), String> {
    // Fetch all existing image types in a single query
    let existing_types = rustyfin_db::repo_image::get_image_types_for_item(pool, item_id)
        .await
        .map_err(|e| e.to_string())?;
    let existing_set: HashSet<&str> = existing_types.iter().map(|s| s.as_str()).collect();

    // Build list of images to insert, skipping types that already exist
    let mut to_insert = Vec::new();
    for img_meta in images {
        if let Some(image_type) = ImageType::from_str(&img_meta.image_type) {
            let type_str = image_type.as_str();
            if !existing_set.contains(type_str) {
                to_insert.push(ItemImage {
                    item_id,
                    image_type,
                    image_index: 0,
                    path: img_meta.url.clone(),
                    provider: provider.to_string(),
                    width: None,
                    height: None,
                });
            }
        }
    }

    // Batch-insert all missing images in a single transaction
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
