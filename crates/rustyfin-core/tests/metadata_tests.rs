//! Metadata refresh (B31): a refresh saves the item in place, so the rows that reference it
//! survive, it keeps its id and date added, and search and the filter counts follow it.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use uuid::Uuid;

/// An NFO that renames a movie and fills its plot and genre.
const NFO: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<movie>
  <title>New Title</title>
  <plot>A plot from the NFO.</plot>
  <year>2001</year>
  <mpaa>R</mpaa>
  <genre>Drama</genre>
</movie>
";

/// The tables whose rows for an item `ON DELETE CASCADE` removes, and how to count them.
const ITEM_ROWS: [(&str, &str); 8] = [
    (
        "media_sources",
        "SELECT COUNT(*) FROM media_sources WHERE item_id = ?1",
    ),
    (
        "item_images",
        "SELECT COUNT(*) FROM item_images WHERE item_id = ?1",
    ),
    (
        "chapters",
        "SELECT COUNT(*) FROM chapters WHERE item_id = ?1",
    ),
    (
        "trickplay",
        "SELECT COUNT(*) FROM trickplay WHERE item_id = ?1",
    ),
    (
        "playlist_items",
        "SELECT COUNT(*) FROM playlist_items WHERE item_id = ?1",
    ),
    (
        "collection_items",
        "SELECT COUNT(*) FROM collection_items WHERE item_id = ?1",
    ),
    (
        "media_segments",
        "SELECT COUNT(*) FROM media_segments WHERE item_id = ?1",
    ),
    (
        "live_tv_recordings",
        "SELECT COUNT(*) FROM live_tv_recordings WHERE media_item_id = ?1",
    ),
];

/// A database and a movie library in a temporary directory that is removed on drop.
struct Fixture {
    pool: SqlitePool,
    dir: PathBuf,
    library_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_meta_{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("film")).unwrap();
        let db = dir.join("test.db");
        let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
        let library = Library {
            id: Uuid::new_v4(),
            name: "Film".to_string(),
            content_type: LibraryContentType::Movies,
            paths: vec![dir.join("film").to_string_lossy().into_owned()],
        };
        rustyfin_db::repo_library::create_library(&pool, &library)
            .await
            .unwrap();
        Self {
            pool,
            dir,
            library_id: library.id,
        }
    }

    /// Stores a movie from 2001 rated R, added on 2 January 2020, under `name`. Writes `nfo`
    /// beside its file when given.
    async fn movie(&self, name: &str, nfo: Option<&str>) -> MediaItem {
        let film = self.dir.join("film");
        if let Some(nfo) = nfo {
            std::fs::write(film.join(format!("{name}.nfo")), nfo).unwrap();
        }
        let item = MediaItem {
            id: Uuid::new_v4(),
            name: name.to_string(),
            sort_name: Some(name.to_lowercase()),
            media_type: MediaType::Movie,
            path: film
                .join(format!("{name}.mkv"))
                .to_string_lossy()
                .into_owned(),
            library_id: self.library_id,
            production_year: Some(2001),
            official_rating: Some("R".to_string()),
            date_created: Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap(),
            ..MediaItem::default_dto_fields()
        };
        rustyfin_db::repo_media::upsert_media_item(&self.pool, &item)
            .await
            .unwrap();
        item
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn exec(pool: &SqlitePool, sql: &'static str, binds: &[&str]) {
    let mut query = sqlx::query(sql);
    for b in binds {
        query = query.bind(b.to_string());
    }
    query
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Gives `item` one row in each of the `ITEM_ROWS` tables.
async fn add_item_rows(pool: &SqlitePool, item: Uuid) {
    let item = item.to_string();
    let new_id = || Uuid::new_v4().to_string();
    let (user, set, list, channel) = (new_id(), new_id(), new_id(), new_id());
    exec(
        pool,
        "INSERT INTO users (id, name, password_hash) VALUES (?1, 'B31 user', 'x')",
        &[&user],
    )
    .await;
    exec(
        pool,
        "INSERT INTO collections (id, name, user_id) VALUES (?1, 'Set', ?2)",
        &[&set, &user],
    )
    .await;
    exec(
        pool,
        "INSERT INTO playlists (id, name, user_id) VALUES (?1, 'List', ?2)",
        &[&list, &user],
    )
    .await;
    exec(
        pool,
        "INSERT INTO live_tv_channels (id, name) VALUES (?1, 'B31 channel')",
        &[&channel],
    )
    .await;
    exec(
        pool,
        "INSERT INTO media_sources (id, item_id, uid, path) VALUES (?1, ?2, ?1, '/film/a.mkv')",
        &[&new_id(), &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO item_images (item_id, image_type, path) VALUES (?1, 'Primary', '/a.jpg')",
        &[&item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO chapters (id, item_id, chapter_number, start_ticks, end_ticks)
         VALUES (?1, ?2, 1, 0, 600000000)",
        &[&new_id(), &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO trickplay
         (id, item_id, width, height, interval_ticks, thumbnail_count, directory_path)
         VALUES (?1, ?2, 320, 180, 100000000, 10, '/trickplay')",
        &[&new_id(), &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?1, ?2, 1)",
        &[&list, &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO collection_items (collection_id, item_id, sort_order) VALUES (?1, ?2, 1)",
        &[&set, &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO media_segments
         (id, item_id, segment_type, segment_start_ticks, segment_end_ticks)
         VALUES (?1, ?2, 'Intro', 0, 300000000)",
        &[&new_id(), &item],
    )
    .await;
    exec(
        pool,
        "INSERT INTO live_tv_recordings (id, media_item_id, channel_id, name)
         VALUES (?1, ?2, ?3, 'B31 recording')",
        &[&new_id(), &item, &channel],
    )
    .await;
}

/// Counts `item`'s rows in each of the `ITEM_ROWS` tables.
async fn item_rows(pool: &SqlitePool, item: Uuid) -> Vec<(&'static str, i64)> {
    let mut rows = Vec::new();
    for (table, sql) in ITEM_ROWS {
        let n: i64 = sqlx::query_scalar(sql)
            .bind(item.to_string())
            .fetch_one(pool)
            .await
            .unwrap();
        rows.push((table, n));
    }
    rows
}

/// The names search finds for `query`.
async fn search(pool: &SqlitePool, query: &str) -> Vec<String> {
    rustyfin_db::repo_search::search_items(pool, query, &[], 10)
        .await
        .unwrap()
        .into_iter()
        .map(|item| item.name)
        .collect()
}

/// How many entries in the search index match `query`, whether or not an item still has them.
async fn index_entries(pool: &SqlitePool, query: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM media_items_fts WHERE media_items_fts MATCH ?1")
        .bind(query)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn rowid(pool: &SqlitePool, item: Uuid) -> i64 {
    sqlx::query_scalar("SELECT rowid FROM media_items WHERE id = ?1")
        .bind(item.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn refresh(pool: &SqlitePool, item: &MediaItem) {
    rustyfin_core::metadata::refresh_item_metadata(
        pool,
        &reqwest::Client::new(),
        item,
        None,
        None,
        None,
    )
    .await
    .unwrap();
}

/// A refresh that changes an item keeps the rows that reference it. It used to save with
/// `INSERT OR REPLACE`, whose delete cascaded to all of them.
#[tokio::test]
async fn refresh_keeps_rows_that_reference_the_item() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let stored = fx.movie("Old.Title", Some(NFO)).await;
    add_item_rows(pool, stored.id).await;
    let before = item_rows(pool, stored.id).await;
    assert!(before.iter().all(|&(_, n)| n == 1), "{before:?}");

    // The refresh saves the copy it was given, which can be older than the stored item.
    let copy = MediaItem {
        date_created: Utc::now(),
        ..stored.clone()
    };
    refresh(pool, &copy).await;

    let after = rustyfin_db::repo_media::get_item_by_id(pool, stored.id)
        .await
        .unwrap()
        .expect("the item is still stored under its id");
    assert_eq!(after.name, "New Title", "the refresh applied the NFO");
    assert_eq!(after.date_created, stored.date_created);
    assert_eq!(item_rows(pool, stored.id).await, before);
}

/// A refresh that renames an item keeps its rowid, which the search index is keyed on, so search
/// finds the new name once and the old one not at all, and the year and rating counts stay right.
/// `INSERT OR REPLACE` gave the item a new rowid and left its old entry in the index.
#[tokio::test]
async fn refresh_renames_the_item_in_search() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let stored = fx.movie("Old.Title", Some(NFO)).await;
    fx.movie("Other", None).await;
    let before = rowid(pool, stored.id).await;

    refresh(pool, &stored).await;

    assert_eq!(search(pool, "New").await, ["New Title"]);
    assert_eq!(search(pool, "Old").await, Vec::<String>::new());
    assert_eq!(index_entries(pool, "new*").await, 1);
    assert_eq!(
        index_entries(pool, "old*").await,
        0,
        "the old name has no entry left"
    );
    assert_eq!(
        rowid(pool, stored.id).await,
        before,
        "the item keeps its rowid"
    );
    let years: Vec<(i64, i64)> = sqlx::query_as("SELECT year, item_count FROM filter_year_index")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(years, [(2001, 2)]);
    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT rating, item_count FROM filter_rating_index")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(ratings, [("R".to_string(), 2)]);
}
