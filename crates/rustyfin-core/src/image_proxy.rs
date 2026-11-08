use image::imageops::FilterType;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::fs;

/// Serve an image with optional resizing, using a cache directory.
/// If `http_client` is provided, it will be reused for remote image downloads,
/// avoiding the overhead of creating a new client per request.
pub async fn get_or_resize_image(
    source_path: &str,
    max_width: Option<u32>,
    max_height: Option<u32>,
    cache_dir: &Path,
    http_client: Option<&reqwest::Client>,
) -> Result<Vec<u8>, String> {
    // If no resize requested, return original
    if max_width.is_none() && max_height.is_none() {
        return read_image_bytes(source_path, http_client).await;
    }

    let w = max_width.unwrap_or(0);
    let h = max_height.unwrap_or(0);

    // The name is a full SHA-256: a short hash let different paths share a file.
    let hash: String = Sha256::digest(source_path.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let cache_name = format!("{}_{}x{}.jpg", hash, w, h);
    let cache_path = cache_dir.join(&cache_name);

    // Return cached version if exists
    if cache_path.exists()
        && let Ok(bytes) = fs::read(&cache_path).await
    {
        return Ok(bytes);
    }

    // Read and resize
    let source_bytes = read_image_bytes(source_path, http_client).await?;
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

async fn read_image_bytes(
    source: &str,
    http_client: Option<&reqwest::Client>,
) -> Result<Vec<u8>, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        // Download remote image with proper headers and status check
        // Reuse provided client if available, otherwise create a new one
        let client = match http_client {
            Some(client) => client,
            None => &reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; RustyFin/1.0)")
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .map_err(|e| format!("failed to create HTTP client: {}", e))?,
        };

        let resp = client
            .get(source)
            .send()
            .await
            .map_err(|e| format!("failed to fetch remote image: {}", e))?;

        // Check for successful HTTP status
        if !resp.status().is_success() {
            return Err(format!(
                "remote image returned status {}: {}",
                resp.status(),
                resp.status().canonical_reason().unwrap_or("unknown")
            ));
        }

        // Check content-type header to verify it's an image
        let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE);
        if let Some(ct) = content_type
            && let Ok(ct_str) = ct.to_str()
            && !ct_str.starts_with("image/")
        {
            return Err(format!(
                "remote image returned non-image content type: {}",
                ct_str
            ));
        }

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

/// Compute a blurhash string for an image file.
/// Returns a short blurhash (8x8 components) suitable for progressive image loading.
/// If `http_client` is provided, it will be reused for remote image downloads.
pub async fn compute_blurhash(
    source_path: &str,
    http_client: Option<&reqwest::Client>,
) -> Option<String> {
    let bytes = read_image_bytes(source_path, http_client).await.ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    // Downscale to a small image for faster encoding
    let small = img.resize(16, 16, FilterType::Triangle);
    let rgba = small.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    let pixels = rgba.as_raw();

    // Use 4x4 components for a short hash
    let hash = blurhash::encode(4, 4, width as u32, height as u32, pixels.as_slice()).ok()?;
    Some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    // B27: cache keys

    /// Removes a test's directory when the test ends, pass or fail.
    struct RemoveOnDrop(std::path::PathBuf);

    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The djb2 hash that named resized images before B27.
    fn djb2(s: &str) -> u64 {
        s.bytes().fold(5381, |hash: u64, b| {
            hash.wrapping_mul(33).wrapping_add(u64::from(b))
        })
    }

    /// Writes a 60x90 JPEG of one colour.
    fn write_poster(path: &Path, rgb: [u8; 3]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(60, 90, image::Rgb(rgb))
            .save(path)
            .unwrap();
    }

    /// The mean colour of an image.
    fn mean_rgb(bytes: &[u8]) -> [u64; 3] {
        let img = image::load_from_memory(bytes).unwrap().to_rgb8();
        let mut sum = [0u64; 3];
        for pixel in img.pixels() {
            for (total, channel) in sum.iter_mut().zip(pixel.0) {
                *total += u64::from(channel);
            }
        }
        let pixels = u64::from(img.width()) * u64::from(img.height());
        sum.map(|total| total / pixels)
    }

    #[tokio::test]
    async fn b27_resized_images_of_paths_that_collide_under_djb2_are_kept_apart() {
        let root = std::env::temp_dir().join(format!("rustyfin_b27_{}", uuid::Uuid::new_v4()));
        let _remove = RemoveOnDrop(root.clone());
        let red = root.join("media").join("ab").join("poster.jpg");
        let blue = root.join("media").join("bA").join("poster.jpg");
        write_poster(&red, [255, 0, 0]);
        write_poster(&blue, [0, 0, 255]);
        let (red, blue) = (red.to_str().unwrap(), blue.to_str().unwrap());
        // "ab" and "bA" add the same to djb2, so the two paths had one name.
        assert_eq!(djb2(red), djb2(blue));
        let cache_dir = root.join("cache");

        let red_small = get_or_resize_image(red, Some(8), None, &cache_dir, None)
            .await
            .unwrap();
        let blue_small = get_or_resize_image(blue, Some(8), None, &cache_dir, None)
            .await
            .unwrap();
        let [r, _, b] = mean_rgb(&red_small);
        assert!(r > 200 && b < 60, "ab's poster came back {:?}", [r, b]);
        let [r, _, b] = mean_rgb(&blue_small);
        assert!(b > 200 && r < 60, "bA's poster came back {:?}", [r, b]);

        // Each has its own file, named by a full hash (64 hex digits) and the size...
        let mut names: Vec<String> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names.len(), 2, "{names:?}");
        for name in &names {
            let (hash, size) = name.split_once('_').unwrap_or((name, ""));
            assert!(
                hash.len() == 64 && hash.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')),
                "{name}"
            );
            assert_eq!(size, "8x0.jpg", "{name}");
        }
        // ...from which each is served again.
        let again = get_or_resize_image(red, Some(8), None, &cache_dir, None).await;
        assert_eq!(again.unwrap(), red_small);
        let again = get_or_resize_image(blue, Some(8), None, &cache_dir, None).await;
        assert_eq!(again.unwrap(), blue_small);
    }
}
