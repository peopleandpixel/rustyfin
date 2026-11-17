// SQLx queries inherently produce complex tuple types and functions with many parameters
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod busy;
pub mod pool_watch;
pub mod repo_activity;
pub mod repo_api_key;
pub mod repo_by_name;
pub mod repo_chapter;
pub mod repo_collection;
pub mod repo_device;
pub mod repo_extras;
pub mod repo_image;
pub mod repo_instant_mix;
pub mod repo_jellyfin_import;
pub mod repo_library;
pub mod repo_live_tv;
pub mod repo_lyrics;
pub mod repo_media;
pub mod repo_media_segment;
pub mod repo_media_source;
pub mod repo_misc;
pub mod repo_notification;
pub mod repo_oidc;
pub mod repo_package;
pub mod repo_parental;
pub mod repo_person;
pub mod repo_photo;
pub mod repo_playback;
pub mod repo_playlist;
pub mod repo_plugin_config;
pub mod repo_plugin_priority;
pub mod repo_search;
pub mod repo_session;
pub mod repo_setup_code;
pub mod repo_sync_play;
pub mod repo_token;
pub mod repo_trickplay;
pub mod repo_user;
pub mod writer;

pub use writer::{WriteTx, begin_write};

#[cfg(test)]
mod pool_tests;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

/// Compile-time verified migration runner.
/// SQLx scans the `migrations` directory at build time and tracks applied migrations
/// in the `sqlx_migrations` table, so each migration runs only once.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

/// How long a pool caller waits for a free connection before the call fails.
pub const ACQUIRE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a statement waits for another connection's write lock. Shorter than
/// `ACQUIRE_TIMEOUT`, so writers stuck behind a long write give their connections back before
/// the callers queued for one time out.
pub const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Each connection's page cache in KiB when `RUSTYFIN_DB_CACHE_KIB` is unset.
const DEFAULT_CACHE_KIB: u32 = 4096;

/// Each connection's page cache in KiB: `RUSTYFIN_DB_CACHE_KIB`, from 64 to 1048576 (1 GiB), else
/// `DEFAULT_CACHE_KIB`.
fn cache_kib() -> u32 {
    match std::env::var("RUSTYFIN_DB_CACHE_KIB") {
        Err(_) => DEFAULT_CACHE_KIB,
        Ok(v) => match v.trim().parse::<u32>() {
            Ok(kib) if (64..=1_048_576).contains(&kib) => kib,
            _ => {
                tracing::warn!(value = %v, default = DEFAULT_CACHE_KIB,
                    "RUSTYFIN_DB_CACHE_KIB is not a whole number from 64 to 1048576");
                DEFAULT_CACHE_KIB
            }
        },
    }
}

/// The options of every connection, in both pools.
fn connect_options(database_url: &str) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(database_url)
        .create_if_missing(true)
        // WAL mode for better concurrent read/write performance
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(BUSY_TIMEOUT)
        // No shared cache: its table locks wait with no timeout, and a shared-cache connection
        // that waited on a lock nobody would release held its pool slot for good.
        .shared_cache(false)
        // PRAGMA: synchronous NORMAL for better write performance with WAL
        .pragma("synchronous", "NORMAL")
        // PRAGMA: each connection's own page cache, `cache_kib` (negative = KiB). The main pool
        // (`RUSTYFIN_DB_POOL_SIZE`, 10 by default) and the read-only pool (10) take at most
        // their connection count times that; reads within the mmap below skip it.
        .pragma("cache_size", format!("-{}", cache_kib()))
        // PRAGMA: memory-mapped I/O 256MB for large libraries
        .pragma("mmap_size", "268435456")
        // PRAGMA: enforce foreign key constraints
        .pragma("foreign_keys", "ON")
        // PRAGMA: use memory for temp files instead of disk
        .pragma("temp_store", "MEMORY")
        // PRAGMA: WAL auto-checkpoint at 500 pages to control WAL file size
        .pragma("wal_autocheckpoint", "500")
}

/// The pool settings both pools share. sqlx already warns about an acquire over 2 s and a
/// statement over 1 s once it ends; `pool_watch` covers the ones that never end.
fn pool_options(max_connections: u32) -> SqlitePoolOptions {
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(1)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(3600))
}

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // Connection pool size configurable via RUSTYFIN_DB_POOL_SIZE (default: 10)
    let max_connections: u32 = std::env::var("RUSTYFIN_DB_POOL_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let releases = pool_watch::Releases::default();
    let pool = releases
        .count(pool_options(max_connections))
        .connect_with(connect_options(database_url))
        .await?;

    // Additional PRAGMAs that only need to run once on the main connection
    // WAL checkpoint to ensure clean state
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await
        .ok();

    // Run migrations — SQLx tracks applied migrations in `sqlx_migrations` table,
    // so each migration runs only once. Idempotent and safe to call repeatedly.
    MIGRATOR.run(&pool).await?;
    pool_watch::watch("main", &pool, releases);

    Ok(pool)
}

#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let path = tests::temp_dir().join("test.db");
    init_db(path.to_str().unwrap()).await.unwrap()
}

#[cfg(test)]
pub async fn insert_test_user(pool: &SqlitePool, id: uuid::Uuid) {
    let uid = id.to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, name, password_hash, is_administrator, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&uid)
    .bind(format!("user_{}", id))
    .bind("hashed")
    .bind(1)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .unwrap();
}

#[cfg(test)]
pub async fn insert_test_library(pool: &SqlitePool, id: uuid::Uuid) {
    let lid = id.to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO libraries (id, name, content_type, paths) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&lid)
    .bind("Test Library")
    .bind("Movie")
    .bind("[]")
    .execute(pool)
    .await
    .unwrap();
}

#[cfg(test)]
pub async fn insert_test_item(pool: &SqlitePool, id: uuid::Uuid) {
    let iid = id.to_string();
    let lib_id = uuid::Uuid::new_v4();
    insert_test_library(pool, lib_id).await;
    let lid = lib_id.to_string();
    sqlx::query(
        "INSERT OR IGNORE INTO media_items (id, name, media_type, path, library_id, date_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&iid)
    .bind(format!("Item {}", id))
    .bind("Movie")
    .bind("/tmp/test.mp4")
    .bind(&lid)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .unwrap();
}

/// Initialize a read-only connection pool for query-heavy operations.
/// Uses `PRAGMA query_only = ON` to prevent accidental writes and allow
/// SQLite to optimize read paths by skipping write-lock overhead.
pub async fn init_read_only_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // Read-only mode — prevents accidental writes and allows SQLite to optimize reads
    let options = connect_options(database_url).pragma("query_only", "ON");

    let releases = pool_watch::Releases::default();
    let pool = releases
        .count(pool_options(10))
        .connect_with(options)
        .await?;
    pool_watch::watch("read-only", &pool, releases);

    Ok(pool)
}

#[cfg(test)]
mod tests {
    // G2 follow-up: master 2099567 ships migrations 001-031 and G1's 033_user_easy_password.sql
    // takes version 33, so every schema change on this branch must live in 032. These run the real
    // MIGRATOR, or the part of it that a master build had already applied.
    use super::*;
    use sqlx::migrate::Migrator;

    /// The last migration version on master 2099567.
    const MASTER_LAST_VERSION: i64 = 31;

    fn subset(max_version: i64) -> Migrator {
        Migrator::with_migrations(
            MIGRATOR
                .iter()
                .filter(|m| m.version <= max_version)
                .cloned()
                .collect(),
        )
    }

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

    pub(crate) fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_{}", uuid::Uuid::new_v4()));
        remove_when_test_ends(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn plain_pool(path: &std::path::Path) -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
            .unwrap()
    }

    /// Every table, index, trigger and view, without SQLite's and sqlx's own bookkeeping.
    async fn schema(pool: &SqlitePool) -> Vec<(String, String, String, Option<String>)> {
        sqlx::query_as(
            "SELECT type, name, tbl_name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY type, name",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn assert_reset_pins_hardened(pool: &SqlitePool) {
        let cols: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('password_reset_pins')")
                .fetch_all(pool)
                .await
                .unwrap();
        assert!(
            cols.iter().any(|c| c == "failed_attempts"),
            "password_reset_pins.failed_attempts exists: {cols:?}"
        );
        let unique: Option<i64> = sqlx::query_scalar(
            "SELECT \"unique\" FROM pragma_index_list('password_reset_pins') \
             WHERE name = 'idx_password_reset_pins_user'",
        )
        .fetch_optional(pool)
        .await
        .unwrap();
        assert_eq!(
            unique,
            Some(1),
            "the unique index idx_password_reset_pins_user exists"
        );
    }

    #[tokio::test]
    async fn migrations_through_032_harden_reset_pins() {
        let dir = temp_dir();
        let pool = plain_pool(&dir.join("v32.db")).await;
        subset(32).run(&pool).await.unwrap();
        assert_reset_pins_hardened(&pool).await;
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_master_db_upgrades_to_the_fresh_schema() {
        let dir = temp_dir();
        let master = dir.join("master.db");
        let pool = plain_pool(&master).await;
        subset(MASTER_LAST_VERSION).run(&pool).await.unwrap();
        pool.close().await;

        let upgraded = init_db(master.to_str().unwrap()).await.unwrap();
        let fresh = init_db(dir.join("fresh.db").to_str().unwrap())
            .await
            .unwrap();
        assert_reset_pins_hardened(&fresh).await;
        assert_reset_pins_hardened(&upgraded).await;
        assert_eq!(
            schema(&upgraded).await,
            schema(&fresh).await,
            "a 2099567 DB and a fresh DB end with the same schema"
        );
        upgraded.close().await;
        fresh.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn b33_a_9d9cfb2_db_applies_072_and_keeps_its_items() {
        // Master 9d9cfb2 ships 001-035 and 061. At the next start its episodes get an empty end,
        // and its items the FileSystem location type that a scan corrects for seasons with no folder.
        let dir = temp_dir();
        let master = dir.join("9d9cfb2.db");
        let pool = plain_pool(&master).await;
        subset(61).run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO libraries (id, name, content_type) VALUES ('lib', 'TV', 'tvshows')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO media_items (id, name, media_type, path, library_id, index_number) \
             VALUES ('ep', 'Hotel Series - S01E03-E04', 'Episode', '/tv/ep.mkv', 'lib', 3)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let upgraded = init_db(master.to_str().unwrap()).await.unwrap();
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&upgraded)
                .await
                .unwrap();
        assert!(applied.contains(&72), "072 is applied: {applied:?}");
        let episode: (Option<i64>, Option<i64>, String) = sqlx::query_as(
            "SELECT index_number, index_number_end, location_type FROM media_items WHERE id = 'ep'",
        )
        .fetch_one(&upgraded)
        .await
        .unwrap();
        assert_eq!(episode, (Some(3), None, "FileSystem".to_string()));
        let fresh = init_db(dir.join("fresh.db").to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            schema(&upgraded).await,
            schema(&fresh).await,
            "a 9d9cfb2 DB and a fresh DB end with the same schema"
        );
        upgraded.close().await;
        fresh.close().await;
    }

    #[tokio::test]
    async fn g2c_a_latest_db_without_032_upgrades_to_the_fresh_schema() {
        // The :latest image (master 75fea9a) shipped 001-031 and G1's 033 but not 032. SQLx applies
        // the missing 032 after 033 at the next start.
        let dir = temp_dir();
        let latest = dir.join("latest.db");
        let pool = plain_pool(&latest).await;
        Migrator::with_migrations(
            MIGRATOR
                .iter()
                .filter(|m| m.version <= MASTER_LAST_VERSION || m.version == 33)
                .cloned()
                .collect(),
        )
        .run(&pool)
        .await
        .unwrap();
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(
            applied.contains(&33) && !applied.contains(&32),
            "the :latest DB has 033 and no 032: {applied:?}"
        );
        pool.close().await;

        let upgraded = init_db(latest.to_str().unwrap()).await.unwrap();
        let fresh = init_db(dir.join("fresh.db").to_str().unwrap())
            .await
            .unwrap();
        assert_reset_pins_hardened(&upgraded).await;
        assert_eq!(
            schema(&upgraded).await,
            schema(&fresh).await,
            "a :latest DB and a fresh DB end with the same schema"
        );
        upgraded.close().await;
        fresh.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rowids of the search index entries that match `query`.
    async fn search_entries(pool: &SqlitePool, query: &str) -> Vec<i64> {
        sqlx::query_scalar(
            "SELECT rowid FROM media_items_fts WHERE media_items_fts MATCH ?1 ORDER BY rowid",
        )
        .bind(query)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn b31_071_drops_search_entries_left_at_old_rowids() {
        // Before 071 a metadata refresh saved an item with INSERT OR REPLACE. The item came back
        // under a new rowid, and the search entry for its old name stayed at the old rowid. 071
        // rebuilds the search index from media_items once.
        let dir = temp_dir();
        let old = dir.join("old.db");
        let pool = plain_pool(&old).await;
        subset(70).run(&pool).await.unwrap();
        for statement in [
            "INSERT INTO libraries (id, name, content_type) VALUES ('lib', 'Film', 'movies')",
            "INSERT INTO media_items (id, library_id, name, media_type, path) \
             VALUES ('item', 'lib', 'Old Title', 'Movie', '/media/movies/item.mkv')",
            "INSERT OR REPLACE INTO media_items (id, library_id, name, media_type, path) \
             VALUES ('item', 'lib', 'New Title', 'Movie', '/media/movies/item.mkv')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        assert_eq!(
            search_entries(&pool, "old*").await.len(),
            1,
            "the old save left an entry for the old name"
        );
        pool.close().await;

        let upgraded = init_db(old.to_str().unwrap()).await.unwrap();
        let rowid: i64 = sqlx::query_scalar("SELECT rowid FROM media_items WHERE id = 'item'")
            .fetch_one(&upgraded)
            .await
            .unwrap();
        assert_eq!(
            search_entries(&upgraded, "old*").await,
            Vec::<i64>::new(),
            "no entry for the old name is left"
        );
        assert_eq!(
            search_entries(&upgraded, "new*").await,
            [rowid],
            "the new name has one entry, at the item's rowid"
        );
        upgraded.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn r3sortname_sort(pool: &SqlitePool, id: &str) -> (Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT m.sort_name, e.forced_sort_name FROM media_items m \
             LEFT JOIN item_metadata_extras e ON e.item_id = m.id WHERE m.id = ?",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn r3sortname_094_keeps_forced_sort_names_and_the_pass_remakes_the_rest() {
        // Before 094 a forced sort name was only media_items.sort_name, as typed; an automatic
        // one was the lowercased name.
        let dir = temp_dir();
        let old = dir.join("old.db");
        let pool = plain_pool(&old).await;
        subset(93).run(&pool).await.unwrap();
        for statement in [
            "INSERT INTO libraries (id, name, content_type) VALUES ('lib', 'Film', 'movies')",
            "INSERT INTO media_items (id, library_id, name, sort_name, media_type, path) VALUES \
             ('matrix', 'lib', 'The Matrix', 'the matrix', 'Movie', '/m/1.mkv'), \
             ('alpha', 'lib', 'The Alpha', 'Alpha, The', 'Movie', '/m/2.mkv'), \
             ('algen', 'lib', 'Ålgen', 'ålgen', 'Movie', '/m/3.mkv'), \
             ('elk', 'lib', 'Élan', 'Zebra', 'Movie', '/m/4.mkv'), \
             ('ole', 'lib', 'Ølé', 'ole', 'Movie', '/m/7.mkv'), \
             ('locked', 'lib', 'The Locked', 'the locked', 'Movie', '/m/5.mkv'), \
             ('ep', 'lib', 'The Pilot', 'show s01e02', 'Episode', '/m/6.mkv'), \
             ('season', 'lib', 'Season 1', 'season 0001', 'Season', '/m/s1')",
            "UPDATE media_items SET parent_index_number = 1, index_number = 2 WHERE id = 'ep'",
            "INSERT INTO item_metadata_extras (item_id, locked_fields) VALUES ('locked', 'Name')",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        pool.close().await;

        let upgraded = init_db(old.to_str().unwrap()).await.unwrap();
        assert_eq!(
            r3sortname_sort(&upgraded, "alpha").await,
            (Some("Alpha, The".into()), Some("Alpha, The".into())),
            "094 keeps a sort name that is not the lowercased name as the forced one"
        );
        for id in ["matrix", "algen", "elk", "ole", "locked", "ep", "season"] {
            assert_eq!(r3sortname_sort(&upgraded, id).await.1, None, "{id}");
        }

        // The startup step: Rust lowercases what SQLite cannot. 'ålgen' was automatic, and so
        // is 'ole', the sort name made from 'Ølé'; 'Zebra' was forced. It runs once.
        assert_eq!(
            repo_media::classify_legacy_sort_names(&upgraded)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            r3sortname_sort(&upgraded, "elk").await,
            (Some("Zebra".into()), Some("Zebra".into()))
        );
        assert_eq!(
            repo_media::classify_legacy_sort_names(&upgraded)
                .await
                .unwrap(),
            0
        );

        let rules = rustyfin_model::sort_name::SortRules::default();
        let changed = repo_media::recompute_sort_names(&upgraded, &rules)
            .await
            .unwrap();
        let expect = [
            ("matrix", "matrix", None),
            ("alpha", "alpha, the", Some("Alpha, The")),
            ("algen", "algen", None),
            ("elk", "zebra", Some("Zebra")),
            ("ole", "ole", None),
            // A lock doesn't keep a sort name: it is made from the (kept) name, as in Jellyfin
            ("locked", "locked", None),
            ("ep", "001 - 0002 - The Pilot", None),
            ("season", "season 0001", None),
        ];
        for (id, sort, forced) in expect {
            assert_eq!(
                r3sortname_sort(&upgraded, id).await,
                (Some(sort.to_string()), forced.map(str::to_string)),
                "{id}"
            );
        }
        assert_eq!(changed, 6);
        assert_eq!(
            repo_media::recompute_sort_names(&upgraded, &rules)
                .await
                .unwrap(),
            0,
            "the same rules leave the sort names alone"
        );

        // Other rules make them again
        let rules = rustyfin_model::sort_name::SortRules {
            remove_words: vec![],
            ..Default::default()
        };
        assert_eq!(
            repo_media::recompute_sort_names(&upgraded, &rules)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            r3sortname_sort(&upgraded, "matrix").await.0.as_deref(),
            Some("the matrix")
        );
        upgraded.close().await;
    }

    #[tokio::test]
    async fn r3sortname_an_upsert_keeps_only_a_forced_sort_name() {
        let pool = test_pool().await;
        let lib = uuid::Uuid::new_v4();
        insert_test_library(&pool, lib).await;
        let mut items = Vec::new();
        for name in ["Forced", "Locked", "Free"] {
            let mut item = rustyfin_model::media::MediaItem::default_dto_fields();
            item.id = uuid::Uuid::new_v4();
            item.library_id = lib;
            item.name = name.to_string();
            item.sort_name = Some("stored".to_string());
            item.path = format!("/m/{name}.mkv");
            repo_media::upsert_media_item(&pool, &item).await.unwrap();
            items.push(item);
        }
        repo_media::set_forced_sort_name(&pool, items[0].id, Some("Stored"))
            .await
            .unwrap();
        let locks = repo_media::ItemLocks {
            lock_data: false,
            fields: vec!["SortName".to_string()],
        };
        repo_media::save_item_extras_edit(&pool, items[1].id, None, None, &locks)
            .await
            .unwrap();
        for item in &mut items {
            item.sort_name = Some("rescanned".to_string());
        }
        repo_media::batch_upsert_media_items(&pool, &items[..2])
            .await
            .unwrap();
        repo_media::upsert_media_item(&pool, &items[2])
            .await
            .unwrap();
        let sorts: Vec<Option<String>> = r3sortname_sorts(&pool, &items).await;
        assert_eq!(
            sorts,
            [
                Some("stored".into()),
                Some("rescanned".into()),
                Some("rescanned".into())
            ]
        );
    }

    async fn r3sortname_sorts(
        pool: &SqlitePool,
        items: &[rustyfin_model::media::MediaItem],
    ) -> Vec<Option<String>> {
        let mut sorts = Vec::new();
        for item in items {
            sorts.push(r3sortname_sort(pool, &item.id.to_string()).await.0);
        }
        sorts
    }
}
