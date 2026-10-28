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
