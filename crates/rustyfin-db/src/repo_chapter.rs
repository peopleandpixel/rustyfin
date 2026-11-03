use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use rustyfin_model::chapter::Chapter;

/// Get all chapters for a video item.
pub async fn get_chapters(pool: &SqlitePool, item_id: Uuid) -> Result<Vec<Chapter>, sqlx::Error> {
    let item_id_str = item_id.to_string();
    let rows = sqlx::query(
        "SELECT id, item_id, chapter_number, name, start_ticks, end_ticks, image_path
         FROM chapters
         WHERE item_id = ?1
         ORDER BY chapter_number ASC",
    )
    .bind(&item_id_str)
    .try_map(|row: sqlx::sqlite::SqliteRow| {
        let id: String = row.try_get("id")?;
        let item_id: String = row.try_get("item_id")?;
        let chapter_number: i32 = row.try_get("chapter_number")?;
        let name: String = row.try_get("name")?;
        let start_ticks: i64 = row.try_get("start_ticks")?;
        let end_ticks: i64 = row.try_get("end_ticks")?;
        let image_path: Option<String> = row.try_get("image_path")?;
        Ok(Chapter {
            id,
            item_id,
            chapter_number,
            name,
            start_position_ticks: start_ticks,
            end_position_ticks: end_ticks,
            image_path,
        })
    })
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Save chapters for a video item, deleting existing ones first.
pub async fn save_chapters(
    pool: &SqlitePool,
    item_id: Uuid,
    chapters: Vec<Chapter>,
) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    // Delete existing chapters
    sqlx::query("DELETE FROM chapters WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    // Insert new chapters
    for chapter in &chapters {
        sqlx::query(
            "INSERT INTO chapters (id, item_id, chapter_number, name, start_ticks, end_ticks, image_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&chapter.id)
        .bind(&item_id_str)
        .bind(chapter.chapter_number)
        .bind(&chapter.name)
        .bind(chapter.start_position_ticks)
        .bind(chapter.end_position_ticks)
        .bind(&chapter.image_path)
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Batch-get chapters for multiple items in a single query.
/// Returns a HashMap of item_id -> Vec<Chapter>.
pub async fn get_chapters_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<HashMap<String, Vec<Chapter>>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders: Vec<String> = (1..=item_ids.len()).map(|i| format!("?{}", i)).collect();
    let placeholder_str = placeholders.join(",");
    let sql = format!(
        "SELECT id, item_id, chapter_number, name, start_ticks, end_ticks, image_path
         FROM chapters
         WHERE item_id IN ({})
         ORDER BY item_id, chapter_number ASC",
        placeholder_str
    );

    struct ChapterRow {
        id: String,
        item_id: String,
        chapter_number: i32,
        name: String,
        start_ticks: i64,
        end_ticks: i64,
        image_path: Option<String>,
    }

    impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for ChapterRow {
        fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
            Ok(ChapterRow {
                id: row.try_get("id")?,
                item_id: row.try_get("item_id")?,
                chapter_number: row.try_get("chapter_number")?,
                name: row.try_get("name")?,
                start_ticks: row.try_get("start_ticks")?,
                end_ticks: row.try_get("end_ticks")?,
                image_path: row.try_get("image_path")?,
            })
        }
    }

    let mut query = sqlx::query_as::<_, ChapterRow>(sqlx::AssertSqlSafe(sql));
    for id in item_ids {
        query = query.bind(id.to_string());
    }

    let rows = query.fetch_all(pool).await?;

    let mut map: HashMap<String, Vec<Chapter>> = HashMap::new();
    for row in rows {
        let chapter = Chapter {
            id: row.id,
            item_id: row.item_id.clone(),
            chapter_number: row.chapter_number,
            name: row.name,
            start_position_ticks: row.start_ticks,
            end_position_ticks: row.end_ticks,
            image_path: row.image_path,
        };
        map.entry(chapter.item_id.clone())
            .or_default()
            .push(chapter);
    }
    Ok(map)
}

/// Delete all chapters for an item.
pub async fn delete_chapters(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    sqlx::query("DELETE FROM chapters WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, test_pool};

    fn make_chapter(id: &str, number: i32, name: &str) -> Chapter {
        Chapter {
            id: id.to_string(),
            item_id: "dummy".to_string(),
            chapter_number: number,
            name: name.to_string(),
            start_position_ticks: number as i64 * 10000,
            end_position_ticks: (number + 1) as i64 * 10000,
            image_path: None,
        }
    }

    #[tokio::test]
    async fn test_get_chapters_empty() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        assert!(get_chapters(&pool, item).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_save_and_get_chapters() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        let chapters = vec![
            make_chapter("c1", 1, "Chapter 1"),
            make_chapter("c2", 2, "Chapter 2"),
        ];
        save_chapters(&pool, item, chapters).await.unwrap();
        let got = get_chapters(&pool, item).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Chapter 1");
    }

    #[tokio::test]
    async fn test_save_chapters_overwrites() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        save_chapters(&pool, item, vec![make_chapter("c1", 1, "Old")])
            .await
            .unwrap();
        save_chapters(&pool, item, vec![make_chapter("c2", 1, "New")])
            .await
            .unwrap();
        let got = get_chapters(&pool, item).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "New");
    }

    #[tokio::test]
    async fn test_delete_chapters() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        save_chapters(&pool, item, vec![make_chapter("c1", 1, "Del")])
            .await
            .unwrap();
        delete_chapters(&pool, item).await.unwrap();
        assert!(get_chapters(&pool, item).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_chapters_batch_empty() {
        let pool = test_pool().await;
        assert!(get_chapters_batch(&pool, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_chapters_batch() {
        let pool = test_pool().await;
        let item1 = Uuid::new_v4();
        let item2 = Uuid::new_v4();
        insert_test_item(&pool, item1).await;
        insert_test_item(&pool, item2).await;
        save_chapters(&pool, item1, vec![make_chapter("c1", 1, "Ch1")])
            .await
            .unwrap();
        save_chapters(&pool, item2, vec![make_chapter("c2", 1, "Ch2")])
            .await
            .unwrap();
        let batch = get_chapters_batch(&pool, &[item1, item2]).await.unwrap();
        assert_eq!(batch.len(), 2);
    }
}
