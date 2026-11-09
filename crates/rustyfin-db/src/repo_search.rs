use chrono::{DateTime, Utc};
use rustyfin_model::media::MediaItem;
use sqlx::SqlitePool;
use std::collections::HashMap;
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

/// A library item that a hints search matched. The hint filters, the score and the paging run on
/// these.
pub struct HintCandidate {
    pub id: Uuid,
    pub name: String,
    /// The `media_type` as stored. It may be a name that is not a `BaseItemKind`.
    pub kind: String,
    /// The `tags` column split on `|`, each entry trimmed, empty entries dropped.
    pub tags: Vec<String>,
    pub production_year: Option<i32>,
    pub runtime_ticks: Option<i64>,
    /// The `index_number`: an episode's or a season's number.
    pub index_number: Option<i32>,
    /// The `parent_index_number`: an episode's season number.
    pub parent_index_number: Option<i32>,
    /// The `end_date` when it parses as RFC 3339. A value that does not parse is `None`.
    pub end_date: Option<DateTime<Utc>>,
    /// The `status` as stored.
    pub status: Option<String>,
}

/// True only when FTS5 cannot parse the search term itself, such as `Star Trek: Discovery*`
/// (it reads `Trek` as a column) or `Mr. Robot*` (a syntax error). Any other error, such as a
/// missing index, is not a query error and must not fall back to matching names alone.
fn is_fts_query_error(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => {
            let message = db.message();
            message.starts_with("fts5: syntax error")
                || message.starts_with("no such column: ")
                || message.starts_with("unknown special query")
                || message.starts_with("expected integer, got ")
                || message.starts_with("fts5 expression tree is too large")
                || message == "fts5: parser stack overflow"
                || message == "unterminated string"
        }
        _ => false,
    }
}

/// Hint candidates by FTS5. `?1` is the FTS query and `?2` the parent id, or NULL for every
/// library. A parent holds the items of that library and the descendants of that item, not the
/// item itself. `UNION` (not `UNION ALL`) ends a walk that runs into a cycle of parents.
const HINT_FTS_SQL: &str =
    "SELECT m.id, m.name, m.media_type, m.production_year, m.runtime_ticks, m.tags,
            m.index_number, m.parent_index_number, m.end_date, m.status
     FROM media_items m
     JOIN media_items_fts f ON f.rowid = m.rowid
     WHERE media_items_fts MATCH ?1
       AND (?2 IS NULL OR m.library_id = ?2 OR (m.id <> ?2 AND m.id IN (
            WITH RECURSIVE below(id) AS (
                SELECT c.id FROM media_items c WHERE c.parent_id = ?2
                UNION
                SELECT c.id FROM media_items c JOIN below b ON c.parent_id = b.id
            )
            SELECT id FROM below)))";

/// The LIKE fallback of `HINT_FTS_SQL`. `?1` is the `query%` prefix pattern.
const HINT_LIKE_SQL: &str =
    "SELECT m.id, m.name, m.media_type, m.production_year, m.runtime_ticks, m.tags,
            m.index_number, m.parent_index_number, m.end_date, m.status
     FROM media_items m
     WHERE m.name LIKE ?1
       AND (?2 IS NULL OR m.library_id = ?2 OR (m.id <> ?2 AND m.id IN (
            WITH RECURSIVE below(id) AS (
                SELECT c.id FROM media_items c WHERE c.parent_id = ?2
                UNION
                SELECT c.id FROM media_items c JOIN below b ON c.parent_id = b.id
            )
            SELECT id FROM below)))";

type HintRow = (
    String,
    String,
    String,
    Option<i32>,
    Option<i64>,
    Option<String>,
    Option<i32>,
    Option<i32>,
    Option<String>,
    Option<String>,
);

/// Every library item that a hints search for `term` (trimmed) matches inside `parent`. The match
/// is FTS5 over the names, overviews and genres, with the name prefix as the fallback when FTS5
/// rejects the query. The result is neither ordered nor limited.
pub async fn hint_candidates(
    pool: &SqlitePool,
    term: &str,
    parent: Option<Uuid>,
) -> Result<Vec<HintCandidate>, sqlx::Error> {
    let scope = parent.map(|id| id.to_string());
    let fts_query = normalize_fts_query(term);
    let rows = sqlx::query_as::<_, HintRow>(HINT_FTS_SQL)
        .bind(&fts_query)
        .bind(scope.clone())
        .fetch_all(pool)
        .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(e) if is_fts_query_error(&e) => {
            // Using `query%` instead of `%query%` enables B-tree index scan on idx_media_items_name
            let like_prefix = format!("{term}%");
            sqlx::query_as::<_, HintRow>(HINT_LIKE_SQL)
                .bind(&like_prefix)
                .bind(scope)
                .fetch_all(pool)
                .await?
        }
        Err(e) => return Err(e),
    };

    Ok(rows
        .into_iter()
        .filter_map(
            |(
                id,
                name,
                kind,
                production_year,
                runtime_ticks,
                tags,
                index_number,
                parent_index_number,
                end_date,
                status,
            )| {
                Some(HintCandidate {
                    id: Uuid::parse_str(&id).ok()?,
                    name,
                    kind,
                    tags: split_tags(tags.as_deref()),
                    production_year,
                    runtime_ticks,
                    index_number,
                    parent_index_number,
                    end_date: end_date.and_then(|text| text.parse::<DateTime<Utc>>().ok()),
                    status,
                })
            },
        )
        .collect())
}

/// One ancestor of a library item, with its `media_type` as stored.
pub struct HintAncestor {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
}

/// The ancestors of the items whose ids are the JSON array `?1`. Each row is an item, the depth of
/// one of its ancestors (the parent is 1) and that ancestor. The walk stops at depth 32, which ends
/// a cycle of parents.
const HINT_ANCESTORS_SQL: &str = "WITH RECURSIVE up(item_id, depth, id) AS (
        SELECT m.id, 1, m.parent_id
        FROM json_each(?1) j JOIN media_items m ON m.id = j.value
        WHERE m.parent_id IS NOT NULL
        UNION ALL
        SELECT up.item_id, up.depth + 1, p.parent_id
        FROM up JOIN media_items p ON p.id = up.id
        WHERE p.parent_id IS NOT NULL AND up.depth < 32
    )
    SELECT up.item_id, up.depth, a.id, a.name, a.media_type
    FROM up JOIN media_items a ON a.id = up.id
    ORDER BY up.item_id, up.depth";

type HintAncestorRow = (String, i64, String, String, String);

/// The ancestors of each of `item_ids`, nearest first. An item without a parent has no entry, and
/// a repeated id is read once. The ids go to SQLite at most 500 per query.
pub async fn hint_ancestors(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<HintAncestor>>, sqlx::Error> {
    let mut ids = item_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut ancestors: HashMap<Uuid, Vec<HintAncestor>> = HashMap::new();
    for chunk in ids.chunks(500) {
        let texts: Vec<String> = chunk.iter().map(Uuid::to_string).collect();
        let json = serde_json::to_string(&texts).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        let rows = sqlx::query_as::<_, HintAncestorRow>(HINT_ANCESTORS_SQL)
            .bind(json)
            .fetch_all(pool)
            .await?;
        for (item_id, _depth, id, name, kind) in rows {
            let (Ok(item_id), Ok(id)) = (Uuid::parse_str(&item_id), Uuid::parse_str(&id)) else {
                continue;
            };
            ancestors
                .entry(item_id)
                .or_default()
                .push(HintAncestor { id, name, kind });
        }
    }
    Ok(ancestors)
}

/// The entries of a `tags` value: split on `|`, trimmed, with the empty ones dropped.
fn split_tags(tags: Option<&str>) -> Vec<String> {
    tags.unwrap_or_default()
        .split('|')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// Full search returning MediaItem results. `item_types` are the stored kind names to match, bound
/// as they are; an empty list matches every kind.
/// Uses FTS5 for full-text search across name, overview, and genres,
/// with LIKE prefix matching as fallback.
pub async fn search_items(
    pool: &SqlitePool,
    query: &str,
    item_types: &[&str],
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    // Try FTS5 first for full-text search
    let fts_query = normalize_fts_query(query);
    let rows = if item_types.is_empty() {
        // Search all types via FTS5
        sqlx::query_as::<_, super::repo_media::MediaItemRow>(
            "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
             m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
             m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
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
        let placeholders: Vec<&str> = item_types.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT m.id, m.name, m.sort_name, m.overview, m.media_type, m.path, m.parent_id, m.library_id, \
             m.production_year, m.community_rating, m.official_rating, m.genres, m.date_created, \
             m.premiere_date, m.runtime_ticks, m.index_number, m.parent_index_number, m.index_number_end, m.location_type, m.tmdb_id, m.tvdb_id, \
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
        for t in item_types {
            q = q.bind(t);
        }
        q = q.bind(limit);
        q.fetch_all(pool).await
    };

    // If FTS5 succeeds, return results
    match rows {
        Ok(rows) => Ok(rows.into_iter().map(Into::into).collect()),
        Err(e) if is_fts_query_error(&e) => {
            // Fallback to LIKE search with prefix matching for index utilization
            // Using `query%` instead of `%query%` enables B-tree index scan
            let like_prefix = format!("{}%", query);

            if item_types.is_empty() {
                let rows = sqlx::query_as::<_, super::repo_media::MediaItemRow>(
                    "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
                     production_year, community_rating, official_rating, genres, date_created, \
                     premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
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
                let placeholders: Vec<&str> = item_types.iter().map(|_| "?").collect();
                let sql = format!(
                    "SELECT id, name, sort_name, overview, media_type, path, parent_id, library_id, \
                     production_year, community_rating, official_rating, genres, date_created, \
                     premiere_date, runtime_ticks, index_number, parent_index_number, index_number_end, location_type, tmdb_id, tvdb_id, \
                     tags, status, original_language, tagline, production_countries, budget, revenue, \
                     homepage, end_date, vote_count, popularity \
                     FROM media_items WHERE name LIKE ? AND media_type IN ({}) LIMIT ?",
                    placeholders.join(",")
                );
                let mut q =
                    sqlx::query_as::<_, super::repo_media::MediaItemRow>(sqlx::AssertSqlSafe(sql));
                q = q.bind(&like_prefix);
                for t in item_types {
                    q = q.bind(t);
                }
                q = q.bind(limit);
                let rows = q.fetch_all(pool).await?;
                Ok(rows.into_iter().map(Into::into).collect())
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_fts_query_simple() {
        assert_eq!(normalize_fts_query("hello"), "hello*");
    }

    #[test]
    fn test_normalize_fts_query_escape_star() {
        assert_eq!(normalize_fts_query("hello*world"), "hello\\*world*");
    }

    #[test]
    fn test_normalize_fts_query_escape_quote() {
        assert_eq!(normalize_fts_query("say \"hi\""), "say \\\"hi\\\"*");
    }

    #[test]
    fn test_normalize_fts_query_escape_backslash() {
        assert_eq!(normalize_fts_query("path\\to\\file"), "path\\\\to\\\\file*");
    }

    #[test]
    fn test_normalize_fts_query_escape_question() {
        assert_eq!(normalize_fts_query("what?"), "what\\?*");
    }

    #[tokio::test]
    async fn test_search_hints_empty() {
        let pool = crate::test_pool().await;
        let hints = search_hints(&pool, "nonexistent", 10).await.unwrap();
        assert!(hints.is_empty());
    }

    #[tokio::test]
    async fn test_search_items_empty() {
        let pool = crate::test_pool().await;
        let items = search_items(&pool, "nonexistent", &[], 10).await.unwrap();
        assert!(items.is_empty());
    }
}
