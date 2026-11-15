use crate::query::Query;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
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

/// Whether an `If-None-Match` header names `etag` (or is `*`). Weak and strong forms both match,
/// as RFC 9110 has it for GET.
fn etag_matches(if_none_match: Option<&HeaderValue>, etag: &str) -> bool {
    let Some(value) = if_none_match.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    value.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

#[allow(clippy::too_many_arguments)]
async fn serve_image(
    state: &AppState,
    id: Uuid,
    image_type_str: &str,
    index: i32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    tag: Option<&str>,
    if_none_match: Option<&HeaderValue>,
) -> Result<Response, ApiError> {
    let mut img = if image_type_str.eq_ignore_ascii_case("chapter") {
        chapter_image(state, id, index).await?
    } else {
        let image_type = ImageType::from_str(image_type_str).ok_or_else(|| {
            ApiError::BadRequest(format!("unknown image type: {}", image_type_str))
        })?;
        let found =
            rustyfin_db::repo_image::get_image(&state.db_ro, id, &image_type, index).await?;
        // Imported people keep their images in `person_images`; try those before giving up.
        match found {
            Some(img) => img,
            None => rustyfin_db::repo_image::get_person_image(&state.db_ro, id, &image_type, index)
                .await?
                .ok_or_else(|| ApiError::NotFound("image not found".into()))?,
        }
    };

    // The URL's tag names one version of the picture. When it is the current one, the answer
    // never changes and is kept for a year. A missing or stale tag gets whatever picture is
    // current, which can change under the same URL, so it is kept only briefly.
    let current_tag = rustyfin_core::image_proxy::image_tag(&img.path, img.stamp);
    let cache_control = if tag.is_some_and(|t| t.eq_ignore_ascii_case(&current_tag)) {
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=300"
    };

    // A remote image is kept in the data dir on first use, so a restart reads it from disk rather
    // than the origin. The DB row keeps the URL. If only the write failed, the URL is served.
    if img.path.starts_with("http://") || img.path.starts_with("https://") {
        let store = state.data_dir.join("metadata").join("remote-images");
        match rustyfin_core::image_proxy::localize_remote_image_checked(
            &img.path,
            &store,
            Some(&state.http_client),
        )
        .await
        {
            Ok(path) => img.path = path.to_string_lossy().into_owned(),
            Err(rustyfin_core::image_proxy::LocalizeError::Download(e)) => {
                tracing::warn!(error = %e, path = %img.path, "remote image could not be downloaded");
                return Err(ApiError::NotFound(format!("image not found: {}", e)));
            }
            Err(rustyfin_core::image_proxy::LocalizeError::Store(e)) => {
                tracing::warn!(error = %e, path = %img.path, "remote image could not be stored; serving the URL");
            }
        }
    }

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

    // The ETag names the bytes sent: this version of the picture at this rendition. A client
    // that has them gets a 304 before anything is read or rendered.
    let etag = format!("\"{}-{}\"", current_tag, rendition.key());
    if etag_matches(if_none_match, &etag) {
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, cache_control)
            .body(Body::empty())
            .unwrap());
    }

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
            .header(header::ETAG, &etag)
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
        // A copy at one of the grid widths (GRID_COPY_WIDTHS) not made yet is answered with the
        // pre-generated grid master, larger, at once (the browser scales it down) rather than
        // rendered now; the copy is made in the background (at most PREPARING_MAX at a time;
        // with no room, a later request asks again). The ETag names the master's bytes, which are
        // what is sent, and the answer is kept 5 minutes, not as immutable: the browser, the
        // service worker (it keeps immutable answers only) and any proxy then fetch the copy.
        // Other widths are rendered now, as before.
        let grid_width =
            max_width.is_some_and(|w| rustyfin_core::image_proxy::GRID_COPY_WIDTHS.contains(&w));
        if grid_width
            && let Some(bytes) = rustyfin_core::image_proxy::grid_master_stand_in(
                &img.path, img.stamp, rendition, &cache_dir,
            )
            .await
        {
            rustyfin_core::image_proxy::prepare_copy_in_background(
                img.path.clone(),
                img.stamp,
                rendition,
                cache_dir,
            );
            let master = rustyfin_core::image_proxy::Rendition::Copy {
                width: rustyfin_core::image_proxy::GRID_MASTER.0,
                height: rustyfin_core::image_proxy::GRID_MASTER.1,
            };
            return Ok(Response::builder()
                .header(header::CONTENT_TYPE, "image/jpeg")
                .header(header::CACHE_CONTROL, "public, max-age=300")
                .header(
                    header::ETAG,
                    format!("\"{}-{}\"", current_tag, master.key()),
                )
                .header(header::CONTENT_LENGTH, bytes.len().to_string())
                .body(Body::from(bytes))
                .unwrap());
        }
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
        .header(header::ETAG, &etag)
        .body(Body::from(bytes))
        .unwrap())
}

/// The image of chapter `index` (from 0, in start order) of item `id`, as an image row. Chapter
/// images live in the chapters table, not with the item's other images; the row's type is never
/// read. 404 when the chapter has no image.
async fn chapter_image(state: &AppState, id: Uuid, index: i32) -> Result<ItemImage, ApiError> {
    let chapters = rustyfin_db::repo_chapter::get_chapters(&state.db_ro, id).await?;
    let chapter = usize::try_from(index)
        .ok()
        .and_then(|i| chapters.get(i))
        .ok_or_else(|| ApiError::NotFound("chapter not found".into()))?;
    let path = chapter
        .image_path
        .clone()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::NotFound("chapter image not found".into()))?;
    let (stamp, width, height) =
        rustyfin_core::local_images::local_image_version(std::path::Path::new(&path));
    Ok(ItemImage {
        item_id: id,
        image_type: ImageType::Screenshot,
        image_index: index,
        path,
        provider: "chapter".to_string(),
        width,
        height,
        blurhash: None,
        stamp,
    })
}

async fn get_item_image(
    State(state): State<AppState>,
    Path((id, image_type_str)): Path<(Uuid, String)>,
    Query(query): Query<ImageQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let index = query.image_index.unwrap_or(0);
    serve_image(
        &state,
        id,
        &image_type_str,
        index,
        query.max_width,
        query.max_height,
        query.tag.as_deref(),
        headers.get(header::IF_NONE_MATCH),
    )
    .await
}

async fn get_item_image_indexed(
    State(state): State<AppState>,
    Path((id, image_type_str, image_index)): Path<(Uuid, String, i32)>,
    Query(query): Query<ImageQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    serve_image(
        &state,
        id,
        &image_type_str,
        image_index,
        query.max_width,
        query.max_height,
        query.tag.as_deref(),
        headers.get(header::IF_NONE_MATCH),
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
    rustyfin_core::image_warmer::kick();

    Ok(Json(json!({ "status": "ok" })))
}

async fn refresh_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Result<Json<Value>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;

    // A refresh that replaces all images tries failed chapter images and trickplay again.
    let replace_all = query.as_deref().is_some_and(|q| {
        q.split('&').any(|pair| {
            pair.split_once('=').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("ReplaceAllImages") && value.eq_ignore_ascii_case("true")
            })
        })
    });
    if replace_all {
        rustyfin_db::repo_trickplay::clear_failures(&state.db, id).await?;
    }

    rustyfin_core::metadata::refresh_item_metadata(
        &state.db,
        &state.http_client,
        &item,
        &state.plugin_manager,
    )
    .await
    .map_err(ApiError::Internal)?;

    // Invalidate cached item details and images after metadata refresh
    state.cache.invalidate_item(&id.to_string());
    state.image_cache.invalidate_item(&id.to_string());
    rustyfin_core::image_warmer::kick();

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
