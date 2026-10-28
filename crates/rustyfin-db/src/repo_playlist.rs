use chrono::{NaiveDateTime, TimeZone, Utc};
use rustyfin_model::media::MediaItem;
use rustyfin_model::playlist::PlaylistDto;
use sqlx::SqlitePool;
use uuid::Uuid;

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn create_playlist(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
    user_id: Uuid,
    media_type: Option<&str>,
    item_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let pid = id.to_string();
    let uid = user_id.to_string();
    sqlx::query("INSERT INTO playlists (id, name, user_id, media_type) VALUES (?1, ?2, ?3, ?4)")
        .bind(&pid)
        .bind(name)
        .bind(&uid)
        .bind(media_type)
        .execute(pool)
        .await?;

    for (i, item_id) in item_ids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?1, ?2, ?3)",
        )
        .bind(&pid)
        .bind(item_id.to_string())
        .bind(i as i32)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn get_playlists(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<PlaylistDto>, sqlx::Error> {
    let uid = user_id.to_string();
    let rows = sqlx::query_as::<_, (String, String, String, Option<String>, i64, String)>(
        "SELECT p.id, p.name, p.user_id, p.media_type, COUNT(pi.item_id), p.created_at
         FROM playlists p
         LEFT JOIN playlist_items pi ON pi.playlist_id = p.id
         WHERE p.user_id = ?1
         GROUP BY p.id
         ORDER BY p.name",
    )
    .bind(&uid)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(PlaylistDto {
                id: Uuid::parse_str(&r.0).ok()?,
                name: r.1,
                user_id: Uuid::parse_str(&r.2).ok()?,
                media_type: r.3,
                child_count: r.4,
                date_created: parse_dt(&r.5),
            })
        })
        .collect())
}

pub async fn get_playlist_items(
    pool: &SqlitePool,
    playlist_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let pid = playlist_id.to_string();
    let ids = sqlx::query_as::<_, (String,)>(
        "SELECT item_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY sort_order",
    )
    .bind(&pid)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::with_capacity(ids.len());
    for (id_str,) in ids {
        if let Ok(id) = Uuid::parse_str(&id_str)
            && let Ok(Some(item)) = crate::repo_media::get_item_by_id(pool, id).await
        {
            items.push(item);
        }
    }
    Ok(items)
}

pub async fn get_playlist(pool: &SqlitePool, id: Uuid) -> Result<Option<PlaylistDto>, sqlx::Error> {
    let pid = id.to_string();
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, i64, String)>(
        "SELECT p.id, p.name, p.user_id, p.media_type, COUNT(pi.item_id), p.created_at
         FROM playlists p
         LEFT JOIN playlist_items pi ON pi.playlist_id = p.id
         WHERE p.id = ?1
         GROUP BY p.id",
    )
    .bind(&pid)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|r| {
        Some(PlaylistDto {
            id: Uuid::parse_str(&r.0).ok()?,
            name: r.1,
            user_id: Uuid::parse_str(&r.2).ok()?,
            media_type: r.3,
            child_count: r.4,
            date_created: parse_dt(&r.5),
        })
    }))
}

pub async fn delete_playlist(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    let pid = id.to_string();
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?1")
        .bind(&pid)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM playlists WHERE id = ?1")
        .bind(&pid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn add_item_to_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let iid = item_id.to_string();
    sqlx::query("INSERT OR IGNORE INTO playlist_items (playlist_id, item_id) VALUES (?1, ?2)")
        .bind(&pid)
        .bind(&iid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_item_from_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let iid = item_id.to_string();
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?1 AND item_id = ?2")
        .bind(&pid)
        .bind(&iid)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update a playlist's name and/or media type
pub async fn update_playlist(
    pool: &SqlitePool,
    id: Uuid,
    name: Option<&str>,
    media_type: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let pid = id.to_string();

    match (name, media_type) {
        (Some(n), Some(mt)) => {
            sqlx::query("UPDATE playlists SET name = ?1, media_type = ?2 WHERE id = ?3")
                .bind(n)
                .bind(mt)
                .bind(&pid)
                .execute(pool)
                .await?;
        }
        (Some(n), None) => {
            sqlx::query("UPDATE playlists SET name = ?1 WHERE id = ?2")
                .bind(n)
                .bind(&pid)
                .execute(pool)
                .await?;
        }
        (None, Some(mt)) => {
            sqlx::query("UPDATE playlists SET media_type = ?1 WHERE id = ?2")
                .bind(mt)
                .bind(&pid)
                .execute(pool)
                .await?;
        }
        (None, None) => {}
    }

    Ok(true)
}
