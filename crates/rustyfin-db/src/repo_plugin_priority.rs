//! Repository for plugin priority ordering.
//!
//! Plugins can be ordered by priority (lower number = higher priority).
//! The priority is stored in the `plugin_priority` table.

use sqlx::SqlitePool;

/// Get the priority for a plugin. Returns 100 (default) if not set.
pub async fn get_plugin_priority(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<i32, sqlx::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT priority FROM plugin_priority WHERE plugin_id = ?1",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(p,)| p).unwrap_or(100))
}

/// Set or update the priority for a plugin.
pub async fn set_plugin_priority(
    pool: &SqlitePool,
    plugin_id: &str,
    priority: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO plugin_priority (plugin_id, priority)
         VALUES (?1, ?2)
         ON CONFLICT(plugin_id) DO UPDATE SET
             priority = excluded.priority",
    )
    .bind(plugin_id)
    .bind(priority)
    .execute(pool)
    .await?;

    Ok(())
}

/// Delete the priority for a plugin (resets to default).
pub async fn delete_plugin_priority(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM plugin_priority WHERE plugin_id = ?1")
        .bind(plugin_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Get all plugin priorities ordered by priority value.
pub async fn list_plugin_priorities(
    pool: &SqlitePool,
) -> Result<Vec<(String, i32)>, sqlx::Error> {
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "SELECT plugin_id, priority FROM plugin_priority ORDER BY priority ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}
