//! Extras: trailers, special features and theme media. Each is a media item with an `extra_type` and the
//! `owner_id` of the movie, series or season it belongs to (migration 100). Library listings leave them out.

use std::collections::HashMap;

use rustyfin_model::media::MediaItem;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::repo_media::PathScope;

/// A stored item that can own extras: a movie, series or season that is not an extra itself.
pub struct OwnerRow {
    pub id: String,
    pub media_type: String,
    pub path: String,
}

/// The movies, series and seasons of `library`.
pub async fn owner_candidates(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<OwnerRow>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, media_type, path FROM media_items
         WHERE library_id = ?1 AND extra_type IS NULL
           AND media_type IN ('Movie', 'Series', 'Season')
         ORDER BY path, id",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, media_type, path)| OwnerRow {
            id,
            media_type,
            path,
        })
        .collect())
}

/// A stored item of a library by its path: its id, extra type and owner.
pub struct PathRow {
    pub id: String,
    pub extra_type: Option<String>,
    pub owner_id: Option<String>,
}

/// The items of `library` in `scope`, by path. A path held by several items gives the first by id.
pub async fn items_by_path(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<HashMap<String, PathRow>, sqlx::Error> {
    let rows: Vec<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, path, extra_type, owner_id FROM media_items
             WHERE library_id = ?1 AND {} ORDER BY id DESC",
            scope.sql("path", 2)
        )))
        .bind(library_id.to_string())
        .bind(scope.param())
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, path, extra_type, owner_id)| {
            (
                path,
                PathRow {
                    id,
                    extra_type,
                    owner_id,
                },
            )
        })
        .collect())
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
                SUM(extra_type NOT IN ('Trailer', 'ThemeSong', 'ThemeVideo'))
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

/// The owner of the extra `id`, if it is one.
pub async fn owner_of(pool: &SqlitePool, id: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    let owner: Option<Option<String>> =
        sqlx::query_scalar("SELECT owner_id FROM media_items WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(owner.flatten().and_then(|o| Uuid::parse_str(&o).ok()))
}
