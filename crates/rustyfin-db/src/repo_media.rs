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

    sqlx::query(
        "INSERT OR REPLACE INTO media_items \
         (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
          production_year, community_rating, official_rating, genres, date_created, \
          premiere_date, runtime_ticks, index_number, parent_index_number) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

        sqlx::query(
            "INSERT OR REPLACE INTO media_items \
             (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
              production_year, community_rating, official_rating, genres, date_created, \
              premiere_date, runtime_ticks, index_number, parent_index_number) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        "SELECT * FROM media_items WHERE library_id = ? ORDER BY sort_name, name",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_item_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query_as::<_, MediaItemRow>("SELECT * FROM media_items WHERE id = ?")
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
        "SELECT * FROM media_items WHERE parent_id = ? ORDER BY index_number, sort_name, name",
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
        "SELECT * FROM media_items WHERE parent_id = ? ORDER BY index_number, sort_name, name LIMIT ? OFFSET ?",
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
    // Static SQL: always the same string regardless of which params are provided.
    // NULL/empty params are skipped by the WHERE clause, so SQLite reuses the query plan.
    let mt = media_type.unwrap_or("");
    let lid = library_id.map(|u| u.to_string()).unwrap_or_default();

    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT * FROM media_items \
         WHERE (? IS NULL OR ? = '' OR media_type = ?) \
         AND (? IS NULL OR ? = '' OR library_id = ?) \
         ORDER BY date_created DESC LIMIT ?",
    )
    .bind(media_type)
    .bind(media_type)
    .bind(mt)
    .bind(&lid)
    .bind(&lid)
    .bind(lid.clone())
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
fn hash_combine(mut hash: u64, key: u64) -> u64 {
    hash ^= key + 0x9e3779b97f4a7c15 + (hash << 6) + (hash >> 2);
    hash
}

/// Build a cache key from the filter array sizes.
/// Same array sizes → same key → same cached query plan.
fn filter_cache_key(
    media_types_len: usize,
    exclude_media_types_len: usize,
    genres_len: usize,
    years_len: usize,
    official_ratings_len: usize,
    tags_len: usize,
    media_types_filter_len: usize,
) -> u64 {
    let mut h = 0u64;
    h = hash_combine(h, media_types_len as u64);
    h = hash_combine(h, exclude_media_types_len as u64);
    h = hash_combine(h, genres_len as u64);
    h = hash_combine(h, years_len as u64);
    h = hash_combine(h, official_ratings_len as u64);
    h = hash_combine(h, tags_len as u64);
    h = hash_combine(h, media_types_filter_len as u64);
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
                "(genres = ? OR genres LIKE '?|%' OR genres LIKE '%|?' OR genres LIKE '%|?|%')"
                    .to_string(),
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
            where_parts.push(
                "(tags = ? OR tags LIKE '?|%' OR tags LIKE '%|?' OR tags LIKE '%|?|%')".to_string(),
            );
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
        AND (? IS NULL OR ? = '' OR premiere_date >= ?)
        AND (? IS NULL OR ? = '' OR premiere_date <= ?)
        AND (? IS NULL OR ? = '' OR premiere_date >= ?)
        AND (? IS NULL OR ? = '' OR date_created >= ?)
        AND (? IS NULL OR ? = '' OR runtime_ticks >= ?)
        AND (? IS NULL OR ? = '' OR runtime_ticks <= ?)
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
        "SELECT * FROM media_items WHERE {}{} {} LIMIT ? OFFSET ?",
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
    let sd_str = start_date.unwrap_or_default();
    let ed_str = end_date.unwrap_or_default();
    let pd_str = premiere_date.unwrap_or_default();
    let dc_str = date_created.unwrap_or_default();
    let (min_ticks, max_ticks) = runtime
        .map(|(min, max)| {
            (
                (min as i64) * 60 * 10_000_000,
                (max as i64) * 60 * 10_000_000,
            )
        })
        .unwrap_or((0i64, 0i64));

    let sort_by_str = sort_by.unwrap_or("SortName");
    let sort_order_str = sort_order.unwrap_or("Ascending");

    // Build cache key from array filter sizes and recursive flag
    let mut cache_key = filter_cache_key(
        media_types.map(|t| t.len()).unwrap_or(0),
        exclude_media_types.map(|t| t.len()).unwrap_or(0),
        genres.map(|g| g.len()).unwrap_or(0),
        years.map(|y| y.len()).unwrap_or(0),
        official_ratings.map(|r| r.len()).unwrap_or(0),
        tags.map(|t| t.len()).unwrap_or(0),
        media_types_filter.map(|t| t.len()).unwrap_or(0),
    );
    // Include recursive flag in cache key since it affects the WHERE clause
    cache_key = hash_combine(cache_key, recursive as u64);

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
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    // Base single-value params (same order in both count and data queries)
    count_q = count_q
        .bind(&recursive_flag)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&search_like)
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
        .bind(&min_ticks)
        .bind(&max_ticks)
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
    let mut data_q = sqlx::query_as::<_, MediaItemRow>(&data_sql);
    data_q = data_q
        .bind(&recursive_flag)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&recursive_flag)
        .bind(&recursive_flag)
        .bind(&pid_str)
        .bind(&pid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&lid_str)
        .bind(&search_like)
        .bind(&search_like)
        .bind(&search_like)
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
        .bind(&min_ticks)
        .bind(&max_ticks)
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

pub async fn get_similar_items(
    pool: &SqlitePool,
    item: &MediaItem,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let media_type = item.media_type.item_kind().to_string();
    // Use ROWID-based random sampling instead of ORDER BY RANDOM() for performance
    // Generate a random offset and fetch from there
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
        "SELECT * FROM media_items WHERE media_type = ? AND id != ? AND library_id = ? \
         ORDER BY ROWID LIMIT ? OFFSET ?",
    )
    .bind(&media_type)
    .bind(item.id.to_string())
    .bind(item.library_id.to_string())
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Get filter values (genres, years, ratings) using static SQL for plan reuse.
pub async fn get_filters(
    pool: &SqlitePool,
    library_id: Option<Uuid>,
) -> Result<(Vec<String>, Vec<i32>, Vec<String>), sqlx::Error> {
    // Static SQL: library_id filter uses (? = '' OR library_id = ?) to skip when empty.
    let lid = library_id.map(|u| u.to_string()).unwrap_or_default();

    // Genres
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

    // Years
    let years: Vec<i32> = sqlx::query_scalar::<_, i32>(
        "SELECT DISTINCT production_year FROM media_items \
         WHERE production_year IS NOT NULL AND (? = '' OR library_id = ?) \
         ORDER BY production_year DESC",
    )
    .bind(&lid)
    .bind(lid.clone())
    .fetch_all(pool)
    .await?;

    // Official ratings
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
    let row = sqlx::query_as::<_, MediaItemRow>("SELECT * FROM media_items WHERE path = ?")
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
        "SELECT m.* FROM media_items m \
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
        "SELECT m.* FROM media_items m \
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
            end_date: None,
            series_name: None,
            series_id: None,
            season_name: None,
            status: None,
            custom_rating: None,
            critic_rating: None,
            audience_rating: None,
            tags: Vec::new(),
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
        "SELECT m.* FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.id NOT IN (
             SELECT item_id FROM user_item_data
             WHERE user_id = ? AND played = 1
         )
         ORDER BY m.parent_index_number, m.index_number"
    } else {
        "SELECT m.* FROM media_items m
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
        "SELECT m.* FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.parent_id IN ({})",
        season_placeholders.join(",")
    );

    if uid.is_some() {
        sql.push_str(" AND m.id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)");
    }
    sql.push_str(" ORDER BY m.parent_index_number, m.index_number");

    let mut q = sqlx::query_as::<_, MediaItemRow>(&sql);
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
        "SELECT m.* FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.premiere_date > ?
         AND m.id NOT IN (
             SELECT item_id FROM user_item_data
             WHERE user_id = ? AND played = 1
         )
         ORDER BY m.premiere_date ASC"
    } else {
        "SELECT m.* FROM media_items m
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
        "SELECT m.* FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.premiere_date > ?
         AND m.parent_id IN ({})",
        season_placeholders.join(",")
    );

    if uid.is_some() {
        sql.push_str(" AND m.id NOT IN (SELECT item_id FROM user_item_data WHERE user_id = ? AND played = 1)");
    }
    sql.push_str(" ORDER BY m.premiere_date ASC");

    let mut q = sqlx::query_as::<_, MediaItemRow>(&sql);
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
        "SELECT m.* FROM media_items m
         WHERE m.media_type = 'Episode'
         AND m.parent_id IN (
             SELECT s.id FROM media_items s
             WHERE s.parent_id = ? AND s.media_type = 'Season' AND s.index_number = ?
         )
         ORDER BY m.parent_index_number, m.index_number"
    } else {
        "SELECT m.* FROM media_items m
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
        "SELECT * FROM media_items
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
        "SELECT * FROM media_items
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
