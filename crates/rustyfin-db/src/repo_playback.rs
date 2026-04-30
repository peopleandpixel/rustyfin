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

    let row = sqlx::query_as::<_, (i64, i64, bool, bool, Option<String>)>(
        "SELECT playback_position_ticks, play_count, is_favorite, played, last_played_date \
         FROM user_item_data WHERE user_id = ? AND item_id = ?",
    )
    .bind(&uid)
    .bind(&iid)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some((pos, count, fav, played, last)) => UserItemDataDto {
            playback_position_ticks: pos,
            play_count: count,
            is_favorite: fav,
            played,
            last_played_date: last,
        },
        None => UserItemDataDto {
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite: false,
            played: false,
            last_played_date: None,
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
