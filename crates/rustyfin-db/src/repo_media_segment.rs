use sqlx::Row;
use sqlx::SqlitePool;
use uuid::Uuid;

use rustyfin_model::media_segment::MediaSegment;

/// Get all media segments for an item.
pub async fn get_media_segments(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Vec<MediaSegment>, sqlx::Error> {
    let item_id_str = item_id.to_string();
    let rows = sqlx::query(
        "SELECT id, item_id, segment_type, segment_start_ticks, segment_end_ticks
         FROM media_segments
         WHERE item_id = ?1
         ORDER BY segment_start_ticks ASC",
    )
    .bind(&item_id_str)
    .try_map(|row: sqlx::sqlite::SqliteRow| {
        let _id: String = row.try_get("id")?;
        let item_id: String = row.try_get("item_id")?;
        let segment_type: String = row.try_get("segment_type")?;
        let segment_start_ticks: i64 = row.try_get("segment_start_ticks")?;
        let segment_end_ticks: i64 = row.try_get("segment_end_ticks")?;
        Ok(MediaSegment {
            item_id,
            segment_type,
            segment_start_ticks,
            segment_end_ticks,
        })
    })
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Save a media segment for an item.
pub async fn save_media_segment(
    pool: &SqlitePool,
    item_id: Uuid,
    segment_type: &str,
    segment_start_ticks: i64,
    segment_end_ticks: i64,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let item_id_str = item_id.to_string();
    sqlx::query(
        "INSERT OR REPLACE INTO media_segments (id, item_id, segment_type, segment_start_ticks, segment_end_ticks)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&id)
    .bind(&item_id_str)
    .bind(segment_type)
    .bind(segment_start_ticks)
    .bind(segment_end_ticks)
    .execute(pool)
    .await?;

    Ok(())
}

/// Delete all media segments for an item.
pub async fn delete_media_segments(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    sqlx::query("DELETE FROM media_segments WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    Ok(())
}
