pub mod repo_user;
pub mod repo_library;
pub mod repo_media;
pub mod repo_image;
pub mod repo_playback;
pub mod repo_session;
pub mod repo_search;
pub mod repo_collection;
pub mod repo_playlist;
pub mod repo_person;
pub mod repo_activity;
pub mod repo_notification;

use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(20)
        .min_connections(2)
        .idle_timeout(std::time::Duration::from_secs(300))
        .max_lifetime(std::time::Duration::from_secs(3600))
        .connect(database_url)
        .await?;

    // Enable WAL mode for better concurrent read/write performance
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await?;
    // Increase cache size (negative = KiB, -32000 = ~32MB)
    sqlx::query("PRAGMA cache_size=-32000")
        .execute(&pool)
        .await?;
    // Enable memory-mapped I/O (256MB) for large libraries
    sqlx::query("PRAGMA mmap_size=268435456")
        .execute(&pool)
        .await?;
    // Synchronous NORMAL for better write performance with WAL
    sqlx::query("PRAGMA synchronous=NORMAL")
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/002_images.sql"))
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/002_user_configuration.sql"))
        .execute(&pool)
        .await
        .ok(); // OK to fail if column already exists

    sqlx::query(include_str!("../migrations/003_user_item_data.sql"))
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/004_sessions_devices.sql"))
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/005_phase4.sql"))
        .execute(&pool)
        .await?;

    Ok(pool)
}
