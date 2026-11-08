use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::search::SearchHint;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Normalize a search query for FTS5: escape special characters and add prefix wildcard
fn normalize_fts_query(query: &str) -> String {
    // Escape FTS5 special characters: * ? " \
    let escaped = query
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('*', "\\*")
        .replace('?', "\\?");
    // Add wildcard for prefix matching
    format!("{}*", escaped)
}

/// Fallback LIKE search using prefix matching for better index utilization
/// Uses `query%` instead of `%query%` so B-tree indexes can be used
fn like_search_hints_query() -> &'static str {
    "SELECT id, name, media_type, production_year, runtime_ticks
     FROM media_items
     WHERE name LIKE ?1
     LIMIT ?2"
}

pub async fn search_hints(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchHint>, sqlx::Error> {
    // Try FTS5 first for full-text search across name, overview, and genres
    let fts_query = normalize_fts_query(query);
    let rows = sqlx::query_as::<_, (String, String, String, Option<i32>, Option<i64>)>(
        "SELECT m.id, m.name, m.media_type, m.production_year, m.runtime_ticks
         FROM media_items m
         JOIN media_items_fts f ON f.rowid = m.rowid
         WHERE media_items_fts MATCH ?1
         LIMIT ?2",
    )
    .bind(&fts_query)
    .bind(limit)
    .fetch_all(pool)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(_) => {
            // Fallback to LIKE search with prefix matching for index utilization
            // Using `query%` instead of `%query%` enables B-tree index scan on idx_media_items_name
            let like_prefix = format!("{}%", query);
            sqlx::query_as::<_, (String, String, String, Option<i32>, Option<i64>)>(
                like_search_hints_query(),
            )
            .bind(&like_prefix)
            .bind(limit)
            .fetch_all(pool)
            .await?
        }
    };

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let id = Uuid::parse_str(&r.0).ok()?;
            Some(SearchHint {
                id,
                name: r.1,
                item_type: r.2.clone(),
                media_type: Some(r.2),
                production_year: r.3,
                run_time_ticks: r.4,
            })
        })
        .collect())
}

/// Full search returning MediaItem results
/// Uses FTS5 for full-text search across name, overview, and genres,
/// with LIKE prefix matching as fallback.
pub async fn search_items(
    pool: &SqlitePool,
    query: &str,
    item_types: &[MediaType],
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    // Build media type filter
    let type_filters: Vec<&'static str> = item_types.iter().map(|t| t.item_kind()).collect();

    // Try FTS5 first for full-text search
    let fts_query = normalize_fts_query(query);
    let rows = if type_filters.is_empty() {
        // Search all types via FTS5
        sqlx::query_as::<_, super::repo_media::MediaItemRow>(
            "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
             m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
             m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
             m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
             m.homepage, m.end_date, m.vote_count, m.popularity \
             FROM media_items m \
             JOIN media_items_fts f ON f.rowid = m.rowid \
             WHERE media_items_fts MATCH ?1 \
             LIMIT ?2",
        )
        .bind(&fts_query)
        .bind(limit)
        .fetch_all(pool)
        .await
    } else {
        // Search with media type filter via FTS5. All placeholders are bare `?`, bound in
        // order: mixing `?N` with `?` makes SQLite reuse ?2 for LIMIT.
        let placeholders: Vec<&str> = type_filters.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
             m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
             m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.tmdb_id, m.tvdb_id, \
             m.tags, m.status, m.original_language, m.tagline, m.production_countries, m.budget, m.revenue, \
             m.homepage, m.end_date, m.vote_count, m.popularity \
             FROM media_items m \
             JOIN media_items_fts f ON f.rowid = m.rowid \
             WHERE media_items_fts MATCH ? AND m.media_type IN ({}) \
             LIMIT ?",
            placeholders.join(",")
        );
        let mut q = sqlx::query_as::<_, super::repo_media::MediaItemRow>(sqlx::AssertSqlSafe(sql));
        q = q.bind(&fts_query);
        for t in &type_filters {
            q = q.bind(t);
        }
        q = q.bind(limit);
        q.fetch_all(pool).await
    };

    // If FTS5 succeeds, return results
    match rows {
        Ok(rows) => Ok(rows.into_iter().map(Into::into).collect()),
        Err(_) => {
            // Fallback to LIKE search with prefix matching for index utilization
            // Using `query%` instead of `%query%` enables B-tree index scan
            let like_prefix = format!("{}%", query);

            if type_filters.is_empty() {
                let rows = sqlx::query_as::<_, super::repo_media::MediaItemRow>(
                    "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
                     production_year, community_rating, official_rating, genres, date_created, \
                     premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
                     tags, status, original_language, tagline, production_countries, budget, revenue, \
                     homepage, end_date, vote_count, popularity \
                     FROM media_items WHERE name LIKE ?1 LIMIT ?2",
                )
                .bind(&like_prefix)
                .bind(limit)
                .fetch_all(pool)
                .await?;
                Ok(rows.into_iter().map(Into::into).collect())
            } else {
                let placeholders: Vec<&str> = type_filters.iter().map(|_| "?").collect();
                let sql = format!(
                    "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
                     production_year, community_rating, official_rating, genres, date_created, \
                     premiere_date, runtime_ticks, index_number, parent_index_number, tmdb_id, tvdb_id, \
                     tags, status, original_language, tagline, production_countries, budget, revenue, \
                     homepage, end_date, vote_count, popularity \
                     FROM media_items WHERE name LIKE ? AND media_type IN ({}) LIMIT ?",
                    placeholders.join(",")
                );
                let mut q =
                    sqlx::query_as::<_, super::repo_media::MediaItemRow>(sqlx::AssertSqlSafe(sql));
                q = q.bind(&like_prefix);
                for t in &type_filters {
                    q = q.bind(t);
                }
                q = q.bind(limit);
                let rows = q.fetch_all(pool).await?;
                Ok(rows.into_iter().map(Into::into).collect())
            }
        }
    }
}
