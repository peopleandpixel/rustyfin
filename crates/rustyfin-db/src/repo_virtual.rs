//! Virtual items (Jellyfin 12.4's `IsVirtualItem`): the unaired and missing episodes, and the
//! seasons holding only those, that the TMDb missing-episode task makes from TMDb's episode lists.
//! They have no file (an empty path, `LocationType` Virtual). See migration 119.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

/// A series the missing-episode task looks at: one with a TMDb id.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesRow {
    pub id: Uuid,
    pub library_id: Uuid,
    pub name: String,
    pub tmdb_id: i64,
    /// TMDb's status as stored ("Returning Series", "Ended", "Canceled", ...)
    pub status: Option<String>,
}

/// A season of a series, virtual or not.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonRow {
    pub id: Uuid,
    pub index_number: Option<i32>,
    pub is_virtual: bool,
}

/// An episode of a series, virtual or not.
#[derive(Debug, Clone, PartialEq)]
pub struct EpisodeRow {
    pub id: Uuid,
    pub season_id: Option<Uuid>,
    /// The season number (`ParentIndexNumber`)
    pub season_number: Option<i32>,
    pub index_number: Option<i32>,
    pub index_number_end: Option<i32>,
    pub tmdb_id: Option<i64>,
    pub premiere_date: Option<DateTime<Utc>>,
    pub is_virtual: bool,
}

fn uuid(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text).ok()
}

fn date(text: Option<String>) -> Option<DateTime<Utc>> {
    text.and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Every series with a TMDb id, in name order.
pub async fn series_with_tmdb_id(pool: &SqlitePool) -> Result<Vec<SeriesRow>, sqlx::Error> {
    let rows: Vec<(String, String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, library_id, name, tmdb_id, status FROM media_items
         WHERE media_type = 'Series' AND tmdb_id IS NOT NULL AND tmdb_id > 0
           AND extra_type IS NULL
         ORDER BY sort_name, name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, library_id, name, tmdb_id, status)| {
            Some(SeriesRow {
                id: uuid(&id)?,
                library_id: uuid(&library_id)?,
                name,
                tmdb_id,
                status,
            })
        })
        .collect())
}

/// The seasons of a series.
pub async fn series_seasons(
    pool: &SqlitePool,
    series_id: Uuid,
) -> Result<Vec<SeasonRow>, sqlx::Error> {
    let rows: Vec<(String, Option<i32>, i64)> = sqlx::query_as(
        "SELECT id, index_number, is_virtual_item FROM media_items
         WHERE parent_id = ?1 AND media_type = 'Season' AND extra_type IS NULL",
    )
    .bind(series_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, index_number, is_virtual)| {
            Some(SeasonRow {
                id: uuid(&id)?,
                index_number,
                is_virtual: is_virtual != 0,
            })
        })
        .collect())
}

type EpisodeTuple = (
    String,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<i64>,
    Option<String>,
    i64,
);

/// The episodes of a series: those in its seasons and those directly under it.
pub async fn series_episodes(
    pool: &SqlitePool,
    series_id: Uuid,
) -> Result<Vec<EpisodeRow>, sqlx::Error> {
    let rows: Vec<EpisodeTuple> = sqlx::query_as(
        "SELECT e.id, e.parent_id, e.parent_index_number, e.index_number, e.index_number_end,
                e.tmdb_id, e.premiere_date, e.is_virtual_item
         FROM media_items e
         WHERE e.media_type = 'Episode' AND e.extra_type IS NULL
           AND (e.parent_id = ?1 OR e.parent_id IN (
                SELECT s.id FROM media_items s WHERE s.parent_id = ?1 AND s.media_type = 'Season'))",
    )
    .bind(series_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(id, parent, season, index, end, tmdb_id, premiere, is_virtual)| {
                Some(EpisodeRow {
                    id: uuid(&id)?,
                    season_id: parent.as_deref().and_then(uuid),
                    season_number: season,
                    index_number: index,
                    index_number_end: end,
                    tmdb_id,
                    premiere_date: date(premiere),
                    is_virtual: is_virtual != 0,
                })
            },
        )
        .collect())
}

/// Marks the items as virtual items.
pub async fn mark_virtual(pool: &SqlitePool, ids: &[Uuid]) -> Result<(), sqlx::Error> {
    for id in ids {
        sqlx::query("UPDATE media_items SET is_virtual_item = 1 WHERE id = ?1")
            .bind(id.to_string())
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Turns virtual items into real ones: a file has arrived for each (the upsert that stored the
/// file's path has already cleared the flag). Their date added becomes `now`, as a new item's
/// would be, so the episode shows in Latest. Returns how many changed.
pub async fn materialize(
    pool: &SqlitePool,
    ids: &[Uuid],
    now: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    let mut changed = 0;
    for id in ids {
        changed += sqlx::query(
            "UPDATE media_items SET is_virtual_item = 0, date_created = ?2 WHERE id = ?1",
        )
        .bind(id.to_string())
        .bind(now.to_rfc3339())
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(changed)
}

/// The virtual seasons of a library's series, by (series id, season number), and its virtual
/// episodes by (series id, season number, episode number): a scan gives a file that arrives for one
/// of them that item's id, so the item keeps its id and its user data.
#[derive(Debug, Default)]
pub struct LibraryVirtuals {
    pub seasons: HashMap<(Uuid, i32), Uuid>,
    pub episodes: HashMap<(Uuid, i32, i32), Uuid>,
}

pub async fn library_virtuals(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<LibraryVirtuals, sqlx::Error> {
    let lib = library_id.to_string();
    let seasons: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT id, parent_id, index_number FROM media_items
         WHERE library_id = ?1 AND media_type = 'Season' AND is_virtual_item = 1
           AND parent_id IS NOT NULL AND index_number IS NOT NULL",
    )
    .bind(&lib)
    .fetch_all(pool)
    .await?;
    let episodes: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "SELECT e.id, s.parent_id, e.parent_index_number, e.index_number
         FROM media_items e JOIN media_items s ON s.id = e.parent_id
         WHERE e.library_id = ?1 AND e.media_type = 'Episode' AND e.is_virtual_item = 1
           AND s.parent_id IS NOT NULL
           AND e.parent_index_number IS NOT NULL AND e.index_number IS NOT NULL",
    )
    .bind(&lib)
    .fetch_all(pool)
    .await?;
    let mut out = LibraryVirtuals::default();
    for (id, series, number) in seasons {
        if let (Some(id), Some(series)) = (uuid(&id), uuid(&series)) {
            out.seasons.insert((series, number), id);
        }
    }
    for (id, series, season, episode) in episodes {
        if let (Some(id), Some(series)) = (uuid(&id), uuid(&series)) {
            out.episodes.insert((series, season, episode), id);
        }
    }
    Ok(out)
}

/// Every virtual episode with a TMDb id (the ones the task made), across all libraries.
pub async fn all_virtual_episodes(pool: &SqlitePool) -> Result<Vec<Uuid>, sqlx::Error> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media_items
         WHERE media_type = 'Episode' AND is_virtual_item = 1 AND tmdb_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(ids.iter().filter_map(|id| uuid(id)).collect())
}

/// The virtual seasons with no episode left, across all libraries or in one series.
pub async fn empty_virtual_seasons(
    pool: &SqlitePool,
    series_id: Option<Uuid>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT s.id FROM media_items s
         WHERE s.media_type = 'Season' AND s.is_virtual_item = 1
           AND (?1 IS NULL OR s.parent_id = ?1)
           AND NOT EXISTS (SELECT 1 FROM media_items c WHERE c.parent_id = s.id)",
    )
    .bind(series_id.map(|id| id.to_string()))
    .fetch_all(pool)
    .await?;
    Ok(ids.iter().filter_map(|id| uuid(id)).collect())
}

/// The virtual seasons that hold a real episode: they become real seasons.
pub async fn settle_virtual_seasons(
    pool: &SqlitePool,
    series_id: Uuid,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE media_items SET is_virtual_item = 0
         WHERE media_type = 'Season' AND is_virtual_item = 1 AND parent_id = ?1
           AND EXISTS (SELECT 1 FROM media_items c
                       WHERE c.parent_id = media_items.id AND c.is_virtual_item = 0)",
    )
    .bind(series_id.to_string())
    .execute(pool)
    .await?
    .rows_affected())
}

/// Deletes a virtual item with its rows (user data, images, people links), and the virtual items
/// under it first. A real item, or one with a real child, is left alone. Returns whether it was
/// deleted.
pub async fn delete_virtual(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let mut tx = crate::begin_write(pool).await?;
    let deleted = delete_virtual_in(&mut tx, id).await?;
    tx.commit().await?;
    Ok(deleted)
}

/// `delete_virtual` on a connection that holds a write transaction.
pub async fn delete_virtual_in(
    conn: &mut sqlx::SqliteConnection,
    id: Uuid,
) -> Result<bool, sqlx::Error> {
    let id = id.to_string();
    // A row with a path is a file's, whatever its flag says: never deleted here
    let is_virtual: Option<i64> =
        sqlx::query_scalar("SELECT is_virtual_item FROM media_items WHERE id = ?1 AND path = ''")
            .bind(&id)
            .fetch_optional(&mut *conn)
            .await?;
    if is_virtual != Some(1) {
        return Ok(false);
    }
    if delete_virtual_children_in(conn, &id).await? {
        crate::repo_media::delete_item_rows_in(conn, &id).await?;
        return Ok(true);
    }
    Ok(false)
}

/// Deletes the virtual items under `id` (two levels: a series' virtual seasons, and the virtual
/// episodes in its seasons). Returns whether `id` has no child left.
pub async fn delete_virtual_children_in(
    conn: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<bool, sqlx::Error> {
    let children: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media_items WHERE parent_id = ?1 AND is_virtual_item = 1 AND path = ''",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    for child in children {
        let grandchildren: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM media_items WHERE parent_id = ?1 AND is_virtual_item = 1 AND path = ''",
        )
        .bind(&child)
        .fetch_all(&mut *conn)
        .await?;
        for grandchild in grandchildren {
            crate::repo_media::delete_item_rows_in(conn, &grandchild).await?;
        }
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_items WHERE parent_id = ?1")
            .bind(&child)
            .fetch_one(&mut *conn)
            .await?;
        if left == 0 {
            crate::repo_media::delete_item_rows_in(conn, &child).await?;
        }
    }
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM media_items WHERE parent_id = ?1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(left == 0)
}

/// Whether the item is a virtual item.
pub async fn is_virtual(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let flag: Option<i64> =
        sqlx::query_scalar("SELECT is_virtual_item FROM media_items WHERE id = ?1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(flag == Some(1))
}

/// Sets a series' status, as TMDb gives it now.
pub async fn set_series_status(
    pool: &SqlitePool,
    series_id: Uuid,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE media_items SET status = ?2 WHERE id = ?1 AND media_type = 'Series'")
        .bind(series_id.to_string())
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}
