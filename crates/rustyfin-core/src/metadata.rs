use rustyfin_model::image::{ImageType, ItemImage};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::path::Path;
use uuid::Uuid;

use crate::local_images;
use crate::nfo_parser;

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
    for img in &local_images {
        rustyfin_db::repo_image::upsert_image(pool, img)
            .await
            .map_err(|e| e.to_string())?;
    }
    tracing::debug!("found {} local images for '{}'", local_images.len(), item.name);

    // 2. Parse NFO file if present
    if let Some(nfo_path) = nfo_parser::find_nfo_for_media(media_path) {
        if let Some(meta) = nfo_parser::parse_nfo(&nfo_path).await {
            // Update media item metadata from NFO
            apply_metadata_to_item(pool, item, &meta).await?;

            // Store image URLs from NFO
            for img_meta in &meta.image_urls {
                if let Some(image_type) = ImageType::from_str(&img_meta.image_type) {
                    let img = ItemImage {
                        item_id: item.id,
                        image_type,
                        image_index: 0,
                        path: img_meta.url.clone(),
                        provider: "nfo".to_string(),
                        width: None,
                        height: None,
                    };
                    // Only store if no local image of this type exists
                    let existing = rustyfin_db::repo_image::get_image(pool, item.id, &image_type, 0)
                        .await
                        .map_err(|e| e.to_string())?;
                    if existing.is_none() {
                        rustyfin_db::repo_image::upsert_image(pool, &img)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
        }
    }

    // 3. TMDb provider
    if let Some(api_key) = tmdb_api_key {
        let meta = match item.media_type {
            MediaType::Movie => {
                crate::provider_tmdb::search_movie(api_key, &item.name, item.production_year).await
            }
            MediaType::Series => {
                crate::provider_tmdb::search_tv(api_key, &item.name).await
            }
            _ => None,
        };

        if let Some(meta) = meta {
            apply_metadata_to_item(pool, item, &meta).await?;
            store_provider_images(pool, item.id, &meta.image_urls, "tmdb").await?;
        }
    }

    // 4. OMDb provider
    if let Some(api_key) = omdb_api_key {
        if matches!(item.media_type, MediaType::Movie | MediaType::Series) {
            if let Some(meta) = crate::provider_omdb::search(api_key, &item.name, item.production_year).await {
                apply_metadata_to_item(pool, item, &meta).await?;
                store_provider_images(pool, item.id, &meta.image_urls, "omdb").await?;
            }
        }
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

    if updated.overview.is_none() {
        if let Some(ref v) = meta.overview {
            updated.overview = Some(v.clone());
            changed = true;
        }
    }
    if updated.production_year.is_none() {
        if let Some(v) = meta.production_year {
            updated.production_year = Some(v);
            changed = true;
        }
    }
    if updated.community_rating.is_none() {
        if let Some(v) = meta.community_rating {
            updated.community_rating = Some(v);
            changed = true;
        }
    }
    if updated.official_rating.is_none() {
        if let Some(ref v) = meta.official_rating {
            updated.official_rating = Some(v.clone());
            changed = true;
        }
    }
    if updated.genres.is_empty() {
        if let Some(ref v) = meta.genres {
            if !v.is_empty() {
                updated.genres = v.clone();
                changed = true;
            }
        }
    }
    if updated.runtime_ticks.is_none() {
        if let Some(v) = meta.runtime_ticks {
            updated.runtime_ticks = Some(v);
            changed = true;
        }
    }

    if changed {
        rustyfin_db::repo_media::upsert_media_item(pool, &updated)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

async fn store_provider_images(
    pool: &SqlitePool,
    item_id: Uuid,
    images: &[rustyfin_model::metadata::MetadataImage],
    provider: &str,
) -> Result<(), String> {
    for img_meta in images {
        if let Some(image_type) = ImageType::from_str(&img_meta.image_type) {
            // Only store if no image of this type exists yet
            let existing = rustyfin_db::repo_image::get_image(pool, item_id, &image_type, 0)
                .await
                .map_err(|e| e.to_string())?;
            if existing.is_none() {
                let img = ItemImage {
                    item_id,
                    image_type,
                    image_index: 0,
                    path: img_meta.url.clone(),
                    provider: provider.to_string(),
                    width: None,
                    height: None,
                };
                rustyfin_db::repo_image::upsert_image(pool, &img)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}
