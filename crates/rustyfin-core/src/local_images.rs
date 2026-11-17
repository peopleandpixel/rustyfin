use crate::image_proxy::read_image_stamp;
use rustyfin_model::image::{ItemImage, detect_image_type_from_filename};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use uuid::Uuid;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff"];

/// The directory that images extracted from media files (embedded cover art) are written to. Set once by
/// the server at start-up; unset means no extraction.
static EMBEDDED_IMAGES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Sets the directory for extracted images. A second call is ignored.
pub fn set_embedded_images_dir(dir: PathBuf) {
    let _ = EMBEDDED_IMAGES_DIR.set(dir);
}

/// The directory for extracted images, if one was set.
pub fn embedded_images_dir() -> Option<&'static Path> {
    EMBEDDED_IMAGES_DIR.get().map(PathBuf::as_path)
}

/// The time limit for extracting one cover. A longer run is killed.
const COVER_EXTRACT_LIMIT: Duration = Duration::from_secs(30);

/// The file extension for a picture with stream codec `codec`, or `None` when the codec is not a known
/// image format.
pub fn cover_extension(codec: &str) -> Option<&'static str> {
    match codec {
        "mjpeg" => Some("jpg"),
        "png" => Some("png"),
        "webp" => Some("webp"),
        "bmp" => Some("bmp"),
        "gif" => Some("gif"),
        _ => None,
    }
}

/// Runs `cmd` and waits for it for at most `limit`. A program still running after that is killed, and the
/// call fails with `ErrorKind::TimedOut`. The child is also killed if this future is dropped.
pub async fn run_bounded(
    mut cmd: tokio::process::Command,
    limit: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    match tokio::time::timeout(limit, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            // The kill is best effort; the child may already have exited.
            let _ = child.kill().await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("the program did not finish within {limit:?}"),
            ))
        }
    }
}

/// The extracted covers (files under `embedded_images_dir()`) that the items of `library_id` refer to in
/// `item_images`. Taken before items are deleted, so that [`remove_orphaned_covers`] can remove the files
/// of the items that go. A query failure is logged and yields no paths.
pub async fn embedded_cover_paths(pool: &SqlitePool, library_id: Uuid) -> Vec<PathBuf> {
    let Some(dir) = embedded_images_dir() else {
        return Vec::new();
    };
    let paths: Result<Vec<String>, sqlx::Error> = sqlx::query_scalar(
        "SELECT DISTINCT i.path FROM item_images i JOIN media_items m ON m.id = i.item_id
         WHERE m.library_id = ?",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await;
    match paths {
        Ok(paths) => paths
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| path.starts_with(dir))
            .collect(),
        Err(e) => {
            tracing::warn!("could not list the embedded covers of library {library_id}: {e}");
            Vec::new()
        }
    }
}

/// Removes each file of `paths` that no `item_images` row refers to any more, that is, the covers of
/// items deleted since [`embedded_cover_paths`] was taken. Failures are logged and do not stop the rest.
pub async fn remove_orphaned_covers(pool: &SqlitePool, paths: &[PathBuf]) {
    let Some(dir) = embedded_images_dir() else {
        return;
    };
    if paths.is_empty() {
        return;
    }
    // One query finds every cover in the embedded directory that an item still refers to. A failed
    // query removes nothing, since the referenced set is then unknown.
    let prefix = format!("{}{}", dir.to_string_lossy(), std::path::MAIN_SEPARATOR);
    let pattern = format!("{}%", escape_like(&prefix));
    let referenced: Result<Vec<String>, sqlx::Error> =
        sqlx::query_scalar("SELECT path FROM item_images WHERE path LIKE ? ESCAPE '\\'")
            .bind(pattern)
            .fetch_all(pool)
            .await;
    let referenced: std::collections::HashSet<String> = match referenced {
        Ok(paths) => paths.into_iter().collect(),
        Err(e) => {
            tracing::warn!("could not list the referenced covers: {}", e);
            return;
        }
    };
    for path in paths {
        if referenced.contains(path.to_string_lossy().as_ref()) {
            continue;
        }
        if let Err(e) = tokio::fs::remove_file(path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "could not remove the orphaned cover {}: {}",
                path.display(),
                e
            );
        }
    }
}

/// Escapes `%`, `_` and `\` so that `text` matches only itself in a `LIKE ... ESCAPE '\'` pattern.
fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Extracts the attached picture of `track` (with stream codec `codec`) to `<embedded dir>/<album id>.<ext>`
/// and returns it as the album's Primary image. Returns `None`, logging a warning, on any failure. Nothing is
/// written when no embedded directory is set.
pub async fn extract_embedded_cover(
    track: &Path,
    album_id: Uuid,
    codec: &str,
) -> Option<ItemImage> {
    let dir = embedded_images_dir()?;
    // A picture in a codec with no known image extension is not saved.
    let ext = cover_extension(codec)?;
    let out = dir.join(format!("{album_id}.{ext}"));
    let result = async {
        tokio::fs::create_dir_all(dir).await?;
        let mut cmd = tokio::process::Command::new(crate::media_tools::ffmpeg());
        cmd.args(["-nostdin", "-loglevel", "error", "-y", "-i"])
            .arg(track)
            .args(["-an", "-map", "0:v:0", "-c", "copy", "-frames:v", "1"])
            .args(["-fs", "20000000"])
            .arg(&out);
        let status = run_bounded(cmd, COVER_EXTRACT_LIMIT).await?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "ffmpeg exited with {status}"
            )));
        }
        Ok::<(), std::io::Error>(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(
            "could not extract the cover of {} to {}: {}",
            track.display(),
            out.display(),
            e
        );
        return None;
    }
    let (stamp, width, height) = local_image_version(&out);
    Some(ItemImage {
        item_id: album_id,
        image_type: rustyfin_model::image::ImageType::Primary,
        image_index: 0,
        path: out.to_string_lossy().to_string(),
        provider: "local".to_string(),
        width,
        height,
        blurhash: None,
        stamp,
    })
}

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
