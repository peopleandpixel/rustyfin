pub mod repo_user;
pub mod repo_library;
pub mod repo_media;
pub mod repo_image;

use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    sqlx::query(include_str!("../migrations/001_initial.sql"))
        .execute(&pool)
        .await?;

    sqlx::query(include_str!("../migrations/002_images.sql"))
        .execute(&pool)
        .await?;

    Ok(pool)
}
