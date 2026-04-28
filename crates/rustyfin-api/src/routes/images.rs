use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
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
        .route("/Items/{id}/Images/{image_type}", get(get_item_image).post(upload_item_image))
        .route("/Items/{id}/Refresh", post(refresh_item))
}

async fn get_item_images(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ImageInfo>>, ApiError> {
    let images = rustyfin_db::repo_image::get_images_for_item(&state.db, id)
        .await?;

    let infos: Vec<ImageInfo> = images
        .into_iter()
        .map(|img| ImageInfo {
            image_type: img.image_type,
            image_index: img.image_index,
            image_tag: format!("{}_{}", img.image_type.as_str(), img.image_index),
            path: img.path,
            width: img.width,
            height: img.height,
        })
        .collect();

    Ok(Json(infos))
}

#[derive(Deserialize)]
struct ImageQuery {
    #[serde(rename = "maxWidth")]
    max_width: Option<u32>,
    #[serde(rename = "maxHeight")]
    max_height: Option<u32>,
    #[serde(rename = "imageIndex")]
    image_index: Option<i32>,
}

async fn get_item_image(
    State(state): State<AppState>,
    Path((id, image_type_str)): Path<(Uuid, String)>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    let image_type = ImageType::from_str(&image_type_str)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown image type: {}", image_type_str)))?;

    let index = query.image_index.unwrap_or(0);
    let img = rustyfin_db::repo_image::get_image(&state.db, id, &image_type, index)
        .await?
        .ok_or_else(|| ApiError::NotFound("image not found".into()))?;

    let cache_dir = state.data_dir.join("cache").join("images");
    let bytes = rustyfin_core::image_proxy::get_or_resize_image(
        &img.path,
        query.max_width,
        query.max_height,
        &cache_dir,
    )
    .await
    .map_err(|e| ApiError::Internal(e))?;

    let content_type = guess_content_type(&img.path, query.max_width.is_some() || query.max_height.is_some());

    Ok(Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(bytes))
        .unwrap())
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

    let img = ItemImage {
        item_id: id,
        image_type,
        image_index: 0,
        path: file_path.to_string_lossy().to_string(),
        provider: "upload".to_string(),
        width: None,
        height: None,
    };

    rustyfin_db::repo_image::upsert_image(&state.db, &img).await?;

    Ok(Json(json!({ "status": "ok" })))
}

async fn refresh_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;

    let config = state.config.read().await;
    let tmdb_key = config.tmdb_api_key.as_deref();
    let omdb_key = config.omdb_api_key.as_deref();

    rustyfin_core::metadata::refresh_item_metadata(&state.db, &item, tmdb_key, omdb_key)
        .await
        .map_err(|e| ApiError::Internal(e))?;

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
    } else if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        "webp"
    } else {
        "jpg"
    }
}
