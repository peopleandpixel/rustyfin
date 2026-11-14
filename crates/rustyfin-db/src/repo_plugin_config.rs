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
    let row: Option<(String,)> =
        sqlx::query_as("SELECT config_json FROM plugin_config WHERE plugin_id = ?1")
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

/// Merge a configuration patch into the stored configuration for a plugin.
///
/// The patch is an RFC 7396 merge patch applied with SQLite `json_patch`:
/// keys in the patch replace stored ones, a `null` value removes a key, and
/// a non-object patch replaces the whole value. Invalid stored JSON is
/// replaced by the patch. The merge runs in one atomic statement.
pub async fn set_plugin_config(
    pool: &SqlitePool,
    plugin_id: &str,
    config: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    let json_str =
        serde_json::to_string(config).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

    // `?2` is the raw patch on both paths: the insert path strips nulls via
    // `json_patch('{}', ...)`, while the conflict path needs the nulls to
    // remove stored keys.
    sqlx::query(
        "INSERT INTO plugin_config (plugin_id, config_json, updated_at)
         VALUES (?1, json_patch('{}', ?2), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
         ON CONFLICT(plugin_id) DO UPDATE SET
             config_json = CASE WHEN json_valid(plugin_config.config_json)
                 THEN json_patch(plugin_config.config_json, ?2)
                 ELSE json_patch('{}', ?2)
             END,
             updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(plugin_id)
    .bind(json_str)
    .execute(pool)
    .await?;

    Ok(())
}

/// Delete the configuration for a plugin.
pub async fn delete_plugin_config(pool: &SqlitePool, plugin_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM plugin_config WHERE plugin_id = ?1")
        .bind(plugin_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// List all plugin IDs that have stored configurations.
pub async fn list_plugin_configs(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT plugin_id FROM plugin_config ORDER BY plugin_id")
            .fetch_all(pool)
            .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_pool;
    use serde_json::json;

    // B142: a status write and a config write must not wipe each other.
    #[tokio::test]
    async fn b142_set_plugin_config_merges_top_level_keys() {
        let pool = test_pool().await;
        set_plugin_config(&pool, "tmdb", &json!({ "api_key": "k1" }))
            .await
            .unwrap();
        set_plugin_config(&pool, "tmdb", &json!({ "status": "Active" }))
            .await
            .unwrap();
        let stored = get_plugin_config(&pool, "tmdb").await.unwrap().unwrap();
        assert_eq!(stored, json!({ "api_key": "k1", "status": "Active" }));

        set_plugin_config(&pool, "tmdb", &json!({ "api_key": "k2" }))
            .await
            .unwrap();
        let stored = get_plugin_config(&pool, "tmdb").await.unwrap().unwrap();
        assert_eq!(stored, json!({ "api_key": "k2", "status": "Active" }));
    }

    #[tokio::test]
    async fn b142_set_plugin_config_null_removes_a_key() {
        let pool = test_pool().await;
        set_plugin_config(
            &pool,
            "omdb",
            &json!({ "api_key": "k", "status": "Active" }),
        )
        .await
        .unwrap();
        set_plugin_config(&pool, "omdb", &json!({ "api_key": null, "status": null }))
            .await
            .unwrap();
        let stored = get_plugin_config(&pool, "omdb").await.unwrap().unwrap();
        assert_eq!(stored, json!({}));

        // A first write stores no nulls either.
        set_plugin_config(
            &pool,
            "fanart",
            &json!({ "api_key": null, "status": "Disabled" }),
        )
        .await
        .unwrap();
        let stored = get_plugin_config(&pool, "fanart").await.unwrap().unwrap();
        assert_eq!(stored, json!({ "status": "Disabled" }));
    }

    #[tokio::test]
    async fn b142_set_plugin_config_replaces_invalid_stored_json() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO plugin_config (plugin_id, config_json) VALUES ('x', 'not json')")
            .execute(&pool)
            .await
            .unwrap();
        set_plugin_config(&pool, "x", &json!({ "status": "Active" }))
            .await
            .unwrap();
        let stored = get_plugin_config(&pool, "x").await.unwrap().unwrap();
        assert_eq!(stored, json!({ "status": "Active" }));
    }
}
