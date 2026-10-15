use image::imageops::FilterType;
use std::path::Path;
use tokio::fs;

/// Serve an image with optional resizing, using a cache directory
pub async fn get_or_resize_image(
    source_path: &str,
    max_width: Option<u32>,
    max_height: Option<u32>,
    cache_dir: &Path,
) -> Result<Vec<u8>, String> {
    // If no resize requested, return original
    if max_width.is_none() && max_height.is_none() {
        return read_image_bytes(source_path).await;
    }

    let w = max_width.unwrap_or(0);
    let h = max_height.unwrap_or(0);

    // Build cache key
    let hash = simple_hash(source_path);
    let cache_name = format!("{}_{}_{}x{}.jpg", hash, filename_stem(source_path), w, h);
    let cache_path = cache_dir.join(&cache_name);

    // Return cached version if exists
    if cache_path.exists()
        && let Ok(bytes) = fs::read(&cache_path).await
    {
        return Ok(bytes);
    }

    // Read and resize
    let source_bytes = read_image_bytes(source_path).await?;
    let img = image::load_from_memory(&source_bytes)
        .map_err(|e| format!("failed to decode image: {}", e))?;

    let resized = if w > 0 && h > 0 {
        img.resize(w, h, FilterType::Lanczos3)
    } else if w > 0 {
        let ratio = w as f64 / img.width() as f64;
        let new_h = (img.height() as f64 * ratio) as u32;
        img.resize(w, new_h, FilterType::Lanczos3)
    } else {
        let ratio = h as f64 / img.height() as f64;
        let new_w = (img.width() as f64 * ratio) as u32;
        img.resize(new_w, h, FilterType::Lanczos3)
    };

    // Encode as JPEG
    let mut buf = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| format!("failed to encode image: {}", e))?;
    let bytes = buf.into_inner();

    // Cache
    let _ = fs::create_dir_all(cache_dir).await;
    let _ = fs::write(&cache_path, &bytes).await;

    Ok(bytes)
}

async fn read_image_bytes(source: &str) -> Result<Vec<u8>, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        // Download remote image
        let resp = reqwest::get(source)
            .await
            .map_err(|e| format!("failed to fetch remote image: {}", e))?;
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("failed to read remote image bytes: {}", e))
    } else {
        fs::read(source)
            .await
            .map_err(|e| format!("failed to read local image: {}", e))
    }
}

fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for b in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}

fn filename_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}
