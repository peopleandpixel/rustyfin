//! Repository for plugin configuration storage.
//!
//! Plugins store their configuration as JSON in the `plugin_config` table,
//! independent of the server's `config.json`.

use sqlx::SqlitePool;

/// Get the configuration JSON for a plugin.
pub async fn get_plugin_config(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT config_json FROM plugin_config WHERE plugin_id = ?1",
    )
    .bind(plugin_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((json_str,)) => {
            let value: serde_json::Value =
                serde_json::from_str(&json_str).unwrap_or_else(|_| serde_json::json!({}));
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Save or update the configuration JSON for a plugin.
pub async fn set_plugin_config(
    pool: &SqlitePool,
    plugin_id: &str,
    config: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let json_str = serde_json::to_string(config)
        .map_err(|e| sqlx::Error::Protocol(e.to_string().into()))?;

    sqlx::query(
        "INSERT INTO plugin_config (plugin_id, config_json, updated_at)
         VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(plugin_id) DO UPDATE SET
             config_json = excluded.config_json,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(plugin_id)
    .bind(json_str)
    .execute(pool)
    .await?;

    Ok(())
}

/// Delete the configuration for a plugin.
pub async fn delete_plugin_config(
    pool: &SqlitePool,
    plugin_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM plugin_config WHERE plugin_id = ?1")
        .bind(plugin_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// List all plugin IDs that have stored configurations.
pub async fn list_plugin_configs(
    pool: &SqlitePool,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT plugin_id FROM plugin_config ORDER BY plugin_id",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}
