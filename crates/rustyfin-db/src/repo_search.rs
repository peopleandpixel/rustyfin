use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::search::SearchHint;
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn search_hints(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchHint>, sqlx::Error> {
    // Try FTS first, fall back to LIKE
    let fts_query = format!("{}*", query.replace('"', ""));
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
            // Fallback to LIKE search
            let like = format!("%{}%", query);
            sqlx::query_as::<_, (String, String, String, Option<i32>, Option<i64>)>(
                "SELECT id, name, media_type, production_year, runtime_ticks
                 FROM media_items
                 WHERE name LIKE ?1 OR overview LIKE ?1
                 LIMIT ?2",
            )
            .bind(&like)
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
pub async fn search_items(
    pool: &SqlitePool,
    query: &str,
    item_types: &[MediaType],
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    // Build media type filter
    let type_filters: Vec<String> = item_types
        .iter()
        .map(|t| serde_json::to_string(t).unwrap_or_default())
        .collect();

    if type_filters.is_empty() {
        // Fallback: search all types
        let like = format!("%{}%", query);
        let rows = sqlx::query_as::<_, super::repo_media::MediaItemRow>(
            "SELECT * FROM media_items WHERE name LIKE ?1 OR overview LIKE ?1 LIMIT ?2",
        )
        .bind(&like)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        return Ok(rows.into_iter().map(Into::into).collect());
    }

    let placeholders: Vec<&str> = type_filters.iter().map(|_| "?").collect();
    let mut sql = format!(
        "SELECT * FROM media_items WHERE (name LIKE ? OR overview LIKE ?) AND media_type IN ({})",
        placeholders.join(",")
    );
    sql.push_str(" LIMIT ?");

    let mut q = sqlx::query_as::<_, super::repo_media::MediaItemRow>(&sql);
    let like = format!("%{}%", query);
    q = q.bind(&like).bind(&like);
    for t in &type_filters {
        q = q.bind(t);
    }
    q = q.bind(limit);

    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}
