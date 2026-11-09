use rustyfin_model::media::MediaType;
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
            played_percentage: None,
        },
        None => UserItemDataDto::default(),
    })
}

/// For each Series or Season in `folder_ids`, counts its episodes and the ones `user_id` has not
/// played, as `(episodes, unplayed)`. A season's episodes are its children; a series' episodes
/// are those of its seasons, specials included. Folders with no episodes have no entry.
pub async fn get_unplayed_counts(
    pool: &SqlitePool,
    user_id: Uuid,
    folder_ids: &[Uuid],
) -> Result<HashMap<String, (i64, i64)>, sqlx::Error> {
    if folder_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let uid = user_id.to_string();
    // ?1 is the user, so the folder ids start at ?2. Both branches reuse the same placeholders.
    let placeholders: Vec<String> = (2..=folder_ids.len() + 1)
        .map(|i| format!("?{}", i))
        .collect();
    let placeholder_str = placeholders.join(",");
    let sql = format!(
        "SELECT ef.folder_id, COUNT(*), SUM(CASE WHEN ud.played = 1 THEN 0 ELSE 1 END) \
         FROM ( \
             SELECT e.id AS episode_id, p.id AS folder_id \
             FROM media_items e JOIN media_items p ON p.id = e.parent_id \
             WHERE e.media_type = 'Episode' AND p.media_type = 'Season' AND p.id IN ({ph}) \
             UNION ALL \
             SELECT e.id, g.id \
             FROM media_items e \
             JOIN media_items p ON p.id = e.parent_id AND p.media_type = 'Season' \
             JOIN media_items g ON g.id = p.parent_id \
             WHERE e.media_type = 'Episode' AND g.media_type = 'Series' AND g.id IN ({ph}) \
         ) ef \
         LEFT JOIN user_item_data ud ON ud.item_id = ef.episode_id AND ud.user_id = ?1 \
         GROUP BY ef.folder_id",
        ph = placeholder_str
    );

    let mut query = sqlx::query_as::<_, (String, i64, i64)>(sqlx::AssertSqlSafe(sql));
    query = query.bind(&uid);
    for id in folder_ids {
        query = query.bind(id.to_string());
    }
    let rows = query.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|(folder_id, episodes, unplayed)| (folder_id, (episodes, unplayed)))
        .collect())
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

/// Jellyfin's default resume rules (MinResumePct, MaxResumePct, MinResumeDurationSeconds). Fixed,
/// not settings.
const MIN_RESUME_PCT: i128 = 5;
const MAX_RESUME_PCT: i128 = 90;
const MIN_RESUME_DURATION_TICKS: i128 = 300 * 10_000_000;

/// Whether a position is past `MaxResumePct` of the runtime, or at its end. Needs a runtime above 0.
fn past_resume_end(position: i64, runtime: i64) -> bool {
    let pos = position as i128;
    let rt = runtime as i128;
    pos * 100 > MAX_RESUME_PCT * rt || pos >= rt
}

/// The resume position and played flag after a client reports playback stopped at `position`.
/// Follows Jellyfin's UserDataManager.UpdatePlayState: past `MaxResumePct` (or at the end) the item
/// is played with no resume point; before `MinResumePct` the resume point is cleared and the played
/// flag kept; a short item (under `MinResumeDuration`) stopped in the middle counts as played;
/// otherwise the position is kept. An item with no runtime above 0 counts as played.
pub fn play_state_after_stop(runtime: Option<i64>, position: i64) -> (i64, bool) {
    match runtime.filter(|rt| *rt > 0) {
        None => (0, true),
        Some(rt) if past_resume_end(position, rt) => (0, true),
        Some(rt) if (position as i128) * 100 < MIN_RESUME_PCT * rt as i128 => (0, false),
        Some(rt) if (rt as i128) < MIN_RESUME_DURATION_TICKS => (0, true),
        Some(_) => (position, false),
    }
}

/// The resume position and played flag after a progress report. Only playing past `MaxResumePct`
/// (or to the end) changes anything: the item is played with no resume point. Otherwise the reported
/// position is kept and the played flag is left alone.
fn play_state_after_progress(runtime: Option<i64>, position: i64) -> (i64, bool) {
    match runtime.filter(|rt| *rt > 0) {
        Some(rt) if past_resume_end(position, rt) => (0, true),
        _ => (position, false),
    }
}

/// How a report sets the played flag: `Assign` sets it, `Keep` only raises it and never clears it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayedUpdate {
    Assign(bool),
    Keep(bool),
}

/// The played flag a start sets, as Jellyfin's OnPlaybackStart does: an item that resumes starts
/// unplayed, one that does not (Audio, Photo) starts played, and folders are left alone.
fn played_on_start(media_type: MediaType) -> PlayedUpdate {
    match media_type {
        MediaType::Series | MediaType::Season | MediaType::MusicAlbum | MediaType::Genre => {
            PlayedUpdate::Keep(false)
        }
        MediaType::Audio | MediaType::Photo => PlayedUpdate::Assign(true),
        _ => PlayedUpdate::Assign(false),
    }
}

/// One upsert for a playback report: sets the resume position, updates the played flag as `played`
/// says, adds `play_count_delta` to the count and stamps the last played date.
async fn write_playback_state(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: i64,
    played: PlayedUpdate,
    play_count_delta: i64,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let (assign, flag) = match played {
        PlayedUpdate::Assign(flag) => (true, flag),
        PlayedUpdate::Keep(flag) => (false, flag),
    };

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, playback_position_ticks, played, play_count, last_played_date) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         playback_position_ticks = excluded.playback_position_ticks, \
         played = CASE WHEN ? THEN excluded.played ELSE MAX(user_item_data.played, excluded.played) END, \
         play_count = user_item_data.play_count + excluded.play_count, \
         last_played_date = excluded.last_played_date",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(position_ticks)
    .bind(flag)
    .bind(play_count_delta)
    .bind(&now)
    .bind(assign)
    .execute(pool)
    .await?;

    Ok(())
}

/// A client reported playback started: counts a play, sets the played flag as Jellyfin's
/// OnPlaybackStart does for the item's media type, and stores the reported position (0 when the
/// client sends none).
pub async fn record_playback_start(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: i64,
    media_type: MediaType,
) -> Result<(), sqlx::Error> {
    write_playback_state(
        pool,
        user_id,
        item_id,
        position_ticks,
        played_on_start(media_type),
        1,
    )
    .await
}

/// A client reported progress: stores the position, or marks the item played with no resume point
/// once it is past `MaxResumePct`.
pub async fn record_playback_progress(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: i64,
    runtime_ticks: Option<i64>,
) -> Result<(), sqlx::Error> {
    let (position, played) = play_state_after_progress(runtime_ticks, position_ticks);
    write_playback_state(
        pool,
        user_id,
        item_id,
        position,
        PlayedUpdate::Keep(played),
        0,
    )
    .await
}

/// A client reported playback stopped. With a position, `play_state_after_stop` decides. Without
/// one, Jellyfin assumes the item was played to the end: the item is played and the play counted.
pub async fn record_playback_stop(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    position_ticks: Option<i64>,
    runtime_ticks: Option<i64>,
) -> Result<(), sqlx::Error> {
    let (position, played, play_count_delta) = match position_ticks {
        Some(position) => {
            let (position, played) = play_state_after_stop(runtime_ticks, position);
            (position, played, 0)
        }
        None => (0, true, 1),
    };
    write_playback_state(
        pool,
        user_id,
        item_id,
        position,
        PlayedUpdate::Keep(played),
        play_count_delta,
    )
    .await
}

/// A remembered track: the index of the stream a user chose, with the key of that stream when it
/// was chosen (`StreamIdentity::key`). The key is None when no stream of that kind was there, and
/// for subtitles off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamChoice {
    pub index: i32,
    pub key: Option<String>,
}

/// Remember the audio and subtitle stream a user chose for an item. A `None` leaves the stored
/// index and key as they are; a chosen index replaces both. A subtitle of -1 means subtitles were
/// turned off.
pub async fn save_stream_choice(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    audio: Option<&StreamChoice>,
    subtitle: Option<&StreamChoice>,
) -> Result<(), sqlx::Error> {
    if audio.is_none() && subtitle.is_none() {
        return Ok(());
    }
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, audio_stream_index, subtitle_stream_index, \
         audio_stream_key, subtitle_stream_key) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         audio_stream_index = COALESCE(excluded.audio_stream_index, user_item_data.audio_stream_index), \
         audio_stream_key = CASE WHEN excluded.audio_stream_index IS NULL \
         THEN user_item_data.audio_stream_key ELSE excluded.audio_stream_key END, \
         subtitle_stream_index = COALESCE(excluded.subtitle_stream_index, user_item_data.subtitle_stream_index), \
         subtitle_stream_key = CASE WHEN excluded.subtitle_stream_index IS NULL \
         THEN user_item_data.subtitle_stream_key ELSE excluded.subtitle_stream_key END",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(audio.map(|choice| choice.index))
    .bind(subtitle.map(|choice| choice.index))
    .bind(audio.and_then(|choice| choice.key.as_deref()))
    .bind(subtitle.and_then(|choice| choice.key.as_deref()))
    .execute(pool)
    .await?;

    Ok(())
}

/// The audio and subtitle stream a user last chose for an item, each with its key; `(None, None)`
/// when nothing is remembered.
pub async fn get_stream_choice(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<(Option<StreamChoice>, Option<StreamChoice>), sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();

    let row = sqlx::query_as::<_, (Option<i32>, Option<String>, Option<i32>, Option<String>)>(
        "SELECT audio_stream_index, audio_stream_key, subtitle_stream_index, subtitle_stream_key \
         FROM user_item_data WHERE user_id = ? AND item_id = ?",
    )
    .bind(&uid)
    .bind(&iid)
    .fetch_optional(pool)
    .await?;

    let (audio_index, audio_key, subtitle_index, subtitle_key) = row.unwrap_or_default();
    let choice =
        |index: Option<i32>, key: Option<String>| index.map(|index| StreamChoice { index, key });
    Ok((
        choice(audio_index, audio_key),
        choice(subtitle_index, subtitle_key),
    ))
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

/// Update last activity date for multiple items. Ids that are not media items are skipped.
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
             SELECT ?, id, ? FROM media_items WHERE id = ? \
             ON CONFLICT(user_id, item_id) DO UPDATE SET \
             last_played_date = excluded.last_played_date",
        )
        .bind(&uid)
        .bind(&now)
        .bind(&iid)
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
                played_percentage: None,
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

    const HOUR: i64 = 36_000_000_000;

    #[test]
    fn stop_past_the_end_marks_played_without_a_resume_point() {
        assert_eq!(play_state_after_stop(Some(HOUR), HOUR), (0, true));
        assert_eq!(
            play_state_after_stop(Some(HOUR), HOUR * 91 / 100),
            (0, true)
        );
        assert_eq!(
            play_state_after_stop(Some(HOUR), HOUR * 90 / 100),
            (HOUR * 90 / 100, false)
        );
    }

    #[test]
    fn stop_before_five_percent_clears_the_resume_point() {
        assert_eq!(play_state_after_stop(Some(HOUR), 0), (0, false));
        assert_eq!(play_state_after_stop(Some(HOUR), -5), (0, false));
        assert_eq!(play_state_after_stop(Some(HOUR), HOUR / 100), (0, false));
    }

    #[test]
    fn short_item_stopped_midway_counts_as_played() {
        let short = 299 * 10_000_000;
        assert_eq!(play_state_after_stop(Some(short), short / 2), (0, true));
        let exact = 300 * 10_000_000;
        assert_eq!(
            play_state_after_stop(Some(exact), exact / 2),
            (exact / 2, false)
        );
    }

    #[test]
    fn unknown_runtime_counts_as_played() {
        assert_eq!(play_state_after_stop(None, 5_000), (0, true));
        assert_eq!(play_state_after_stop(Some(0), 5_000), (0, true));
    }

    #[test]
    fn progress_only_marks_played_past_the_end() {
        assert_eq!(
            play_state_after_progress(Some(HOUR), HOUR / 2),
            (HOUR / 2, false)
        );
        assert_eq!(
            play_state_after_progress(Some(HOUR), HOUR * 95 / 100),
            (0, true)
        );
        assert_eq!(play_state_after_progress(None, HOUR), (HOUR, false));
    }
}
