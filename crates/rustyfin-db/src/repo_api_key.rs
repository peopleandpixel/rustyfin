use sqlx::SqlitePool;

use crate::writer::ExecuteWrite;

// API keys are keyed by their hash. The hashing happens in rustyfin-auth; this layer only stores it.

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiKeyRow {
    pub id: String,
    /// The key's last four characters
    pub hint: String,
    pub app_name: String,
    /// ISO 8601 UTC
    pub date_created: String,
    /// When the key was last used, ISO 8601 UTC
    pub date_last_activity: Option<String>,
}

pub async fn insert(
    pool: &SqlitePool,
    id: &str,
    token_hash: &str,
    hint: &str,
    app_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO api_keys (id, token_hash, hint, app_name) VALUES (?1, ?2, ?3, ?4)")
        .bind(id)
        .bind(token_hash)
        .bind(hint)
        .bind(app_name)
        .execute_write(pool)
        .await?;
    Ok(())
}

/// Every key, oldest first.
pub async fn list(pool: &SqlitePool) -> Result<Vec<ApiKeyRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiKeyRow>(
        "SELECT id, hint, app_name, date_created, date_last_activity FROM api_keys
         ORDER BY date_created, id",
    )
    .fetch_all(pool)
    .await
}

/// Whether a key with this hash exists; a found key's last activity is set to now.
pub async fn use_key(pool: &SqlitePool, token_hash: &str) -> Result<bool, sqlx::Error> {
    let found: Option<String> = sqlx::query_scalar("SELECT id FROM api_keys WHERE token_hash = ?1")
        .bind(token_hash)
        .fetch_optional(pool)
        .await?;
    let Some(id) = found else {
        return Ok(false);
    };
    // At most once a minute, so a busy key doesn't write on every request
    sqlx::query(
        "UPDATE api_keys SET date_last_activity = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1 AND (date_last_activity IS NULL
             OR date_last_activity < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 minute'))",
    )
    .bind(&id)
    .execute_write(pool)
    .await?;
    Ok(true)
}

/// Deletes the key with this id or this hash. Returns whether one was deleted.
pub async fn delete(pool: &SqlitePool, id: &str, token_hash: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM api_keys WHERE id = ?1 OR token_hash = ?2")
        .bind(id)
        .bind(token_hash)
        .execute_write(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}
