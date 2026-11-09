use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_model::image::{ImageInfo, ImageType, ItemImage};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Items/{id}/Images", get(get_item_images))
        .route("/Items/{id}/Images/{image_type}", get(get_item_image))
        .route(
            "/Items/{id}/Images/{image_type}/{image_index}",
            get(get_item_image_indexed),
        )
}

/// Administrator-only image/refresh routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Items/{id}/Images/{image_type}", post(upload_item_image))
        .route("/Items/{id}/Refresh", post(refresh_item))
}

async fn get_item_images(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ImageInfo>>, ApiError> {
    let images = rustyfin_db::repo_image::get_images_for_item(&state.db_ro, id).await?;

    let infos: Vec<ImageInfo> = images
        .into_iter()
        .map(|img| ImageInfo {
            image_type: img.image_type,
            image_index: img.image_index,
            image_tag: rustyfin_core::image_proxy::image_tag(&img.path, img.stamp),
            path: img.path,
            width: img.width,
            height: img.height,
        })
        .collect();

    Ok(Json(infos))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ImageQuery {
    #[serde(
        alias = "maxWidth",
        alias = "max_width",
        alias = "width",
        alias = "fillWidth"
    )]
    max_width: Option<u32>,
    #[serde(
        alias = "maxHeight",
        alias = "max_height",
        alias = "height",
        alias = "fillHeight"
    )]
    max_height: Option<u32>,
    #[serde(alias = "imageIndex", alias = "image_index")]
    image_index: Option<i32>,
    tag: Option<String>,
    format: Option<String>,
    quality: Option<u32>,
}

async fn serve_image(
    state: &AppState,
    id: Uuid,
    image_type_str: &str,
    index: i32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    tagged: bool,
) -> Result<Response, ApiError> {
    let image_type = ImageType::from_str(image_type_str)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown image type: {}", image_type_str)))?;
    // A tag names one version of the picture, so a tagged URL can be kept long. An untagged URL
    // can change under the same address when the picture is replaced, so it is kept only briefly.
    let cache_control = if tagged {
        "public, max-age=86400"
    } else {
        "public, max-age=300"
    };

    let found = rustyfin_db::repo_image::get_image(&state.db_ro, id, &image_type, index).await?;
    // Imported people keep their images in `person_images`; try those before giving up.
    let img = match found {
        Some(img) => img,
        None => rustyfin_db::repo_image::get_person_image(&state.db_ro, id, &image_type, index)
            .await?
            .ok_or_else(|| ApiError::NotFound("image not found".into()))?,
    };

    // A stored size spares the header read, but counts only when both sides are known.
    let stored_size = match (img.width, img.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
        _ => None,
    };
    let rendition = match rustyfin_core::image_proxy::plan_rendition(
        &img.path,
        stored_size,
        max_width,
        max_height,
    )
    .await
    {
        Ok(rendition) => rendition,
        Err(e) => {
            tracing::warn!(error = %e, path = %img.path, "image could not be read or resized");
            return Err(ApiError::NotFound(format!("image not found: {}", e)));
        }
    };

    // A local original too big for the memory cache is streamed from its file, not held in memory
    let is_remote = img.path.starts_with("http://") || img.path.starts_with("https://");
    if rendition == rustyfin_core::image_proxy::Rendition::Original
        && !is_remote
        && let Ok(meta) = tokio::fs::metadata(&img.path).await
        && meta.len() > crate::cache::ImageCache::MAX_ENTRY_SIZE as u64
    {
        let file = match tokio::fs::File::open(&img.path).await {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(error = %e, path = %img.path, "image could not be read or resized");
                return Err(ApiError::NotFound(format!(
                    "image not found: failed to read local image: {}",
                    e
                )));
            }
        };
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, guess_content_type(&img.path, false))
            .header(header::CACHE_CONTROL, cache_control)
            .header(header::CONTENT_LENGTH, meta.len().to_string())
            .body(Body::from_stream(tokio_util::io::ReaderStream::new(file)))
            .unwrap());
    }

    // Build cache key for in-memory image cache. The stamp changes with the file, so a replaced
    // picture is not served from the old one.
    let stamp = img.stamp.unwrap_or(0);
    let cache_key = format!(
        "{}:{}:{}:{}:{}",
        id,
        image_type_str,
        index,
        stamp,
        rendition.key()
    );

    // Check in-memory cache first
    let bytes = if let Some(cached) = state.image_cache.get(&cache_key) {
        cached
    } else {
        let cache_dir = state.data_dir.join("cache").join("images");
        let bytes = match rustyfin_core::image_proxy::render_image(
            &img.path,
            img.stamp,
            rendition,
            &cache_dir,
            Some(&state.http_client),
        )
        .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, path = %img.path, "image could not be read or resized");
                return Err(ApiError::NotFound(format!("image not found: {}", e)));
            }
        };
        // Store in in-memory cache
        state.image_cache.insert(cache_key.clone(), bytes.clone());
        bytes
    };

    let content_type = guess_content_type(
        &img.path,
        rendition != rustyfin_core::image_proxy::Rendition::Original,
    );

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(bytes))
        .unwrap())
}

async fn get_item_image(
    State(state): State<AppState>,
    Path((id, image_type_str)): Path<(Uuid, String)>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    let index = query.image_index.unwrap_or(0);
    serve_image(
        &state,
        id,
        &image_type_str,
        index,
        query.max_width,
        query.max_height,
        query.tag.as_deref().is_some_and(|t| !t.is_empty()),
    )
    .await
}

async fn get_item_image_indexed(
    State(state): State<AppState>,
    Path((id, image_type_str, image_index)): Path<(Uuid, String, i32)>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    serve_image(
        &state,
        id,
        &image_type_str,
        image_index,
        query.max_width,
        query.max_height,
        query.tag.as_deref().is_some_and(|t| !t.is_empty()),
    )
    .await
}

async fn upload_item_image(
    State(state): State<AppState>,
    Path((id, image_type_str)): Path<(Uuid, String)>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let image_type = ImageType::from_str(&image_type_str)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown image type: {}", image_type_str)))?;

    // Verify item exists
    rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;

    // Save to data_dir/images/{item_id}/{type}.jpg
    let img_dir = state.data_dir.join("images").join(id.to_string());
    tokio::fs::create_dir_all(&img_dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let ext = detect_image_extension(&body);
    let filename = format!("{}.{}", image_type.as_str(), ext);
    let file_path = img_dir.join(&filename);

    tokio::fs::write(&file_path, &body)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // The file's stamp and size, so that the new picture has its own tag and copies
    let (stamp, width, height) = rustyfin_core::local_images::local_image_version(&file_path);
    let img = ItemImage {
        item_id: id,
        image_type,
        image_index: 0,
        path: file_path.to_string_lossy().to_string(),
        provider: "upload".to_string(),
        width,
        height,
        blurhash: None,
        stamp,
    };

    rustyfin_db::repo_image::upsert_image(&state.db, &img).await?;
    // Drop the answers and the pictures that name the old one
    state.cache.invalidate_item(&id.to_string());
    state.image_cache.invalidate_item(&id.to_string());

    Ok(Json(json!({ "status": "ok" })))
}

async fn refresh_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;

    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
    let omdb_key = config.omdb_api_key.as_deref();
    let fanart_key = config.fanart_api_key.as_deref();

    rustyfin_core::metadata::refresh_item_metadata(
        &state.db,
        &state.http_client,
        &item,
        tmdb.as_ref(),
        omdb_key,
        fanart_key,
    )
    .await
    .map_err(ApiError::Internal)?;

    // Invalidate cached item details and images after metadata refresh
    state.cache.invalidate_item(&id.to_string());
    state.image_cache.invalidate_item(&id.to_string());

    Ok(Json(json!({ "status": "ok" })))
}

fn guess_content_type(path: &str, resized: bool) -> &'static str {
    if resized {
        return "image/jpeg";
    }
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "image/jpeg"
    }
}

fn detect_image_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "png"
    } else if bytes.starts_with(&[0x47, 0x49, 0x46]) {
        "gif"
    } else if bytes.len() >= 4
        && &bytes[0..4] == b"RIFF"
        && bytes.len() >= 12
        && &bytes[8..12] == b"WEBP"
    {
        "webp"
    } else {
        "jpg"
    }
}
