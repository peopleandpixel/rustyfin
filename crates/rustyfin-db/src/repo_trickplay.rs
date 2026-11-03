use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use rustyfin_model::trickplay::TrickplayInfo;

/// Get trickplay info for an item.
pub async fn get_trickplay(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Option<TrickplayInfo>, sqlx::Error> {
    let item_id_str = item_id.to_string();
    let row = sqlx::query(
        "SELECT id, item_id, width, height, interval_ticks, thumbnail_count, directory_path
         FROM trickplay
         WHERE item_id = ?1
         LIMIT 1",
    )
    .bind(&item_id_str)
    .try_map(|row: sqlx::sqlite::SqliteRow| {
        let _id: String = row.try_get("id")?;
        let item_id: String = row.try_get("item_id")?;
        let width: u32 = row.try_get("width")?;
        let height: u32 = row.try_get("height")?;
        let interval_ticks: i64 = row.try_get("interval_ticks")?;
        let thumbnail_count: u32 = row.try_get("thumbnail_count")?;
        let directory_path: String = row.try_get("directory_path")?;
        Ok(TrickplayInfo {
            item_id,
            width,
            height,
            interval_ticks,
            thumbnail_count,
            directory_path,
        })
    })
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// Save trickplay info for an item.
pub async fn save_trickplay(
    pool: &SqlitePool,
    item_id: Uuid,
    info: &TrickplayInfo,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let item_id_str = item_id.to_string();
    sqlx::query(
        "INSERT OR REPLACE INTO trickplay (id, item_id, width, height, interval_ticks, thumbnail_count, directory_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&id)
    .bind(&item_id_str)
    .bind(info.width)
    .bind(info.height)
    .bind(info.interval_ticks)
    .bind(info.thumbnail_count)
    .bind(&info.directory_path)
    .execute(pool)
    .await?;

    Ok(())
}

/// Batch-get trickplay info for multiple items in a single query.
/// Returns a HashMap of item_id -> TrickplayInfo.
pub async fn get_trickplay_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<HashMap<String, TrickplayInfo>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: Vec<String> = (1..=item_ids.len()).map(|i| format!("?{}", i)).collect();
    let placeholder_str = placeholders.join(",");
    let sql = format!(
        "SELECT id, item_id, width, height, interval_ticks, thumbnail_count, directory_path
         FROM trickplay
         WHERE item_id IN ({})",
        placeholder_str
    );

    struct TrickplayRow {
        item_id: String,
        width: u32,
        height: u32,
        interval_ticks: i64,
        thumbnail_count: u32,
        directory_path: String,
    }

    impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for TrickplayRow {
        fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
            Ok(TrickplayRow {
                item_id: row.try_get("item_id")?,
                width: row.try_get("width")?,
                height: row.try_get("height")?,
                interval_ticks: row.try_get("interval_ticks")?,
                thumbnail_count: row.try_get("thumbnail_count")?,
                directory_path: row.try_get("directory_path")?,
            })
        }
    }

    let mut query = sqlx::query_as::<_, TrickplayRow>(sqlx::AssertSqlSafe(sql));
    for id in item_ids {
        query = query.bind(id.to_string());
    }

    let rows = query.fetch_all(pool).await?;

    let mut map = HashMap::new();
    for row in rows {
        map.insert(
            row.item_id.clone(),
            TrickplayInfo {
                item_id: row.item_id,
                width: row.width,
                height: row.height,
                interval_ticks: row.interval_ticks,
                thumbnail_count: row.thumbnail_count,
                directory_path: row.directory_path,
            },
        );
    }
    Ok(map)
}

/// Delete trickplay info for an item.
pub async fn delete_trickplay(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    sqlx::query("DELETE FROM trickplay WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, test_pool};

    fn make_trickplay(item_id: &str) -> TrickplayInfo {
        TrickplayInfo {
            item_id: item_id.to_string(),
            width: 80,
            height: 45,
            interval_ticks: 100000,
            thumbnail_count: 100,
            directory_path: "/tmp/trickplay".to_string(),
        }
    }

    #[tokio::test]
    async fn test_get_trickplay_empty() {
        let pool = test_pool().await;
        assert!(
            get_trickplay(&pool, Uuid::new_v4())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_save_and_get_trickplay() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        let info = make_trickplay(&item.to_string());
        save_trickplay(&pool, item, &info).await.unwrap();
        let got = get_trickplay(&pool, item).await.unwrap().unwrap();
        assert_eq!(got.width, 80);
    }

    #[tokio::test]
    async fn test_delete_trickplay() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        save_trickplay(&pool, item, &make_trickplay(&item.to_string()))
            .await
            .unwrap();
        delete_trickplay(&pool, item).await.unwrap();
        assert!(get_trickplay(&pool, item).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_trickplay_batch() {
        let pool = test_pool().await;
        let item1 = Uuid::new_v4();
        let item2 = Uuid::new_v4();
        insert_test_item(&pool, item1).await;
        insert_test_item(&pool, item2).await;
        save_trickplay(&pool, item1, &make_trickplay(&item1.to_string()))
            .await
            .unwrap();
        save_trickplay(&pool, item2, &make_trickplay(&item2.to_string()))
            .await
            .unwrap();
        let batch = get_trickplay_batch(&pool, &[item1, item2]).await.unwrap();
        assert_eq!(batch.len(), 2);
    }
}
