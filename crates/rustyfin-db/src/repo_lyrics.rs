//! The lyrics an audio item has (migration 107): the lyric file a scan found beside the track,
//! the one uploaded into the data folder, and whether the track's tags hold lyrics.

use crate::repo_media::PathScope;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// The stored lyrics of an item.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct ItemLyrics {
    pub sidecar_path: Option<String>,
    pub uploaded_path: Option<String>,
    pub embedded: bool,
}

const PRUNE: &str = "DELETE FROM item_lyrics WHERE item_id = ? \
     AND sidecar_path IS NULL AND uploaded_path IS NULL AND embedded = 0";

/// The stored lyrics of `item`, if it has any.
pub async fn get(pool: &SqlitePool, item: Uuid) -> Result<Option<ItemLyrics>, sqlx::Error> {
    sqlx::query_as::<_, ItemLyrics>(
        "SELECT sidecar_path, uploaded_path, embedded FROM item_lyrics WHERE item_id = ?",
    )
    .bind(item.to_string())
    .fetch_optional(pool)
    .await
}

/// Which of `items` have lyrics, as stored ids.
pub async fn with_lyrics(
    pool: &SqlitePool,
    items: &[Uuid],
) -> Result<HashSet<String>, sqlx::Error> {
    let mut found = HashSet::new();
    for chunk in items.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT item_id FROM item_lyrics WHERE item_id IN ({marks})");
        let mut query = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql));
        for id in chunk {
            query = query.bind(id.to_string());
        }
        found.extend(query.fetch_all(pool).await?);
    }
    Ok(found)
}

/// Records whether the tags of `item` hold lyrics.
pub async fn set_embedded(
    pool: &SqlitePool,
    item: Uuid,
    embedded: bool,
) -> Result<(), sqlx::Error> {
    let id = item.to_string();
    if embedded {
        sqlx::query(
            "INSERT INTO item_lyrics (item_id, embedded) VALUES (?, 1) \
             ON CONFLICT(item_id) DO UPDATE SET embedded = 1",
        )
        .bind(&id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE item_lyrics SET embedded = 0 WHERE item_id = ?")
            .bind(&id)
            .execute(pool)
            .await?;
        sqlx::query(PRUNE).bind(&id).execute(pool).await?;
    }
    Ok(())
}

/// Records the lyric file uploaded for `item`, or that it has none.
pub async fn set_uploaded(
    pool: &SqlitePool,
    item: Uuid,
    path: Option<&str>,
) -> Result<(), sqlx::Error> {
    let id = item.to_string();
    sqlx::query(
        "INSERT INTO item_lyrics (item_id, uploaded_path) VALUES (?1, ?2) \
         ON CONFLICT(item_id) DO UPDATE SET uploaded_path = ?2",
    )
    .bind(&id)
    .bind(path)
    .execute(pool)
    .await?;
    sqlx::query(PRUNE).bind(&id).execute(pool).await?;
    Ok(())
}

/// Records the lyric file beside `item`, or that it has none.
pub async fn set_sidecar(
    pool: &SqlitePool,
    item: Uuid,
    path: Option<&str>,
) -> Result<(), sqlx::Error> {
    set_sidecars(pool, &[(item, path.map(str::to_string))]).await
}

/// Records the lyric file beside each item, in one transaction.
pub async fn set_sidecars(
    pool: &SqlitePool,
    found: &[(Uuid, Option<String>)],
) -> Result<(), sqlx::Error> {
    if found.is_empty() {
        return Ok(());
    }
    let mut tx = crate::begin_write(pool).await?;
    for (item, path) in found {
        let id = item.to_string();
        sqlx::query(
            "INSERT INTO item_lyrics (item_id, sidecar_path) VALUES (?1, ?2) \
             ON CONFLICT(item_id) DO UPDATE SET sidecar_path = ?2",
        )
        .bind(&id)
        .bind(path.as_deref())
        .execute(&mut *tx)
        .await?;
        sqlx::query(PRUNE).bind(&id).execute(&mut *tx).await?;
    }
    tx.commit().await
}

/// The uploaded lyric files of the items of `library`, so that the ones whose item a removal
/// deletes can be deleted after it (see [`still_uploaded`]).
pub async fn uploads_in_library(
    pool: &SqlitePool,
    library: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT l.uploaded_path FROM item_lyrics AS l JOIN media_items AS m ON m.id = l.item_id \
         WHERE m.library_id = ? AND l.uploaded_path IS NOT NULL",
    )
    .bind(library.to_string())
    .fetch_all(pool)
    .await
}

/// Whether an item still has `path` as its uploaded lyric file.
pub async fn still_uploaded(pool: &SqlitePool, path: &str) -> Result<bool, sqlx::Error> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM item_lyrics WHERE uploaded_path = ? LIMIT 1")
            .bind(path)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// The audio items of `library` within `scope`, as id -> (file path, stored sidecar path).
pub async fn audio_sidecars(
    pool: &SqlitePool,
    library: Uuid,
    scope: &PathScope,
) -> Result<HashMap<Uuid, (String, Option<String>)>, sqlx::Error> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT m.id, m.path, l.sidecar_path FROM media_items AS m \
         LEFT JOIN item_lyrics AS l ON l.item_id = m.id \
         WHERE m.library_id = ?1 AND m.media_type = 'Audio' AND {}",
        scope.sql("m.path", 2)
    )))
    .bind(library.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, path, sidecar)| Uuid::parse_str(&id).ok().map(|id| (id, (path, sidecar))))
        .collect())
}
