use rustyfin_model::image::{ImageType, ItemImage};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn get_images_for_item(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Vec<ItemImage>, sqlx::Error> {
    let id_str = item_id.to_string();
    let rows = sqlx::query_as::<_, (String, String, i32, String, String, Option<i32>, Option<i32>)>(
        "SELECT item_id, image_type, image_index, path, provider, width, height
         FROM item_images WHERE item_id = ? ORDER BY image_type, image_index",
    )
    .bind(&id_str)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(item_id, img_type, idx, path, provider, w, h)| {
            let image_type = ImageType::from_str(&img_type)?;
            Some(ItemImage {
                item_id: Uuid::parse_str(&item_id).ok()?,
                image_type,
                image_index: idx,
                path,
                provider,
                width: w,
                height: h,
            })
        })
        .collect())
}

pub async fn get_image(
    pool: &SqlitePool,
    item_id: Uuid,
    image_type: &ImageType,
    image_index: i32,
) -> Result<Option<ItemImage>, sqlx::Error> {
    let id_str = item_id.to_string();
    let type_str = image_type.as_str();
    let row = sqlx::query_as::<_, (String, String, i32, String, String, Option<i32>, Option<i32>)>(
        "SELECT item_id, image_type, image_index, path, provider, width, height
         FROM item_images WHERE item_id = ? AND image_type = ? AND image_index = ?",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(image_index)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|(item_id, img_type, idx, path, provider, w, h)| {
        let image_type = ImageType::from_str(&img_type)?;
        Some(ItemImage {
            item_id: Uuid::parse_str(&item_id).ok()?,
            image_type,
            image_index: idx,
            path,
            provider,
            width: w,
            height: h,
        })
    }))
}

pub async fn upsert_image(pool: &SqlitePool, img: &ItemImage) -> Result<(), sqlx::Error> {
    let id_str = img.item_id.to_string();
    let type_str = img.image_type.as_str();
    sqlx::query(
        "INSERT INTO item_images (item_id, image_type, image_index, path, provider, width, height)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id, image_type, image_index) DO UPDATE SET
           path = excluded.path,
           provider = excluded.provider,
           width = excluded.width,
           height = excluded.height",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(img.image_index)
    .bind(&img.path)
    .bind(&img.provider)
    .bind(img.width)
    .bind(img.height)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_images_for_item(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let id_str = item_id.to_string();
    sqlx::query("DELETE FROM item_images WHERE item_id = ?")
        .bind(&id_str)
        .execute(pool)
        .await?;
    Ok(())
}
