// SQLx queries inherently produce complex tuple types and functions with many parameters
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod repo_activity;
pub mod repo_chapter;
pub mod repo_collection;
pub mod repo_device;
pub mod repo_image;
pub mod repo_library;
pub mod repo_live_tv;
pub mod repo_media;
pub mod repo_media_segment;
pub mod repo_media_source;
pub mod repo_misc;
pub mod repo_notification;
pub mod repo_package;
pub mod repo_person;
pub mod repo_playback;
pub mod repo_playlist;
pub mod repo_search;
pub mod repo_session;
pub mod repo_sync_play;
pub mod repo_token;
pub mod repo_trickplay;
pub mod repo_user;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

/// Compile-time verified migration runner.
/// SQLx scans the `migrations` directory at build time and tracks applied migrations
/// in the `sqlx_migrations` table, so each migration runs only once.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    // Configure connection-level PRAGMAs that apply to every connection from the pool
    let options = SqliteConnectOptions::new()
        .filename(database_url)
        .create_if_missing(true)
        // WAL mode for better concurrent read/write performance
        .journal_mode(SqliteJournalMode::Wal)
        // Increase busy timeout to handle concurrent access (30 seconds)
        .busy_timeout(std::time::Duration::from_secs(30))
        // Shared cache allows connections to share parsed schema data
        .shared_cache(true)
        // PRAGMA: synchronous NORMAL for better write performance with WAL
        .pragma("synchronous", "NORMAL")
        // PRAGMA: cache size ~64MB per connection (positive = pages, 65536 pages ≈ 64MB)
        .pragma("cache_size", "65536")
        // PRAGMA: memory-mapped I/O 256MB for large libraries
        .pragma("mmap_size", "268435456")
        // PRAGMA: enforce foreign key constraints
        .pragma("foreign_keys", "ON")
        // PRAGMA: use memory for temp files instead of disk
        .pragma("temp_store", "MEMORY")
        // PRAGMA: WAL auto-checkpoint at 500 pages to control WAL file size
        .pragma("wal_autocheckpoint", "500");

    // Connection pool size configurable via RUSTYFIN_DB_POOL_SIZE (default: 10)
    let max_connections: u32 = std::env::var("RUSTYFIN_DB_POOL_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(1)
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(3600))
        .connect_with(options)
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

    Ok(pool)
}

/// Initialize a read-only connection pool for query-heavy operations.
/// Uses `PRAGMA query_only = ON` to prevent accidental writes and allow
/// SQLite to optimize read paths by skipping write-lock overhead.
pub async fn init_read_only_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(database_url)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(30))
        .shared_cache(true)
        .pragma("synchronous", "NORMAL")
        .pragma("cache_size", "65536")
        .pragma("mmap_size", "268435456")
        .pragma("foreign_keys", "ON")
        .pragma("temp_store", "MEMORY")
        .pragma("wal_autocheckpoint", "500")
        // Read-only mode — prevents accidental writes and allows SQLite to optimize reads
        .pragma("query_only", "ON");

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .min_connections(1)
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(3600))
        .connect_with(options)
        .await?;

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

    fn temp_dir() -> std::path::PathBuf {
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
}
