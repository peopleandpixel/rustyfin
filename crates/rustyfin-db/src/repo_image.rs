use crate::repo_media::PathScope;
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
            Option<String>,
            Option<i64>,
        ),
    >(
        "SELECT item_id, image_type, image_index, path, provider, width, height, blurhash, stamp
         FROM item_images WHERE item_id = ? ORDER BY image_type, image_index",
    )
    .bind(&id_str)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(item_id, img_type, idx, path, provider, w, h, bh, stamp)| {
                let image_type = ImageType::from_str(&img_type)?;
                Some(ItemImage {
                    item_id: Uuid::parse_str(&item_id).ok()?,
                    image_type,
                    image_index: idx,
                    path,
                    provider,
                    width: w,
                    height: h,
                    blurhash: bh,
                    stamp,
                })
            },
        )
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
            Option<String>,
            Option<i64>,
        ),
    >(
        "SELECT item_id, image_type, image_index, path, provider, width, height, blurhash, stamp
         FROM item_images WHERE item_id = ? AND image_type = ? AND image_index = ?",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(image_index)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(
        |(item_id, img_type, idx, path, provider, w, h, bh, stamp)| {
            let image_type = ImageType::from_str(&img_type)?;
            Some(ItemImage {
                item_id: Uuid::parse_str(&item_id).ok()?,
                image_type,
                image_index: idx,
                path,
                provider,
                width: w,
                height: h,
                blurhash: bh,
                stamp,
            })
        },
    ))
}

/// The image of a person, from `person_images`; the returned `item_id` is the person's id.
pub async fn get_person_image(
    pool: &SqlitePool,
    person_id: Uuid,
    image_type: &ImageType,
    image_index: i32,
) -> Result<Option<ItemImage>, sqlx::Error> {
    let id_str = person_id.to_string();
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
            Option<String>,
        ),
    >(
        "SELECT person_id, image_type, image_index, path, provider, width, height, blurhash
         FROM person_images WHERE person_id = ? AND image_type = ? AND image_index = ?",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(image_index)
    .fetch_optional(pool)
    .await?;

    Ok(
        row.and_then(|(pid, img_type, idx, path, provider, w, h, bh)| {
            let image_type = ImageType::from_str(&img_type)?;
            Some(ItemImage {
                item_id: Uuid::parse_str(&pid).ok()?,
                image_type,
                image_index: idx,
                path,
                provider,
                width: w,
                height: h,
                blurhash: bh,
                stamp: None,
            })
        }),
    )
}

pub async fn upsert_image(pool: &SqlitePool, img: &ItemImage) -> Result<(), sqlx::Error> {
    let id_str = img.item_id.to_string();
    let type_str = img.image_type.as_str();
    sqlx::query(
        "INSERT INTO item_images (item_id, image_type, image_index, path, provider, width, height, blurhash, stamp)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id, image_type, image_index) DO UPDATE SET
           path = excluded.path,
           provider = excluded.provider,
           width = excluded.width,
           height = excluded.height,
           blurhash = excluded.blurhash,
           stamp = excluded.stamp",
    )
    .bind(&id_str)
    .bind(type_str)
    .bind(img.image_index)
    .bind(&img.path)
    .bind(&img.provider)
    .bind(img.width)
    .bind(img.height)
    .bind(&img.blurhash)
    .bind(img.stamp)
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
    get_images_batch_of(pool, item_ids, None, None).await
}

/// `get_images_batch` for the image types `types` only (all when None) and the first `per_type`
/// of each (all when None), so a list that shows one picture an item reads one row an item.
pub async fn get_images_batch_of(
    pool: &SqlitePool,
    item_ids: &[Uuid],
    types: Option<&[ImageType]>,
    per_type: Option<usize>,
) -> Result<std::collections::HashMap<String, Vec<ItemImage>>, sqlx::Error> {
    if item_ids.is_empty() || types.is_some_and(|t| t.is_empty()) || per_type == Some(0) {
        return Ok(std::collections::HashMap::new());
    }

    let id_strs: Vec<String> = item_ids.iter().map(|id| id.to_string()).collect();
    let placeholders: Vec<String> = (0..id_strs.len()).map(|i| format!("?{}", i + 1)).collect();
    // The type names come from ImageType::as_str and the count is a number, so both are inlined
    let type_clause = types
        .map(|types| {
            let names: Vec<String> = types.iter().map(|t| format!("'{}'", t.as_str())).collect();
            format!(" AND image_type IN ({})", names.join(","))
        })
        .unwrap_or_default();
    let index_clause = per_type
        .map(|n| format!(" AND image_index < {n}"))
        .unwrap_or_default();
    let query = format!(
        "SELECT item_id, image_type, image_index, path, provider, width, height, blurhash, stamp
         FROM item_images WHERE item_id IN ({}){type_clause}{index_clause}
         ORDER BY item_id, image_type, image_index",
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
            Option<String>,
            Option<i64>,
        ),
    >(sqlx::AssertSqlSafe(query.clone()));

    for id_str in &id_strs {
        q = q.bind(id_str);
    }

    let rows = q.fetch_all(pool).await?;

    let mut map: std::collections::HashMap<String, Vec<ItemImage>> =
        std::collections::HashMap::new();

    for (item_id, img_type, idx, path, provider, w, h, bh, stamp) in rows {
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
            blurhash: bh,
            stamp,
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
    // The write lock is taken first, as the scan's other writes do (see write_probe_outcome).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    for img in images {
        let id_str = img.item_id.to_string();
        let type_str = img.image_type.as_str();
        sqlx::query(
            "INSERT INTO item_images (item_id, image_type, image_index, path, provider, width, height, blurhash, stamp)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(item_id, image_type, image_index) DO UPDATE SET
               path = excluded.path,
               provider = excluded.provider,
               width = excluded.width,
               height = excluded.height,
               blurhash = excluded.blurhash,
               stamp = excluded.stamp",
        )
        .bind(&id_str)
        .bind(type_str)
        .bind(img.image_index)
        .bind(&img.path)
        .bind(&img.provider)
        .bind(img.width)
        .bind(img.height)
        .bind(&img.blurhash)
        .bind(img.stamp)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The local images of a library's items whose path is in `scope`. Remote images are left out, as
/// they have no file to stamp.
pub async fn get_local_images_in_scope(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<Vec<ItemImage>, sqlx::Error> {
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
            Option<String>,
            Option<i64>,
        ),
    >(sqlx::AssertSqlSafe(format!(
        "SELECT ii.item_id, ii.image_type, ii.image_index, ii.path, ii.provider, ii.width, ii.height, ii.blurhash, ii.stamp
         FROM item_images AS ii
         JOIN media_items AS m ON m.id = ii.item_id
         WHERE m.library_id = ?1 AND {} AND ii.path NOT GLOB 'http://*' AND ii.path NOT GLOB 'https://*'",
        scope.sql("ii.path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(item_id, img_type, idx, path, provider, w, h, bh, stamp)| {
                let image_type = ImageType::from_str(&img_type)?;
                Some(ItemImage {
                    item_id: Uuid::parse_str(&item_id).ok()?,
                    image_type,
                    image_index: idx,
                    path,
                    provider,
                    width: w,
                    height: h,
                    blurhash: bh,
                    stamp,
                })
            },
        )
        .collect())
}

/// The first Primary image of every item, in the order libraries list them by default (sort name,
/// then name), so that pre-generation warms the top of each library first.
pub async fn list_primary_images(pool: &SqlitePool) -> Result<Vec<ItemImage>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
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
            Option<String>,
            Option<i64>,
        ),
    >(
        "SELECT ii.item_id, ii.image_type, ii.image_index, ii.path, ii.provider, ii.width, ii.height,
                ii.blurhash, ii.stamp
         FROM item_images ii JOIN media_items mi ON mi.id = ii.item_id
         WHERE ii.image_type = 'Primary' AND ii.image_index = 0
         ORDER BY mi.sort_name, mi.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(item_id, img_type, idx, path, provider, w, h, bh, stamp)| {
                Some(ItemImage {
                    item_id: Uuid::parse_str(&item_id).ok()?,
                    image_type: ImageType::from_str(&img_type)?,
                    image_index: idx,
                    path,
                    provider,
                    width: w,
                    height: h,
                    blurhash: bh,
                    stamp,
                })
            },
        )
        .collect())
}

/// Stores the BlurHash of a stored image that has none yet. The row must still have `img`'s path
/// and stamp, so a picture replaced meanwhile does not get the old one's hash. Returns whether a
/// row changed.
pub async fn set_image_blurhash(
    pool: &SqlitePool,
    img: &ItemImage,
    blurhash: &str,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE item_images SET blurhash = ?
         WHERE item_id = ? AND image_type = ? AND image_index = ? AND path = ? AND stamp IS ?
           AND blurhash IS NULL",
    )
    .bind(blurhash)
    .bind(img.item_id.to_string())
    .bind(img.image_type.as_str())
    .bind(img.image_index)
    .bind(&img.path)
    .bind(img.stamp)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Sets the stamp, size and blurhash of each of `images`, stored images found by their item, type
/// and index, in one transaction, so a batch of restamps costs one commit. Either every image is
/// stored or, on error, none is. An empty `images` returns at once and takes no write lock.
pub async fn update_image_versions(
    pool: &SqlitePool,
    images: &[ItemImage],
) -> Result<(), sqlx::Error> {
    if images.is_empty() {
        return Ok(());
    }
    // The write lock is taken first, as the scan's other writes do (see write_probe_outcome).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    for img in images {
        sqlx::query(
            "UPDATE item_images SET stamp = ?, width = ?, height = ?, blurhash = ?
             WHERE item_id = ? AND image_type = ? AND image_index = ?",
        )
        .bind(img.stamp)
        .bind(img.width)
        .bind(img.height)
        .bind(&img.blurhash)
        .bind(img.item_id.to_string())
        .bind(img.image_type.as_str())
        .bind(img.image_index)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}
