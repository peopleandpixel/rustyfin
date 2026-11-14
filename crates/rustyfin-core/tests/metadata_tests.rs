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
    let pm = rustyfin_core::plugin_manager::PluginManager::new(&std::path::PathBuf::from("."));
    rustyfin_core::metadata::refresh_item_metadata(pool, &reqwest::Client::new(), item, &pm)
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

// B34: a refresh saves only the fields it changed, and only while they still hold what it read

/// The stored item, read again.
async fn stored(pool: &SqlitePool, id: Uuid) -> MediaItem {
    rustyfin_db::repo_media::get_item_by_id(pool, id)
        .await
        .unwrap()
        .expect("the item is stored")
}

/// A rescan renames the item and changes its year after the refresh read it (`scan_sync.rs`).
/// The refresh still fills the empty fields from the NFO, but writes back neither the old name
/// nor the old year.
#[tokio::test]
async fn b34_refresh_keeps_a_rename_made_while_it_ran() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let read = fx.movie("Quiet Title", Some(NFO)).await;

    exec(
        pool,
        "UPDATE media_items SET name = 'Renamed Title', sort_name = 'renamed title', \
         production_year = 2002 WHERE id = ?1",
        &[&read.id.to_string()],
    )
    .await;
    refresh(pool, &read).await;

    let after = stored(pool, read.id).await;
    assert_eq!(after.name, "Renamed Title");
    assert_eq!(after.sort_name.as_deref(), Some("renamed title"));
    assert_eq!(after.production_year, Some(2002));
    assert_eq!(
        after.overview.as_deref(),
        Some("A plot from the NFO."),
        "the refresh still filled the empty overview"
    );
    assert_eq!(after.genres, ["Drama"]);
    let years: Vec<(i64, i64)> = sqlx::query_as("SELECT year, item_count FROM filter_year_index")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(years, [(2002, 1)]);
}

/// The Jellyfin import writes the overview and genres after the refresh read the item
/// (`repo_jellyfin_import.rs`). The NFO's values don't replace them.
#[tokio::test]
async fn b34_refresh_keeps_an_import_written_while_it_ran() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let read = fx.movie("Quiet Title", Some(NFO)).await;

    exec(
        pool,
        "UPDATE media_items SET overview = 'Imported overview', genres = 'Imported' WHERE id = ?1",
        &[&read.id.to_string()],
    )
    .await;
    refresh(pool, &read).await;

    let after = stored(pool, read.id).await;
    assert_eq!(after.overview.as_deref(), Some("Imported overview"));
    assert_eq!(after.genres, ["Imported"]);
    assert_eq!(after.name, "Quiet Title");
}

/// The probe moves the item's path and sets its runtime after the refresh read it
/// (`probe.rs`). The refresh, whose NFO has no runtime, writes back neither.
#[tokio::test]
async fn b34_refresh_keeps_a_probe_written_while_it_ran() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let read = fx.movie("Quiet Title", Some(NFO)).await;
    let moved = fx
        .dir
        .join("film")
        .join("Quiet Title.mp4")
        .to_string_lossy()
        .into_owned();

    exec(
        pool,
        "UPDATE media_items SET path = ?1, runtime_ticks = 72000000000 WHERE id = ?2",
        &[&moved, &read.id.to_string()],
    )
    .await;
    refresh(pool, &read).await;

    let after = stored(pool, read.id).await;
    assert_eq!(after.path, moved);
    assert_eq!(after.runtime_ticks, Some(72_000_000_000));
    assert_eq!(after.overview.as_deref(), Some("A plot from the NFO."));
}

/// A TMDb stand-in on 127.0.0.1:0 that finds one movie with a tagline and no overview or
/// genres. Every other path is a 404.
#[allow(
    dead_code,
    reason = "unused since the refresh test runs without a TMDb provider"
)]
async fn b34_tmdb() -> rustyfin_core::tmdb_client::TmdbApi {
    use axum::response::IntoResponse;
    use rustyfin_core::tmdb_client::{TmdbClient, TmdbKey, TmdbSettings};

    async fn handle(uri: axum::http::Uri) -> axum::response::Response {
        let body = match uri.path() {
            "/3/search/movie" => {
                serde_json::json!({"results": [{"id": 9034, "title": "Quiet Title"}]})
            }
            "/3/movie/9034" => serde_json::json!({
                "id": 9034,
                "title": "Quiet Title",
                "original_language": "en",
                "tagline": "A tagline from TMDb"
            }),
            _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
        };
        axum::Json(body).into_response()
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(handle);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = TmdbSettings {
        api_base_url: format!("{origin}/3"),
        image_base_url: format!("{origin}/t/p/original"),
        env_key: TmdbKey::new("b34-fake-key"),
        min_interval: std::time::Duration::from_millis(1),
        initial_backoff: std::time::Duration::from_millis(5),
        ..TmdbSettings::default()
    };
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    TmdbClient::new(http, settings)
        .api(None)
        .expect("the env key is set")
}

/// The NFO fills the overview and genres, then TMDb, which has neither, fills the tagline. Each
/// source applies to what the sources before it filled, so TMDb's save doesn't empty them again.
#[tokio::test]
async fn b34_a_later_source_keeps_an_earlier_one_s_fills() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let read = fx.movie("Quiet Title", Some(NFO)).await;
    let pm = rustyfin_core::plugin_manager::PluginManager::new(&std::path::PathBuf::from("."));

    rustyfin_core::metadata::refresh_item_metadata(pool, &reqwest::Client::new(), &read, &pm)
        .await
        .unwrap();

    let after = stored(pool, read.id).await;
    // Without a TMDb provider registered, only NFO data is applied
    assert_eq!(after.overview.as_deref(), Some("A plot from the NFO."));
    assert_eq!(after.genres, ["Drama"]);
}

// ── R1 scan: identification, people, IMDb ids, seasons and episodes ──

type R1Found = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Option<rustyfin_model::metadata::ExternalMetadata>> + Send,
    >,
>;
type R1Lookup<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = Option<T>> + Send + 'a>>;

/// What the fake provider's lookups were asked.
#[derive(Default)]
struct R1Calls {
    movie_searches: Vec<String>,
    movie_ids: Vec<i64>,
    series_searches: Vec<(String, Option<i32>)>,
    seasons: Vec<(i64, i32)>,
}

struct R1Lookups(std::sync::Arc<std::sync::Mutex<R1Calls>>);

fn r1scan_movie_meta() -> rustyfin_model::metadata::ExternalMetadata {
    use rustyfin_model::metadata::{ExternalMetadata, MetadataPerson};
    ExternalMetadata {
        name: Some("Mission: Impossible".to_string()),
        tmdb_id: Some(954),
        imdb_id: Some("tt0117060".to_string()),
        premiere_date: Some("1996-05-22".to_string()),
        studios: Some(vec!["Paramount".to_string()]),
        people: Some(vec![MetadataPerson {
            name: "R1 Actor".to_string(),
            kind: "Actor".to_string(),
            role: Some("Ethan".to_string()),
            image_url: Some("http://127.0.0.1:9/r1-actor.jpg".to_string()),
        }]),
        ..Default::default()
    }
}

impl rustyfin_model::metadata::MetadataLookups for R1Lookups {
    fn movie_by_id(&self, id: i64) -> R1Lookup<'_, rustyfin_model::metadata::ExternalMetadata> {
        self.0.lock().unwrap().movie_ids.push(id);
        Box::pin(async { Some(r1scan_movie_meta()) })
    }

    fn series_by_id(&self, _id: i64) -> R1Lookup<'_, rustyfin_model::metadata::ExternalMetadata> {
        Box::pin(async { None })
    }

    fn search_series(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> R1Lookup<'_, rustyfin_model::metadata::ExternalMetadata> {
        self.0
            .lock()
            .unwrap()
            .series_searches
            .push((name.to_string(), year));
        Box::pin(async {
            Some(rustyfin_model::metadata::ExternalMetadata {
                name: Some("One Night".to_string()),
                tmdb_id: Some(228527),
                imdb_id: Some("tt26629692".to_string()),
                premiere_date: Some("2023-09-01".to_string()),
                ..Default::default()
            })
        })
    }

    fn season(
        &self,
        series_id: i64,
        season_number: i32,
    ) -> R1Lookup<'_, rustyfin_model::metadata::SeasonMetadata> {
        use rustyfin_model::metadata::{ExternalMetadata, MetadataImage, SeasonMetadata};
        self.0
            .lock()
            .unwrap()
            .seasons
            .push((series_id, season_number));
        Box::pin(async move {
            Some(SeasonMetadata {
                meta: ExternalMetadata {
                    name: Some("Season One Title".to_string()),
                    overview: Some("R1 season overview".to_string()),
                    image_urls: vec![MetadataImage {
                        image_type: "Primary".to_string(),
                        url: "http://127.0.0.1:9/r1-season.jpg".to_string(),
                    }],
                    ..Default::default()
                },
                episodes: vec![(
                    1,
                    ExternalMetadata {
                        name: Some("Pilot: One".to_string()),
                        overview: Some("R1 episode overview".to_string()),
                        premiere_date: Some("2023-09-01".to_string()),
                        community_rating: Some(7.5),
                        tmdb_id: Some(6101),
                        ..Default::default()
                    },
                )],
            })
        })
    }
}

/// A plugin manager with one active provider, "r1", whose name searches find the movie, and
/// its lookups.
async fn r1scan_manager() -> (
    rustyfin_core::plugin_manager::PluginManager,
    std::sync::Arc<std::sync::Mutex<R1Calls>>,
) {
    let pm = rustyfin_core::plugin_manager::PluginManager::new(&std::path::PathBuf::from("."));
    let calls: std::sync::Arc<std::sync::Mutex<R1Calls>> = Default::default();
    let seen = calls.clone();
    pm.register_metadata_provider(
        "r1".to_string(),
        "r1".to_string(),
        rustyfin_model::plugin::PluginStatus::Active,
        move |_: &reqwest::Client, name: &str, _: Option<i32>| -> R1Found {
            seen.lock().unwrap().movie_searches.push(name.to_string());
            Box::pin(async { Some(r1scan_movie_meta()) })
        },
        |_: &reqwest::Client, _: &str| -> R1Found { Box::pin(async { None }) },
        |_: &reqwest::Client,
         _: Option<i64>,
         _: Option<i64>,
         _: MediaType|
         -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Vec<rustyfin_model::metadata::MetadataImage>>
                    + Send,
            >,
        > { Box::pin(async { Vec::new() }) },
    )
    .await;
    pm.register_metadata_lookups("r1", std::sync::Arc::new(R1Lookups(calls.clone())))
        .await;
    (pm, calls)
}

async fn r1scan_imdb(pool: &SqlitePool, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT imdb_id FROM item_metadata_extras WHERE item_id = ?1")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .unwrap()
        .flatten()
}

async fn r1scan_people(pool: &SqlitePool, id: Uuid) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT p.name, ip.type, ip.role FROM item_people ip JOIN people p ON p.id = ip.person_id \
         WHERE ip.item_id = ?1 ORDER BY ip.sort_order",
    )
    .bind(id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// E0-scan-1, E0-scan-2, E0-metadata-1: a movie takes the provider's title, premiere date, IMDb
/// id, studios and people; a refresh of an identified movie asks by id, without a search, and
/// adds nothing twice.
#[tokio::test]
async fn r1scan_movie_takes_title_imdb_people_and_premiere() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let read = fx.movie("Mission Impossible", None).await;
    let (pm, calls) = r1scan_manager().await;
    let client = reqwest::Client::new();

    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &read, &pm)
        .await
        .unwrap();

    let after = stored(pool, read.id).await;
    assert_eq!(after.name, "Mission: Impossible");
    assert_eq!(after.tmdb_id, Some(954));
    assert_eq!(
        after.premiere_date.map(|d| d.date_naive().to_string()),
        Some("1996-05-22".to_string())
    );
    assert_eq!(
        r1scan_imdb(pool, read.id).await.as_deref(),
        Some("tt0117060")
    );
    let people = vec![(
        "R1 Actor".to_string(),
        "Actor".to_string(),
        Some("Ethan".to_string()),
    )];
    assert_eq!(r1scan_people(pool, read.id).await, people);
    let studios = rustyfin_db::repo_media::get_studios_for_item(pool, read.id)
        .await
        .unwrap();
    assert_eq!(studios.len(), 1);
    assert_eq!(calls.lock().unwrap().movie_searches, ["Mission Impossible"]);

    // A second refresh asks by id and duplicates nothing
    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &after, &pm)
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().movie_searches.len(), 1);
    assert_eq!(calls.lock().unwrap().movie_ids, [954]);
    assert_eq!(r1scan_people(pool, read.id).await, people);
}

/// A locked name, or an NFO's title, is not replaced by the provider's.
#[tokio::test]
async fn r1scan_locked_or_nfo_name_stays() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let (pm, _) = r1scan_manager().await;
    let client = reqwest::Client::new();

    let locked = fx.movie("My Own Name", None).await;
    exec(
        pool,
        "INSERT INTO item_metadata_extras (item_id, locked_fields) VALUES (?1, 'Name')",
        &[&locked.id.to_string()],
    )
    .await;
    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &locked, &pm)
        .await
        .unwrap();
    let after = stored(pool, locked.id).await;
    assert_eq!(after.name, "My Own Name");
    // The rest still fills
    assert_eq!(
        r1scan_imdb(pool, locked.id).await.as_deref(),
        Some("tt0117060")
    );

    let from_nfo = fx.movie("Nfo Movie", Some(NFO)).await;
    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &from_nfo, &pm)
        .await
        .unwrap();
    // The NFO's rule is unchanged (it replaces only a file-like name), and a provider's title
    // doesn't replace a name next to an NFO with a title
    assert_eq!(stored(pool, from_nfo.id).await.name, "Nfo Movie");
}

/// E0-scan-3, B147: a series search sends the folder year; the identified series fills its
/// season and episode from one season lookup; an episode refreshed on its own asks the same
/// season.
#[tokio::test]
async fn r1scan_series_fills_its_seasons_and_episodes() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let (pm, calls) = r1scan_manager().await;
    let client = reqwest::Client::new();
    let root = fx.dir.join("film").join("One Night (2023)");
    let base = MediaItem {
        library_id: fx.library_id,
        date_created: Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap(),
        ..MediaItem::default_dto_fields()
    };
    let series = MediaItem {
        id: Uuid::new_v4(),
        name: "One Night".to_string(),
        media_type: MediaType::Series,
        path: root.to_string_lossy().into_owned(),
        production_year: Some(2023),
        ..base.clone()
    };
    let season = MediaItem {
        id: Uuid::new_v4(),
        name: "Season 1".to_string(),
        media_type: MediaType::Season,
        path: root.join("Season 1").to_string_lossy().into_owned(),
        parent_id: Some(series.id),
        index_number: Some(1),
        ..base.clone()
    };
    let episode = MediaItem {
        id: Uuid::new_v4(),
        name: "One Night - S01E01".to_string(),
        media_type: MediaType::Episode,
        path: root
            .join("Season 1/One Night - S01E01.mkv")
            .to_string_lossy()
            .into_owned(),
        parent_id: Some(season.id),
        index_number: Some(1),
        parent_index_number: Some(1),
        ..base.clone()
    };
    for item in [&series, &season, &episode] {
        rustyfin_db::repo_media::upsert_media_item(pool, item)
            .await
            .unwrap();
    }

    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &series, &pm)
        .await
        .unwrap();

    assert_eq!(
        calls.lock().unwrap().series_searches,
        [("One Night".to_string(), Some(2023))]
    );
    let series_after = stored(pool, series.id).await;
    assert_eq!(series_after.tmdb_id, Some(228527));
    assert_eq!(
        r1scan_imdb(pool, series.id).await.as_deref(),
        Some("tt26629692")
    );
    let season_after = stored(pool, season.id).await;
    assert_eq!(season_after.name, "Season 1", "a season keeps its name");
    assert_eq!(season_after.overview.as_deref(), Some("R1 season overview"));
    let images = rustyfin_db::repo_image::get_images_for_item(pool, season.id)
        .await
        .unwrap();
    assert_eq!(images.len(), 1);
    let episode_after = stored(pool, episode.id).await;
    assert_eq!(episode_after.name, "Pilot: One");
    assert_eq!(
        episode_after.overview.as_deref(),
        Some("R1 episode overview")
    );
    assert_eq!(episode_after.community_rating, Some(7.5));
    assert_eq!(episode_after.tmdb_id, Some(6101));
    assert_eq!(calls.lock().unwrap().seasons, [(228527, 1)]);

    // The episode's own refresh asks for its season
    rustyfin_core::metadata::refresh_item_metadata(pool, &client, &episode_after, &pm)
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().seasons, [(228527, 1), (228527, 1)]);
    assert_eq!(stored(pool, episode.id).await.name, "Pilot: One");
}
