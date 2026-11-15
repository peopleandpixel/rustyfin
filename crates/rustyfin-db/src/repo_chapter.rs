use sqlx::Row;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use rustyfin_model::chapter::Chapter;

/// Get all chapters for a video item.
pub async fn get_chapters(pool: &SqlitePool, item_id: Uuid) -> Result<Vec<Chapter>, sqlx::Error> {
    let item_id_str = item_id.to_string();
    let rows = sqlx::query(
        "SELECT id, item_id, chapter_number, name, start_ticks, end_ticks, image_path,
                image_date_modified
         FROM chapters
         WHERE item_id = ?1
         ORDER BY chapter_number ASC",
    )
    .bind(&item_id_str)
    .try_map(|row: sqlx::sqlite::SqliteRow| chapter_from_row(&row))
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

fn chapter_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Chapter, sqlx::Error> {
    Ok(Chapter {
        id: row.try_get("id")?,
        item_id: row.try_get("item_id")?,
        chapter_number: row.try_get("chapter_number")?,
        name: row.try_get("name")?,
        start_position_ticks: row.try_get("start_ticks")?,
        end_position_ticks: row.try_get("end_ticks")?,
        image_path: row.try_get("image_path")?,
        image_date_modified: row.try_get("image_date_modified")?,
    })
}

/// Save chapters for a video item, replacing the existing ones in one transaction,
/// so a save that fails keeps the old chapters.
pub async fn save_chapters(
    pool: &SqlitePool,
    item_id: Uuid,
    chapters: Vec<Chapter>,
) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    let mut tx = pool.begin().await?;
    // Delete existing chapters
    sqlx::query("DELETE FROM chapters WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(&mut *tx)
        .await?;

    // Insert new chapters
    for chapter in &chapters {
        sqlx::query(
            "INSERT INTO chapters (id, item_id, chapter_number, name, start_ticks, end_ticks,
                                   image_path, image_date_modified)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&chapter.id)
        .bind(&item_id_str)
        .bind(chapter.chapter_number)
        .bind(&chapter.name)
        .bind(chapter.start_position_ticks)
        .bind(chapter.end_position_ticks)
        .bind(&chapter.image_path)
        .bind(&chapter.image_date_modified)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

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
        "SELECT id, item_id, chapter_number, name, start_ticks, end_ticks, image_path,
                image_date_modified
         FROM chapters
         WHERE item_id IN ({})
         ORDER BY item_id, chapter_number ASC",
        placeholder_str
    );

    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
    for id in item_ids {
        query = query.bind(id.to_string());
    }

    let rows = query
        .try_map(|row: sqlx::sqlite::SqliteRow| chapter_from_row(&row))
        .fetch_all(pool)
        .await?;

    let mut map: HashMap<String, Vec<Chapter>> = HashMap::new();
    for chapter in rows {
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

/// Records chapter `chapter_number`'s image of item `item_id`: its path and when it was made. An
/// empty path marks an image that could not be made, so it isn't tried again.
pub async fn set_chapter_image(
    pool: &SqlitePool,
    item_id: Uuid,
    chapter_number: i32,
    image_path: &str,
    date_modified: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE chapters SET image_path = ?3, image_date_modified = ?4
         WHERE item_id = ?1 AND chapter_number = ?2",
    )
    .bind(item_id.to_string())
    .bind(chapter_number)
    .bind(image_path)
    .bind(date_modified)
    .execute(pool)
    .await?;
    Ok(())
}

/// Up to `limit` videos in the libraries `library_ids` that have a chapter with no image tried
/// yet, with their file paths.
pub async fn items_missing_chapter_images(
    pool: &SqlitePool,
    library_ids: &[String],
    limit: i64,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    if library_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (2..library_ids.len() + 2)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT m.id, m.path FROM media_items m
         WHERE m.library_id IN ({placeholders}) AND m.path IS NOT NULL AND m.path != ''
           AND EXISTS (SELECT 1 FROM chapters c WHERE c.item_id = m.id AND c.image_path IS NULL)
         ORDER BY m.id LIMIT ?1"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(limit);
    for id in library_ids {
        query = query.bind(id);
    }
    query
        .try_map(|row: sqlx::sqlite::SqliteRow| {
            let id: String = row.try_get("id")?;
            let path: String = row.try_get("path")?;
            Ok((id, path))
        })
        .fetch_all(pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .filter_map(|(id, path)| Some((Uuid::parse_str(&id).ok()?, path)))
                .collect()
        })
}

/// Up to `limit` videos whose chapters are still to be read once more, with their file paths.
pub async fn pending_chapter_rereads(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT r.item_id, m.path FROM chapter_rereads r
         JOIN media_items m ON m.id = r.item_id
         ORDER BY r.item_id LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, path)| Some((Uuid::parse_str(&id).ok()?, path)))
        .collect())
}

/// Takes `item_id` off the chapter re-read list.
pub async fn finish_chapter_reread(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM chapter_rereads WHERE item_id = ?1")
        .bind(item_id.to_string())
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
            image_date_modified: None,
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

    async fn library_of(pool: &SqlitePool, item: Uuid) -> String {
        sqlx::query_scalar("SELECT library_id FROM media_items WHERE id = ?1")
            .bind(item.to_string())
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn r2chapters_chapter_images_are_recorded_once() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        let lib = library_of(&pool, item).await;
        save_chapters(
            &pool,
            item,
            vec![make_chapter("c1", 1, "A"), make_chapter("c2", 2, "B")],
        )
        .await
        .unwrap();

        let todo = items_missing_chapter_images(&pool, std::slice::from_ref(&lib), 10)
            .await
            .unwrap();
        assert_eq!(todo, [(item, "/tmp/test.mp4".to_string())]);
        // Other libraries' items aren't listed.
        assert!(
            items_missing_chapter_images(&pool, &["other".to_string()], 10)
                .await
                .unwrap()
                .is_empty()
        );

        set_chapter_image(&pool, item, 1, "/data/1.jpg", Some("2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        // A failed image is marked with an empty path and isn't tried again.
        set_chapter_image(&pool, item, 2, "", None).await.unwrap();
        let got = get_chapters(&pool, item).await.unwrap();
        assert_eq!(got[0].image_path.as_deref(), Some("/data/1.jpg"));
        assert_eq!(
            got[0].image_date_modified.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(got[1].image_path.as_deref(), Some(""));
        assert!(
            items_missing_chapter_images(&pool, &[lib], 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn r2chapters_rereads_drain() {
        let pool = test_pool().await;
        let item = Uuid::new_v4();
        insert_test_item(&pool, item).await;
        sqlx::query("INSERT INTO chapter_rereads (item_id) VALUES (?1)")
            .bind(item.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let got = pending_chapter_rereads(&pool, 10).await.unwrap();
        assert_eq!(got, [(item, "/tmp/test.mp4".to_string())]);
        finish_chapter_reread(&pool, item).await.unwrap();
        assert!(pending_chapter_rereads(&pool, 10).await.unwrap().is_empty());
    }
}
