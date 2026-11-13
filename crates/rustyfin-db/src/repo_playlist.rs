use chrono::{NaiveDateTime, TimeZone, Utc};
use rustyfin_model::media::MediaItem;
use rustyfin_model::playlist::PlaylistDto;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Whose playlists a call may touch: the user's own, or any user's for an administrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistCaller {
    Owner(Uuid),
    Admin,
}

/// The owner id to bind into `(?N IS NULL OR user_id = ?N)`: None for an administrator.
fn owner_filter(caller: PlaylistCaller) -> Option<String> {
    match caller {
        PlaylistCaller::Owner(uid) => Some(uid.to_string()),
        PlaylistCaller::Admin => None,
    }
}

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
    caller: PlaylistCaller,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let pid = playlist_id.to_string();
    let owner = owner_filter(caller);
    let ids = sqlx::query_as::<_, (String,)>(
        "SELECT pi.item_id FROM playlist_items pi
         JOIN playlists p ON p.id = pi.playlist_id
         WHERE pi.playlist_id = ?1 AND (?2 IS NULL OR p.user_id = ?2)
         ORDER BY pi.sort_order",
    )
    .bind(&pid)
    .bind(owner.as_deref())
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

pub async fn get_playlist(
    pool: &SqlitePool,
    id: Uuid,
    caller: PlaylistCaller,
) -> Result<Option<PlaylistDto>, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, i64, String)>(
        "SELECT p.id, p.name, p.user_id, p.media_type, COUNT(pi.item_id), p.created_at
         FROM playlists p
         LEFT JOIN playlist_items pi ON pi.playlist_id = p.id
         WHERE p.id = ?1 AND (?2 IS NULL OR p.user_id = ?2)
         GROUP BY p.id",
    )
    .bind(&pid)
    .bind(owner.as_deref())
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

/// Delete a visible playlist and its items. Returns false when no playlist row matched.
pub async fn delete_playlist(
    pool: &SqlitePool,
    id: Uuid,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM playlist_items WHERE playlist_id = ?1 AND EXISTS (
            SELECT 1 FROM playlists WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2))",
    )
    .bind(&pid)
    .bind(owner.as_deref())
    .execute(&mut *tx)
    .await?;
    let deleted =
        sqlx::query("DELETE FROM playlists WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)")
            .bind(&pid)
            .bind(owner.as_deref())
            .execute(&mut *tx)
            .await?
            .rows_affected();
    tx.commit().await?;
    Ok(deleted > 0)
}

pub async fn add_item_to_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
    caller: PlaylistCaller,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let iid = item_id.to_string();
    let owner = owner_filter(caller);
    sqlx::query(
        "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id)
         SELECT ?1, ?2 WHERE EXISTS (
            SELECT 1 FROM playlists WHERE id = ?1 AND (?3 IS NULL OR user_id = ?3))",
    )
    .bind(&pid)
    .bind(&iid)
    .bind(owner.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_item_from_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
    caller: PlaylistCaller,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let iid = item_id.to_string();
    let owner = owner_filter(caller);
    sqlx::query(
        "DELETE FROM playlist_items WHERE playlist_id = ?1 AND item_id = ?2 AND EXISTS (
            SELECT 1 FROM playlists WHERE id = ?1 AND (?3 IS NULL OR user_id = ?3))",
    )
    .bind(&pid)
    .bind(&iid)
    .bind(owner.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}

/// Update a playlist's name and/or media type. Returns true when a visible playlist exists.
pub async fn update_playlist(
    pool: &SqlitePool,
    id: Uuid,
    name: Option<&str>,
    media_type: Option<&str>,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);

    let found = match (name, media_type) {
        (Some(n), Some(mt)) => {
            sqlx::query(
                "UPDATE playlists SET name = ?1, media_type = ?2 WHERE id = ?3 AND (?4 IS NULL OR user_id = ?4)",
            )
            .bind(n)
            .bind(mt)
            .bind(&pid)
            .bind(owner.as_deref())
            .execute(pool)
            .await?
            .rows_affected()
                > 0
        }
        (Some(n), None) => {
            sqlx::query("UPDATE playlists SET name = ?1 WHERE id = ?2 AND (?3 IS NULL OR user_id = ?3)")
                .bind(n)
                .bind(&pid)
                .bind(owner.as_deref())
                .execute(pool)
                .await?
                .rows_affected()
                > 0
        }
        (None, Some(mt)) => {
            sqlx::query("UPDATE playlists SET media_type = ?1 WHERE id = ?2 AND (?3 IS NULL OR user_id = ?3)")
                .bind(mt)
                .bind(&pid)
                .bind(owner.as_deref())
                .execute(pool)
                .await?
                .rows_affected()
                > 0
        }
        (None, None) => sqlx::query_scalar::<_, String>(
            "SELECT id FROM playlists WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)",
        )
        .bind(&pid)
        .bind(owner.as_deref())
        .fetch_optional(pool)
        .await?
        .is_some(),
    };

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, insert_test_user, test_pool};
    use sqlx::SqlitePool;
    use uuid::Uuid;

    async fn setup() -> (SqlitePool, Uuid, Uuid) {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let item = Uuid::new_v4();
        insert_test_user(&pool, user).await;
        insert_test_item(&pool, item).await;
        (pool, user, item)
    }

    #[tokio::test]
    async fn test_create_and_get_playlist() {
        let (pool, user, item) = setup().await;
        let id = Uuid::new_v4();
        create_playlist(&pool, id, "My Playlist", user, Some("Movie"), &[item])
            .await
            .unwrap();
        let playlist = get_playlist(&pool, id, PlaylistCaller::Owner(user))
            .await
            .unwrap();
        assert!(playlist.is_some());
        let playlist = playlist.unwrap();
        assert_eq!(playlist.name, "My Playlist");
        assert_eq!(playlist.child_count, 1);
    }

    #[tokio::test]
    async fn test_get_playlist_not_found() {
        let pool = test_pool().await;
        assert!(
            get_playlist(&pool, Uuid::new_v4(), PlaylistCaller::Admin)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_playlists_empty() {
        let (pool, user, _) = setup().await;
        assert!(get_playlists(&pool, user).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_playlists() {
        let (pool, user, _) = setup().await;
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        create_playlist(&pool, id1, "Alpha", user, None, &[])
            .await
            .unwrap();
        create_playlist(&pool, id2, "Beta", user, None, &[])
            .await
            .unwrap();
        let playlists = get_playlists(&pool, user).await.unwrap();
        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].name, "Alpha");
    }

    #[tokio::test]
    async fn test_delete_playlist() {
        let (pool, user, _) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "To Delete", user, None, &[])
            .await
            .unwrap();
        delete_playlist(&pool, pid, PlaylistCaller::Owner(user))
            .await
            .unwrap();
        assert!(
            get_playlist(&pool, pid, PlaylistCaller::Owner(user))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_add_and_remove_item() {
        let (pool, user, item) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "Test", user, None, &[])
            .await
            .unwrap();
        let caller = PlaylistCaller::Owner(user);
        add_item_to_playlist(&pool, pid, item, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.child_count, 1);
        remove_item_from_playlist(&pool, pid, item, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.child_count, 0);
    }

    #[tokio::test]
    async fn test_update_playlist_name() {
        let (pool, user, _) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "Old", user, None, &[])
            .await
            .unwrap();
        let caller = PlaylistCaller::Owner(user);
        update_playlist(&pool, pid, Some("New"), None, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.name, "New");
    }

    #[tokio::test]
    async fn test_get_playlist_items_empty() {
        let pool = test_pool().await;
        let items = get_playlist_items(&pool, Uuid::new_v4(), PlaylistCaller::Admin)
            .await
            .unwrap();
        assert!(items.is_empty());
    }
}
