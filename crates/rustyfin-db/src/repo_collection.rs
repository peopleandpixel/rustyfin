use chrono::{NaiveDateTime, TimeZone, Utc};
use rustyfin_model::collection::CollectionDto;
use rustyfin_model::media::MediaItem;
use sqlx::SqlitePool;
use uuid::Uuid;

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn create_collection(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
    user_id: Uuid,
    item_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let cid = id.to_string();
    let uid = user_id.to_string();
    sqlx::query("INSERT INTO collections (id, name, user_id) VALUES (?1, ?2, ?3)")
        .bind(&cid)
        .bind(name)
        .bind(&uid)
        .execute(pool)
        .await?;

    for (i, item_id) in item_ids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO collection_items (collection_id, item_id, sort_order) VALUES (?1, ?2, ?3)",
        )
        .bind(&cid)
        .bind(item_id.to_string())
        .bind(i as i32)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn get_collections(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<CollectionDto>, sqlx::Error> {
    let uid = user_id.to_string();
    let rows = sqlx::query_as::<_, (String, String, String, i64, String)>(
        "SELECT c.id, c.name, c.user_id, COUNT(ci.item_id), c.created_at
         FROM collections c
         LEFT JOIN collection_items ci ON ci.collection_id = c.id
         WHERE c.user_id = ?1
         GROUP BY c.id
         ORDER BY c.name",
    )
    .bind(&uid)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(CollectionDto {
                id: Uuid::parse_str(&r.0).ok()?,
                name: r.1,
                user_id: Uuid::parse_str(&r.2).ok()?,
                child_count: r.3,
                date_created: parse_dt(&r.4),
            })
        })
        .collect())
}

pub async fn get_collection(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<CollectionDto>, sqlx::Error> {
    let cid = id.to_string();
    let row = sqlx::query_as::<_, (String, String, String, i64, String)>(
        "SELECT c.id, c.name, c.user_id, COUNT(ci.item_id), c.created_at
         FROM collections c
         LEFT JOIN collection_items ci ON ci.collection_id = c.id
         WHERE c.id = ?1
         GROUP BY c.id",
    )
    .bind(&cid)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|r| {
        Some(CollectionDto {
            id: Uuid::parse_str(&r.0).ok()?,
            name: r.1,
            user_id: Uuid::parse_str(&r.2).ok()?,
            child_count: r.3,
            date_created: parse_dt(&r.4),
        })
    }))
}

pub async fn delete_collection(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    let cid = id.to_string();
    sqlx::query("DELETE FROM collection_items WHERE collection_id = ?1")
        .bind(&cid)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM collections WHERE id = ?1")
        .bind(&cid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_collection_items(
    pool: &SqlitePool,
    collection_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let cid = collection_id.to_string();
    let ids: Vec<(String,)> = sqlx::query_as(
        "SELECT item_id FROM collection_items WHERE collection_id = ?1 ORDER BY sort_order",
    )
    .bind(&cid)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::with_capacity(ids.len());
    for (id_str,) in ids {
        let id = if let Ok(id) = Uuid::parse_str(&id_str) {
            id
        } else {
            continue;
        };
        if let Ok(Some(item)) = super::repo_media::get_item_by_id(pool, id).await {
            items.push(item);
        }
    }
    Ok(items)
}

pub async fn add_item_to_collection(
    pool: &SqlitePool,
    collection_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let cid = collection_id.to_string();
    let iid = item_id.to_string();

    // Get max sort_order
    let max_order: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(sort_order), -1) FROM collection_items WHERE collection_id = ?1",
    )
    .bind(&cid)
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO collection_items (collection_id, item_id, sort_order) VALUES (?1, ?2, ?3)",
    )
    .bind(&cid)
    .bind(&iid)
    .bind(max_order.0 + 1)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_item_from_collection(
    pool: &SqlitePool,
    collection_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let cid = collection_id.to_string();
    let iid = item_id.to_string();
    sqlx::query("DELETE FROM collection_items WHERE collection_id = ?1 AND item_id = ?2")
        .bind(&cid)
        .bind(&iid)
        .execute(pool)
        .await?;
    Ok(())
}
