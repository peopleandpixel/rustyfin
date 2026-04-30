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
