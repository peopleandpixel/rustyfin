use sqlx::Row;
use sqlx::SqlitePool;
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

/// Delete all chapters for an item.
pub async fn delete_chapters(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    sqlx::query("DELETE FROM chapters WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    Ok(())
}
