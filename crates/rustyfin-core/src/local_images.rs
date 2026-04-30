use rustyfin_model::image::{ItemImage, detect_image_type_from_filename};
use std::path::Path;
use uuid::Uuid;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

/// Scan for local images next to a media file or in its directory
pub fn find_local_images(media_path: &Path, item_id: Uuid) -> Vec<ItemImage> {
    let mut images = Vec::new();
    let parent = match media_path.parent() {
        Some(p) => p,
        None => return images,
    };

    if !parent.exists() {
        return images;
    }

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return images,
    };

    // Track indices per image type
    let mut type_indices: std::collections::HashMap<String, i32> = std::collections::HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        let filename = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        if let Some(image_type) = detect_image_type_from_filename(filename) {
            let type_key = image_type.as_str().to_string();
            let index = type_indices.entry(type_key).or_insert(0);

            images.push(ItemImage {
                item_id,
                image_type,
                image_index: *index,
                path: path.to_string_lossy().to_string(),
                provider: "local".to_string(),
                width: None,
                height: None,
            });

            *index += 1;
        }
    }

    images
}
