use rustyfin_model::playback::UserItemDataDto;
use sqlx::SqlitePool;
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
        },
        None => UserItemDataDto {
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            played: false,
            last_played_date: None,
            likes: None,
            user_rating: None,
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
