use rustyfin_model::image::{ItemImage, detect_image_type_from_filename};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

/// Scan a single directory for local images, optionally filtering by media file name.
/// When `media_path` is provided, only images whose filenames match the media file are included.
/// This prevents unrelated posters in shared directories from being assigned to wrong items.
fn scan_directory(
    dir: &Path,
    item_id: Uuid,
    type_indices: &mut HashMap<String, i32>,
    media_path: Option<&Path>,
) -> Vec<ItemImage> {
    let mut images = Vec::new();

    if !dir.exists() {
        return images;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return images,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        if let Some(image_type) = detect_image_type_from_filename(filename) {
            // If media_path is provided, only include images that match the media file name
            if let Some(mp) = media_path
                && !image_matches_media(&path, mp)
            {
                continue;
            }

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

/// Check if an image filename matches the media file's stem name.
/// Used when scanning directories to avoid assigning unrelated posters to items.
/// Example: "Saw X.mkv" matches "Saw X-poster.jpg" but not "Mirrors-poster.jpg".
/// Also handles Jellyfin-style naming: "movie.mkv-poster.jpg" matches "movie.mkv".
fn image_matches_media(image_path: &Path, media_path: &Path) -> bool {
    let Some(media_stem) = media_path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(image_stem) = image_path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };

    // Extract the base name from the image, removing common suffixes like "-poster", "-backdrop", etc.
    // Check suffixes directly on the image stem without trimming first.
    let image_base = image_stem
        .strip_suffix("-poster")
        .or_else(|| image_stem.strip_suffix("-backdrop"))
        .or_else(|| image_stem.strip_suffix("-thumb"))
        .or_else(|| image_stem.strip_suffix("-art"))
        .or_else(|| image_stem.strip_suffix("-logo"))
        .or_else(|| image_stem.strip_suffix("-clearart"))
        .or_else(|| image_stem.strip_suffix("-landscape"))
        .or_else(|| image_stem.strip_suffix("-banner"))
        .or_else(|| image_stem.strip_suffix("-disc"))
        .unwrap_or(image_stem);

    // Normalize both strings: lowercase, replace separators with spaces, trim
    let normalize = |s: &str| -> String {
        s.to_lowercase()
            .replace(['.', '_', '-'], " ")
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
    };

    let media_norm = normalize(media_stem);
    let image_norm = normalize(image_base);

    // Exact match is preferred
    if media_norm == image_norm {
        return true;
    }

    // The image name should be contained in the media name or vice versa
    media_norm.contains(&image_norm) || image_norm.contains(&media_norm)
}

/// Scan for local images next to a media file, in its directory, and in parent directories.
/// Priority: same directory > parent directory (for movies in subdirectories) > grandparent.
/// Also checks for Jellyfin-style cache images in common locations.
///
/// All scans match image filenames against the media file name to avoid assigning
/// unrelated posters (e.g., "Saw-poster.jpg" shouldn't apply to "Das.Spiel.mkv").
pub fn find_local_images(media_path: &Path, item_id: Uuid) -> Vec<ItemImage> {
    let mut type_indices: HashMap<String, i32> = HashMap::new();
    let mut images = Vec::new();

    // 1. Same directory as the media file — match by name to avoid shared-directory conflicts
    if let Some(parent) = media_path.parent() {
        images.extend(scan_directory(
            parent,
            item_id,
            &mut type_indices,
            Some(media_path),
        ));
    }

    // 2. Parent directory (for movies in subdirectories like "/path/Movie Name/movie.mkv")
    // Only add images whose filenames match the media file name.
    if let Some(parent) = media_path.parent().and_then(|p| p.parent()) {
        let missing_primary = !type_indices.contains_key("Primary");
        let missing_backdrop = !type_indices.contains_key("Backdrop");

        if missing_primary || missing_backdrop {
            let parent_images =
                scan_directory(parent, item_id, &mut type_indices, Some(media_path));

            for img in parent_images {
                // Skip if we already have this type from same-directory scan
                if type_indices.contains_key(img.image_type.as_str()) {
                    continue;
                }
                type_indices.insert(img.image_type.as_str().to_string(), img.image_index + 1);
                images.push(img);
            }
        }
    }

    // 3. Grandparent directory (for deeper nesting)
    // Same matching rules as parent directory.
    if let Some(grandparent) = media_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    {
        let missing_primary = !type_indices.contains_key("Primary");
        let missing_backdrop = !type_indices.contains_key("Backdrop");

        if missing_primary || missing_backdrop {
            let gp_images =
                scan_directory(grandparent, item_id, &mut type_indices, Some(media_path));

            for img in gp_images {
                if type_indices.contains_key(img.image_type.as_str()) {
                    continue;
                }
                type_indices.insert(img.image_type.as_str().to_string(), img.image_index + 1);
                images.push(img);
            }
        }
    }

    images
}
