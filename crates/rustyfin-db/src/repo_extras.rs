//! Extras: trailers, special features and theme media. Each is a media item with an `extra_type` and the
//! `owner_id` of the movie, series, season or episode it belongs to (migration 100). Library listings leave
//! them out.

use std::collections::HashMap;

use rustyfin_model::media::MediaItem;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::repo_media::PathScope;

/// The stored movies, series, seasons and episodes of `library` at `paths` (not extras themselves), as path to id. A
/// path held by several gives the first by id. Only these paths can own extras: the caller passes the files
/// and folders a scan just found, so a row whose file is gone never owns anything.
pub async fn owners_at(
    pool: &SqlitePool,
    library_id: Uuid,
    paths: &[String],
) -> Result<HashMap<String, String>, sqlx::Error> {
    if paths.is_empty() {
        return Ok(HashMap::new());
    }
    let paths = serde_json::to_string(paths).unwrap_or_else(|_| "[]".to_string());
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT path, MIN(id) FROM media_items
         WHERE library_id = ?1 AND extra_type IS NULL
           AND media_type IN ('Movie', 'Series', 'Season', 'Episode')
           AND path IN (SELECT value FROM json_each(?2))
         GROUP BY path",
    )
    .bind(library_id.to_string())
    .bind(paths)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// A stored item of a library by its path: its id, extra type and owner, its name, and the size and
/// modification time stored with its source at that path, when it has one.
pub struct PathRow {
    pub id: String,
    pub extra_type: Option<String>,
    pub owner_id: Option<String>,
    pub name: String,
    pub stamp: Option<(i64, i64)>,
}

/// The items of `library` in `scope`, by path. A path held by several items gives the first by id.
pub async fn items_by_path(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<HashMap<String, PathRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        Option<i64>,
        Option<i64>,
    )> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT m.id, m.path, m.extra_type, m.owner_id, m.name, s.size, s.modified
         FROM media_items AS m
         LEFT JOIN media_sources AS s ON s.item_id = m.id AND s.path = m.path
         WHERE m.library_id = ?1 AND {} ORDER BY m.id DESC",
        scope.sql("m.path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, path, extra_type, owner_id, name, size, modified)| {
            (
                path,
                PathRow {
                    id,
                    extra_type,
                    owner_id,
                    name,
                    stamp: size.zip(modified),
                },
            )
        })
        .collect())
}

/// Gives the stored extra `id`, whose file is gone, the file at `path` (renamed or moved): it keeps its id,
/// so its user data, and takes `owner_id`, `extra_type` and `media_type`. Its name becomes `name` when it was
/// its old file's name, and its source at `old_path` moves to `path`, so a probe of the unchanged file keeps
/// it. In one transaction.
#[allow(clippy::too_many_arguments)]
pub async fn move_extra(
    pool: &SqlitePool,
    id: &str,
    old_path: &str,
    old_name: &str,
    path: &str,
    owner_id: &str,
    extra_type: &str,
    media_type: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = crate::begin_write(pool).await?;
    sqlx::query(
        "UPDATE media_items SET path = ?2, owner_id = ?3, extra_type = ?4, media_type = ?5,
                name = CASE WHEN name = ?6 THEN ?7 ELSE name END,
                sort_name = CASE WHEN name = ?6 THEN ?7 ELSE sort_name END,
                parent_id = NULL
         WHERE id = ?1",
    )
    .bind(id)
    .bind(path)
    .bind(owner_id)
    .bind(extra_type)
    .bind(media_type)
    .bind(old_name)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE media_sources SET path = ?3 WHERE item_id = ?1 AND path = ?2")
        .bind(id)
        .bind(old_path)
        .bind(path)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Makes the item `id` an extra of `owner_id`: sets its type, extra type and name. An item that was a
/// library item before (a trailer that an older scan read as a movie) loses its parent.
pub async fn set_extra(
    pool: &SqlitePool,
    id: &str,
    owner_id: &str,
    extra_type: &str,
    media_type: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE media_items SET owner_id = ?2, extra_type = ?3, media_type = ?4, name = ?5,
                sort_name = ?5, parent_id = NULL
         WHERE id = ?1",
    )
    .bind(id)
    .bind(owner_id)
    .bind(extra_type)
    .bind(media_type)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}

/// Adds new extras, each `(item, owner id, extra type)`, in one write transaction: a row is written with
/// its owner and extra type, so no listing ever sees an extra as a library item.
pub async fn insert_extras(
    pool: &SqlitePool,
    extras: &[(MediaItem, String, &str)],
) -> Result<(), sqlx::Error> {
    if extras.is_empty() {
        return Ok(());
    }
    let mut tx = crate::begin_write(pool).await?;
    for (item, owner, extra_type) in extras {
        sqlx::query(
            "INSERT INTO media_items
                 (id, name, sort_name, media_type, path, library_id, genres, date_created,
                  location_type, extra_type, owner_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, '', ?7, ?8, ?9, ?10)",
        )
        .bind(item.id.to_string())
        .bind(&item.name)
        .bind(&item.sort_name)
        .bind(item.media_type.item_kind())
        .bind(&item.path)
        .bind(item.library_id.to_string())
        .bind(item.date_created.to_rfc3339())
        .bind(item.location_type.as_str())
        .bind(*extra_type)
        .bind(owner)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Merges each user's data on `part_id`, a movie that becomes a later part of `movie_id` (two movies
/// that a scan now stacks), into the movie: played or favourite if either was, the larger play count,
/// the later last played date. The movie keeps its own resume point, since the part's position is in
/// another file.
pub async fn merge_part_user_data(
    pool: &SqlitePool,
    part_id: &str,
    movie_id: &str,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, played, is_favorite, play_count, last_played_date)
         SELECT user_id, ?2, played, is_favorite, play_count, last_played_date
         FROM user_item_data WHERE item_id = ?1
         ON CONFLICT(user_id, item_id) DO UPDATE SET
             played = MAX(user_item_data.played, excluded.played),
             is_favorite = MAX(user_item_data.is_favorite, excluded.is_favorite),
             play_count = MAX(user_item_data.play_count, excluded.play_count),
             last_played_date = NULLIF(MAX(COALESCE(user_item_data.last_played_date, ''),
                                           COALESCE(excluded.last_played_date, '')), '')",
    )
    .bind(part_id)
    .bind(movie_id)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// Makes the stored extras of `library_id` at `paths` library items of type `media_type` again, keeping
/// their ids and user data: a file that is no longer an extra (its folder no longer holds its movie).
pub async fn release_extras(
    pool: &SqlitePool,
    library_id: Uuid,
    paths: &[String],
    media_type: &str,
) -> Result<u64, sqlx::Error> {
    if paths.is_empty() {
        return Ok(0);
    }
    let paths = serde_json::to_string(paths).unwrap_or_else(|_| "[]".to_string());
    let done = sqlx::query(
        "UPDATE media_items SET extra_type = NULL, owner_id = NULL, media_type = ?3
         WHERE library_id = ?1 AND extra_type IS NOT NULL
           AND path IN (SELECT value FROM json_each(?2))",
    )
    .bind(library_id.to_string())
    .bind(paths)
    .bind(media_type)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// Makes every stored extra of `library_id` in `scope` a library item again, keeping its id and user data:
/// for a scan that reads no extras (a library whose content type no longer has them). A video becomes
/// `video_type`; a theme song stays `Audio`. Returns how many were released.
pub async fn release_all_extras(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
    video_type: &str,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE media_items SET extra_type = NULL, owner_id = NULL,
                media_type = CASE media_type WHEN 'Audio' THEN 'Audio' ELSE ?3 END
         WHERE library_id = ?1 AND extra_type IS NOT NULL AND {}",
        scope.sql("path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .bind(video_type)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// The extras of `owner` whose extra type is one of `types`, by name: their ids and extra types.
pub async fn extras_of(
    pool: &SqlitePool,
    owner: Uuid,
    types: &[&str],
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let types = serde_json::to_string(types).unwrap_or_else(|_| "[]".to_string());
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, extra_type FROM media_items
         WHERE owner_id = ?1 AND extra_type IN (SELECT value FROM json_each(?2))
         ORDER BY sort_name COLLATE NOCASE, name, path",
    )
    .bind(owner.to_string())
    .bind(types)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, extra_type)| Uuid::parse_str(&id).ok().map(|id| (id, extra_type)))
        .collect())
}

/// How many local trailers and special features each of `owners` has, by owner id (as stored). Owners
/// with none are left out.
pub async fn extra_counts(
    pool: &SqlitePool,
    owners: &[String],
) -> Result<HashMap<String, (i32, i32)>, sqlx::Error> {
    if owners.is_empty() {
        return Ok(HashMap::new());
    }
    let ids = serde_json::to_string(owners).unwrap_or_else(|_| "[]".to_string());
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT owner_id,
                SUM(extra_type = 'Trailer'),
                SUM(extra_type NOT IN ('Trailer', 'ThemeSong', 'ThemeVideo', 'Part'))
         FROM media_items
         WHERE owner_id IN (SELECT value FROM json_each(?1))
         GROUP BY owner_id",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(owner, trailers, specials)| (owner, (trailers as i32, specials as i32)))
        .collect())
}

/// The ids of the items that own an extra with one of `types`: `/Items?HasTrailer`, `HasSpecialFeature`,
/// `HasThemeSong` and `HasThemeVideo` keep (true) or leave out (false) these. Only owners are read, so the
/// list is as long as the items that have such extras, never the whole library.
pub async fn owners_with_extras(
    pool: &SqlitePool,
    types: &[&str],
) -> Result<Vec<String>, sqlx::Error> {
    let types = serde_json::to_string(types).unwrap_or_else(|_| "[]".to_string());
    sqlx::query_scalar(
        "SELECT DISTINCT owner_id FROM media_items
         WHERE owner_id IS NOT NULL AND extra_type IN (SELECT value FROM json_each(?1))",
    )
    .bind(types)
    .fetch_all(pool)
    .await
}

/// How many parts after its first a movie split into files has (extras of type `Part`).
pub async fn additional_part_count(pool: &SqlitePool, owner: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM media_items WHERE owner_id = ?1 AND extra_type = 'Part'",
    )
    .bind(owner.to_string())
    .fetch_one(pool)
    .await
}

/// The owner of the extra `id`, if it is one.
pub async fn owner_of(pool: &SqlitePool, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    let owner: Option<Option<String>> =
        sqlx::query_scalar("SELECT owner_id FROM media_items WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(owner.flatten().and_then(|o| Uuid::parse_str(&o).ok()))
}
