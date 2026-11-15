//! Chapter images, trickplay tiles and the chapter re-read, run with the real ffmpeg on a short
//! made-up clip.

use std::path::{Path, PathBuf};
use std::process::Command;

use rustyfin_core::trickplay::Cancel;
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

/// Whether ffmpeg is here. Without it the test says so, as SKIPPED, and checks nothing.
fn ffmpeg_or_skip() -> bool {
    let here = have_ffmpeg();
    if !here {
        let test = std::thread::current().name().unwrap_or("?").to_string();
        eprintln!("SKIPPED {test}: ffmpeg is not on PATH, so this test checked nothing");
    }
    here
}

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A handle that is never cancelled.
fn go() -> Cancel {
    Cancel::new()
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
    if !ffmpeg_or_skip() {
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
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_images");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &clip).await;
    let libs = [lib];

    // Trickplay: one thumbnail every 10 s, 320 wide, in one 10x10 tile.
    let made = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &libs, &go())
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
    let again = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &libs, &go())
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
    let made = rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs, &go())
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
    let again = rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs, &go())
        .await
        .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn r2chapters_nothing_is_made_where_a_library_turns_it_off() {
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_off");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, _lib, item) = movie_db(&dir.0, &clip).await;
    let made = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &[], &go())
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
    if !ffmpeg_or_skip() {
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

    let with_chapters = rustyfin_core::chapter_images::reread_chapters(&pool, &go())
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

/// Adds a movie at `path` to library `lib` and returns its id.
async fn add_movie(pool: &SqlitePool, lib: &str, path: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, name, media_type, path, library_id, runtime_ticks)
         VALUES (?1, 'Other', 'Movie', ?2, ?3, 350000000)",
    )
    .bind(id.to_string())
    .bind(path)
    .bind(lib)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Moves the modified time of the file at `path` an hour on, as a replaced file would have.
fn touch_later(path: &str) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let later = file.metadata().unwrap().modified().unwrap() + std::time::Duration::from_secs(3600);
    file.set_modified(later).unwrap();
}

#[tokio::test]
async fn r2chapters_failed_trickplay_waits_for_the_file_to_change() {
    use rustyfin_core::trickplay::{
        FAILURE_KIND, file_version, generate_attempts, generate_for_libraries,
    };
    use rustyfin_db::repo_trickplay::{get_failure, record_failure};
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_failures");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &clip).await;
    let junk = dir.0.join("junk.mkv");
    std::fs::write(&junk, b"not a video").unwrap();
    let junk = junk.to_string_lossy().into_owned();
    let broken = add_movie(&pool, &lib, &junk).await;
    let libs = [lib];

    // The clip failed before on this very file, so it isn't tried; the junk fails and is recorded.
    record_failure(&pool, item, FAILURE_KIND, file_version(&clip).await)
        .await
        .unwrap();
    assert_eq!(
        generate_for_libraries(&pool, &data, &libs, &go())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        get_failure(&pool, broken, FAILURE_KIND).await.unwrap(),
        Some(file_version(&junk).await)
    );
    assert!(!data.join("trickplay").join(item.to_string()).exists());
    assert_eq!(generate_attempts(&clip), 0, "the clip was skipped");
    assert_eq!(generate_attempts(&junk), 1);

    // Once the clip's file changes it is tried again; the junk, unchanged, still isn't.
    touch_later(&clip);
    assert_eq!(
        generate_for_libraries(&pool, &data, &libs, &go())
            .await
            .unwrap(),
        1
    );
    assert_eq!(get_failure(&pool, item, FAILURE_KIND).await.unwrap(), None);
    assert_eq!(generate_attempts(&clip), 1);
    assert_eq!(
        generate_attempts(&junk),
        1,
        "the unchanged junk was not tried again"
    );

    // A refresh that replaces all images forgets the junk's failure, so it is tried once more.
    rustyfin_db::repo_trickplay::clear_failures(&pool, broken)
        .await
        .unwrap();
    generate_for_libraries(&pool, &data, &libs, &go())
        .await
        .unwrap();
    assert_eq!(generate_attempts(&junk), 2);
}

#[tokio::test]
async fn r2chapters_failed_chapter_images_wait_for_the_file_to_change() {
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_chapter_failures");
    let junk = dir.0.join("junk.mkv");
    std::fs::write(&junk, b"not a video").unwrap();
    let junk = junk.to_string_lossy().into_owned();
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &junk).await;
    let chapter = rustyfin_model::chapter::Chapter {
        id: Uuid::new_v4().to_string(),
        item_id: item.to_string(),
        chapter_number: 1,
        name: "One".to_string(),
        start_position_ticks: 0,
        end_position_ticks: 10_000_000,
        image_path: None,
        image_date_modified: None,
    };
    let save = || rustyfin_db::repo_chapter::save_chapters(&pool, item, vec![chapter.clone()]);
    save().await.unwrap();
    let libs = [lib];
    let never = go();
    let run = || rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs, &never);

    assert_eq!(run().await.unwrap(), 0);
    let missing = || rustyfin_db::repo_chapter::items_missing_chapter_images(&pool, &libs, 10);
    assert!(missing().await.unwrap().is_empty(), "the failure is marked");
    assert_eq!(run().await.unwrap(), 0);

    // A re-probe of a changed file replaces the chapters, so they are tried again.
    save().await.unwrap();
    assert_eq!(missing().await.unwrap().len(), 1);
    assert_eq!(run().await.unwrap(), 0);
    // So does a refresh that replaces all images.
    rustyfin_db::repo_trickplay::clear_failures(&pool, item)
        .await
        .unwrap();
    assert_eq!(missing().await.unwrap().len(), 1);
}

#[tokio::test]
async fn r2chapters_old_tiles_and_chapter_images_are_removed() {
    use rustyfin_core::trickplay::{generate_for_libraries, remove_item_files, sweep_unused_files};
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_cleanup");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &clip).await;
    let libs = [lib];
    let tiles = data.join("trickplay").join(item.to_string());
    let images = data.join("chapters").join(item.to_string());
    let make_all = || async {
        rustyfin_core::probe::probe_and_save(&pool, &item, &clip)
            .await
            .unwrap();
        generate_for_libraries(&pool, &data, &libs, &go())
            .await
            .unwrap();
        rustyfin_core::chapter_images::extract_for_libraries(&pool, &data, &libs, &go())
            .await
            .unwrap();
        assert!(tiles.exists() && images.exists());
    };

    // The same file probed again (a newer probe version, say) keeps them.
    make_all().await;
    rustyfin_core::probe::probe_and_save(&pool, &item, &clip)
        .await
        .unwrap();
    assert_eq!(
        rustyfin_db::repo_trickplay::get_trickplay(&pool, item)
            .await
            .unwrap()
            .len(),
        1
    );
    let kept = rustyfin_db::repo_chapter::get_chapters(&pool, item)
        .await
        .unwrap();
    assert_eq!(kept.len(), 2);
    assert!(
        kept.iter()
            .all(|c| c.image_path.as_deref().is_some_and(|p| !p.is_empty()))
    );
    sweep_unused_files(&pool, &data).await.unwrap();
    assert!(tiles.exists() && images.exists(), "nothing is swept");

    // The file changed and was probed again: its tile set and chapter images are dropped, and
    // the sweep removes their files.
    touch_later(&clip);
    rustyfin_core::probe::probe_and_save(&pool, &item, &clip)
        .await
        .unwrap();
    assert!(
        rustyfin_db::repo_chapter::get_chapters(&pool, item)
            .await
            .unwrap()
            .iter()
            .all(|c| c.image_path.is_none())
    );
    assert!(
        rustyfin_db::repo_trickplay::get_trickplay(&pool, item)
            .await
            .unwrap()
            .is_empty()
    );
    sweep_unused_files(&pool, &data).await.unwrap();
    assert!(!tiles.exists());
    assert!(!images.exists());

    // The item is deleted: its rows go with it, and the sweep removes its files.
    make_all().await;
    sqlx::query("DELETE FROM media_items WHERE id = ?1")
        .bind(item.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sweep_unused_files(&pool, &data).await.unwrap();
    assert!(!tiles.exists() && !images.exists());

    // Deleting through the API removes them at once.
    std::fs::create_dir_all(&tiles).unwrap();
    std::fs::create_dir_all(&images).unwrap();
    remove_item_files(&data, item).await;
    assert!(!tiles.exists() && !images.exists());
}

#[tokio::test]
async fn r2chapters_a_cancelled_run_makes_and_records_nothing() {
    if !ffmpeg_or_skip() {
        return;
    }
    let dir = TempDir::new("r2chapters_cancel");
    let clip = make_clip(&dir.0);
    let data = dir.0.join("data");
    let (pool, lib, item) = movie_db(&dir.0, &clip).await;
    let stopped = Cancel::new();
    stopped.cancel();
    let made = rustyfin_core::trickplay::generate_for_libraries(&pool, &data, &[lib], &stopped)
        .await
        .unwrap();
    assert_eq!(made, 0);
    let failure = rustyfin_db::repo_trickplay::get_failure(
        &pool,
        item,
        rustyfin_core::trickplay::FAILURE_KIND,
    )
    .await
    .unwrap();
    assert_eq!(failure, None, "a stop is not a failure");
}
