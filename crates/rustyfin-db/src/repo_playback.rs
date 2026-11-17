use rustyfin_model::media::MediaType;
use rustyfin_model::playback::UserItemDataDto;
use rustyfin_model::server_info::ResumeRules;

use crate::writer::ExecuteWrite;
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
            item_id: None,
        },
        None => UserItemDataDto::default(),
    }
    .for_item(item_id))
}

/// For each Series or Season in `folder_ids`, counts its episodes and the ones `user_id` has not
/// played, as `(episodes, unplayed)`. A season's episodes are its children; a series' episodes
/// are those of its seasons, specials included. Virtual episodes (missing and unaired ones, with
/// no file) never count, as in Jellyfin. Only the episodes that pass `visible` (a
/// `repo_parental::filter_json` filter) count; `None` counts them all. Folders with no episodes
/// have no entry.
pub async fn get_unplayed_counts(
    pool: &SqlitePool,
    user_id: Uuid,
    folder_ids: &[Uuid],
    visible: Option<&str>,
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
    // The filter comes after the folder ids.
    let episodes = folder_episodes_sql(
        |folder| format!("{folder} IN ({placeholder_str})"),
        &format!("?{}", folder_ids.len() + 2),
    );
    let sql = format!(
        "SELECT ef.folder_id, COUNT(*), SUM(CASE WHEN ud.played = 1 THEN 0 ELSE 1 END) \
         FROM ({episodes}) ef \
         LEFT JOIN user_item_data ud ON ud.item_id = ef.episode_id AND ud.user_id = ?1 \
         GROUP BY ef.folder_id"
    );

    let mut query = sqlx::query_as::<_, (String, i64, i64)>(sqlx::AssertSqlSafe(sql));
    query = query.bind(&uid);
    for id in folder_ids {
        query = query.bind(id.to_string());
    }
    query = query.bind(visible);
    let rows = query.fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|(folder_id, episodes, unplayed)| (folder_id, (episodes, unplayed)))
        .collect())
}

/// The episodes that decide whether a Series or Season is played, as rows `(episode_id,
/// folder_id)`: a season's episodes, and a series' through its seasons, specials included. As in
/// Jellyfin 12.4, a virtual episode (missing or unaired, with no file) never counts, nor one the
/// user may not see: only those that pass the `repo_parental::filter_json` filter bound at
/// `visible_param` (NULL lets every episode pass). `scope` gives the condition on the folder's id
/// column it is handed (`s.id` for a season, `g.id` for a series); the episode is `e`. The one
/// definition behind the played badges and counts (`get_unplayed_counts`) and the IsPlayed and
/// IsUnplayed filters (`repo_media::get_items_filtered`).
pub(crate) fn folder_episodes_sql(scope: impl Fn(&str) -> String, visible_param: &str) -> String {
    let shown = crate::repo_parental::visible_sql("e.id", "e.library_id", visible_param);
    format!(
        "SELECT e.id AS episode_id, s.id AS folder_id \
         FROM media_items e JOIN media_items s ON s.id = e.parent_id AND s.media_type = 'Season' \
         WHERE e.media_type = 'Episode' AND e.is_virtual_item = 0 AND {shown} AND {season} \
         UNION ALL \
         SELECT e.id, g.id \
         FROM media_items e \
         JOIN media_items s ON s.id = e.parent_id AND s.media_type = 'Season' \
         JOIN media_items g ON g.id = s.parent_id AND g.media_type = 'Series' \
         WHERE e.media_type = 'Episode' AND e.is_virtual_item = 0 AND {shown} AND {series}",
        season = scope("s.id"),
        series = scope("g.id"),
    )
}

/// Whether a Series or Season counts as played from its `(episodes, unplayed)` counts: it has
/// episodes and all of them are played. Its stored flag does not decide.
pub fn folder_played(counts: (i64, i64)) -> bool {
    let (episodes, unplayed) = counts;
    episodes > 0 && unplayed == 0
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
    .execute_write(pool)
    .await?;

    Ok(())
}

/// The resume position, and whether the item is now played, after a client reports a position
/// in a progress or stopped report. Follows Jellyfin's `UserDataManager.UpdatePlayState` with the
/// server's resume rules (`ServerConfiguration::resume_rules`):
/// - before `MinResumePct` of the runtime, the resume point is cleared;
/// - past `MaxResumePct`, or within the last second, the item is played with no resume point;
/// - otherwise an item shorter than `MinResumeDurationSeconds` is played with no resume point,
///   and a longer one keeps the position;
/// - an item with no runtime above 0 is played with no resume point.
///
/// Played is only ever raised here, never cleared. A position of 0 or less keeps no resume point.
/// Rustyfin has no audiobook items, so the audiobook rules don't apply.
pub fn update_play_state(rules: &ResumeRules, runtime: Option<i64>, position: i64) -> (i64, bool) {
    let Some(runtime) = runtime.filter(|rt| *rt > 0) else {
        return (0, true);
    };
    if position <= 0 {
        return (0, false);
    }
    let (pos, rt) = (i128::from(position), i128::from(runtime));
    if pos * 100 < i128::from(rules.min_resume_pct) * rt {
        (0, false)
    } else if pos * 100 > i128::from(rules.max_resume_pct) * rt
        || pos >= rt - 10_000_000
        // Short items stopped midway count as played
        || rt < i128::from(rules.min_resume_duration_seconds) * 10_000_000
    {
        (0, true)
    } else {
        (position, false)
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
        MediaType::Series
        | MediaType::Season
        | MediaType::MusicAlbum
        | MediaType::MusicArtist
        | MediaType::Genre
        | MediaType::BoxSet
        | MediaType::Playlist => PlayedUpdate::Keep(false),
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
    .execute_write(pool)
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

/// Whether an item of `media_type` has a played flag (Jellyfin's `SupportsPlayedStatus`): videos,
/// audio and folders do; a photo or a genre (a plain `BaseItem` in Jellyfin) does not, so a
/// report leaves it unplayed with no resume point.
pub fn supports_played_status(media_type: MediaType) -> bool {
    !matches!(media_type, MediaType::Photo | MediaType::Genre)
}

/// A client reported progress: stores the resume position and raises the played flag as
/// `update_play_state` decides with the server's resume `rules`. An item whose runtime isn't
/// known yet (its probe hasn't run) is left alone: Jellyfin would count it played to the end,
/// which a stop still does.
pub async fn record_playback_progress(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    media_type: MediaType,
    position_ticks: i64,
    runtime_ticks: Option<i64>,
    rules: &ResumeRules,
) -> Result<(), sqlx::Error> {
    let played = if !supports_played_status(media_type) {
        (0, PlayedUpdate::Assign(false))
    } else if runtime_ticks.is_some_and(|rt| rt > 0) {
        let (position, played) = update_play_state(rules, runtime_ticks, position_ticks);
        (position, PlayedUpdate::Keep(played))
    } else {
        return Ok(());
    };
    write_playback_state(pool, user_id, item_id, played.0, played.1, 0).await
}

/// A client reported playback stopped. With a position, `update_play_state` decides with the
/// server's resume `rules`. Without one, Jellyfin assumes the item was played to the end: the
/// item is played and the play counted.
pub async fn record_playback_stop(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    media_type: MediaType,
    position_ticks: Option<i64>,
    runtime_ticks: Option<i64>,
    rules: &ResumeRules,
) -> Result<(), sqlx::Error> {
    let (position, played, play_count_delta) = match position_ticks {
        Some(position) => {
            let (position, played) = update_play_state(rules, runtime_ticks, position);
            (position, played, 0)
        }
        None => (0, true, 1),
    };
    // Without a played flag: no resume point and unplayed, the play still counted
    let (position, played) = if supports_played_status(media_type) {
        (position, PlayedUpdate::Keep(played))
    } else {
        (0, PlayedUpdate::Assign(false))
    };
    write_playback_state(pool, user_id, item_id, position, played, play_count_delta).await
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
    .execute_write(pool)
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

/// The episodes under a Series (through its seasons) or a Season, as a CTE `eps(id)` over `?1`.
/// Other items have none. Jellyfin's Folder.MarkPlayed/MarkUnplayed reach these episodes.
// Virtual episodes (missing and unaired, no file) are left out, as Jellyfin 12.4's
// Folder.MarkPlayed leaves them: a file that later takes one over must not arrive played.
const EPISODES_UNDER_FOLDER: &str = "WITH eps(id) AS ( \
     SELECT e.id FROM media_items f \
     JOIN media_items s ON s.parent_id = f.id AND s.media_type = 'Season' \
     JOIN media_items e ON e.parent_id = s.id AND e.media_type = 'Episode' \
       AND e.is_virtual_item = 0 \
     WHERE f.id = ?1 AND f.media_type = 'Series' \
     UNION \
     SELECT e.id FROM media_items f \
     JOIN media_items e ON e.parent_id = f.id AND e.media_type = 'Episode' \
       AND e.is_virtual_item = 0 \
     WHERE f.id = ?1 AND f.media_type = 'Season' ) ";

/// Marks an item played for the user now. See `mark_played_on`.
pub async fn mark_played(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<UserItemDataDto, sqlx::Error> {
    mark_played_on(pool, user_id, item_id, None).await
}

/// Marks an item played for the user, with `date_played` as its last played date, or now. As
/// Jellyfin's MarkPlayed does, the resume point is cleared. A Series or Season also marks each of
/// its episodes played, as an episode is marked alone, and the folder's own row is written too; all
/// in one transaction.
pub async fn mark_played_on(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    date_played: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let now = date_played.unwrap_or_else(chrono::Utc::now).to_rfc3339();
    let mut tx = crate::begin_write(pool).await?;

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, played, play_count, last_played_date) \
         VALUES (?, ?, 1, 1, ?) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 1, \
         play_count = play_count + 1, \
         playback_position_ticks = 0, \
         last_played_date = excluded.last_played_date",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    let episodes = format!(
        "{EPISODES_UNDER_FOLDER} INSERT INTO user_item_data (user_id, item_id, played, play_count, last_played_date) \
         SELECT ?2, id, 1, 1, ?3 FROM eps WHERE true \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 1, \
         play_count = user_item_data.play_count + 1, \
         playback_position_ticks = 0, \
         last_played_date = excluded.last_played_date"
    );
    sqlx::query(sqlx::AssertSqlSafe(episodes))
        .bind(&iid)
        .bind(&uid)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    get_user_item_data(pool, user_id, item_id).await
}

/// Marks an item unplayed for the user: not played, no plays, no resume point and no last played
/// date, as Jellyfin's MarkUnplayed leaves it. A Series or Season also marks each of its episodes
/// unplayed, as an episode is marked alone, and the folder's own row is written too; all in one
/// transaction.
pub async fn mark_unplayed(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    let mut tx = crate::begin_write(pool).await?;

    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, played, play_count) \
         VALUES (?, ?, 0, 0) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 0, \
         play_count = 0, \
         playback_position_ticks = 0, \
         last_played_date = NULL",
    )
    .bind(&uid)
    .bind(&iid)
    .execute(&mut *tx)
    .await?;

    let episodes = format!(
        "{EPISODES_UNDER_FOLDER} INSERT INTO user_item_data (user_id, item_id, played, play_count) \
         SELECT ?2, id, 0, 0 FROM eps WHERE true \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         played = 0, \
         play_count = 0, \
         playback_position_ticks = 0, \
         last_played_date = NULL"
    );
    sqlx::query(sqlx::AssertSqlSafe(episodes))
        .bind(&iid)
        .bind(&uid)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
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
    .execute_write(pool)
    .await?;

    get_user_item_data(pool, user_id, item_id).await
}

/// Sets the fields `update` carries and keeps the others (Jellyfin's UserDataManager.SaveUserData
/// for `POST /UserItems/{itemId}/UserData`). A Series or Season is written as stored; its answer
/// is derived from its episodes by the caller. `LastPlayedDate` is stored in RFC 3339 like every
/// other writer's (`parse_client_date`), and dropped when it doesn't parse.
pub async fn update_user_data(
    pool: &SqlitePool,
    user_id: Uuid,
    item_id: Uuid,
    update: &rustyfin_model::playback::UpdateUserItemDataDto,
) -> Result<UserItemDataDto, sqlx::Error> {
    let uid = user_id.to_string();
    let iid = item_id.to_string();
    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, playback_position_ticks, play_count, \
         is_favorite, likes, played, last_played_date) \
         VALUES (?1, ?2, COALESCE(?3, 0), COALESCE(?4, 0), COALESCE(?5, 0), ?6, COALESCE(?7, 0), ?8) \
         ON CONFLICT(user_id, item_id) DO UPDATE SET \
         playback_position_ticks = COALESCE(?3, playback_position_ticks), \
         play_count = COALESCE(?4, play_count), \
         is_favorite = COALESCE(?5, is_favorite), \
         likes = COALESCE(?6, likes), \
         played = COALESCE(?7, played), \
         last_played_date = COALESCE(?8, last_played_date)",
    )
    .bind(&uid)
    .bind(&iid)
    .bind(update.playback_position_ticks.map(|t| t.max(0)))
    .bind(update.play_count.map(|c| c.max(0)))
    .bind(update.is_favorite)
    .bind(update.likes)
    .bind(update.played)
    .bind(
        update
            .last_played_date
            .as_deref()
            .and_then(rustyfin_model::playback::parse_client_date)
            .map(|date| date.to_rfc3339()),
    )
    .execute_write(pool)
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
            .execute_write(pool)
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
            .execute_write(pool)
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
            .execute_write(pool)
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
            .execute_write(pool)
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
                .execute_write(pool)
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
        .execute_write(pool)
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
                item_id: None,
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
    .execute_write(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 36_000_000_000;

    /// The play state under Jellyfin's default resume rules.
    fn state(runtime: Option<i64>, position: i64) -> (i64, bool) {
        update_play_state(&ResumeRules::default(), runtime, position)
    }

    #[test]
    fn stop_past_the_end_marks_played_without_a_resume_point() {
        assert_eq!(state(Some(HOUR), HOUR), (0, true));
        assert_eq!(state(Some(HOUR), HOUR * 91 / 100), (0, true));
        assert_eq!(state(Some(HOUR), HOUR * 90 / 100), (HOUR * 90 / 100, false));
    }

    #[test]
    fn stop_before_five_percent_clears_the_resume_point() {
        assert_eq!(state(Some(HOUR), 0), (0, false));
        assert_eq!(state(Some(HOUR), -5), (0, false));
        assert_eq!(state(Some(HOUR), HOUR / 100), (0, false));
    }

    #[test]
    fn short_item_stopped_midway_counts_as_played() {
        let short = 299 * 10_000_000;
        assert_eq!(state(Some(short), short / 2), (0, true));
        let exact = 300 * 10_000_000;
        assert_eq!(state(Some(exact), exact / 2), (exact / 2, false));
    }

    #[test]
    fn unknown_runtime_counts_as_played() {
        assert_eq!(state(None, 5_000), (0, true));
        assert_eq!(state(Some(0), 5_000), (0, true));
    }

    /// The server's own MinResumePct, MaxResumePct and MinResumeDurationSeconds decide, not
    /// Jellyfin's defaults.
    #[test]
    fn r3gaps3_configured_resume_rules_decide() {
        let rules = ResumeRules {
            min_resume_pct: 20,
            max_resume_pct: 50,
            min_resume_duration_seconds: 2 * 3600,
            ..ResumeRules::default()
        };
        let state = |runtime, position| update_play_state(&rules, Some(runtime), position);
        // 10 %: kept under the defaults, before MinResumePct 20 here
        assert_eq!(state(4 * HOUR, 4 * HOUR / 10), (0, false));
        // 60 %: past MaxResumePct 50
        assert_eq!(state(4 * HOUR, 4 * HOUR * 6 / 10), (0, true));
        // 30 % of 4 hours: kept
        assert_eq!(
            state(4 * HOUR, 4 * HOUR * 3 / 10),
            (4 * HOUR * 3 / 10, false)
        );
        // 30 % of 1 hour: shorter than MinResumeDurationSeconds (2 hours), so played
        assert_eq!(state(HOUR, HOUR * 3 / 10), (0, true));

        let no_minimum = ResumeRules {
            min_resume_pct: 0,
            max_resume_pct: 100,
            min_resume_duration_seconds: 0,
            ..ResumeRules::default()
        };
        assert_eq!(
            update_play_state(&no_minimum, Some(HOUR), HOUR / 100),
            (HOUR / 100, false)
        );
        assert_eq!(
            update_play_state(&no_minimum, Some(HOUR), HOUR * 99 / 100),
            (HOUR * 99 / 100, false)
        );
        assert_eq!(update_play_state(&no_minimum, Some(HOUR), HOUR), (0, true));
        // The last second counts as the end
        let near_end = HOUR - 5_000_000;
        assert_eq!(
            update_play_state(&no_minimum, Some(HOUR), near_end),
            (0, true)
        );
    }
}
