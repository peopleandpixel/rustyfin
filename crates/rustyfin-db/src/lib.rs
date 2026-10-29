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
pub mod repo_trickplay;
pub mod repo_user;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

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

    // Migrations are executed idempotently — OK to fail if already applied
    // This allows re-initialization on shared in-memory databases (e.g., tests)
    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/002_images.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/002_user_configuration.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/003_user_item_data.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/004_sessions_devices.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/005_phase4.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/006_media_sources.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/007_optimization.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/008_playstate.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/009_phase2.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/010_media_segments.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/011_chapters.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/012_trickplay.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/013_native_types.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/014_genres_tags_index.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/015_covering_index.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/016_external_ids.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/017_user_features.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/018_password_reset.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/019_client_capabilities.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/020_live_tv.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/021_new_controllers.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/022_performance.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/023_fix_image_indices.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!(
        "../migrations/024_fix_local_image_matching.sql"
    ))
    .execute(&pool)
    .await
    .ok();

    sqlx::query(include_str!("../migrations/025_extended_metadata.sql"))
        .execute(&pool)
        .await
        .ok();

    sqlx::query(include_str!("../migrations/026_filter_triggers.sql"))
        .execute(&pool)
        .await
        .ok();

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
