use crate::image_proxy::read_image_stamp;
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

            let (stamp, width, height) = local_image_version(&path);
            images.push(ItemImage {
                item_id,
                image_type,
                image_index: *index,
                path: path.to_string_lossy().to_string(),
                provider: "local".to_string(),
                width,
                height,
                blurhash: None,
                stamp,
            });

            *index += 1;
        }
    }

    images
}

/// The stamp, width and height that a local image's row stores, read from its file. They are all
/// `None` when the file's metadata cannot be read, and the size is `None` when its header cannot.
pub fn local_image_version(path: &Path) -> (Option<i64>, Option<i32>, Option<i32>) {
    let Ok(read) = read_image_stamp(path) else {
        return (None, None, None);
    };
    let (width, height) = match read.size {
        Some((w, h)) => (i32::try_from(w).ok(), i32::try_from(h).ok()),
        None => (None, None),
    };
    (Some(read.stamp), width, height)
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

/// Scan a folder item's own directory (Series, Season, MusicAlbum) for exactly-named artwork
/// such as `folder.jpg`, `poster.png` or `fanart.jpg`. Keyword-only names like
/// `season01-poster.jpg` are not taken. The names mirror the exact-name arm of
/// `detect_image_type_from_filename`. Files are sorted by name, so the indices do not depend
/// on `read_dir` order.
fn scan_folder_item_images(
    dir: &Path,
    item_id: Uuid,
    type_indices: &mut HashMap<String, i32>,
) -> Vec<ItemImage> {
    const EXACT_NAMES: &[&str] = &[
        "poster",
        "cover",
        "folder",
        "default",
        "cover_art",
        "fanart",
        "backdrop",
        "background",
        "artwork",
        "banner",
        "thumb",
        "thumbnail",
        "landscape",
        "logo",
        "clearlogo",
        "clearart",
        "art",
        "disc",
        "discart",
        "cdart",
    ];

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut images = Vec::new();
    for path in files {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        let Some(filename) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        let lower = filename.to_lowercase();
        let stem = lower.split('.').next().unwrap_or("");
        if !EXACT_NAMES.contains(&stem) {
            continue;
        }
        let Some(image_type) = detect_image_type_from_filename(filename) else {
            continue;
        };

        let index = type_indices
            .entry(image_type.as_str().to_string())
            .or_insert(0);
        let (stamp, width, height) = local_image_version(&path);
        images.push(ItemImage {
            item_id,
            image_type,
            image_index: *index,
            path: path.to_string_lossy().to_string(),
            provider: "local".to_string(),
            width,
            height,
            blurhash: None,
            stamp,
        });
        *index += 1;
    }

    images
}

/// Scan for local images next to a media file, in its directory, and in parent directories.
/// Priority: a folder item's own folder > same directory > parent directory (for movies in
/// subdirectories) > grandparent.
/// Also checks for Jellyfin-style cache images in common locations.
///
/// All scans except a folder item's own folder match image filenames against the media
/// file name to avoid assigning unrelated posters (e.g., "Saw-poster.jpg" shouldn't apply
/// to "Das.Spiel.mkv").
pub fn find_local_images(media_path: &Path, item_id: Uuid) -> Vec<ItemImage> {
    let mut type_indices: HashMap<String, i32> = HashMap::new();
    let mut images = Vec::new();

    // 0. Folder items (Series, Season, MusicAlbum): the folder's own artwork comes first
    if media_path.is_dir() {
        images.extend(scan_folder_item_images(
            media_path,
            item_id,
            &mut type_indices,
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyfin_model::image::ImageType;
    use std::path::PathBuf;

    /// Temp dirs this test made. libtest runs each test on its own thread, so dropping this
    /// thread-local removes them when the test ends, also when it panics.
    struct TempDirs(std::cell::RefCell<Vec<std::path::PathBuf>>);

    impl Drop for TempDirs {
        fn drop(&mut self) {
            for dir in self.0.get_mut().drain(..) {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    thread_local! {
        static TEMP_DIRS: TempDirs = const { TempDirs(std::cell::RefCell::new(Vec::new())) };
    }

    /// Removes `dir` and everything in it when the current test ends.
    fn remove_when_test_ends(dir: &std::path::Path) {
        TEMP_DIRS.with(|dirs| dirs.0.borrow_mut().push(dir.to_path_buf()));
    }

    /// Creates empty `files` (paths relative to a fresh temp dir) and returns `<temp dir>/media`.
    /// Every test path sits at least three levels below the temp dir, so the parent and
    /// grandparent scans never reach the shared system temp directory.
    fn media_tree(files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("rustyfin_local_images_{}", Uuid::new_v4()))
            .join("media");
        remove_when_test_ends(root.parent().unwrap());
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"").unwrap();
        }
        root
    }

    fn paths(images: &[ItemImage], image_type: ImageType) -> Vec<String> {
        images
            .iter()
            .filter(|img| img.image_type == image_type)
            .map(|img| img.path.clone())
            .collect()
    }

    fn path_string(path: PathBuf) -> String {
        path.to_string_lossy().to_string()
    }

    #[test]
    fn test_series_folder_jpg_is_primary() {
        let media = media_tree(&["TV/Show/folder.jpg", "TV/Show/Season 1/Show - S01E01.mkv"]);
        let series = media.join("TV/Show");

        let images = find_local_images(&series, Uuid::new_v4());

        assert_eq!(
            paths(&images, ImageType::Primary),
            vec![path_string(series.join("folder.jpg"))]
        );
    }

    #[test]
    fn test_series_poster_and_fanart() {
        let media = media_tree(&["TV/Show/poster.jpg", "TV/Show/fanart.jpg"]);
        let series = media.join("TV/Show");

        let images = find_local_images(&series, Uuid::new_v4());

        assert_eq!(
            paths(&images, ImageType::Primary),
            vec![path_string(series.join("poster.jpg"))]
        );
        assert_eq!(
            paths(&images, ImageType::Backdrop),
            vec![path_string(series.join("fanart.jpg"))]
        );
    }

    #[test]
    fn test_season_poster_in_series_folder_is_not_series_primary() {
        let media = media_tree(&[
            "TV/Show/season01-poster.jpg",
            "TV/Show/Season 1/Show - S01E01.mkv",
        ]);

        let images = find_local_images(&media.join("TV/Show"), Uuid::new_v4());

        assert!(paths(&images, ImageType::Primary).is_empty(), "{images:?}");
    }

    #[test]
    fn test_season_folder_image_is_primary_case_insensitive() {
        let media = media_tree(&[
            "TV/Show/Season 1/Folder.JPG",
            "TV/Show/Season 1/Show - S01E01.mkv",
        ]);
        let season = media.join("TV/Show/Season 1");

        let images = find_local_images(&season, Uuid::new_v4());

        assert_eq!(
            paths(&images, ImageType::Primary),
            vec![path_string(season.join("Folder.JPG"))]
        );
    }

    #[test]
    fn test_episode_does_not_take_season_folder_image() {
        let media = media_tree(&[
            "TV/Show/Season 1/folder.jpg",
            "TV/Show/Season 1/Show - S01E01.mkv",
        ]);
        let episode = media.join("TV/Show/Season 1/Show - S01E01.mkv");

        let images = find_local_images(&episode, Uuid::new_v4());

        assert!(images.is_empty(), "{images:?}");
    }

    #[test]
    fn test_movie_named_poster_is_primary() {
        let media = media_tree(&[
            "Movies/Film (2000)/Film (2000).mkv",
            "Movies/Film (2000)/Film (2000)-poster.jpg",
        ]);
        let movie_dir = media.join("Movies/Film (2000)");

        let images = find_local_images(&movie_dir.join("Film (2000).mkv"), Uuid::new_v4());

        assert_eq!(
            paths(&images, ImageType::Primary),
            vec![path_string(movie_dir.join("Film (2000)-poster.jpg"))]
        );
    }
}
