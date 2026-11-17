use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use rustyfin_model::trickplay::TrickplayInfo;

const COLUMNS: &str =
    "item_id, width, height, tile_width, tile_height, thumbnail_count, interval_ms, bandwidth";

fn info_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<(String, TrickplayInfo), sqlx::Error> {
    let get = |name: &str| -> Result<u32, sqlx::Error> {
        let v: i64 = row.try_get(name)?;
        Ok(u32::try_from(v).unwrap_or(0))
    };
    Ok((
        row.try_get("item_id")?,
        TrickplayInfo {
            width: get("width")?,
            height: get("height")?,
            tile_width: get("tile_width")?,
            tile_height: get("tile_height")?,
            thumbnail_count: get("thumbnail_count")?,
            interval: get("interval_ms")?,
            bandwidth: get("bandwidth")?,
        },
    ))
}

/// The trickplay tile sets of an item, narrowest first.
pub async fn get_trickplay(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Vec<TrickplayInfo>, sqlx::Error> {
    let sql = format!("SELECT {COLUMNS} FROM trickplay_infos WHERE item_id = ?1 ORDER BY width");
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(item_id.to_string())
        .try_map(|row: sqlx::sqlite::SqliteRow| info_from_row(&row))
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|(_, info)| info).collect())
}

/// The trickplay tile set of an item at `width`, if it has one.
pub async fn get_trickplay_width(
    pool: &SqlitePool,
    item_id: Uuid,
    width: u32,
) -> Result<Option<TrickplayInfo>, sqlx::Error> {
    let sql = format!("SELECT {COLUMNS} FROM trickplay_infos WHERE item_id = ?1 AND width = ?2");
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(item_id.to_string())
        .bind(i64::from(width))
        .try_map(|row: sqlx::sqlite::SqliteRow| info_from_row(&row))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(_, info)| info))
}

/// Saves an item's tile set, replacing the one at the same width.
pub async fn save_trickplay(
    pool: &SqlitePool,
    item_id: Uuid,
    info: &TrickplayInfo,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO trickplay_infos
         (item_id, width, height, tile_width, tile_height, thumbnail_count, interval_ms, bandwidth)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(item_id.to_string())
    .bind(i64::from(info.width))
    .bind(i64::from(info.height))
    .bind(i64::from(info.tile_width))
    .bind(i64::from(info.tile_height))
    .bind(i64::from(info.thumbnail_count))
    .bind(i64::from(info.interval))
    .bind(i64::from(info.bandwidth))
    .execute(pool)
    .await?;
    Ok(())
}

/// The trickplay tile sets of several items, by item id, narrowest first.
pub async fn get_trickplay_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<HashMap<String, Vec<TrickplayInfo>>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (1..=item_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {COLUMNS} FROM trickplay_infos WHERE item_id IN ({placeholders})
         ORDER BY item_id, width"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in item_ids {
        query = query.bind(id.to_string());
    }
    let rows = query
        .try_map(|row: sqlx::sqlite::SqliteRow| info_from_row(&row))
        .fetch_all(pool)
        .await?;
    let mut map: HashMap<String, Vec<TrickplayInfo>> = HashMap::new();
    for (item_id, info) in rows {
        map.entry(item_id).or_default().push(info);
    }
    Ok(map)
}

/// Up to `limit` videos in the libraries `library_ids` with no tile set at `width` yet, with
/// their file paths and runtimes in ticks. Items `skip` names are left out (ones that failed).
pub async fn items_missing_trickplay(
    pool: &SqlitePool,
    library_ids: &[String],
    width: u32,
    skip: &[Uuid],
    limit: i64,
) -> Result<Vec<(Uuid, String, i64)>, sqlx::Error> {
    if library_ids.is_empty() {
        return Ok(Vec::new());
    }
    let libs = (3..library_ids.len() + 3)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let first_skip = library_ids.len() + 3;
    let skips = (first_skip..first_skip + skip.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT m.id, m.path, m.runtime_ticks FROM media_items m
         WHERE m.library_id IN ({libs}) AND m.media_type IN ('Movie', 'Episode')
           AND m.path IS NOT NULL AND m.path != '' AND m.runtime_ticks > 0
           AND m.id NOT IN ({skips})
           AND NOT EXISTS (SELECT 1 FROM trickplay_infos t WHERE t.item_id = m.id AND t.width = ?1)
         ORDER BY m.id LIMIT ?2"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(i64::from(width))
        .bind(limit);
    for id in library_ids {
        query = query.bind(id);
    }
    for id in skip {
        query = query.bind(id.to_string());
    }
    let rows = query
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            let id: String = row.try_get("id")?;
            let path: String = row.try_get("path")?;
            let runtime: i64 = row.try_get("runtime_ticks")?;
            Ok((id, path, runtime))
        })
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, path, runtime)| Some((Uuid::parse_str(&id).ok()?, path, runtime)))
        .collect())
}

/// For the trickplay task's progress, in one query over the libraries `library_ids`: how many
/// videos `items_missing_trickplay` would take if none had a tile set, and how many of those
/// already have one at `width` or a recorded `failure_kind` failure.
pub async fn trickplay_progress(
    pool: &SqlitePool,
    library_ids: &[String],
    width: u32,
    failure_kind: &str,
) -> Result<(u64, u64), sqlx::Error> {
    if library_ids.is_empty() {
        return Ok((0, 0));
    }
    let libs = (3..library_ids.len() + 3)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(
             EXISTS (SELECT 1 FROM trickplay_infos t WHERE t.item_id = m.id AND t.width = ?1)
             OR EXISTS (SELECT 1 FROM video_image_failures f
                        WHERE f.item_id = m.id AND f.kind = ?2)), 0)
         FROM media_items m
         WHERE m.library_id IN ({libs}) AND m.media_type IN ('Movie', 'Episode')
           AND m.path IS NOT NULL AND m.path != '' AND m.runtime_ticks > 0"
    );
    let mut query = sqlx::query_as::<_, (i64, i64)>(sqlx::AssertSqlSafe(sql))
        .bind(i64::from(width))
        .bind(failure_kind);
    for id in library_ids {
        query = query.bind(id);
    }
    let (total, done) = query.fetch_one(pool).await?;
    Ok((total.max(0) as u64, done.max(0) as u64))
}

/// The file version (size, modified seconds) an item's last `kind` image job failed on, if any.
pub async fn get_failure(
    pool: &SqlitePool,
    item_id: Uuid,
    kind: &str,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT file_size, file_mtime FROM video_image_failures WHERE item_id = ?1 AND kind = ?2",
    )
    .bind(item_id.to_string())
    .bind(kind)
    .fetch_optional(pool)
    .await
}

/// Records that an item's `kind` image job failed on the file at `version` (size, modified
/// seconds), so it isn't tried again until the file changes.
pub async fn record_failure(
    pool: &SqlitePool,
    item_id: Uuid,
    kind: &str,
    version: (i64, i64),
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO video_image_failures (item_id, kind, file_size, file_mtime)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(item_id.to_string())
    .bind(kind)
    .bind(version.0)
    .bind(version.1)
    .execute(pool)
    .await?;
    Ok(())
}

/// The file version (size, modified seconds) an item's tile set and chapter images were made
/// from, if any were made or tried.
pub async fn get_image_version(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as("SELECT file_size, file_mtime FROM video_image_versions WHERE item_id = ?1")
        .bind(item_id.to_string())
        .fetch_optional(pool)
        .await
}

/// Records the file version an item's images are made from, or forgets it (`None`).
pub async fn set_image_version(
    pool: &SqlitePool,
    item_id: Uuid,
    version: Option<(i64, i64)>,
) -> Result<(), sqlx::Error> {
    match version {
        Some((size, mtime)) => {
            sqlx::query(
                "INSERT OR REPLACE INTO video_image_versions (item_id, file_size, file_mtime)
                 VALUES (?1, ?2, ?3)",
            )
            .bind(item_id.to_string())
            .bind(size)
            .bind(mtime)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM video_image_versions WHERE item_id = ?1")
                .bind(item_id.to_string())
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Forgets an item's `kind` failure.
pub async fn clear_failure(
    pool: &SqlitePool,
    item_id: Uuid,
    kind: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM video_image_failures WHERE item_id = ?1 AND kind = ?2")
        .bind(item_id.to_string())
        .bind(kind)
        .execute(pool)
        .await?;
    Ok(())
}

/// Forgets every image failure of an item, and its chapters' failed-image marks, so they are
/// tried again: for a changed file, or a refresh that replaces all images.
pub async fn clear_failures(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM video_image_failures WHERE item_id = ?1")
        .bind(item_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("UPDATE chapters SET image_path = NULL WHERE item_id = ?1 AND image_path = ''")
        .bind(item_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes all of an item's tile sets.
pub async fn delete_trickplay(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM trickplay_infos WHERE item_id = ?1")
        .bind(item_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, test_pool};

    fn make_trickplay(width: u32) -> TrickplayInfo {
        TrickplayInfo {
            width,
            height: width * 9 / 16,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: 42,
            interval: 10_000,
            bandwidth: 12_345,
        }
    }

    async fn library_of(pool: &SqlitePool, item: Uuid) -> String {
        sqlx::query_scalar("SELECT library_id FROM media_items WHERE id = ?1")
            .bind(item.to_string())
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn r2chapters_trickplay_save_get_delete() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        assert!(get_trickplay(&pool, item).await.unwrap().is_empty());

        save_trickplay(&pool, item, &make_trickplay(320))
            .await
            .unwrap();
        save_trickplay(&pool, item, &make_trickplay(160))
            .await
            .unwrap();
        // Saving the same width again replaces it.
        let mut again = make_trickplay(320);
        again.thumbnail_count = 7;
        save_trickplay(&pool, item, &again).await.unwrap();

        let got = get_trickplay(&pool, item).await.unwrap();
        assert_eq!(got, [make_trickplay(160), again.clone()]);
        assert_eq!(
            get_trickplay_width(&pool, item, 320).await.unwrap(),
            Some(again)
        );
        assert_eq!(get_trickplay_width(&pool, item, 640).await.unwrap(), None);
        let batch = get_trickplay_batch(&pool, &[item, Uuid::new_v4()])
            .await
            .unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[&item.to_string()].len(), 2);

        delete_trickplay(&pool, item).await.unwrap();
        assert!(get_trickplay(&pool, item).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn r3trickplay_progress_counts_finished_and_failed_videos() {
        let pool = test_pool().await;
        let (made, failed, todo) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut libs = Vec::new();
        for item in [made, failed, todo] {
            insert_test_item(&pool, item).await;
            sqlx::query("UPDATE media_items SET runtime_ticks = 600000000 WHERE id = ?1")
                .bind(item.to_string())
                .execute(&pool)
                .await
                .unwrap();
            libs.push(library_of(&pool, item).await);
        }
        // Not a video the task takes: no runtime
        let other = Uuid::new_v4();
        insert_test_item(&pool, other).await;
        libs.push(library_of(&pool, other).await);

        assert_eq!(
            trickplay_progress(&pool, &libs, 320, "trickplay")
                .await
                .unwrap(),
            (3, 0)
        );
        save_trickplay(&pool, made, &make_trickplay(320))
            .await
            .unwrap();
        record_failure(&pool, failed, "trickplay", (1, 2))
            .await
            .unwrap();
        // A tile set at another width, or another kind of failure, isn't done
        save_trickplay(&pool, todo, &make_trickplay(160))
            .await
            .unwrap();
        record_failure(&pool, todo, "other", (1, 2)).await.unwrap();
        assert_eq!(
            trickplay_progress(&pool, &libs, 320, "trickplay")
                .await
                .unwrap(),
            (3, 2)
        );
        assert_eq!(
            trickplay_progress(&pool, &libs[..1], 320, "trickplay")
                .await
                .unwrap(),
            (1, 1)
        );
        assert_eq!(
            trickplay_progress(&pool, &[], 320, "trickplay")
                .await
                .unwrap(),
            (0, 0)
        );
    }

    #[tokio::test]
    async fn r2chapters_items_missing_trickplay() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        let lib = library_of(&pool, item).await;
        sqlx::query("UPDATE media_items SET runtime_ticks = 600000000 WHERE id = ?1")
            .bind(item.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let libs = std::slice::from_ref(&lib);

        let got = items_missing_trickplay(&pool, libs, 320, &[], 10)
            .await
            .unwrap();
        assert_eq!(got, [(item, "/tmp/test.mp4".to_string(), 600_000_000)]);
        assert!(
            items_missing_trickplay(&pool, libs, 320, &[item], 10)
                .await
                .unwrap()
                .is_empty()
        );
        save_trickplay(&pool, item, &make_trickplay(320))
            .await
            .unwrap();
        assert!(
            items_missing_trickplay(&pool, libs, 320, &[], 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            items_missing_trickplay(&pool, libs, 160, &[], 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
