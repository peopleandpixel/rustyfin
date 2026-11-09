use rustyfin_model::playback::UserItemDataDto;
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_user_item_data(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    let row = sqlx::query_as::<_, (i64, i64, bool, bool, Option<String>, Option<i32>, Option<bool>)>(
        "SELECT playback_position_ticks, play_count, is_favorite, played, last_played_date, user_rating, likes \
         FROM user_item_data WHERE user_id = ? AND item_id = ?",
    )
    .bind(&uid)
    .bind(&iid)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some((pos, count, fav, played, last, rating, liked)) => UserItemDataDto {
            playback_position_ticks: pos,
            play_count: count,
            is_favorite: fav,
            played,
            last_played_date: last,
            likes: liked,
            user_rating: rating,
            key: None,
            rating: None,
            unplayed_item_count: None,
            favorite: Some(fav),
        },
        None => UserItemDataDto {
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            played: false,
            last_played_date: None,
            likes: None,
            user_rating: None,
            key: None,
            rating: None,
            unplayed_item_count: None,
            favorite: None,
        },
    })
}

pub async fn update_playback_position(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: i64,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, playback_position_ticks, last_played_date) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         playback_position_ticks = excluded.playback_position_ticks, \
         last_played_date = excluded.last_played_date",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(position_ticks)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn mark_played(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, played, play_count, last_played_date) \
         VALUES (?, ?, 1, 1, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 1, \
         play_count = play_count + 1, \
         last_played_date = excluded.last_played_date",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(&now)
    .execute(pool)
    .await?;

    get_user_item_data(pool, user_id, item_id).await
}

pub async fn mark_unplayed(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, played, play_count) \
         VALUES (?, ?, 0, 0) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 0, \
         play_count = 0, \
         playback_position_ticks = 0",
    )
    .bind(&uid)
    .bind(&iid)
    .execute(pool)
    .await?;

    get_user_item_data(pool, user_id, item_id).await
}

pub async fn set_favorite(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    favorite: bool,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, is_favorite) \
         VALUES (?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         is_favorite = excluded.is_favorite",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(favorite)
    .execute(pool)
    .await?;

    get_user_item_data(pool, user_id, item_id).await
}

pub async fn on_playback_stopped(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: Option<i64>,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let pos = position_ticks.unwrap_or(0);

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, playback_position_ticks, last_played_date) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         playback_position_ticks = excluded.playback_position_ticks, \
         last_played_date = excluded.last_played_date",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(pos)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(())
}

/// Set user rating (likes and/or 1-5 star rating) for an item
pub async fn set_user_rating(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    likes: Option<bool>,
    user_rating: Option<i32>,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    // Handle all combinations of likes/user_rating explicitly
    match (likes, user_rating) {
        (Some(liked), Some(rating)) => {
            let liked_int = if liked { 1 } else { 0 };
            sqlx::query(
                "INSERT INTO user_item_data (user_id, item_id, likes, user_rating) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(user_id, item_id) DO UPDATE SET \
                 likes = excluded.likes, user_rating = excluded.user_rating",
            )
            .bind(&uid)
            .bind(&iid)
            .bind(liked_int)
            .bind(rating)
            .execute(pool)
            .await?;
        }
        (Some(liked), None) => {
            let liked_int = if liked { 1 } else { 0 };
            sqlx::query(
                "INSERT INTO user_item_data (user_id, item_id, likes) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(user_id, item_id) DO UPDATE SET \
                 likes = excluded.likes",
            )
            .bind(&uid)
            .bind(&iid)
            .bind(liked_int)
            .execute(pool)
            .await?;
        }
        (None, Some(rating)) => {
            sqlx::query(
                "INSERT INTO user_item_data (user_id, item_id, user_rating) \
                 VALUES (?, ?, ?) \
                 ON CONFLICT(user_id, item_id) DO UPDATE SET \
                 user_rating = excluded.user_rating",
            )
            .bind(&uid)
            .bind(&iid)
            .bind(rating)
            .execute(pool)
            .await?;
        }
        (None, None) => {
            // Clear rating
            sqlx::query(
                "INSERT INTO user_item_data (user_id, item_id) VALUES (?, ?) \
                 ON CONFLICT(user_id, item_id) DO UPDATE SET \
                 user_rating = NULL, likes = NULL",
            )
            .bind(&uid)
            .bind(&iid)
            .execute(pool)
            .await?;
        }
    }

    get_user_item_data(pool, user_id, item_id).await
}

/// Pin an episode for a series (used for "continue watching" a specific series)
pub async fn pin_episode(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Get the episode details to find the series
    let episode = sqlx::query_as::<_, (String, Option<String>, Option<i64>)>(
        "SELECT id, parent_id, index_number FROM media_items WHERE id = ?",
    )
    .bind(&iid)
    .fetch_optional(pool)
    .await?;

    match episode {
        Some((_ep_id, Some(season_id), ep_index)) => {
            // Get the series from the season
            let series =
                sqlx::query_as::<_, (String,)>("SELECT parent_id FROM media_items WHERE id = ?")
                    .bind(&season_id)
                    .fetch_optional(pool)
                    .await?;

            if let Some((series_id,)) = series {
                // Get season index
                let season_index = sqlx::query_as::<_, (Option<i64>,)>(
                    "SELECT index_number FROM media_items WHERE id = ?",
                )
                .bind(&season_id)
                .fetch_optional(pool)
                .await?;

                // Upsert pinned episode
                sqlx::query(
                    "INSERT INTO pinned_episodes (user_id, series_id, episode_id, season_id, season_index, episode_index, created_date) \
                     VALUES (?, ?, ?, ?, ?, ?, ?) \
                     ON CONFLICT(user_id, series_id) DO UPDATE SET \
                     episode_id = excluded.episode_id, \
                     season_id = excluded.season_id, \
                     season_index = excluded.season_index, \
                     episode_index = excluded.episode_index, \
                     created_date = excluded.created_date",
                )
                .bind(&uid)
                .bind(&series_id)
                .bind(&iid)
                .bind(&season_id)
                .bind(season_index.and_then(|s| s.0))
                .bind(ep_index)
                .bind(&now)
                .execute(pool)
                .await?;
            }
        }
        _ => {
            tracing::warn!(
                "Cannot pin episode: item {} is not a valid episode",
                item_id
            );
        }
    }

    Ok(())
}

/// Update last activity date for multiple items
pub async fn update_last_activity_date(
    pool: &SqlitePool,
    user_id: Uuid,
    item_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(());
    }

    let uid = user_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    for item_id in item_ids {
        let iid = item_id.to_string();
        sqlx::query(
            "INSERT INTO user_item_data (user_id, item_id, last_played_date) \
             VALUES (?, ?, ?) \
             ON CONFLICT(user_id, item_id) DO UPDATE SET \
             last_played_date = excluded.last_played_date",
        )
        .bind(&uid)
        .bind(&iid)
        .bind(&now)
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Get pinned episode for a user and series
pub async fn get_pinned_episode(
    pool: &SqlitePool,
    user_id: Uuid,
    series_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let uid = user_id.to_string();
    let sid = series_id.to_string();

    let row = sqlx::query_as::<_, (String,)>(
        "SELECT episode_id FROM pinned_episodes WHERE user_id = ? AND series_id = ?",
    )
    .bind(&uid)
    .bind(&sid)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((id_str,)) => Uuid::parse_str(&id_str).map(Some).map_err(|e| {
            tracing::error!("Invalid UUID in pinned_episodes: {}", e);
            sqlx::Error::RowNotFound
        }),
        None => Ok(None),
    }
}

/// Batch-get user item data for a single user and multiple items in one query.
/// Returns a HashMap of item_id -> UserItemDataDto.
pub async fn get_user_item_data_batch(
    pool: &SqlitePool,
    user_id: Uuid,
    item_ids: &[Uuid],
) -> Result<HashMap<String, UserItemDataDto>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let uid = user_id.to_string();
    // ?1 is the user, so the item ids start at ?2.
    let placeholders: Vec<String> = (2..=item_ids.len() + 1)
        .map(|i| format!("?{}", i))
        .collect();
    let placeholder_str = placeholders.join(",");
    let sql = format!(
        "SELECT item_id, playback_position_ticks, play_count, is_favorite, played, \
         last_played_date, user_rating, likes \
         FROM user_item_data WHERE user_id = ?1 AND item_id IN ({})",
        placeholder_str
    );

    let mut query = sqlx::query_as::<
        _,
        (
            String,
            i64,
            i64,
            bool,
            bool,
            Option<String>,
            Option<i32>,
            Option<bool>,
        ),
    >(sqlx::AssertSqlSafe(sql));
    query = query.bind(&uid);
    for id in item_ids {
        query = query.bind(id.to_string());
    }

    let rows = query.fetch_all(pool).await?;

    let mut map = HashMap::new();
    for (
        item_id,
        playback_position_ticks,
        play_count,
        is_favorite,
        played,
        last_played_date,
        user_rating,
        likes,
    ) in rows
    {
        map.insert(
            item_id.clone(),
            UserItemDataDto {
                playback_position_ticks,
                play_count,
                is_favorite,
                played,
                last_played_date,
                likes,
                user_rating,
                key: None,
                rating: None,
                unplayed_item_count: None,
                favorite: Some(is_favorite),
            },
        );
    }
    Ok(map)
}

/// Clear user rating for an item
pub async fn clear_user_rating(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    sqlx::query(
        "UPDATE user_item_data SET user_rating = NULL, likes = NULL WHERE user_id = ? AND item_id = ?",
    )
    .bind(&uid)
    .bind(&iid)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, insert_test_user, test_pool};

    async fn setup() -> (SqlitePool, Uuid, Uuid) {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let item = Uuid::new_v4();
        insert_test_user(&pool, user).await;
        insert_test_item(&pool, item).await;
        (pool, user, item)
    }

    #[tokio::test]
    async fn test_get_user_item_data_empty() {
        let (pool, user, item) = setup().await;
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.playback_position_ticks, 0);
        assert_eq!(data.play_count, 0);
        assert!(!data.is_favorite);
        assert!(!data.played);
        assert!(data.last_played_date.is_none());
        assert!(data.user_rating.is_none());
        assert!(data.favorite.is_none());
    }

    #[tokio::test]
    async fn test_update_playback_position() {
        let (pool, user, item) = setup().await;
        update_playback_position(&pool, user, item, 12345)
            .await
            .unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.playback_position_ticks, 12345);
        assert!(data.last_played_date.is_some());
    }

    #[tokio::test]
    async fn test_update_playback_position_overwrite() {
        let (pool, user, item) = setup().await;
        update_playback_position(&pool, user, item, 100)
            .await
            .unwrap();
        update_playback_position(&pool, user, item, 200)
            .await
            .unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.playback_position_ticks, 200);
    }

    #[tokio::test]
    async fn test_mark_played() {
        let (pool, user, item) = setup().await;
        let data = mark_played(&pool, user, item).await.unwrap();
        assert!(data.played);
        assert_eq!(data.play_count, 1);
        assert!(data.last_played_date.is_some());
    }

    #[tokio::test]
    async fn test_mark_played_increment() {
        let (pool, user, item) = setup().await;
        mark_played(&pool, user, item).await.unwrap();
        mark_played(&pool, user, item).await.unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.play_count, 2);
    }

    #[tokio::test]
    async fn test_mark_unplayed() {
        let (pool, user, item) = setup().await;
        mark_played(&pool, user, item).await.unwrap();
        let data = mark_unplayed(&pool, user, item).await.unwrap();
        assert!(!data.played);
        assert_eq!(data.play_count, 0);
        assert_eq!(data.playback_position_ticks, 0);
    }

    #[tokio::test]
    async fn test_set_favorite() {
        let (pool, user, item) = setup().await;
        let data = set_favorite(&pool, user, item, true).await.unwrap();
        assert!(data.is_favorite);
        let data = set_favorite(&pool, user, item, false).await.unwrap();
        assert!(!data.is_favorite);
    }

    #[tokio::test]
    async fn test_on_playback_stopped() {
        let (pool, user, item) = setup().await;
        on_playback_stopped(&pool, user, item, Some(999))
            .await
            .unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.playback_position_ticks, 999);
    }

    #[tokio::test]
    async fn test_on_playback_stopped_none() {
        let (pool, user, item) = setup().await;
        on_playback_stopped(&pool, user, item, None).await.unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.playback_position_ticks, 0);
    }

    #[tokio::test]
    async fn test_set_user_rating_both() {
        let (pool, user, item) = setup().await;
        let data = set_user_rating(&pool, user, item, Some(true), Some(5))
            .await
            .unwrap();
        assert_eq!(data.likes, Some(true));
        assert_eq!(data.user_rating, Some(5));
    }

    #[tokio::test]
    async fn test_set_user_rating_likes_only() {
        let (pool, user, item) = setup().await;
        let data = set_user_rating(&pool, user, item, Some(false), None)
            .await
            .unwrap();
        assert_eq!(data.likes, Some(false));
        assert_eq!(data.user_rating, None);
    }

    #[tokio::test]
    async fn test_set_user_rating_stars_only() {
        let (pool, user, item) = setup().await;
        let data = set_user_rating(&pool, user, item, None, Some(3))
            .await
            .unwrap();
        assert_eq!(data.user_rating, Some(3));
    }

    #[tokio::test]
    async fn test_set_user_rating_clear() {
        let (pool, user, item) = setup().await;
        set_user_rating(&pool, user, item, Some(true), Some(5))
            .await
            .unwrap();
        let data = set_user_rating(&pool, user, item, None, None)
            .await
            .unwrap();
        assert_eq!(data.likes, None);
        assert_eq!(data.user_rating, None);
    }

    #[tokio::test]
    async fn test_clear_user_rating() {
        let (pool, user, item) = setup().await;
        set_user_rating(&pool, user, item, Some(true), Some(4))
            .await
            .unwrap();
        clear_user_rating(&pool, user, item).await.unwrap();
        let data = get_user_item_data(&pool, user, item).await.unwrap();
        assert_eq!(data.likes, None);
        assert_eq!(data.user_rating, None);
    }

    #[tokio::test]
    async fn test_update_last_activity_date() {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let item1 = Uuid::new_v4();
        let item2 = Uuid::new_v4();
        insert_test_user(&pool, user).await;
        insert_test_item(&pool, item1).await;
        insert_test_item(&pool, item2).await;
        update_last_activity_date(&pool, user, &[item1, item2])
            .await
            .unwrap();
        let d1 = get_user_item_data(&pool, user, item1).await.unwrap();
        let d2 = get_user_item_data(&pool, user, item2).await.unwrap();
        assert!(d1.last_played_date.is_some());
        assert!(d2.last_played_date.is_some());
    }

    #[tokio::test]
    async fn test_update_last_activity_date_empty() {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        update_last_activity_date(&pool, user, &[]).await.unwrap();
    }

    #[tokio::test]
    async fn test_get_user_item_data_batch() {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let item1 = Uuid::new_v4();
        let item2 = Uuid::new_v4();
        let item3 = Uuid::new_v4();
        insert_test_user(&pool, user).await;
        insert_test_item(&pool, item1).await;
        insert_test_item(&pool, item2).await;
        insert_test_item(&pool, item3).await;
        mark_played(&pool, user, item1).await.unwrap();
        set_favorite(&pool, user, item2, true).await.unwrap();
        let batch = get_user_item_data_batch(&pool, user, &[item1, item2, item3])
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.get(&item1.to_string()).unwrap().played);
        assert!(batch.get(&item2.to_string()).unwrap().is_favorite);
        assert!(!batch.contains_key(&item3.to_string()));
    }

    #[tokio::test]
    async fn test_get_user_item_data_batch_empty() {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let batch = get_user_item_data_batch(&pool, user, &[]).await.unwrap();
        assert!(batch.is_empty());
    }

    #[tokio::test]
    async fn test_get_pinned_episode_empty() {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let series = Uuid::new_v4();
        let pinned = get_pinned_episode(&pool, user, series).await.unwrap();
        assert!(pinned.is_none());
    }
}
