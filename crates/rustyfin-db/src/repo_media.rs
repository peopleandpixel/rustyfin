#![allow(
    clippy::needless_borrow,
    clippy::collapsible_if,
    clippy::needless_borrows_for_generic_args
)]

use chrono::{DateTime, Utc};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::LazyLock;
use uuid::Uuid;

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
        "INSERT OR REPLACE INTO media_items \
         (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
          production_year, community_rating, official_rating, genres, date_created, \
          premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
          tags, status, original_language, tagline, production_countries, budget, revenue, \
          homepage, end_date, vote_count, popularity) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(item.vote_count)
    .bind(item.popularity)
    .execute(pool)
    .await?;

    Ok(())
}

/// Batch-insert multiple media items in a single transaction.
/// Dramatically reduces WAL contention compared to per-item inserts.
pub async fn batch_upsert_media_items(
    pool: &SqlitePool,
    items: &[MediaItem],
) -> Result<(), sqlx::Error> {
    if items.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

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
            "INSERT OR REPLACE INTO media_items \
             (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
              production_year, community_rating, official_rating, genres, date_created, \
              premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
              tags, status, original_language, tagline, production_countries, budget, revenue, \
              homepage, end_date, vote_count, popularity) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(item.vote_count)
        .bind(item.popularity)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn get_items_by_library(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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

pub async fn delete_item(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    // Delete children first (recursive via foreign key would be nice, but let's be explicit)
    sqlx::query("DELETE FROM media_items WHERE parent_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    let result = sqlx::query("DELETE FROM media_items WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
/// Maps a query signature (hash of the SQL structure) to a cached SQL string,
/// so repeated calls with the same filter shape reuse the same query plan.
///
/// The signature is built from the number of array elements in each filter,
/// not the actual values, so different values with the same shape share a plan.
use std::sync::Mutex;

type QueryCache = Mutex<HashMap<u64, (String, String)>>;

static FILTER_QUERY_CACHE: LazyLock<QueryCache> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Simple hash combiner for building a cache key from multiple u64 values.
pub(crate) fn hash_combine(mut hash: u64, key: u64) -> u64 {
    hash ^= key
        .wrapping_add(0x9e3779b97f4a7c15)
        .wrapping_add(hash << 6)
        .wrapping_add(hash >> 2);
    hash
}

fn opt_bool_to_u64(v: Option<bool>) -> u64 {
    match v {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

/// Build a cache key from the filter array sizes and options.
/// Same filter shape and sorting → same key → same cached query plan.
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
) -> u64 {
    let mut h = 0u64;
    h = hash_combine(h, media_types_len as u64);
    h = hash_combine(h, exclude_media_types_len as u64);
    h = hash_combine(h, genres_len as u64);
    h = hash_combine(h, years_len as u64);
    h = hash_combine(h, official_ratings_len as u64);
    h = hash_combine(h, tags_len as u64);
    h = hash_combine(h, media_types_filter_len as u64);
    h = hash_combine(h, recursive as u64);
    h = hash_combine(h, opt_bool_to_u64(is_favorite));
    h = hash_combine(h, opt_bool_to_u64(is_unplayed));
    h = hash_combine(h, opt_bool_to_u64(is_played));
    for b in sort_by.as_bytes() {
        h = hash_combine(h, *b as u64);
    }
    for b in sort_order.as_bytes() {
        h = hash_combine(h, *b as u64);
    }
    h
}

/// Build the SQL query with a static base for single-value params and dynamic
/// parts only for array filters. Uses the cache to reuse query plans when the
/// filter array sizes match a previous call.
#[allow(clippy::too_many_arguments)]
fn build_filtered_query(
    cache_key: u64,
    _recursive: bool,
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
    // Check cache first
    {
        let cache = FILTER_QUERY_CACHE.lock().unwrap();
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

    // IsFavorite / IsPlayed / IsUnplayed — static subqueries
    if is_favorite == Some(true) {
        where_parts
            .push("id IN (SELECT item_id FROM user_item_data WHERE is_favorite = 1)".to_string());
    } else if is_favorite == Some(false) {
        where_parts.push(
            "id NOT IN (SELECT item_id FROM user_item_data WHERE is_favorite = 1)".to_string(),
        );
    }
    if is_played == Some(true) {
        where_parts.push("id IN (SELECT item_id FROM user_item_data WHERE played = 1)".to_string());
    } else if is_played == Some(false) {
        where_parts
            .push("id NOT IN (SELECT item_id FROM user_item_data WHERE played = 1)".to_string());
    }
    if is_unplayed == Some(true) {
        where_parts
            .push("id NOT IN (SELECT item_id FROM user_item_data WHERE played = 1)".to_string());
    } else if is_unplayed == Some(false) {
        where_parts.push("id IN (SELECT item_id FROM user_item_data WHERE played = 1)".to_string());
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
    ";

    // Combine base WHERE with array filter WHERE parts
    let array_where = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" AND {}", where_parts.join(" AND "))
    };

    let is_random = sort_by == "Random";
    let order_col = match sort_by {
        "DateCreated" | "DateAdded" => "date_created",
        "PremiereDate" => "premiere_date",
        "ProductionYear" => "production_year",
        "CommunityRating" | "CriticRating" => "community_rating",
        "Name" => "name",
        "Random" => "ROWID",
        "Runtime" => "runtime_ticks",
        _ => "sort_name",
    };
    let dir = if sort_order == "Descending" {
        "DESC"
    } else {
        "ASC"
    };

    let order_clause = if is_random {
        "ORDER BY ROWID".to_string()
    } else {
        format!("ORDER BY {} {}", order_col, dir)
    };

    let count_sql = format!(
        "SELECT COUNT(*) as cnt FROM media_items WHERE {}{}",
        base_where, array_where
    );
    let data_sql = format!(
        "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
         production_year, community_rating, official_rating, genres, date_created, \
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
         tags, status, original_language, tagline, production_countries, budget, revenue, \
         homepage, end_date, vote_count, popularity \
         FROM media_items WHERE {}{} {} LIMIT ? OFFSET ?",
        base_where, array_where, order_clause
    );

    // Cache the result
    {
        let mut cache = FILTER_QUERY_CACHE.lock().unwrap();
        cache.insert(cache_key, (count_sql.clone(), data_sql.clone()));
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
    let (min_ticks, max_ticks) = runtime
        .map(|(min, max)| {
            (
                Some((min as i64) * 60 * 10_000_000),
                Some((max as i64) * 60 * 10_000_000),
            )
        })
        .unwrap_or((None, None));

    let sort_by_str = sort_by.unwrap_or("SortName");
    let sort_order_str = sort_order.unwrap_or("Ascending");

    // Build cache key from array filter sizes and options
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
        sort_by_str,
        sort_order_str,
    );

    // Get cached query strings — same array sizes → same plan
    let (count_sql, data_sql) = build_filtered_query(
        cache_key,
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
        .bind(&max_ticks);

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
        .bind(&max_ticks);

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

    // Pagination
    let is_random = sort_by_str == "Random";
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
             premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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

/// Fetch all existing paths for items in a specific library.
pub async fn get_library_paths(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT path FROM media_items WHERE library_id = ?")
        .bind(library_id.to_string())
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Fetch all items of a specific media type for a library, returning path -> id mapping.
/// Used by the scanner to pre-load series/season/artist/album IDs before directory traversal,
/// avoiding per-item database queries during scanning.
pub async fn get_library_items_by_type(
    pool: &SqlitePool,
    library_id: Uuid,
    media_type: &str,
) -> Result<HashMap<String, Uuid>, sqlx::Error> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT path, id FROM media_items WHERE library_id = ? AND media_type = ?")
            .bind(library_id.to_string())
            .bind(media_type)
            .fetch_all(pool)
            .await?;
    let map = rows
        .into_iter()
        .filter_map(|(path, id)| Uuid::parse_str(&id).ok().map(|uid| (path, uid)))
        .collect();
    Ok(map)
}

/// Get items with playback position > 0 (resume items) for all users
pub async fn get_resume_items(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m \
         INNER JOIN user_item_data u ON u.item_id = m.id \
         WHERE u.playback_position_ticks > 0 \
         ORDER BY u.last_played_date DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
            location_type: rustyfin_model::media::LocationType::FileSystem,
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

/// Get "Next Up" episodes: unwatched episodes for series that have been watched
pub async fn get_next_up(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let uid = user_id.map(|u| u.to_string());

    let sql = if uid.is_some() {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.id NOT IN (
             SELECT item_id FROM user_item_data
             WHERE user_id = ? AND played = 1
         )
         ORDER BY m.parent_index_number, m.index_number"
    } else {
        "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
         m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         ORDER BY m.parent_index_number, m.index_number"
    };

    let mut q = sqlx::query_as::<_, MediaItemRow>(sql);
    if let Some(ref u) = uid {
        q = q.bind(u);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get "Next Up" episodes for a specific series
pub async fn get_next_up_for_series(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
    series_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
         m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
         m.homepage, m.end_date, m.vote_count, m.popularity \
         FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.parent_id IN ({})",
        season_placeholders.join(",")
    );

    if uid.is_some() {
        sql.push_str(" AND m.id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)");
    }
    sql.push_str(" ORDER BY m.parent_index_number, m.index_number");

    let mut q = sqlx::query_as::<_, MediaItemRow>(sqlx::AssertSqlSafe(sql));
    for s in &seasons {
        q = q.bind(s);
    }
    if let Some(ref u) = uid {
        q = q.bind(u);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
         m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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
         premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
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

/// Get artists (people) for an audio item.
/// Returns (person_id, person_name, role) tuples.
pub async fn get_artists_for_audio(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
) -> Result<Vec<(String, String, Option<String>)>, sqlx::Error> {
    let iid = item_id.to_string();
    sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT p.id, p.name, ip.role FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id = ?",
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

    #[test]
    fn test_hash_combine_no_overflow() {
        let mut h = 0u64;
        for i in 0..100 {
            h = super::hash_combine(h, i);
        }
        assert_ne!(h, 0);

        let key = super::filter_cache_key(
            1,
            0,
            2,
            1,
            0,
            0,
            0,
            false,
            None,
            None,
            None,
            "SortName",
            "Ascending",
        );
        assert_ne!(key, 0);

        let key2 = super::filter_cache_key(
            1,
            0,
            2,
            1,
            0,
            0,
            0,
            false,
            Some(true),
            None,
            None,
            "SortName",
            "Ascending",
        );
        assert_ne!(key, key2);
    }
}
