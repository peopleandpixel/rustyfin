use sqlx::Row;
use sqlx::SqlitePool;
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

/// Delete trickplay info for an item.
pub async fn delete_trickplay(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let item_id_str = item_id.to_string();
    sqlx::query("DELETE FROM trickplay WHERE item_id = ?1")
        .bind(&item_id_str)
        .execute(pool)
        .await?;

    Ok(())
}
