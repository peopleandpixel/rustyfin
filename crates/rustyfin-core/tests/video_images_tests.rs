//! Chapter images, trickplay tiles and the chapter re-read, run with the real ffmpeg on a short
//! made-up clip.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustyfin_model::library::{Library, LibraryContentType, LibraryOptions};
use sqlx::SqlitePool;
use uuid::Uuid;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_{label}_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A 35-second 640x360 clip with two chapters: "Opening" at 0 s and an untitled one at 20 s.
fn make_clip(dir: &Path) -> String {
    let meta = dir.join("chapters.txt");
    std::fs::write(
        &meta,
        ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=20000\ntitle=Opening\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=20000\nEND=35000\n",
    )
    .unwrap();
    let clip = dir.join("clip.mkv");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=35:size=640x360:rate=10")
        .arg("-i")
        .arg(&meta)
        .args([
            "-map_metadata",
            "1",
            "-map_chapters",
            "1",
            "-c:v",
            "mpeg4",
            "-g",
            "50",
        ])
        .arg(&clip)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg could not write the clip");
    clip.to_string_lossy().into_owned()
}

/// A database with a library that turns chapter images and trickplay on, holding the clip as a
/// movie. Returns the pool, the library's id and the movie's id.
async fn movie_db(dir: &Path, clip: &str) -> (SqlitePool, String, Uuid) {
    let pool = rustyfin_db::init_db(dir.join("test.db").to_str().unwrap())
        .await
        .unwrap();
    let lib = Library {
        id: Uuid::new_v4(),
        name: "Films".to_string(),
        content_type: LibraryContentType::Movies,
        paths: vec![dir.to_str().unwrap().to_string()],
    };
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let options = LibraryOptions {
        enable_chapter_extraction: true,
        enable_trickplay: true,
        ..LibraryOptions::default()
    };
    rustyfin_db::repo_library::set_library_options(&pool, lib.id, &options)
        .await
        .unwrap();
    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
         VALUES (?1, 'Clip', 'Movie', ?2, ?3, 350000000)",
    )
    .bind(item_id.to_string())
    .bind(clip)
    .bind(lib.id.to_string())
    .execute(&pool)
    .await
    .unwrap();
    (pool, lib.id.to_string(), item_id)
}

#[tokio::test]
async fn r2chapters_probe_reads_the_clips_chapters() {
    if !have_ffmpeg() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = TempDir::new("r2chapters_probe");
    let clip = make_clip(&dir.0);
    let probed = rustyfin_core::probe::probe_file(&clip).await.unwrap();
    let got: Vec<_> = probed
        .chapters
        .iter()
        .map(|c| (c.name.as_str(), c.start_position_ticks))
        .collect();
    assert_eq!(got, [("Opening", 0), ("Chapter 2", 200_000_000)]);
}

#[tokio::test]
async fn r2chapters_trickplay_and_chapter_images_are_made_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = TempDir::new("r2chapters_images");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &clip).await;
    let libs = [lib];

    // Trickplay: one thumbnail every 10 s, 320 wide, in one 10x10 tile.
    let made = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &libs)
        .await
        .unwrap();
    assert_eq!(made, 1);
    let sets = rustyfin_db::repo_trickplay::get_trickplay(&pool, item)
        .await
        .unwrap();
    assert_eq!(sets.len(), 1);
    let set = &sets[0];
    assert_eq!((set.width, set.height), (320, 180));
    assert_eq!(
        (set.tile_width, set.tile_height, set.interval),
        (10, 10, 10_000)
    );
    assert!((3..=4).contains(&set.thumbnail_count), "{set:?}");
    assert!(set.bandwidth > 0);
    let tile = rustyfin_core::trickplay::tile_path(&data, item, set, 0);
    let (w, h) = image::image_dimensions(&tile).unwrap();
    assert_eq!((w, h), (3200, 1800), "a full 10x10 tile");
    // Nothing is left over from the work, and a second run has nothing to do.
    let item_dir = data.join("trickplay").join(item.to_string());
    let left: Vec<_> = std::fs::read_dir(&item_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["320 - 10x10"]);
    let again = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &libs)
        .await
        .unwrap();
    assert_eq!(again, 0);

    // Chapter images: one per chapter, recorded, and made once.
    let probed = rustyfin_core::probe::probe_file(&clip).await.unwrap();
    let mut chapters = probed.chapters;
    for c in &mut chapters {
        c.item_id = item.to_string();
    }
    rustyfin_db::repo_chapter::save_chapters(&pool, item, chapters)
        .await
        .unwrap();
    let made = rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs)
        .await
        .unwrap();
    assert_eq!(made, 2);
    let stored = rustyfin_db::repo_chapter::get_chapters(&pool, item)
        .await
        .unwrap();
    for c in &stored {
        let path = c.image_path.as_deref().unwrap();
        assert_eq!(
            Path::new(path),
            rustyfin_core::chapter_images::image_path(&data, item, c.start_position_ticks)
        );
        assert_eq!(image::image_dimensions(path).unwrap(), (640, 360));
        assert!(c.image_date_modified.is_some());
    }
    let again = rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs)
        .await
        .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn r2chapters_nothing_is_made_where_a_library_turns_it_off() {
    if !have_ffmpeg() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = TempDir::new("r2chapters_off");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, _lib, item) = movie_db(&dir.0, &clip).await;
    let made = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &[])
        .await
        .unwrap();
    assert_eq!(made, 0);
    assert!(
        rustyfin_db::repo_trickplay::get_trickplay(&pool, item)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!data.exists(), "no ffmpeg ran");
}

#[tokio::test]
async fn r2chapters_reread_fills_chapters_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: no ffmpeg");
        return;
    }
    let dir = TempDir::new("r2chapters_reread");
    let clip = make_clip(&dir.0);
    let (pool, _lib, item) = movie_db(&dir.0, &clip).await;
    sqlx::query("INSERT INTO chapter_rereads (item_id) VALUES (?1)")
        .bind(item.to_string())
        .execute(&pool)
        .await
        .unwrap();

    let with_chapters = rustyfin_core::chapter_images::reread_chapters(&pool)
        .await
        .unwrap();
    assert_eq!(with_chapters, 1);
    let names: Vec<_> = rustyfin_db::repo_chapter::get_chapters(&pool, item)
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["Opening", "Chapter 2"]);
    assert!(
        rustyfin_db::repo_chapter::pending_chapter_rereads(&pool, 10)
            .await
            .unwrap()
            .is_empty()
    );
}
