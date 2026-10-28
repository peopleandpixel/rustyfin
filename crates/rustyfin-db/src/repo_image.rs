use rustyfin_model::image::{ImageType, ItemImage};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Return the set of image types that already exist for an item.
/// Used to avoid N+1 queries when storing provider images.
pub async fn get_image_types_for_item(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let id_str = item_id.to_string();
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT image_type FROM item_images WHERE item_id = ?",
    )
    .bind(&id_str)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(t,)| t).collect())
}

pub async fn get_images_for_item(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Vec<ItemImage>, sqlx::Error> {
    let id_str = item_id.to_string();
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            i32,
            String,
            String,
            Option<i32>,
            Option<i32>,
        ),
    >(
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
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            i32,
            String,
            String,
            Option<i32>,
            Option<i32>,
        ),
    >(
        "SELECT item_id, image_type, image_index, path, provider, width, height
         FROM item_images WHERE item_id = ? AND image_type = ? AND image_index = ?",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(image_index)
    .fetch_optional(pool)
    .await?;

    Ok(
        row.and_then(|(item_id, img_type, idx, path, provider, w, h)| {
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
        }),
    )
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

pub async fn delete_images_for_item(pool: &SqlitePool, item_id: Uuid) -> Result<(), sqlx::Error> {
    let id_str = item_id.to_string();
    sqlx::query("DELETE FROM item_images WHERE item_id = ?")
        .bind(&id_str)
        .execute(pool)
        .await?;
    Ok(())
}

/// Batch-fetch images for multiple items in a single query.
/// Returns a map of item_id -> Vec<ItemImage>.
pub async fn get_images_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<std::collections::HashMap<String, Vec<ItemImage>>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let id_strs: Vec<String> = item_ids.iter().map(|id| id.to_string()).collect();
    let placeholders: Vec<String> = (0..id_strs.len()).map(|i| format!("?{}", i + 1)).collect();
    let query = format!(
        "SELECT item_id, image_type, image_index, path, provider, width, height
         FROM item_images WHERE item_id IN ({}) ORDER BY item_id, image_type, image_index",
        placeholders.join(",")
    );

    let mut q = sqlx::query_as::<
        _,
        (
            String,
            String,
            i32,
            String,
            String,
            Option<i32>,
            Option<i32>,
        ),
    >(sqlx::AssertSqlSafe(query.clone()));

    for id_str in &id_strs {
        q = q.bind(id_str);
    }

    let rows = q.fetch_all(pool).await?;

    let mut map: std::collections::HashMap<String, Vec<ItemImage>> =
        std::collections::HashMap::new();

    for (item_id, img_type, idx, path, provider, w, h) in rows {
        let image_type = match ImageType::from_str(&img_type) {
            Some(t) => t,
            None => continue,
        };
        let uuid = match Uuid::parse_str(&item_id) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let img = ItemImage {
            item_id: uuid,
            image_type,
            image_index: idx,
            path,
            provider,
            width: w,
            height: h,
        };
        map.entry(item_id).or_default().push(img);
    }

    Ok(map)
}

/// Batch-insert multiple images in a single transaction.
/// Uses INSERT OR REPLACE so existing images of the same type are updated.
pub async fn batch_upsert_images(
    pool: &SqlitePool,
    images: &[ItemImage],
) -> Result<(), sqlx::Error> {
    if images.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for img in images {
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
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
