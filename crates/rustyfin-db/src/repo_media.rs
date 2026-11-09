#![allow(
    clippy::needless_borrow,
    clippy::collapsible_if,
    clippy::needless_borrows_for_generic_args
)]

use chrono::{DateTime, Utc};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};
use std::collections::HashMap;
use std::sync::LazyLock;
use uuid::Uuid;

/// Saves an item. An existing item is updated in place, so it keeps its rowid, which the search
/// index is keyed on, and the rows that reference it; `INSERT OR REPLACE` deleted it first, and
/// the delete cascaded to them. It also keeps its stored date added (`date_created`), because
/// callers can save copies they read earlier.
pub async fn upsert_media_item(pool: &SqlitePool, item: &MediaItem) -> Result<(), sqlx::Error> {
    let id = item.id.to_string();
    let parent_id = item.parent_id.map(|p| p.to_string());
    let library_id = item.library_id.to_string();
    let media_type = item.media_type.item_kind().to_string();
    let genres = item.genres.join("|");
    let created = item.date_created.to_rfc3339();
    let premiere = item.premiere_date.map(|d| d.to_rfc3339());
    let end_date = item.end_date.map(|d| d.to_rfc3339());
    let tags = item.tags.join("|");

    sqlx::query(
        "INSERT INTO media_items \
         (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
          production_year, community_rating, official_rating, genres, date_created, \
          premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
          tags, status, original_language, tagline, production_countries, budget, revenue, \
          homepage, end_date, vote_count, popularity) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
          COALESCE((SELECT date_created FROM media_items WHERE id = ?1), ?13), \
          ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, \
          ?28, ?29, ?30, ?31, ?32) \
         ON CONFLICT(id) DO UPDATE SET \
          name = excluded.name, sort_name = excluded.sort_name, overview = excluded.overview, \
          media_type = excluded.media_type, path = excluded.path, parent_id = excluded.parent_id, \
          library_id = excluded.library_id, production_year = excluded.production_year, \
          community_rating = excluded.community_rating, \
          official_rating = excluded.official_rating, genres = excluded.genres, \
          premiere_date = excluded.premiere_date, runtime_ticks = excluded.runtime_ticks, \
          index_number = excluded.index_number, \
          index_number_end = excluded.index_number_end, location_type = excluded.location_type, \
          parent_index_number = excluded.parent_index_number, tmdb_id = excluded.tmdb_id, \
          tvdb_id = excluded.tvdb_id, tags = excluded.tags, status = excluded.status, \
          original_language = excluded.original_language, tagline = excluded.tagline, \
          production_countries = excluded.production_countries, budget = excluded.budget, \
          revenue = excluded.revenue, homepage = excluded.homepage, end_date = excluded.end_date, \
          vote_count = excluded.vote_count, popularity = excluded.popularity",
    )
    .bind(&id)
    .bind(&item.name)
    .bind(&item.sort_name)
    .bind(&item.overview)
    .bind(&media_type)
    .bind(&item.path)
    .bind(&parent_id)
    .bind(&library_id)
    .bind(item.production_year)
    .bind(item.community_rating)
    .bind(&item.official_rating)
    .bind(&genres)
    .bind(&created)
    .bind(&premiere)
    .bind(item.runtime_ticks)
    .bind(item.index_number)
    .bind(item.parent_index_number)
    .bind(item.index_number_end)
    .bind(item.location_type.as_str())
    .bind(item.tmdb_id)
    .bind(item.tvdb_id)
    .bind(&tags)
    .bind(&item.status)
    .bind(&item.original_language)
    .bind(item.taglines.first().map(|s| s.as_str()))
    .bind(item.production_locations.join("|"))
    .bind(item.budget)
    .bind(item.revenue)
    .bind(&item.homepage)
    .bind(&end_date)
    // The column is NOT NULL DEFAULT 0, which INSERT OR REPLACE filled in for a missing count.
    .bind(item.vote_count.unwrap_or(0))
    .bind(item.popularity)
    .execute(pool)
    .await?;

    Ok(())
}

/// A value bound into a refresh save's UPDATE. Each value is bound once in the SET list, once in
/// the WHERE guards, and once in the RETURNING flags, so it is cloned on every bind.
#[derive(Clone, PartialEq)]
enum RefreshValue {
    Text(Option<String>),
    Int(Option<i64>),
    Real(Option<f64>),
}

impl From<Option<String>> for RefreshValue {
    fn from(v: Option<String>) -> Self {
        Self::Text(v)
    }
}

impl From<Option<i32>> for RefreshValue {
    fn from(v: Option<i32>) -> Self {
        Self::Int(v.map(i64::from))
    }
}

impl From<Option<i64>> for RefreshValue {
    fn from(v: Option<i64>) -> Self {
        Self::Int(v)
    }
}

impl From<Option<f64>> for RefreshValue {
    fn from(v: Option<f64>) -> Self {
        Self::Real(v)
    }
}

/// One change a refresh save makes, for a column whose held value differs from the new one.
enum RefreshChange {
    /// A column guarded by `col IS ?held`, or by `(col IS NULL OR col = '')` when `blank` is set.
    Column {
        col: &'static str,
        blank: bool,
        held: RefreshValue,
        new: RefreshValue,
    },
    /// name and sort_name, guarded together by `name IS ? AND sort_name IS ?`.
    Names {
        held: (RefreshValue, RefreshValue),
        new: (RefreshValue, RefreshValue),
    },
}

/// Records a scalar column's change, if its held value differs from the new one.
fn scalar_change(
    changes: &mut Vec<RefreshChange>,
    col: &'static str,
    held: impl Into<RefreshValue>,
    new: impl Into<RefreshValue>,
) {
    let (held, new) = (held.into(), new.into());
    if held != new {
        changes.push(RefreshChange::Column {
            col,
            blank: false,
            held,
            new,
        });
    }
}

/// Records a text column's change, if its held value differs from the new one. A held value that
/// is NULL or empty is guarded as blank, so a stored blank still matches.
fn text_change(
    changes: &mut Vec<RefreshChange>,
    col: &'static str,
    held: Option<String>,
    new: Option<String>,
) {
    if held != new {
        let blank = held.as_deref().unwrap_or("").is_empty();
        changes.push(RefreshChange::Column {
            col,
            blank,
            held: RefreshValue::Text(held),
            new: RefreshValue::Text(new),
        });
    }
}

fn bind_value(qb: &mut QueryBuilder<Sqlite>, value: &RefreshValue) {
    match value.clone() {
        RefreshValue::Text(v) => qb.push_bind(v),
        RefreshValue::Int(v) => qb.push_bind(v),
        RefreshValue::Real(v) => qb.push_bind(v),
    };
}

/// Pushes the guard of one change, without the surrounding AND or OR.
fn push_guard(qb: &mut QueryBuilder<Sqlite>, change: &RefreshChange) {
    match change {
        RefreshChange::Column {
            col, blank: true, ..
        } => {
            qb.push("(")
                .push(*col)
                .push(" IS NULL OR ")
                .push(*col)
                .push(" = '')");
        }
        RefreshChange::Column { col, held, .. } => {
            qb.push(*col).push(" IS ");
            bind_value(qb, held);
        }
        RefreshChange::Names { held, .. } => {
            qb.push("(name IS ");
            bind_value(qb, &held.0);
            qb.push(" AND sort_name IS ");
            bind_value(qb, &held.1);
            qb.push(")");
        }
    }
}

/// Saves the columns a metadata refresh changed between `read` and `updated` as one autocommit
/// UPDATE on the pool, with no transaction. The statement sets only the changed columns, and each
/// one only while its stored value is still the one in `read`, so a write that landed since the
/// read wins. Returns the columns that hold the refresh's value after the save; an empty list
/// when no column changed or the row no longer passes any guard.
pub async fn save_refresh_changes(
    pool: &SqlitePool,
    read: &MediaItem,
    updated: &MediaItem,
) -> Result<Vec<&'static str>, sqlx::Error> {
    let mut changes = Vec::new();

    scalar_change(
        &mut changes,
        "overview",
        read.overview.clone(),
        updated.overview.clone(),
    );
    scalar_change(
        &mut changes,
        "production_year",
        read.production_year,
        updated.production_year,
    );
    scalar_change(
        &mut changes,
        "community_rating",
        read.community_rating,
        updated.community_rating,
    );
    scalar_change(
        &mut changes,
        "official_rating",
        read.official_rating.clone(),
        updated.official_rating.clone(),
    );
    text_change(
        &mut changes,
        "genres",
        Some(read.genres.join("|")),
        Some(updated.genres.join("|")),
    );
    scalar_change(
        &mut changes,
        "runtime_ticks",
        read.runtime_ticks,
        updated.runtime_ticks,
    );
    scalar_change(&mut changes, "tmdb_id", read.tmdb_id, updated.tmdb_id);
    scalar_change(&mut changes, "tvdb_id", read.tvdb_id, updated.tvdb_id);
    scalar_change(
        &mut changes,
        "status",
        read.status.clone(),
        updated.status.clone(),
    );
    scalar_change(
        &mut changes,
        "original_language",
        read.original_language.clone(),
        updated.original_language.clone(),
    );
    text_change(
        &mut changes,
        "tagline",
        read.taglines.first().cloned(),
        updated.taglines.first().cloned(),
    );
    text_change(
        &mut changes,
        "production_countries",
        Some(read.production_locations.join("|")),
        Some(updated.production_locations.join("|")),
    );
    scalar_change(
        &mut changes,
        "end_date",
        read.end_date.map(|d| d.to_rfc3339()),
        updated.end_date.map(|d| d.to_rfc3339()),
    );
    scalar_change(&mut changes, "budget", read.budget, updated.budget);
    scalar_change(&mut changes, "revenue", read.revenue, updated.revenue);
    scalar_change(
        &mut changes,
        "homepage",
        read.homepage.clone(),
        updated.homepage.clone(),
    );
    // The column is NOT NULL DEFAULT 0, so a missing count is stored as 0.
    scalar_change(
        &mut changes,
        "vote_count",
        Some(read.vote_count.unwrap_or(0)),
        Some(updated.vote_count.unwrap_or(0)),
    );
    scalar_change(
        &mut changes,
        "popularity",
        read.popularity,
        updated.popularity,
    );

    if read.name != updated.name || read.sort_name != updated.sort_name {
        changes.push(RefreshChange::Names {
            held: (
                RefreshValue::Text(Some(read.name.clone())),
                RefreshValue::Text(read.sort_name.clone()),
            ),
            new: (
                RefreshValue::Text(Some(updated.name.clone())),
                RefreshValue::Text(updated.sort_name.clone()),
            ),
        });
    }

    if changes.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb = QueryBuilder::<Sqlite>::new("UPDATE media_items SET ");
    for (i, change) in changes.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        match change {
            RefreshChange::Column { col, new, .. } => {
                qb.push(*col).push(" = CASE WHEN ");
                push_guard(&mut qb, change);
                qb.push(" THEN ");
                bind_value(&mut qb, new);
                qb.push(" ELSE ").push(*col).push(" END");
            }
            RefreshChange::Names { new, .. } => {
                qb.push("name = CASE WHEN ");
                push_guard(&mut qb, change);
                qb.push(" THEN ");
                bind_value(&mut qb, &new.0);
                qb.push(" ELSE name END, sort_name = CASE WHEN ");
                push_guard(&mut qb, change);
                qb.push(" THEN ");
                bind_value(&mut qb, &new.1);
                qb.push(" ELSE sort_name END");
            }
        }
    }

    qb.push(" WHERE id = ");
    qb.push_bind(read.id.to_string());
    qb.push(" AND (");
    for (i, change) in changes.iter().enumerate() {
        if i > 0 {
            qb.push(" OR ");
        }
        push_guard(&mut qb, change);
    }
    qb.push(")");

    // One flag per change, 1 when the column holds the refresh's value after the save.
    qb.push(" RETURNING ");
    for (i, change) in changes.iter().enumerate() {
        if i > 0 {
            qb.push(", ");
        }
        match change {
            RefreshChange::Column { col, new, .. } => {
                qb.push(*col).push(" IS ");
                bind_value(&mut qb, new);
            }
            RefreshChange::Names { new, .. } => {
                qb.push("(name IS ");
                bind_value(&mut qb, &new.0);
                qb.push(" AND sort_name IS ");
                bind_value(&mut qb, &new.1);
                qb.push(")");
            }
        }
    }

    // fetch_all steps the statement to completion, so a commit error is returned here.
    let rows = qb.build().fetch_all(pool).await?;
    let mut written = Vec::new();
    if let Some(row) = rows.first() {
        for (i, change) in changes.iter().enumerate() {
            if row.try_get::<bool, _>(i)? {
                match change {
                    RefreshChange::Column { col, .. } => written.push(*col),
                    RefreshChange::Names { .. } => {
                        written.push("name");
                        written.push("sort_name");
                    }
                }
            }
        }
    }
    Ok(written)
}

/// Batch-insert multiple media items in a single transaction.
/// Dramatically reduces WAL contention compared to per-item inserts.
/// A stored item is updated in place and keeps its stored date added, as in `upsert_media_item`.
/// Disables filter triggers during insert to avoid write amplification,
/// then rebuilds filter indexes after the transaction completes.
pub async fn batch_upsert_media_items(
    pool: &SqlitePool,
    items: &[MediaItem],
) -> Result<(), sqlx::Error> {
    if items.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    // Disable filter triggers during batch insert to avoid write amplification.
    // Each trigger fires COUNT(*) on the entire table per row, which is expensive.
    let _ = sqlx::query(
        "DROP TRIGGER IF EXISTS update_filter_indexes_after_insert; \
         DROP TRIGGER IF EXISTS update_filter_indexes_after_update; \
         DROP TRIGGER IF EXISTS update_filter_indexes_after_delete",
    )
    .execute(&mut *tx)
    .await;

    for item in items {
        let id = item.id.to_string();
        let parent_id = item.parent_id.map(|p| p.to_string());
        let library_id = item.library_id.to_string();
        let media_type = item.media_type.item_kind().to_string();
        let genres = item.genres.join("|");
        let created = item.date_created.to_rfc3339();
        let premiere = item.premiere_date.map(|d| d.to_rfc3339());
        let tags = item.tags.join("|");

        sqlx::query(
            "INSERT INTO media_items \
             (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
              production_year, community_rating, official_rating, genres, date_created, \
              premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
              tags, status, original_language, tagline, production_countries, budget, revenue, \
              homepage, end_date, vote_count, popularity) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
              name = excluded.name, sort_name = excluded.sort_name, overview = excluded.overview, \
              media_type = excluded.media_type, path = excluded.path, \
              parent_id = excluded.parent_id, library_id = excluded.library_id, \
              production_year = excluded.production_year, \
              community_rating = excluded.community_rating, \
              official_rating = excluded.official_rating, genres = excluded.genres, \
              premiere_date = excluded.premiere_date, runtime_ticks = excluded.runtime_ticks, \
              index_number = excluded.index_number, \
              index_number_end = excluded.index_number_end, location_type = excluded.location_type, \
              parent_index_number = excluded.parent_index_number, tmdb_id = excluded.tmdb_id, \
              tvdb_id = excluded.tvdb_id, tags = excluded.tags, status = excluded.status, \
              original_language = excluded.original_language, tagline = excluded.tagline, \
              production_countries = excluded.production_countries, budget = excluded.budget, \
              revenue = excluded.revenue, homepage = excluded.homepage, \
              end_date = excluded.end_date, vote_count = excluded.vote_count, \
              popularity = excluded.popularity",
        )
        .bind(&id)
        .bind(&item.name)
        .bind(&item.sort_name)
        .bind(&item.overview)
        .bind(&media_type)
        .bind(&item.path)
        .bind(&parent_id)
        .bind(&library_id)
        .bind(item.production_year)
        .bind(item.community_rating)
        .bind(&item.official_rating)
        .bind(&genres)
        .bind(&created)
        .bind(&premiere)
        .bind(item.runtime_ticks)
        .bind(item.index_number)
        .bind(item.parent_index_number)
        .bind(item.index_number_end)
        .bind(item.location_type.as_str())
        .bind(item.tmdb_id)
        .bind(item.tvdb_id)
        .bind(&tags)
        .bind(&item.status)
        .bind(&item.original_language)
        .bind(item.taglines.first().map(|s| s.as_str()))
        .bind(item.production_locations.join("|"))
        .bind(item.budget)
        .bind(item.revenue)
        .bind(&item.homepage)
        .bind(item.end_date.map(|d| d.to_rfc3339()))
        // NOT NULL DEFAULT 0, as in `upsert_media_item`.
        .bind(item.vote_count.unwrap_or(0))
        .bind(item.popularity)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Recreate filter triggers after batch insert.
    // The scanner calls rebuild_filter_indexes() after the batch insert completes,
    // so the indexes will be up-to-date.
    let _ = sqlx::query(include_str!("../migrations/027_filter_triggers.sql"))
        .execute(pool)
        .await;

    Ok(())
}

pub async fn get_items_by_library(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE library_id = ? ORDER BY sort_name, name",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_item_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(Into::into))
}

pub async fn get_children(
    pool: &SqlitePool,
    parent_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE parent_id = ? ORDER BY index_number, sort_name, name",
    )
    .bind(parent_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_children_paged(
    pool: &SqlitePool,
    parent_id: Uuid,
    start_index: i64,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE parent_id = ? ORDER BY index_number, sort_name, name LIMIT ? OFFSET ?",
    )
    .bind(parent_id.to_string())
    .bind(limit)
    .bind(start_index)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn count_children(pool: &SqlitePool, parent_id: Uuid) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media_items WHERE parent_id = ?")
        .bind(parent_id.to_string())
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

/// Deletes the item `id` on `conn`, which the caller holds in a write transaction. It does not
/// check for child items. These references to media_items have no ON DELETE CASCADE, so they go
/// first. Returns whether the media_items row existed.
pub async fn delete_item_rows_in(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query("DELETE FROM pinned_episodes WHERE series_id=?1 OR episode_id=?1 OR season_id=?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM user_item_data WHERE item_id=?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM item_people WHERE item_id=?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM item_studios WHERE item_id=?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let result = sqlx::query("DELETE FROM media_items WHERE id=?1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Deletes the item `id` and all its descendants, deepest first, in one write transaction.
/// Returns whether the item itself existed.
pub async fn delete_item(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let root = id.to_string();
    // The write lock is taken first, as in scan_sync::delete_item.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let tree: Vec<(String, i64)> = sqlx::query_as(
        "WITH RECURSIVE tree(id, depth) AS (
            SELECT id, 0 FROM media_items WHERE id = ?1
            UNION ALL
            SELECT m.id, t.depth + 1 FROM media_items m JOIN tree t ON m.parent_id = t.id
        )
        SELECT id, MAX(depth) AS depth FROM tree GROUP BY id ORDER BY depth DESC",
    )
    .bind(&root)
    .fetch_all(&mut *tx)
    .await?;
    let mut root_deleted = false;
    for (item_id, _) in &tree {
        let deleted = delete_item_rows_in(&mut tx, item_id).await?;
        if *item_id == root {
            root_deleted = deleted;
        }
    }
    tx.commit().await?;
    Ok(root_deleted)
}

/// Get latest items using a single static SQL query for plan reuse.
/// Uses NULL-coalescing to avoid dynamic SQL string concatenation,
/// so SQLite can always reuse the same query plan.
pub async fn get_latest_items(
    pool: &SqlitePool,
    limit: i64,
    media_type: Option<&str>,
    library_id: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items \
         WHERE (?1 IS NULL OR media_type = ?1) \
         AND (?2 IS NULL OR library_id = ?2) \
         ORDER BY date_created DESC LIMIT ?3",
    )
    .bind(media_type)
    .bind(library_id.map(|u| u.to_string()))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Query plan cache for `get_items_filtered`.
/// Maps a request's shape to its SQL, so repeated calls with the same filter shape reuse the same
/// query plan.
///
/// The shape holds the number of array elements in each filter, the flags and the parsed sort
/// terms, not the actual values, so different values with the same shape share a plan. The map is
/// keyed by the shape itself, so a lookup finds only an entry whose shape equals the request's: two
/// shapes whose hashes collide never share SQL, whose binds would not fit.
/// It holds at most `FILTER_QUERY_CACHE_CAP` shapes, and a query longer than
/// `FILTER_QUERY_MAX_CACHED_SQL` bytes is never stored.
/// Uses parking_lot::Mutex instead of std::sync::Mutex to avoid blocking Tokio threads.
type QueryCache = parking_lot::Mutex<HashMap<FilterShape, (String, String)>>;

static FILTER_QUERY_CACHE: LazyLock<QueryCache> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// At most this many query shapes are cached. Once it is full a new shape is built for each request
/// and not stored, so no request can push out the shapes in use.
const FILTER_QUERY_CACHE_CAP: usize = 256;
/// A query whose SQL (count and data together) is longer than this is never cached: each genre or
/// tag adds a clause, so the size follows the request.
const FILTER_QUERY_MAX_CACHED_SQL: usize = 16 * 1024;

/// What a `get_items_filtered` request's SQL is built from: the length of each array filter, the
/// flags and the parsed sort terms. Equal shapes build the same SQL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FilterShape {
    media_types_len: usize,
    exclude_media_types_len: usize,
    genres_len: usize,
    years_len: usize,
    official_ratings_len: usize,
    tags_len: usize,
    media_types_filter_len: usize,
    recursive: bool,
    is_favorite: Option<bool>,
    is_unplayed: Option<bool>,
    is_played: Option<bool>,
    sort: Vec<SortTerm>,
}

/// Build the cache key: the request's shape. Lists that parse to the same sort terms have equal
/// shapes, so the same cached query plan serves them.
pub(crate) fn filter_cache_key(
    media_types_len: usize,
    exclude_media_types_len: usize,
    genres_len: usize,
    years_len: usize,
    official_ratings_len: usize,
    tags_len: usize,
    media_types_filter_len: usize,
    recursive: bool,
    is_favorite: Option<bool>,
    is_unplayed: Option<bool>,
    is_played: Option<bool>,
    sort_by: &str,
    sort_order: &str,
) -> FilterShape {
    FilterShape {
        media_types_len,
        exclude_media_types_len,
        genres_len,
        years_len,
        official_ratings_len,
        tags_len,
        media_types_filter_len,
        recursive,
        is_favorite,
        is_unplayed,
        is_played,
        sort: parse_sort(sort_by, sort_order),
    }
}

/// A SortBy key as Jellyfin names it. Keys match without case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SortKey {
    SortName,
    Name,
    DateCreated,
    PremiereDate,
    ProductionYear,
    CommunityRating,
    Runtime,
    ParentIndexNumber,
    IndexNumber,
    Random,
    /// Dropped after the keys are paired with the orders, so it never reaches a clause.
    Default,
    /// Any other key; it orders by `sort_name`.
    Other,
}

impl SortKey {
    /// Reads one SortBy entry, matched without case.
    fn parse(entry: &str) -> SortKey {
        match entry.to_ascii_lowercase().as_str() {
            "default" => SortKey::Default,
            "sortname" => SortKey::SortName,
            "name" => SortKey::Name,
            "datecreated" | "dateadded" => SortKey::DateCreated,
            "premieredate" => SortKey::PremiereDate,
            "productionyear" => SortKey::ProductionYear,
            "communityrating" | "criticrating" => SortKey::CommunityRating,
            "runtime" => SortKey::Runtime,
            "parentindexnumber" => SortKey::ParentIndexNumber,
            "indexnumber" => SortKey::IndexNumber,
            "random" => SortKey::Random,
            _ => SortKey::Other,
        }
    }

    /// The SQL expression this key orders by.
    fn expr(self) -> &'static str {
        match self {
            SortKey::SortName | SortKey::Other | SortKey::Default => "sort_name",
            SortKey::Name => "name",
            SortKey::DateCreated => "date_created",
            SortKey::PremiereDate => concat!(
                "COALESCE(substr(premiere_date, 1, 10), ",
                "CASE WHEN production_year IS NOT NULL THEN printf('%04d-01-01', production_year) END)"
            ),
            SortKey::ProductionYear => "production_year",
            SortKey::CommunityRating => "community_rating",
            SortKey::Runtime => "runtime_ticks",
            SortKey::ParentIndexNumber => "parent_index_number",
            SortKey::IndexNumber => "index_number",
            SortKey::Random => "ROWID",
        }
    }
}

/// One sort key with its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SortTerm {
    key: SortKey,
    descending: bool,
}

/// Reads SortBy and SortOrder as Jellyfin does. Both are comma lists, and blank entries and orders
/// other than Ascending and Descending are dropped. Every key takes the order at its position, with
/// keys past the orders taking the first order (Ascending when there is none). Only after pairing
/// do Default and a Random that isn't first go, so later keys keep their orders. A repeated SQL
/// expression keeps only its first term, as EF Core does for Jellyfin. With no keys left,
/// SortName applies with the first order. The result is never empty.
fn parse_sort(sort_by: &str, sort_order: &str) -> Vec<SortTerm> {
    let orders: Vec<bool> = sort_order
        .split(',')
        .map(str::trim)
        .filter_map(|order| {
            if order.eq_ignore_ascii_case("Ascending") {
                Some(false)
            } else if order.eq_ignore_ascii_case("Descending") {
                Some(true)
            } else {
                None
            }
        })
        .collect();
    let first_order = orders.first().copied().unwrap_or(false);
    // Every non-blank key takes the order at its position, Default and Random included.
    let paired = sort_by
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .enumerate()
        .map(|(i, entry)| SortTerm {
            key: SortKey::parse(entry),
            descending: orders.get(i).copied().unwrap_or(first_order),
        });
    // Only then does Default go, and a Random that isn't first of what is left, and a term whose
    // expression is already kept.
    let mut terms: Vec<SortTerm> = Vec::new();
    for term in paired {
        match term.key {
            SortKey::Default => {}
            SortKey::Random if !terms.is_empty() => {}
            _ if terms.iter().any(|kept| kept.key.expr() == term.key.expr()) => {}
            _ => terms.push(term),
        }
    }
    if terms.is_empty() {
        return vec![SortTerm {
            key: SortKey::SortName,
            descending: first_order,
        }];
    }
    terms
}

/// Whether `terms` sort randomly, which holds only when the first key is Random.
fn is_random_sort(terms: &[SortTerm]) -> bool {
    terms
        .first()
        .is_some_and(|term| term.key == SortKey::Random)
}

/// The ORDER BY clause for `terms`: each key in its own direction. A first SortName key brings Name
/// along right after it, in the same direction. Without a SortName or Name key, ties go by
/// sort_name ascending. The id comes last, in the first key's direction, so every order is total
/// and pages stay stable. An expression already in the clause is never added again, so its first
/// part keeps its direction.
fn order_by_clause(terms: &[SortTerm]) -> String {
    let mut parts: Vec<(&'static str, &'static str)> = Vec::new();
    // Adds one (expr, dir) part unless its expression is already in the clause.
    let mut add = |expr: &'static str, dir: &'static str| {
        if !parts.iter().any(|(seen, _)| *seen == expr) {
            parts.push((expr, dir));
        }
    };
    for (i, term) in terms.iter().enumerate() {
        let dir = if term.descending { "DESC" } else { "ASC" };
        add(term.key.expr(), dir);
        if i == 0 && term.key == SortKey::SortName {
            add("name", dir);
        }
    }
    let has_name_key = terms
        .iter()
        .any(|term| matches!(term.key, SortKey::SortName | SortKey::Name));
    if !has_name_key {
        add("sort_name", "ASC");
    }
    let id_dir = if terms.first().is_some_and(|term| term.descending) {
        "DESC"
    } else {
        "ASC"
    };
    add("id", id_dir);
    let clause: Vec<String> = parts
        .iter()
        .map(|(expr, dir)| format!("{expr} {dir}"))
        .collect();
    format!("ORDER BY {}", clause.join(", "))
}

/// Build the SQL query with a static base for single-value params and dynamic
/// parts only for array filters. Reuses the SQL cached for an earlier call of the same shape.
/// The key is built here, from the filter this call renders, so no caller can pair a key with a
/// different filter.
#[allow(clippy::too_many_arguments)]
fn build_filtered_query(
    recursive: bool,
    media_types: Option<&[String]>,
    exclude_media_types: Option<&[String]>,
    genres: Option<&[String]>,
    years: Option<&[i32]>,
    official_ratings: Option<&[String]>,
    tags: Option<&[String]>,
    media_types_filter: Option<&[String]>,
    is_favorite: Option<bool>,
    is_unplayed: Option<bool>,
    is_played: Option<bool>,
    sort_by: &str,
    sort_order: &str,
) -> (String, String) {
    let cache_key = filter_cache_key(
        media_types.map(|t| t.len()).unwrap_or(0),
        exclude_media_types.map(|t| t.len()).unwrap_or(0),
        genres.map(|g| g.len()).unwrap_or(0),
        years.map(|y| y.len()).unwrap_or(0),
        official_ratings.map(|r| r.len()).unwrap_or(0),
        tags.map(|t| t.len()).unwrap_or(0),
        media_types_filter.map(|t| t.len()).unwrap_or(0),
        recursive,
        is_favorite,
        is_unplayed,
        is_played,
        sort_by,
        sort_order,
    );

    // Check cache first: a hit needs a stored shape equal to this one, not just an equal hash.
    {
        let cache = FILTER_QUERY_CACHE.lock();
        if let Some((count_sql, data_sql)) = cache.get(&cache_key) {
            return (count_sql.clone(), data_sql.clone());
        }
    }

    // Build WHERE clause parts for array filters only — single-value params
    // use the (? IS NULL OR col = ?) pattern in the static base query.
    let mut where_parts = Vec::new();

    // Media type filters (include)
    if let Some(types) = media_types {
        if types.is_empty() {
        } else {
            let placeholders: Vec<&str> = types.iter().map(|_| "?").collect();
            where_parts.push(format!("media_type IN ({})", placeholders.join(",")));
        }
    }

    // Media type filters (exclude)
    if let Some(types) = exclude_media_types {
        if types.is_empty() {
        } else {
            let placeholders: Vec<&str> = types.iter().map(|_| "?").collect();
            where_parts.push(format!("media_type NOT IN ({})", placeholders.join(",")));
        }
    }

    // Genres filter — exact delimited match to avoid false positives.
    // Data is stored as "Action|Sci-Fi", so we check all 4 boundary positions:
    //   exact match, start, middle, end.
    if let Some(genre_list) = genres {
        for _g in genre_list {
            where_parts.push(
                "(genres = ? OR genres LIKE ? OR genres LIKE ? OR genres LIKE ?)".to_string(),
            );
        }
    }

    // Years filter
    if let Some(year_list) = years {
        if year_list.is_empty() {
        } else {
            let placeholders: Vec<&str> = year_list.iter().map(|_| "?").collect();
            where_parts.push(format!("production_year IN ({})", placeholders.join(",")));
        }
    }

    // Official ratings filter
    if let Some(ratings) = official_ratings {
        if ratings.is_empty() {
        } else {
            let placeholders: Vec<&str> = ratings.iter().map(|_| "?").collect();
            where_parts.push(format!("official_rating IN ({})", placeholders.join(",")));
        }
    }

    // Tags filter — exact delimited match to avoid false positives.
    // Data is stored as "tag1|tag2", so we check all 4 boundary positions.
    if let Some(tag_list) = tags {
        for _tag in tag_list {
            where_parts.push("(tags = ? OR tags LIKE ? OR tags LIKE ? OR tags LIKE ?)".to_string());
        }
    }

    // Media types filter (Video, Audio, Photo)
    if let Some(mt_filter) = media_types_filter {
        if mt_filter.is_empty() {
        } else {
            let placeholders: Vec<&str> = mt_filter.iter().map(|_| "?").collect();
            where_parts.push(format!("media_type IN ({})", placeholders.join(",")));
        }
    }

    // IsFavorite / IsPlayed / IsUnplayed — subqueries scoped to the caller's user_id, which
    // get_items_filtered binds once per clause present, in this same order.
    if is_favorite == Some(true) {
        where_parts.push(
            "id IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND is_favorite = 1)"
                .to_string(),
        );
    } else if is_favorite == Some(false) {
        where_parts.push(
            "id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND is_favorite = 1)"
                .to_string(),
        );
    }
    if is_played == Some(true) {
        where_parts.push(
            "id IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)"
                .to_string(),
        );
    } else if is_played == Some(false) {
        where_parts.push(
            "id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)"
                .to_string(),
        );
    }
    if is_unplayed == Some(true) {
        where_parts.push(
            "id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)"
                .to_string(),
        );
    } else if is_unplayed == Some(false) {
        where_parts.push(
            "id IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)"
                .to_string(),
        );
    }

    // Build the static base WHERE clause for single-value params.
    // Uses (? IS NULL OR ? = '' OR col = ?) pattern so the query string is always identical.
    let base_where = "
        (? IS NULL OR ? = '' OR (? = 1 AND library_id IN (SELECT DISTINCT library_id FROM media_items WHERE parent_id = ? OR id = ?))
         OR ? = 0 AND (? IS NULL OR ? = '' OR parent_id = ?))
        AND (? IS NULL OR ? = '' OR library_id = ?)
        AND (? IS NULL OR ? = '' OR name LIKE ?)
        AND (? IS NULL OR ? = '' OR (? = '#' AND (sort_name < 'A' OR sort_name > 'z')) OR (? != '#' AND (sort_name LIKE ? OR name LIKE ?)))
        AND (? IS NULL OR ? = '' OR sort_name >= ? OR name >= ?)
        AND (? IS NULL OR ? = '' OR premiere_date >= ?)
        AND (? IS NULL OR ? = '' OR premiere_date <= ?)
        AND (? IS NULL OR ? = '' OR premiere_date >= ?)
        AND (? IS NULL OR ? = '' OR date_created >= ?)
        AND (? IS NULL OR runtime_ticks >= ?)
        AND (? IS NULL OR runtime_ticks <= ?)
        AND (? IS NULL OR id IN (SELECT value FROM json_each(?)))
    ";

    // Combine base WHERE with array filter WHERE parts
    let array_where = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" AND {}", where_parts.join(" AND "))
    };

    let terms = parse_sort(sort_by, sort_order);
    let order_clause = if is_random_sort(&terms) {
        "ORDER BY ROWID".to_string()
    } else {
        order_by_clause(&terms)
    };

    let count_sql = format!(
        "SELECT COUNT(*) as cnt FROM media_items WHERE {}{}",
        base_where, array_where
    );
    let data_sql = format!(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE {}{} {} LIMIT ? OFFSET ?",
        base_where, array_where, order_clause
    );

    // Cache the result, unless the SQL is too long to keep or the cache is full.
    {
        let mut cache = FILTER_QUERY_CACHE.lock();
        if count_sql.len() + data_sql.len() <= FILTER_QUERY_MAX_CACHED_SQL
            && cache.len() < FILTER_QUERY_CACHE_CAP
        {
            cache.insert(cache_key, (count_sql.clone(), data_sql.clone()));
        }
    }

    (count_sql, data_sql)
}

#[allow(clippy::too_many_arguments)]
pub async fn get_items_filtered(
    pool: &SqlitePool,
    parent_id: Option<Uuid>,
    library_id: Option<Uuid>,
    media_types: Option<&[String]>,
    exclude_media_types: Option<&[String]>,
    genres: Option<&[String]>,
    years: Option<&[i32]>,
    search_term: Option<&str>,
    name_starts_with: Option<&str>,
    name_starts_with_or_greater: Option<&str>,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    start_index: i64,
    limit: i64,
    recursive: bool,
    is_favorite: Option<bool>,
    is_unplayed: Option<bool>,
    is_played: Option<bool>,
    official_ratings: Option<&[String]>,
    tags: Option<&[String]>,
    media_types_filter: Option<&[String]>,
    start_date: Option<&str>,
    end_date: Option<&str>,
    premiere_date: Option<&str>,
    date_created: Option<&str>,
    runtime: Option<(i32, i32)>,
    user_id: Option<Uuid>,
    item_ids: Option<&[String]>,
) -> Result<(Vec<MediaItem>, i64), sqlx::Error> {
    // Convert options to strings for the (? IS NULL OR ? = '' OR col = ?) pattern
    let pid_str = parent_id.map(|u| u.to_string()).unwrap_or_default();
    let lid_str = library_id.map(|u| u.to_string()).unwrap_or_default();
    let search_like = search_term.map(|t| format!("%{}%", t)).unwrap_or_default();
    let starts_with_str = name_starts_with.unwrap_or_default();
    let starts_with_like = name_starts_with
        .map(|s| format!("{}%", s))
        .unwrap_or_default();
    let starts_with_or_greater_str = name_starts_with_or_greater.unwrap_or_default();
    let sd_str = start_date.unwrap_or_default();
    let ed_str = end_date.unwrap_or_default();
    let pd_str = premiere_date.unwrap_or_default();
    let dc_str = date_created.unwrap_or_default();
    let uid_str = user_id.unwrap_or(Uuid::nil()).to_string();
    let (min_ticks, max_ticks) = runtime
        .map(|(min, max)| {
            (
                Some((min as i64) * 60 * 10_000_000),
                Some((max as i64) * 60 * 10_000_000),
            )
        })
        .unwrap_or((None, None));
    // The ids an item must be one of, as a JSON array for json_each, or NULL for any item.
    let item_ids_json = item_ids.map(|ids| serde_json::Value::from(ids.to_vec()).to_string());

    let sort_by_str = sort_by.unwrap_or("SortName");
    let sort_order_str = sort_order.unwrap_or("Ascending");

    // Get cached query strings — same array sizes → same plan
    let (count_sql, data_sql) = build_filtered_query(
        recursive,
        media_types,
        exclude_media_types,
        genres,
        years,
        official_ratings,
        tags,
        media_types_filter,
        is_favorite,
        is_unplayed,
        is_played,
        &sort_by_str,
        &sort_order_str,
    );

    // Recursive flag: 1 = recursive, 0 = non-recursive
    let recursive_flag = if recursive { 1i64 } else { 0i64 };

    // Build count query — single-value params use the (? IS NULL OR ? = '' OR col = ?) pattern
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(count_sql));
    // Base single-value params (same order in both count and data queries)
    count_q = count_q
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_like)
        .bind(&starts_with_like)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&sd_str)
        .bind(&sd_str)
        .bind(&sd_str)
        .bind(&ed_str)
        .bind(&ed_str)
        .bind(&ed_str)
        .bind(&pd_str)
        .bind(&pd_str)
        .bind(&pd_str)
        .bind(&dc_str)
        .bind(&dc_str)
        .bind(&dc_str)
        .bind(&min_ticks)
        .bind(&min_ticks)
        .bind(&max_ticks)
        .bind(&max_ticks)
        .bind(&item_ids_json)
        .bind(&item_ids_json);

    // Array filter params
    if let Some(types) = media_types {
        if !types.is_empty() {
            for t in types {
                count_q = count_q.bind(t);
            }
        }
    }
    if let Some(types) = exclude_media_types {
        if !types.is_empty() {
            for t in types {
                count_q = count_q.bind(t);
            }
        }
    }
    if let Some(genre_list) = genres {
        for g in genre_list {
            // 4 binds per genre: exact match, start, middle, end
            count_q = count_q
                .bind(g)
                .bind(format!("{}|%", g))
                .bind(format!("%|{}", g))
                .bind(format!("%|{}|%", g));
        }
    }
    if let Some(year_list) = years {
        if !year_list.is_empty() {
            for y in year_list {
                count_q = count_q.bind(*y);
            }
        }
    }
    if let Some(ratings) = official_ratings {
        if !ratings.is_empty() {
            for r in ratings {
                count_q = count_q.bind(r);
            }
        }
    }
    if let Some(tag_list) = tags {
        for tag in tag_list {
            // 4 binds per tag: exact match, start, middle, end
            count_q = count_q
                .bind(tag)
                .bind(format!("{}|%", tag))
                .bind(format!("%|{}", tag))
                .bind(format!("%|{}|%", tag));
        }
    }
    if let Some(mt_filter) = media_types_filter {
        if !mt_filter.is_empty() {
            for mt in mt_filter {
                count_q = count_q.bind(mt);
            }
        }
    }
    // One user_id per IsFavorite / IsPlayed / IsUnplayed clause, in build_filtered_query's order.
    if is_favorite.is_some() {
        count_q = count_q.bind(&uid_str);
    }
    if is_played.is_some() {
        count_q = count_q.bind(&uid_str);
    }
    if is_unplayed.is_some() {
        count_q = count_q.bind(&uid_str);
    }

    let total: i64 = count_q.fetch_one(pool).await.unwrap_or(0);

    // Build data query — same bind order as count query
    let mut data_q = sqlx::query_as::<_, MediaItemRow>(sqlx::AssertSqlSafe(data_sql));
    data_q = data_q
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_str)
        .bind(&starts_with_like)
        .bind(&starts_with_like)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&starts_with_or_greater_str)
        .bind(&sd_str)
        .bind(&sd_str)
        .bind(&sd_str)
        .bind(&ed_str)
        .bind(&ed_str)
        .bind(&ed_str)
        .bind(&pd_str)
        .bind(&pd_str)
        .bind(&pd_str)
        .bind(&dc_str)
        .bind(&dc_str)
        .bind(&dc_str)
        .bind(&min_ticks)
        .bind(&min_ticks)
        .bind(&max_ticks)
        .bind(&max_ticks)
        .bind(&item_ids_json)
        .bind(&item_ids_json);

    // Array filter params (same order as count query)
    if let Some(types) = media_types {
        if !types.is_empty() {
            for t in types {
                data_q = data_q.bind(t);
            }
        }
    }
    if let Some(types) = exclude_media_types {
        if !types.is_empty() {
            for t in types {
                data_q = data_q.bind(t);
            }
        }
    }
    if let Some(genre_list) = genres {
        for g in genre_list {
            // 4 binds per genre: exact match, start, middle, end
            data_q = data_q
                .bind(g)
                .bind(format!("{}|%", g))
                .bind(format!("%|{}", g))
                .bind(format!("%|{}|%", g));
        }
    }
    if let Some(year_list) = years {
        if !year_list.is_empty() {
            for y in year_list {
                data_q = data_q.bind(*y);
            }
        }
    }
    if let Some(ratings) = official_ratings {
        if !ratings.is_empty() {
            for r in ratings {
                data_q = data_q.bind(r);
            }
        }
    }
    if let Some(tag_list) = tags {
        for tag in tag_list {
            // 4 binds per tag: exact match, start, middle, end
            data_q = data_q
                .bind(tag)
                .bind(format!("{}|%", tag))
                .bind(format!("%|{}", tag))
                .bind(format!("%|{}|%", tag));
        }
    }
    if let Some(mt_filter) = media_types_filter {
        if !mt_filter.is_empty() {
            for mt in mt_filter {
                data_q = data_q.bind(mt);
            }
        }
    }
    // Same user_id binds as the count query, for the same clauses.
    if is_favorite.is_some() {
        data_q = data_q.bind(&uid_str);
    }
    if is_played.is_some() {
        data_q = data_q.bind(&uid_str);
    }
    if is_unplayed.is_some() {
        data_q = data_q.bind(&uid_str);
    }

    // Pagination
    let is_random = is_random_sort(&parse_sort(sort_by_str, sort_order_str));
    if is_random {
        let random_offset = if total > 0 {
            fastrand::u64(0..total as u64) as i64
        } else {
            0
        };
        data_q = data_q.bind(limit).bind(random_offset);
    } else {
        data_q = data_q.bind(limit).bind(start_index);
    }

    let rows = data_q.fetch_all(pool).await?;
    Ok((rows.into_iter().map(Into::into).collect(), total))
}

/// Scored similar item result from local algorithm
struct ScoredItem {
    item: MediaItem,
    score: f64,
}

pub async fn get_similar_items(
    pool: &SqlitePool,
    item: &MediaItem,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let media_type = item.media_type.item_kind().to_string();

    // Get candidate items: same media type, same library, excluding the current item
    let candidates = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items \
         WHERE media_type = ? AND id != ? AND library_id = ? \
         LIMIT 200",
    )
    .bind(&media_type)
    .bind(item.id.to_string())
    .bind(item.library_id.to_string())
    .fetch_all(pool)
    .await?;

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Get person IDs for the current item
    let person_ids: Vec<String> =
        sqlx::query_as::<_, (String,)>("SELECT person_id FROM item_people WHERE item_id = ?")
            .bind(item.id.to_string())
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| r.0)
            .collect();

    // Score each candidate item
    let mut scored: Vec<ScoredItem> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let mut score = 0.0;

        // Convert to MediaItem for genre matching
        let candidate_item: MediaItem = candidate.into();
        let candidate_id = candidate_item.id.to_string();

        // Genre matching: count exact genre matches (highest weight)
        if !item.genres.is_empty() {
            let common_genres = item
                .genres
                .iter()
                .filter(|g| candidate_item.genres.contains(*g))
                .count();
            if common_genres > 0 {
                // Weight: more common genres = higher score
                score += common_genres as f64 * 10.0;
                // Bonus for having the same number of genres (similar profile)
                if item.genres.len() == candidate_item.genres.len()
                    && common_genres == item.genres.len()
                {
                    score += 5.0;
                }
            }
        }

        // Year matching: items from similar years
        if let (Some(item_year), Some(candidate_year)) =
            (item.production_year, candidate_item.production_year)
        {
            let year_diff = (item_year - candidate_year).abs();
            if year_diff <= 2 {
                score += 5.0; // Same or very close year
            } else if year_diff <= 5 {
                score += 3.0; // Within 5 years
            } else if year_diff <= 10 {
                score += 1.0; // Within 10 years
            }
        }

        // Person matching: shared cast/crew
        if !person_ids.is_empty() {
            let shared_persons: Vec<String> = sqlx::query_as::<_, (String,)>(
                "SELECT person_id FROM item_people WHERE item_id = ? AND person_id IN (SELECT person_id FROM item_people WHERE item_id = ?)",
            )
            .bind(&candidate_id)
            .bind(item.id.to_string())
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| r.0)
            .collect();

            if !shared_persons.is_empty() {
                // Weight: more shared persons = higher score
                score += shared_persons.len() as f64 * 3.0;
            }
        }

        // Only include items with a positive score
        if score > 0.0 {
            scored.push(ScoredItem {
                item: candidate_item,
                score,
            });
        }
    }

    // Sort by score descending
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if scored.is_empty() {
        // Fallback: random items of the same media type if no scored items
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM media_items WHERE media_type = ? AND id != ? AND library_id = ?",
        )
        .bind(&media_type)
        .bind(item.id.to_string())
        .bind(item.library_id.to_string())
        .fetch_one(pool)
        .await?;

        if total.0 == 0 {
            return Ok(Vec::new());
        }

        let offset = fastrand::u64(0..total.0 as u64) as i64;
        let rows = sqlx::query_as::<_, MediaItemRow>(
            "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
             production_year, community_rating, official_rating, genres, date_created, \
             premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
             tags, status, original_language, tagline, production_countries, budget, revenue, \
             homepage, end_date, vote_count, popularity \
             FROM media_items WHERE media_type = ? AND id != ? AND library_id = ? \
             ORDER BY ROWID LIMIT ? OFFSET ?",
        )
        .bind(&media_type)
        .bind(item.id.to_string())
        .bind(item.library_id.to_string())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        return Ok(rows.into_iter().map(Into::into).collect());
    }

    // Take top results, up to limit
    let len = scored.len().min(limit as usize);
    Ok(scored.into_iter().take(len).map(|s| s.item).collect())
}

/// Rebuild materialized filter index tables from media_items.
/// Call after scanning or bulk operations to keep indexes up-to-date.
pub async fn rebuild_filter_indexes(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    // Clear existing indexes
    sqlx::query("DELETE FROM filter_genre_index")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM filter_year_index")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM filter_rating_index")
        .execute(&mut *tx)
        .await?;

    // Rebuild genre index from pipe-delimited genres column
    sqlx::query(
        "INSERT INTO filter_genre_index (genre, item_count)
         SELECT genre, COUNT(*) as item_count FROM (
             SELECT TRIM(SUBSTR(genres, start_pos, end_pos - start_pos)) as genre
             FROM media_items,
                  generate_series(1, LENGTH(genres) - LENGTH(REPLACE(genres, '|', '')) + 1) as n
             CROSS JOIN LATERAL (
                 SELECT
                     INSTR(SUBSTR(genres, n), '|') + n - 1 as start_pos,
                     INSTR(SUBSTR(genres, n), '|') + n - 1 +
                     LENGTH(SUBSTR(genres, n)) - LENGTH(REPLACE(SUBSTR(genres, n), '|', '')) - 1 as end_pos
             )
             WHERE genres IS NOT NULL AND genres != ''
         )
         WHERE genre != ''
         GROUP BY genre",
    )
    .execute(&mut *tx)
    .await
    .ok(); // Fallback to simple method if SQLite doesn't support generate_series

    // Simple fallback: parse pipe-delimited genres in Rust
    let genre_rows: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT genres FROM media_items WHERE genres IS NOT NULL AND genres != ''",
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut genre_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for gs in &genre_rows {
        for g in gs.split('|') {
            let g = g.trim().to_string();
            if !g.is_empty() {
                *genre_counts.entry(g).or_insert(0) += 1;
            }
        }
    }

    for (genre, count) in genre_counts {
        sqlx::query("INSERT OR REPLACE INTO filter_genre_index (genre, item_count) VALUES (?, ?)")
            .bind(&genre)
            .bind(count)
            .execute(&mut *tx)
            .await?;
    }

    // Rebuild year index
    sqlx::query(
        "INSERT INTO filter_year_index (year, item_count)
         SELECT production_year, COUNT(*) as item_count
         FROM media_items
         WHERE production_year IS NOT NULL
         GROUP BY production_year",
    )
    .execute(&mut *tx)
    .await?;

    // Rebuild rating index
    sqlx::query(
        "INSERT INTO filter_rating_index (rating, item_count)
         SELECT official_rating, COUNT(*) as item_count
         FROM media_items
         WHERE official_rating IS NOT NULL AND official_rating != ''
         GROUP BY official_rating",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Get filter values from materialized index tables.
/// Falls back to DISTINCT queries if indexes are empty.
pub async fn get_filters(
    pool: &SqlitePool,
    _library_id: Option<Uuid>,
) -> Result<(Vec<String>, Vec<i32>, Vec<String>), sqlx::Error> {
    // Try materialized index tables first
    let genre_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM filter_genre_index")
        .fetch_one(pool)
        .await?;

    if genre_count > 0 {
        // Use materialized indexes
        let genres: Vec<String> =
            sqlx::query_scalar::<_, String>("SELECT genre FROM filter_genre_index ORDER BY genre")
                .fetch_all(pool)
                .await?;

        let years: Vec<i32> =
            sqlx::query_scalar::<_, i32>("SELECT year FROM filter_year_index ORDER BY year DESC")
                .fetch_all(pool)
                .await?;

        let ratings: Vec<String> = sqlx::query_scalar::<_, String>(
            "SELECT rating FROM filter_rating_index ORDER BY rating",
        )
        .fetch_all(pool)
        .await?;

        Ok((genres, years, ratings))
    } else {
        // Fallback to DISTINCT queries
        get_filters_distinct(pool, _library_id).await
    }
}

/// Original DISTINCT-based filter query (fallback when materialized indexes are empty).
async fn get_filters_distinct(
    pool: &SqlitePool,
    library_id: Option<Uuid>,
) -> Result<(Vec<String>, Vec<i32>, Vec<String>), sqlx::Error> {
    let lid = library_id.map(|u| u.to_string()).unwrap_or_default();

    let genre_strs: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT genres FROM media_items WHERE (? = '' OR library_id = ?)",
    )
    .bind(&lid)
    .bind(lid.clone())
    .fetch_all(pool)
    .await?;
    let mut all_genres: Vec<String> = Vec::new();
    for gs in &genre_strs {
        for g in gs.split('|') {
            let g = g.trim().to_string();
            if !g.is_empty() && !all_genres.contains(&g) {
                all_genres.push(g);
            }
        }
    }
    all_genres.sort();

    let years: Vec<i32> = sqlx::query_scalar::<_, i32>(
        "SELECT DISTINCT production_year FROM media_items \
         WHERE production_year IS NOT NULL AND (? = '' OR library_id = ?) \
         ORDER BY production_year DESC",
    )
    .bind(&lid)
    .bind(lid.clone())
    .fetch_all(pool)
    .await?;

    let ratings: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT official_rating FROM media_items \
         WHERE official_rating IS NOT NULL AND (? = '' OR library_id = ?) \
         ORDER BY official_rating",
    )
    .bind(&lid)
    .bind(lid.clone())
    .fetch_all(pool)
    .await?;

    Ok((all_genres, years, ratings))
}

pub async fn get_item_by_path(
    pool: &SqlitePool,
    path: &str,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE path = ?",
    )
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Into::into))
}

/// Fetch all existing paths from media_items in a single query.
/// Used by the scanner to build an in-memory HashSet for O(1) existence checks,
/// avoiding N+1 per-file database queries during scanning.
pub async fn get_all_paths(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT path FROM media_items")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The paths that a query covers. Each variant matches the same paths in Rust ([`PathScope::contains`])
/// and in SQL ([`PathScope::sql`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathScope {
    /// Every path.
    All,
    /// The folder `u` itself and every path under it. `u` has no trailing `/`.
    Under(String),
    /// The paths directly in the folder `r`, not in its subfolders. `r` has no trailing `/`.
    DirectlyIn(String),
}

impl PathScope {
    /// Whether `path` is in the scope.
    pub fn contains(&self, path: &str) -> bool {
        match self {
            PathScope::All => true,
            PathScope::Under(folder) => {
                let folder = folder.as_str();
                path == folder
                    || path
                        .strip_prefix(folder)
                        .is_some_and(|rest| rest.starts_with('/'))
            }
            PathScope::DirectlyIn(folder) => path
                .strip_prefix(folder.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|name| !name.contains('/')),
        }
    }

    /// The SQL condition on the path column `col`. It reads the folder from the numbered parameter
    /// `?n`, so a query binds [`PathScope::param`] at `?n` whatever the variant.
    pub fn sql(&self, col: &str, n: usize) -> String {
        match self {
            PathScope::All => format!("(?{n} IS NOT NULL)"),
            PathScope::Under(_) => {
                format!("({col} = ?{n} OR ({col} >= ?{n} || '/' AND {col} < ?{n} || '0'))")
            }
            PathScope::DirectlyIn(_) => format!(
                "({col} >= ?{n} || '/' AND {col} < ?{n} || '0' AND instr(substr({col}, length(?{n}) + 2), '/') = 0)"
            ),
        }
    }

    /// The value of the folder parameter of [`PathScope::sql`]: the folder, or `""` for `All`.
    pub fn param(&self) -> &str {
        match self {
            PathScope::All => "",
            PathScope::Under(folder) | PathScope::DirectlyIn(folder) => folder.as_str(),
        }
    }
}

/// Fetch the paths of the items of a library that are in `scope`.
pub async fn get_library_paths(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT path FROM media_items WHERE library_id = ?1 AND {}",
        scope.sql("path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fetch the source of each audio item of a library whose source path is in `scope`, as (item id, path,
/// size, modified). The size and modification time are the stamp that the source stores, and are `None`
/// when it has none.
pub async fn get_library_audio_sources(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<Vec<(Uuid, String, Option<i64>, Option<i64>)>, sqlx::Error> {
    let rows: Vec<(String, String, Option<i64>, Option<i64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT m.id, s.path, s.size, s.modified
         FROM media_sources AS s
         JOIN media_items AS m ON m.id = s.item_id
         WHERE m.library_id = ?1 AND m.media_type = 'Audio' AND {}",
            scope.sql("s.path", 2)
        )))
        .bind(library_id.to_string())
        .bind(scope.param())
        .fetch_all(pool)
        .await?;
    let sources = rows
        .into_iter()
        .filter_map(|(id, path, size, modified)| {
            Uuid::parse_str(&id)
                .ok()
                .map(|uid| (uid, path, size, modified))
        })
        .collect();
    Ok(sources)
}

/// Fetch the items of a specific media type for a library whose path is in `scope`, returning path -> id
/// mapping. Used by the scanner to pre-load series/season/artist/album IDs before directory traversal,
/// avoiding per-item database queries during scanning.
pub async fn get_library_items_by_type(
    pool: &SqlitePool,
    library_id: Uuid,
    media_type: &str,
    scope: &PathScope,
) -> Result<HashMap<String, Uuid>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT path, id FROM media_items WHERE library_id = ?1 AND media_type = ?3 AND {}",
        scope.sql("path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .bind(media_type)
    .fetch_all(pool)
    .await?;
    let map = rows
        .into_iter()
        .filter_map(|(path, id)| Uuid::parse_str(&id).ok().map(|uid| (path, uid)))
        .collect();
    Ok(map)
}

/// Fetch the id of the item of a library with a media type and a path, if there is one. Used by the
/// scanner to look up a container that the preload of its scope did not hold.
pub async fn get_library_item_id(
    pool: &SqlitePool,
    library_id: Uuid,
    media_type: &str,
    path: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM media_items WHERE library_id = ? AND media_type = ? AND path = ?",
    )
    .bind(library_id.to_string())
    .bind(media_type)
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(id.and_then(|id| Uuid::parse_str(&id).ok()))
}

/// Fetch the artist rows of a music library (`MusicAlbum` rows with no parent) whose path is in `scope`,
/// plus every artist keyed by name (path `''`), as (id, path, name) tuples. Used by the scanner to reuse
/// the artists of earlier scans.
pub async fn get_library_artists(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<Vec<(Uuid, String, String)>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, path, name FROM media_items \
         WHERE library_id = ?1 AND media_type = 'MusicAlbum' AND parent_id IS NULL \
         AND (path = '' OR {})",
        scope.sql("path", 2)
    )))
    .bind(library_id.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;
    let artists = rows
        .into_iter()
        .filter_map(|(id, path, name)| Uuid::parse_str(&id).ok().map(|uid| (uid, path, name)))
        .collect();
    Ok(artists)
}

/// Fetch the id of the artist (a `MusicAlbum` row with no parent) of a library at a path, if there is one.
/// Used by the scanner to look up an artist that the preload of its scope did not hold.
pub async fn get_library_artist_id(
    pool: &SqlitePool,
    library_id: Uuid,
    path: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM media_items \
         WHERE library_id = ? AND media_type = 'MusicAlbum' AND parent_id IS NULL AND path = ?",
    )
    .bind(library_id.to_string())
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(id.and_then(|id| Uuid::parse_str(&id).ok()))
}

/// Fetch all movie items for a library, returning (path, id) pairs.
/// Used by the scanner to pre-load movie paths for version detection,
/// avoiding per-item database queries during scanning.
pub async fn get_library_movies(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT path, id FROM media_items WHERE library_id = ? AND media_type = ? AND runtime_ticks IS NOT NULL",
    )
    .bind(library_id.to_string())
    .bind("Movie")
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fetch all episode items for a library, returning (path, id, parent_id) triples.
/// Used by the TV scanner to pre-load episode paths for version detection,
/// avoiding per-item database queries during scanning.
pub async fn get_library_episodes(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT path, id, parent_id FROM media_items WHERE library_id = ? AND media_type = ? AND runtime_ticks IS NOT NULL",
    )
    .bind(library_id.to_string())
    .bind("Episode")
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get items with playback position > 0 (resume items) for a specific user
pub async fn get_user_resume_items(
    pool: &SqlitePool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let uid = user_id.to_string();
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m \
         INNER JOIN user_item_data u ON u.item_id = m.id AND u.user_id = ? \
         WHERE u.playback_position_ticks > 0 \
         ORDER BY u.last_played_date DESC LIMIT ?",
    )
    .bind(&uid)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[derive(sqlx::FromRow)]
pub struct MediaItemRow {
    id: String,
    name: String,
    sort_name: Option<String>,
    overview: Option<String>,
    media_type: String,
    path: String,
    parent_id: Option<String>,
    library_id: String,
    production_year: Option<i32>,
    community_rating: Option<f64>,
    official_rating: Option<String>,
    genres: String,
    date_created: String,
    premiere_date: Option<String>,
    runtime_ticks: Option<i64>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
    index_number_end: Option<i32>,
    location_type: String,
    tmdb_id: Option<i64>,
    tvdb_id: Option<i64>,
    tags: String,
    status: Option<String>,
    original_language: Option<String>,
    tagline: Option<String>,
    production_countries: String,
    budget: Option<i64>,
    revenue: Option<i64>,
    homepage: Option<String>,
    end_date: Option<String>,
    vote_count: Option<i32>,
    popularity: Option<f64>,
}

impl From<MediaItemRow> for MediaItem {
    fn from(r: MediaItemRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_default(),
            name: r.name,
            sort_name: r.sort_name,
            overview: r.overview,
            media_type: r.media_type.parse().unwrap_or(MediaType::Unknown),
            path: r.path,
            parent_id: r.parent_id.and_then(|s| Uuid::parse_str(&s).ok()),
            library_id: Uuid::parse_str(&r.library_id).unwrap_or_default(),
            production_year: r.production_year,
            community_rating: r.community_rating,
            official_rating: r.official_rating,
            genres: if r.genres.is_empty() {
                Vec::new()
            } else {
                r.genres
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            },
            date_created: r.date_created.parse::<DateTime<Utc>>().unwrap_or_default(),
            premiere_date: r
                .premiere_date
                .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            runtime_ticks: r.runtime_ticks,
            index_number: r.index_number,
            parent_index_number: r.parent_index_number,
            index_number_end: r.index_number_end,
            tmdb_id: r.tmdb_id,
            tvdb_id: r.tvdb_id,
            media_sources: Vec::new(),
            // --- MediaItem DTO Fields (defaults) ---
            user_data: None,
            image_tags: std::collections::HashMap::new(),
            backdrop_image_tags: Vec::new(),
            parent_backdrop_image_tags: Vec::new(),
            primary_image_aspect_ratio: None,
            can_delete: false,
            can_download: false,
            container: None,
            bitrate: None,
            video_3_d: false,
            end_date: r.end_date.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            series_name: None,
            series_id: None,
            season_name: None,
            status: r.status,
            custom_rating: None,
            critic_rating: None,
            audience_rating: None,
            tags: if r.tags.is_empty() {
                Vec::new()
            } else {
                r.tags
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            },
            people: Vec::new(),
            studios: Vec::new(),
            genre_items: Vec::new(),
            forced_sort_name: None,
            width: None,
            height: None,
            aspect_ratio: None,
            chapter_count: 0,
            media_source_count: 0,
            // 'Virtual' marks a season with no folder (migration 072).
            location_type: match r.location_type.as_str() {
                "Virtual" => rustyfin_model::media::LocationType::Virtual,
                "Remote" => rustyfin_model::media::LocationType::Remote,
                _ => rustyfin_model::media::LocationType::FileSystem,
            },
            intro_start_time_ticks: None,
            intro_end_time_ticks: None,
            is_hd: false,
            is_4_k: false,
            video_type: None,
            // --- Additional BaseItemDto fields (defaults) ---
            server_id: None,
            start_date: None,
            provider_ids: rustyfin_model::media::ProviderIds::default(),
            external_urls: Vec::new(),
            remote_trailers: Vec::new(),
            chapters: Vec::new(),
            media_streams: Vec::new(),
            current_program: None,
            timer_id: None,
            series_timer_id: None,
            channel_id: None,
            channel_name: None,
            channel_number: None,
            album: None,
            album_id: None,
            album_artist: None,
            album_artists: Vec::new(),
            artists: Vec::new(),
            artist_items: Vec::new(),
            song_count: None,
            album_count: None,
            movie_count: None,
            series_count: None,
            episode_count: None,
            music_video_count: None,
            photo_count: None,
            program_count: None,
            child_count: None,
            part_count: None,
            trailer_count: None,
            local_trailer_count: None,
            special_feature_count: None,
            extra_type: None,
            display_order: None,
            display_preferences_id: None,
            etag: None,
            source_type: None,
            playlist_item_id: None,
            date_last_media_added: None,
            completion_percentage: None,
            cumulative_run_time_ticks: None,
            enable_media_source_display: false,
            is_kids: false,
            is_live: false,
            is_movie: false,
            is_news: false,
            is_placeholder: false,
            is_premiere: false,
            is_repeat: false,
            is_series: false,
            is_sports: false,
            lock_data: false,
            locked_fields: Vec::new(),
            number: None,
            episode_title: None,
            series_primary_image_tag: None,
            series_thumb_image_tag: None,
            series_studio: None,
            original_title: None,
            original_language: r.original_language,
            preferred_metadata_country_code: None,
            preferred_metadata_language: None,
            production_locations: if r.production_countries.is_empty() {
                Vec::new()
            } else {
                r.production_countries
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            },
            taglines: r
                .tagline
                .filter(|t| !t.is_empty())
                .map(|t| vec![t])
                .unwrap_or_default(),
            budget: r.budget,
            revenue: r.revenue,
            homepage: r.homepage,
            vote_count: r.vote_count,
            popularity: r.popularity,
            parent_backdrop_item_id: None,
            parent_primary_image_tag: None,
            parent_primary_image_item_id: None,
            parent_thumb_image_tag: None,
            parent_thumb_item_id: None,
            parent_art_image_tag: None,
            parent_art_item_id: None,
            parent_logo_image_tag: None,
            parent_logo_item_id: None,
            screenshot_image_tags: Vec::new(),
            image_blur_hashes: std::collections::HashMap::new(),
            video_3_d_format: None,
            play_access: None,
            official_rating_image_tag: None,
        }
    }
}

/// Get "Next Up": at most one episode per series.
///
/// A series' entry is its first unplayed regular episode after the user's highest played regular
/// episode, or its first unplayed regular episode if none is played. Season 0 is never offered
/// and never sets the position. An episode in progress counts as unplayed. A NULL season or
/// episode number is regular. After a position a NULL season never qualifies, and a NULL episode
/// number qualifies only in a later season. A series with nothing unplayed after its position is
/// left out.
///
/// Candidates are the series the user has started: one of their episodes or specials has a
/// `last_played_date` and is played or has a playback position. With `series_id`, that item alone
/// is the candidate if it is a Series, started or not. Without a user nothing is played, so
/// `series_id` gives the first regular episode, and no `series_id` gives an empty list.
///
/// Order, newest first: the highest played episode's `last_played_date` (a series whose highest
/// played episode has no date comes next, then series with no highest played episode), then the
/// newest qualifying play, then the series id ascending.
pub async fn get_next_up(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
    series_id: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    /// A regular episode of a candidate series, with the user's state for it.
    struct Episode {
        id: String,
        season: Option<i64>,
        number: Option<i64>,
        played: bool,
        played_date: Option<String>,
    }
    /// A series' entry, with the keys it sorts by.
    struct Entry {
        series: String,
        last_played: Option<Option<String>>,
        newest_play: Option<String>,
        episode: String,
    }

    let uid = user_id.map(|u| u.to_string());

    // Candidate series, each with its newest qualifying play.
    let candidates: Vec<(String, Option<String>)> = match (series_id, uid.as_deref()) {
        (Some(sid), _) => sqlx::query_scalar::<_, String>(
            "SELECT id FROM media_items WHERE id = ? AND media_type = 'Series'",
        )
        .bind(sid.to_string())
        .fetch_optional(pool)
        .await?
        .map(|id| vec![(id, None)])
        .unwrap_or_default(),
        (None, Some(user)) => {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT se.parent_id, MAX(ud.last_played_date) \
                 FROM user_item_data ud \
                 JOIN media_items e ON e.id = ud.item_id AND e.media_type = 'Episode' \
                 JOIN media_items se ON se.id = e.parent_id AND se.media_type = 'Season' \
                 WHERE ud.user_id = ? AND ud.last_played_date IS NOT NULL \
                 AND (ud.played = 1 OR ud.playback_position_ticks > 0) \
                 AND se.parent_id IS NOT NULL \
                 GROUP BY se.parent_id",
            )
            .bind(user)
            .fetch_all(pool)
            .await?
        }
        (None, None) => Vec::new(),
    };

    let mut entries = Vec::new();
    for (series, newest_play) in candidates {
        // Season 0 is left out. With no user the LEFT JOIN matches no row, so every
        // episode is unplayed.
        let rows: Vec<(
            String,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT e.id, e.parent_index_number, e.index_number, ud.played, ud.last_played_date \
             FROM media_items e \
             JOIN media_items se ON se.id = e.parent_id AND se.media_type = 'Season' \
             LEFT JOIN user_item_data ud ON ud.item_id = e.id AND ud.user_id = ? \
             WHERE se.parent_id = ? AND e.media_type = 'Episode' \
             AND (e.parent_index_number IS NULL OR e.parent_index_number <> 0)",
        )
        .bind(uid.as_deref())
        .bind(&series)
        .fetch_all(pool)
        .await?;
        let episodes: Vec<Episode> = rows
            .into_iter()
            .map(|(id, season, number, played, played_date)| Episode {
                id,
                season,
                number,
                played: played == Some(1),
                played_date,
            })
            .collect();

        // The highest played episode, by (season, episode) with None lowest.
        let highest = episodes
            .iter()
            .filter(|e| e.played)
            .max_by_key(|e| (e.season, e.number, e.id.as_str()));
        let last_played = highest.map(|e| e.played_date.clone());
        // The position needs both numbers.
        let position = highest.and_then(|e| e.season.zip(e.number));

        let next = episodes
            .iter()
            .filter(|e| !e.played)
            .filter(|e| match position {
                None => true,
                // Jellyfin's comparison on the Options: a NULL season never qualifies, and a
                // NULL episode number qualifies only in a later season.
                Some((ps, pn)) => {
                    e.season > Some(ps) || (e.season == Some(ps) && e.number > Some(pn))
                }
            })
            .min_by_key(|e| (e.season, e.number, e.id.as_str()));

        if let Some(next) = next {
            entries.push(Entry {
                series,
                last_played,
                newest_play,
                episode: next.id.clone(),
            });
        }
    }

    entries.sort_by(|a, b| {
        b.last_played
            .cmp(&a.last_played)
            .then_with(|| b.newest_play.cmp(&a.newest_play))
            .then_with(|| a.series.cmp(&b.series))
    });

    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let Ok(id) = Uuid::parse_str(&entry.episode) else {
            continue;
        };
        if let Some(item) = get_item_by_id(pool, id).await? {
            items.push(item);
        }
    }
    Ok(items)
}

/// Get "Upcoming" episodes: episodes with a future air date
pub async fn get_upcoming(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let uid = user_id.map(|u| u.to_string());

    let sql = if uid.is_some() {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.premiere_date > ?
         AND m.id NOT IN (
             SELECT item_id FROM user_item_data
             WHERE user_id = ? AND played = 1
         )
         ORDER BY m.premiere_date ASC"
    } else {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.premiere_date > ?
         ORDER BY m.premiere_date ASC"
    };

    let mut q = sqlx::query_as::<_, MediaItemRow>(sql);
    q = q.bind(&now);
    if let Some(ref u) = uid {
        q = q.bind(u);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get "Upcoming" episodes for a specific series
pub async fn get_upcoming_for_series(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
    series_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let sid = series_id.to_string();
    let uid = user_id.map(|u| u.to_string());

    let seasons: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM media_items WHERE parent_id = ? AND media_type = 'Season'",
    )
    .bind(&sid)
    .fetch_all(pool)
    .await?;

    if seasons.is_empty() {
        return Ok(Vec::new());
    }

    let season_placeholders: Vec<&str> = seasons.iter().map(|_| "?").collect();
    let mut sql = format!(
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.premiere_date > ?
         AND m.parent_id IN ({})",
        season_placeholders.join(",")
    );

    if uid.is_some() {
        sql.push_str(" AND m.id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)");
    }
    sql.push_str(" ORDER BY m.premiere_date ASC");

    let mut q = sqlx::query_as::<_, MediaItemRow>(sqlx::AssertSqlSafe(sql));
    q = q.bind(&now);
    for s in &seasons {
        q = q.bind(s);
    }
    if let Some(ref u) = uid {
        q = q.bind(u);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get all episodes for a series in a single query using a subquery.
/// Replaces the N+1 pattern of fetching seasons then episodes per season.
pub async fn get_episodes_for_series(
    pool: &SqlitePool,
    series_id: Uuid,
    season_number: Option<i32>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let sid = series_id.to_string();

    // Use a subquery to find all season IDs, then fetch episodes in one query
    let sql = if season_number.is_some() {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.parent_id IN (
             SELECT s.id FROM media_items s
             WHERE s.parent_id = ? AND s.media_type = 'Season' AND s.index_number = ?
         )
         ORDER BY m.parent_index_number, m.index_number"
    } else {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.parent_id IN (
             SELECT s.id FROM media_items s
             WHERE s.parent_id = ? AND s.media_type = 'Season'
         )
         ORDER BY m.parent_index_number, m.index_number"
    };

    let mut q = sqlx::query_as::<_, MediaItemRow>(sql).bind(&sid);
    if let Some(sn) = season_number {
        q = q.bind(sn);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get recommended movies: highly-rated movies sorted by rating
pub async fn get_recommendations(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let movie_type = "Movie".to_string();
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items
         WHERE media_type = ?
         AND community_rating >= 7.0
         ORDER BY community_rating DESC, date_created DESC
         LIMIT ?",
    )
    .bind(&movie_type)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get featured movies: recently added or highly rated
pub async fn get_featured_movies(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let movie_type = "Movie".to_string();
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items
         WHERE media_type = ?
         ORDER BY date_created DESC, community_rating DESC
         LIMIT ?",
    )
    .bind(&movie_type)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get all distinct music genres as MediaItem objects
pub async fn get_music_genres(pool: &SqlitePool) -> Result<Vec<MediaItem>, sqlx::Error> {
    // Extract distinct genres from music items
    let music_type = "Audio".to_string();
    let genre_strs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT genres FROM media_items WHERE media_type = ? AND genres != ''",
    )
    .bind(&music_type)
    .fetch_all(pool)
    .await?;

    let mut all_genres: Vec<String> = Vec::new();
    for gs in &genre_strs {
        for g in gs.split('|') {
            let g = g.trim().to_string();
            if !g.is_empty() && !all_genres.contains(&g) {
                all_genres.push(g);
            }
        }
    }
    all_genres.sort();

    // Return as genre items
    let mut items = Vec::new();
    for genre in &all_genres {
        let item = MediaItem {
            id: uuid::Uuid::new_v4(),
            name: genre.clone(),
            sort_name: Some(genre.to_lowercase()),
            overview: None,
            media_type: MediaType::Genre,
            path: String::new(),
            parent_id: None,
            library_id: uuid::Uuid::nil(),
            production_year: None,
            community_rating: None,
            official_rating: None,
            genres: vec![],
            date_created: chrono::Utc::now(),
            premiere_date: None,
            runtime_ticks: None,
            index_number: None,
            parent_index_number: None,
            media_sources: Vec::new(),
            ..MediaItem::default_dto_fields()
        };
        items.push(item);
    }
    Ok(items)
}

/// Get album artists
pub async fn get_album_artists(_pool: &SqlitePool) -> Result<Vec<MediaItem>, sqlx::Error> {
    Ok(Vec::new())
}

/// Get genres by name
pub async fn get_genres_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let _pattern = format!("%{}%", name);
    let music_type = "Audio".to_string();
    let genre_strs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT genres FROM media_items WHERE media_type = ? AND genres != ''",
    )
    .bind(&music_type)
    .fetch_all(pool)
    .await?;
    let mut items = Vec::new();
    for gs in &genre_strs {
        for g in gs.split('|') {
            let g = g.trim().to_string();
            if !g.is_empty() && g.to_lowercase().contains(&name.to_lowercase()) {
                let item = MediaItem {
                    id: uuid::Uuid::new_v4(),
                    name: g.clone(),
                    sort_name: Some(g.to_lowercase()),
                    overview: None,
                    media_type: MediaType::Genre,
                    path: String::new(),
                    parent_id: None,
                    library_id: uuid::Uuid::nil(),
                    production_year: None,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created: chrono::Utc::now(),
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                items.push(item);
            }
        }
    }
    Ok(items)
}

/// Get studios by name
pub async fn get_studios_by_name(
    pool: &SqlitePool,
    name: &str,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let pattern = format!("%{}%", name);
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT studios FROM media_items WHERE studios LIKE ? AND studios != ''",
    )
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    let mut items = Vec::new();
    for row in &rows {
        let item = MediaItem {
            id: uuid::Uuid::new_v4(),
            name: row.clone(),
            sort_name: Some(row.to_lowercase()),
            overview: None,
            media_type: MediaType::Unknown,
            path: String::new(),
            parent_id: None,
            library_id: uuid::Uuid::nil(),
            production_year: None,
            community_rating: None,
            official_rating: None,
            genres: vec![],
            date_created: chrono::Utc::now(),
            premiere_date: None,
            runtime_ticks: None,
            index_number: None,
            parent_index_number: None,
            media_sources: Vec::new(),
            ..MediaItem::default_dto_fields()
        };
        items.push(item);
    }
    Ok(items)
}

/// Get items by parent ID
pub async fn get_items_by_parent(
    pool: &SqlitePool,
    parent_id: uuid::Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let pid = parent_id.to_string();
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE parent_id = ?",
    )
    .bind(&pid)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get collections containing an item
pub async fn get_item_collections(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let iid = item_id.to_string();
    let collection_ids: Vec<String> =
        sqlx::query_scalar("SELECT collection_id FROM collection_items WHERE item_id = ?")
            .bind(&iid)
            .fetch_all(pool)
            .await?;
    let mut items = Vec::new();
    for cid in &collection_ids {
        if let Ok(uid) = uuid::Uuid::parse_str(cid) {
            if let Some(col) = crate::repo_collection::get_collection(pool, uid)
                .await
                .ok()
                .flatten()
            {
                let item = MediaItem {
                    id: uid,
                    name: col.name,
                    sort_name: None,
                    overview: None,
                    media_type: MediaType::Unknown,
                    path: String::new(),
                    parent_id: None,
                    library_id: uuid::Uuid::nil(),
                    production_year: None,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created: chrono::Utc::now(),
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                items.push(item);
            }
        }
    }
    Ok(items)
}

/// Delete item image
pub async fn delete_item_image(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
    image_type: &str,
) -> Result<(), sqlx::Error> {
    let iid = item_id.to_string();
    sqlx::query("DELETE FROM item_images WHERE item_id = ? AND image_type = ?")
        .bind(&iid)
        .bind(image_type)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete item image by index
pub async fn delete_item_image_index(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
    image_type: &str,
    index: i32,
) -> Result<(), sqlx::Error> {
    let iid = item_id.to_string();
    sqlx::query("DELETE FROM item_images WHERE item_id = ? AND image_type = ? AND image_index = ?")
        .bind(&iid)
        .bind(image_type)
        .bind(index)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get the last scan time for a library (max date_created of items in that library)
pub async fn get_last_scan_time(
    pool: &SqlitePool,
    library_id: uuid::Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
    let lid = library_id.to_string();
    let row: Option<(String,)> =
        sqlx::query_as("SELECT MAX(date_created) FROM media_items WHERE library_id = ?")
            .bind(&lid)
            .fetch_optional(pool)
            .await?;

    Ok(row.and_then(|r| r.0.parse().ok()))
}

/// Count items by media type for a given parent (folder counts).
pub async fn count_items_by_type(
    pool: &SqlitePool,
    parent_id: uuid::Uuid,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    let pid = parent_id.to_string();
    sqlx::query_as::<_, (String, i64)>(
        "SELECT media_type, COUNT(*) FROM media_items WHERE parent_id = ? GROUP BY media_type",
    )
    .bind(&pid)
    .fetch_all(pool)
    .await
}

/// Get a parent item by ID (for inherited image tags and series info).
pub async fn get_parent_item(
    pool: &SqlitePool,
    id: uuid::Uuid,
) -> Result<Option<MediaItem>, sqlx::Error> {
    get_item_by_id(pool, id).await
}

/// Batch-get parent items for a list of item IDs.
pub async fn get_parent_items_batch(
    pool: &SqlitePool,
    parent_ids: &[uuid::Uuid],
) -> Result<std::collections::HashMap<String, MediaItem>, sqlx::Error> {
    if parent_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut map = std::collections::HashMap::new();
    for pid in parent_ids {
        if let Some(item) = get_item_by_id(pool, *pid).await? {
            map.insert(pid.to_string(), item);
        }
    }
    Ok(map)
}

/// Get album info for an audio item (the parent MusicAlbum).
pub async fn get_album_for_audio(
    pool: &SqlitePool,
    parent_id: uuid::Uuid,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let pid = parent_id.to_string();
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT id, name, media_type FROM media_items WHERE id = ?")
            .bind(&pid)
            .fetch_optional(pool)
            .await?;

    match row {
        Some((id, name, media_type)) if media_type == "MusicAlbum" => {
            let uid = match uuid::Uuid::parse_str(&id) {
                Ok(u) => u,
                Err(_) => return Ok(None),
            };
            let album = MediaItem {
                id: uid,
                name,
                sort_name: None,
                overview: None,
                media_type: MediaType::MusicAlbum,
                path: String::new(),
                parent_id: None,
                library_id: uuid::Uuid::nil(),
                production_year: None,
                community_rating: None,
                official_rating: None,
                genres: vec![],
                date_created: chrono::Utc::now(),
                premiere_date: None,
                runtime_ticks: None,
                index_number: None,
                parent_index_number: None,
                tmdb_id: None,
                tvdb_id: None,
                media_sources: Vec::new(),
                ..MediaItem::default_dto_fields()
            };
            Ok(Some(album))
        }
        _ => Ok(None),
    }
}

/// A person credited on an item: (person_id, person_name, role, type, path of the primary
/// image at index 0, if any).
pub type PersonRow = (String, String, Option<String>, String, Option<String>);

/// Get artists (people) for an audio item, in credit order.
pub async fn get_artists_for_audio(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
) -> Result<Vec<PersonRow>, sqlx::Error> {
    let iid = item_id.to_string();
    sqlx::query_as::<_, PersonRow>(
        "SELECT p.id, p.name, ip.role, ip.type, (SELECT pi.path FROM person_images pi WHERE pi.person_id = p.id AND pi.image_type = 'Primary' AND pi.image_index = 0) FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id = ? ORDER BY ip.sort_order IS NULL, ip.sort_order, ip.rowid",
    )
    .bind(&iid)
    .fetch_all(pool)
    .await
}

/// Get studios for a media item.
/// Returns (studio_id, studio_name) tuples.
pub async fn get_studios_for_item(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let iid = item_id.to_string();
    sqlx::query_as::<_, (String, String)>(
        "SELECT s.id, s.name FROM item_studios ist JOIN studios s ON s.id = ist.studio_id WHERE ist.item_id = ?",
    )
    .bind(&iid)
    .fetch_all(pool)
    .await
}

/// Get the last media added date for a folder (max date_created of children).
pub async fn get_last_media_added(
    pool: &SqlitePool,
    parent_id: uuid::Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
    let pid = parent_id.to_string();
    let row: Option<(String,)> =
        sqlx::query_as("SELECT MAX(date_created) FROM media_items WHERE parent_id = ?")
            .bind(&pid)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|r| r.0.parse().ok()))
}

/// Batch-get folder counts for multiple parent IDs at once.
pub async fn get_folder_counts_batch(
    pool: &SqlitePool,
    parent_ids: &[uuid::Uuid],
) -> Result<std::collections::HashMap<String, std::collections::HashMap<String, i64>>, sqlx::Error>
{
    if parent_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let mut results: std::collections::HashMap<String, std::collections::HashMap<String, i64>> =
        std::collections::HashMap::new();
    for pid in parent_ids {
        let counts = count_items_by_type(pool, *pid).await?;
        let mut map = std::collections::HashMap::new();
        for (media_type, count) in counts {
            map.insert(media_type, count);
        }
        results.insert(pid.to_string(), map);
    }
    Ok(results)
}

mod tests {

    /// Serializes the tests that empty or fill the process-wide query cache.
    #[cfg(test)]
    static QUERY_CACHE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_filter_cache_key_tells_flags_apart() {
        let (sort_by, sort_order) = ("SortName", "Ascending");
        let key = |fav| {
            super::filter_cache_key(
                1, 0, 2, 1, 0, 0, 0, false, fav, None, None, sort_by, sort_order,
            )
        };
        assert_eq!(key(None), key(None));
        assert_ne!(key(None), key(Some(true)));
    }

    #[test]
    fn test_order_by_follows_jellyfin_sort_rules() {
        let order_by = |sort_by: &str, sort_order: &str| -> String {
            super::order_by_clause(&super::parse_sort(sort_by, sort_order))
        };
        assert_eq!(
            order_by("DateCreated,SortName,ProductionYear", "Descending"),
            "ORDER BY date_created DESC, sort_name DESC, production_year DESC, id DESC"
        );
        assert_eq!(
            order_by("SortName,ProductionYear", "Ascending"),
            "ORDER BY sort_name ASC, name ASC, production_year ASC, id ASC"
        );
        assert_eq!(
            order_by("CommunityRating", "Descending"),
            "ORDER BY community_rating DESC, sort_name ASC, id DESC"
        );
        assert_eq!(
            order_by("ProductionYear,SortName", "Sideways,Descending"),
            "ORDER BY production_year DESC, sort_name DESC, id DESC"
        );
        // Blank entries and Default are dropped, which leaves the default SortName.
        assert_eq!(
            order_by(" Default, ,", ""),
            "ORDER BY sort_name ASC, name ASC, id ASC"
        );
        // Default takes a place when the orders are paired, then goes, so ProductionYear is
        // Descending. A Random that isn't first is dropped after pairing, so CommunityRating keeps
        // its own order.
        assert_eq!(
            order_by("Default,ProductionYear", "Ascending,Descending"),
            "ORDER BY production_year DESC, sort_name ASC, id DESC"
        );
        assert_eq!(
            order_by(
                "ProductionYear,Random,CommunityRating",
                "Ascending,Ascending,Descending"
            ),
            "ORDER BY production_year ASC, community_rating DESC, sort_name ASC, id ASC"
        );
        // Once Default is gone, Random is first, so the page is random.
        assert!(super::is_random_sort(&super::parse_sort(
            "Default,Random",
            ""
        )));
        // Default alone leaves nothing, so SortName takes the first order.
        assert_eq!(
            order_by("Default", "Descending"),
            "ORDER BY sort_name DESC, name DESC, id DESC"
        );
        // Random sorts only as the first key.
        assert!(super::is_random_sort(&super::parse_sort(
            "random,SortName",
            ""
        )));
        assert!(!super::is_random_sort(&super::parse_sort(
            "SortName,Random",
            ""
        )));
    }

    #[test]
    fn test_order_by_adds_each_expression_once() {
        let order_by = |sort_by: &str, sort_order: &str| -> String {
            super::order_by_clause(&super::parse_sort(sort_by, sort_order))
        };
        // As EF Core does for Jellyfin, a term whose expression is already in the clause is
        // skipped, and the first one keeps its order. SQLite allows 2,000 terms.
        assert_eq!(
            order_by(&["Name"; 2000].join(","), "Ascending"),
            "ORDER BY name ASC, id ASC"
        );
        assert_eq!(
            order_by(
                "ProductionYear,DateAdded,productionyear,DateCreated",
                "Ascending,Descending,Descending"
            ),
            "ORDER BY production_year ASC, date_created DESC, sort_name ASC, id ASC"
        );
        // Nor are the Name that a first SortName brings along and the sort_name tie-break added
        // again.
        assert_eq!(
            order_by("SortName,Name,CommunityRating", "Descending"),
            "ORDER BY sort_name DESC, name DESC, community_rating DESC, id DESC"
        );
        assert_eq!(
            order_by("Album,ProductionYear", "Descending"),
            "ORDER BY sort_name DESC, production_year DESC, id DESC"
        );
        // So a list repeated any number of times sorts as the list itself.
        let keys = [
            "SortName",
            "Name",
            "DateCreated",
            "PremiereDate",
            "ProductionYear",
            "CommunityRating",
            "Runtime",
            "ParentIndexNumber",
            "IndexNumber",
            "Random",
            "Album",
        ];
        assert_eq!(
            order_by(&keys.repeat(500).join(","), "Descending"),
            order_by(&keys.join(","), "Descending")
        );
    }

    #[test]
    fn test_filter_query_cache_is_keyed_on_parsed_terms_and_bounded() {
        let _serial = QUERY_CACHE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let key = |sort_by: &str, sort_order: &str| {
            super::filter_cache_key(
                0, 0, 0, 0, 0, 0, 0, true, None, None, None, sort_by, sort_order,
            )
        };
        let build = |genres: Option<&[String]>, sort_by: &str, sort_order: &str| {
            super::build_filtered_query(
                true, None, None, genres, None, None, None, None, None, None, None, sort_by,
                sort_order,
            )
        };
        // Lists that parse to the same terms share an entry. Different terms never do, even when
        // their raw strings run together alike.
        assert_eq!(
            key("Name,name, Name", "Ascending"),
            key("Name", "ascending,")
        );
        assert_ne!(key("SortName", "Ascending"), key("SortNameA", "scending"));
        // A shape is stored while there is room.
        let sort_by = "DateCreated,SortName,ProductionYear";
        let built = build(None, sort_by, "Descending");
        assert_eq!(
            super::FILTER_QUERY_CACHE
                .lock()
                .get(&key(sort_by, "Descending")),
            Some(&built)
        );
        // A query over 16 KiB is not: each genre adds a clause.
        let genres = vec!["Drama".to_string(); 1000];
        let big = || {
            super::filter_cache_key(
                0,
                0,
                genres.len(),
                0,
                0,
                0,
                0,
                true,
                None,
                None,
                None,
                "SortName",
                "Ascending",
            )
        };
        let (count_sql, data_sql) = build(Some(genres.as_slice()), "SortName", "Ascending");
        assert!(count_sql.len() + data_sql.len() > 16 * 1024);
        assert!(!super::FILTER_QUERY_CACHE.lock().contains_key(&big()));
        // However many distinct lists come, at most 256 entries are kept.
        let keys = [
            "SortName",
            "Name",
            "DateCreated",
            "PremiereDate",
            "ProductionYear",
            "CommunityRating",
            "Runtime",
            "ParentIndexNumber",
            "IndexNumber",
        ];
        let mut lists = 0;
        for a in keys {
            for b in keys {
                for c in keys {
                    if a == b || b == c || a == c {
                        continue;
                    }
                    for order in ["Ascending", "Descending"] {
                        let sort_by = format!("{a},{b},{c}");
                        build(None, &sort_by, order);
                        lists += 1;
                    }
                }
            }
        }
        assert_eq!(lists, 1008);
        assert!(super::FILTER_QUERY_CACHE.lock().len() <= 256);
    }

    /// One included type and 63 excluded types, all else alike, gave the same 64-bit key, so the
    /// shape stored first served its SQL to the other. Each gets its own, in either order.
    #[test]
    fn test_filter_query_cache_serves_each_shape_its_own_sql() {
        let _serial = QUERY_CACHE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let included = ["Movie".to_string()];
        let excluded: Vec<String> = (0..63).map(|i| format!("Type{i}")).collect();
        let (fav, sort_by, sort_order) = (Some(true), "SortName", "Ascending");
        let build = |include: Option<&[String]>, exclude: Option<&[String]>| {
            super::build_filtered_query(
                true, include, exclude, None, None, None, None, None, fav, None, None, sort_by,
                sort_order,
            )
        };
        let one = || build(Some(included.as_slice()), None);
        let many = || build(None, Some(excluded.as_slice()));
        // What each gets from an empty cache.
        super::FILTER_QUERY_CACHE.lock().clear();
        let one_sql = one();
        super::FILTER_QUERY_CACHE.lock().clear();
        let many_sql = many();
        assert!(one_sql.1.contains("media_type IN (?)"));
        let not_in = format!("media_type NOT IN ({})", ["?"; 63].join(","));
        assert!(many_sql.1.contains(&not_in));
        // Whichever is stored first, each still gets its own.
        super::FILTER_QUERY_CACHE.lock().clear();
        assert_eq!(one(), one_sql);
        assert_eq!(many(), many_sql, "after one included type");
        assert_eq!(one(), one_sql);
        super::FILTER_QUERY_CACHE.lock().clear();
        assert_eq!(many(), many_sql);
        assert_eq!(one(), one_sql, "after 63 excluded types");
        assert_eq!(many(), many_sql);
    }

    /// Each input the cache key holds tells two requests apart. Each pair differs in one input, in
    /// which of two lists holds a value, or in the order or directions of the sort terms. The two
    /// get different keys, and whichever is stored first, both get the SQL they get from an empty
    /// cache. A key that lost an input or merged two would serve one the other's SQL, with its
    /// rows in the wrong order or its binds in the wrong places.
    #[test]
    fn test_filter_query_cache_tells_every_shape_input_apart() {
        /// The inputs `build_filtered_query` renders, for one request.
        #[derive(Clone)]
        struct FilterRequest {
            media_types: Option<Vec<String>>,
            exclude_media_types: Option<Vec<String>>,
            genres: Option<Vec<String>>,
            years: Option<Vec<i32>>,
            official_ratings: Option<Vec<String>>,
            tags: Option<Vec<String>>,
            media_types_filter: Option<Vec<String>>,
            is_favorite: Option<bool>,
            is_unplayed: Option<bool>,
            is_played: Option<bool>,
            sort_by: &'static str,
            sort_order: &'static str,
        }
        /// A list of `n` values, or no list for 0.
        fn words(n: usize) -> Option<Vec<String>> {
            (n > 0).then(|| (0..n).map(|i| format!("w{i}")).collect())
        }
        fn years(n: usize) -> Option<Vec<i32>> {
            (n > 0).then(|| (0..n).map(|i| 2000 + i as i32).collect())
        }
        /// `r`'s SQL through the cache.
        fn cached_sql(r: &FilterRequest) -> (String, String) {
            super::build_filtered_query(
                true,
                r.media_types.as_deref(),
                r.exclude_media_types.as_deref(),
                r.genres.as_deref(),
                r.years.as_deref(),
                r.official_ratings.as_deref(),
                r.tags.as_deref(),
                r.media_types_filter.as_deref(),
                r.is_favorite,
                r.is_unplayed,
                r.is_played,
                r.sort_by,
                r.sort_order,
            )
        }
        /// The length of a list, or 0 for no list.
        fn len<T>(list: Option<&[T]>) -> usize {
            list.map_or(0, <[T]>::len)
        }
        /// `r`'s cache key.
        fn key(r: &FilterRequest) -> super::FilterShape {
            super::filter_cache_key(
                len(r.media_types.as_deref()),
                len(r.exclude_media_types.as_deref()),
                len(r.genres.as_deref()),
                len(r.years.as_deref()),
                len(r.official_ratings.as_deref()),
                len(r.tags.as_deref()),
                len(r.media_types_filter.as_deref()),
                true,
                r.is_favorite,
                r.is_unplayed,
                r.is_played,
                r.sort_by,
                r.sort_order,
            )
        }

        let _serial = QUERY_CACHE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let base = FilterRequest {
            media_types: None,
            exclude_media_types: None,
            genres: None,
            years: None,
            official_ratings: None,
            tags: None,
            media_types_filter: None,
            is_favorite: None,
            is_unplayed: None,
            is_played: None,
            sort_by: "SortName",
            sort_order: "Ascending",
        };
        let mut pairs = Vec::new();
        let lists: [(&str, fn(&mut FilterRequest, usize)); 7] = [
            ("media_types_len", |r, n| r.media_types = words(n)),
            ("exclude_media_types_len", |r, n| {
                r.exclude_media_types = words(n)
            }),
            ("genres_len", |r, n| r.genres = words(n)),
            ("years_len", |r, n| r.years = years(n)),
            ("official_ratings_len", |r, n| r.official_ratings = words(n)),
            ("tags_len", |r, n| r.tags = words(n)),
            ("media_types_filter_len", |r, n| {
                r.media_types_filter = words(n)
            }),
        ];
        for (field, set) in lists {
            for (a, b) in [(0, 1), (1, 2)] {
                let (mut x, mut y) = (base.clone(), base.clone());
                set(&mut x, a);
                set(&mut y, b);
                pairs.push((format!("{field} {a} vs {b}"), x, y, false));
            }
        }
        // One value in one list vs one in another, for each two lists. A key that merged two
        // lengths would give both the same key.
        for (i, &(x_field, set_x)) in lists.iter().enumerate() {
            for &(y_field, set_y) in &lists[i + 1..] {
                let (mut x, mut y) = (base.clone(), base.clone());
                set_x(&mut x, 1);
                set_y(&mut y, 1);
                // IncludeItemTypes and MediaTypes both render `media_type IN (?)`, so only their
                // keys tell these two apart.
                let same_sql = (x_field, y_field) == ("media_types_len", "media_types_filter_len");
                pairs.push((format!("{x_field} vs {y_field}"), x, y, same_sql));
            }
        }
        let flags: [(&str, fn(&mut FilterRequest, Option<bool>)); 3] = [
            ("is_favorite", |r, v| r.is_favorite = v),
            ("is_unplayed", |r, v| r.is_unplayed = v),
            ("is_played", |r, v| r.is_played = v),
        ];
        for (field, set) in flags {
            for (a, b) in [
                (None, Some(true)),
                (None, Some(false)),
                (Some(true), Some(false)),
            ] {
                let (mut x, mut y) = (base.clone(), base.clone());
                set(&mut x, a);
                set(&mut y, b);
                pairs.push((format!("{field} {a:?} vs {b:?}"), x, y, false));
            }
        }
        for (a, b) in [
            (("SortName", "Ascending"), ("DateCreated", "Ascending")),
            (("SortName", "Ascending"), ("SortName", "Descending")),
            (
                ("SortName", "Ascending"),
                ("SortName,DateCreated", "Ascending"),
            ),
            // The order of the terms, and a later term's own direction.
            (
                ("SortName,DateCreated", "Ascending"),
                ("DateCreated,SortName", "Ascending"),
            ),
            (
                ("SortName,DateCreated", "Ascending,Ascending"),
                ("SortName,DateCreated", "Ascending,Descending"),
            ),
        ] {
            let x = FilterRequest {
                sort_by: a.0,
                sort_order: a.1,
                ..base.clone()
            };
            let y = FilterRequest {
                sort_by: b.0,
                sort_order: b.1,
                ..base.clone()
            };
            pairs.push((format!("sort {a:?} vs {b:?}"), x, y, false));
        }
        assert_eq!(
            pairs.len(),
            49,
            "7 lists at 2 pairs, 21 list swaps, 3 flags at 3 and 5 sorts"
        );

        let clear = || super::FILTER_QUERY_CACHE.lock().clear();
        let (mut same_key, mut shared) = (Vec::new(), Vec::new());
        for (case, x, y, same_sql) in &pairs {
            // The keys themselves: through the cache, equal keys that hash apart rarely meet.
            if key(x) == key(y) {
                same_key.push(case.as_str());
            }
            // What each gets from an empty cache. The pair must render different SQL, except the
            // two lists that render the same clause.
            clear();
            let x_sql = cached_sql(x);
            clear();
            let y_sql = cached_sql(y);
            if *same_sql {
                assert_eq!(x_sql, y_sql, "{case}: expected the same SQL");
            } else {
                assert_ne!(x_sql, y_sql, "{case}: both render the same SQL");
            }
            // Each order, from an empty cache.
            clear();
            let x_first = (cached_sql(x), cached_sql(y));
            clear();
            let y_first = (cached_sql(y), cached_sql(x));
            if x_first != (x_sql.clone(), y_sql.clone()) || y_first != (y_sql, x_sql) {
                shared.push(case.as_str());
            }
        }
        assert!(
            same_key.is_empty() && shared.is_empty(),
            "these pairs got the same key: {same_key:?}; these shared SQL: {shared:?}"
        );
    }
}
