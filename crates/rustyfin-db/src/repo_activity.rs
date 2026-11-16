use rustyfin_model::activity::{ActivityLogEntry, ActivityLogResult};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn log_activity(
    pool: &SqlitePool,
    name: &str,
    activity_type: &str,
    user_id: Option<Uuid>,
    item_id: Option<Uuid>,
    severity: &str,
    overview: Option<&str>,
) -> Result<(), sqlx::Error> {
    let uid = user_id.map(|u| u.to_string());
    let iid = item_id.map(|i| i.to_string());
    sqlx::query(
        "INSERT INTO activity_log (name, type, user_id, item_id, severity, overview) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(name)
    .bind(activity_type)
    .bind(&uid)
    .bind(&iid)
    .bind(severity)
    .bind(overview)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_activity_log(
    pool: &SqlitePool,
    start_index: i64,
    limit: i64,
) -> Result<ActivityLogResult, sqlx::Error> {
    query_activity_log(pool, start_index, limit, None, None).await
}

/// One page of the activity log, newest first, as Jellyfin's `GET /System/ActivityLog/Entries`:
/// only entries at or after `min_date` (an ISO 8601 date) when given, and with `has_user_id`,
/// only the entries that name a user (`true`) or that name none (`false`). Dates come back as
/// ISO 8601 UTC (`2026-11-15T08:30:00.000Z`).
pub async fn query_activity_log(
    pool: &SqlitePool,
    start_index: i64,
    limit: i64,
    min_date: Option<&str>,
    has_user_id: Option<bool>,
) -> Result<ActivityLogResult, sqlx::Error> {
    // Both queries filter alike: ?1 the earliest date, ?2 whether the entry names a user.
    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM activity_log
         WHERE (?1 IS NULL OR julianday(date) >= julianday(?1))
           AND (?2 IS NULL OR (?2 = (user_id IS NOT NULL AND user_id != '')))",
    )
    .bind(min_date)
    .bind(has_user_id)
    .fetch_one(pool)
    .await?;

    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
        ),
    >(
        "SELECT id, name, type, user_id, item_id, severity, overview,
                strftime('%Y-%m-%dT%H:%M:%fZ', date)
         FROM activity_log
         WHERE (?1 IS NULL OR julianday(date) >= julianday(?1))
           AND (?2 IS NULL OR (?2 = (user_id IS NOT NULL AND user_id != '')))
         ORDER BY date DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(min_date)
    .bind(has_user_id)
    .bind(limit)
    .bind(start_index)
    .fetch_all(pool)
    .await?;

    Ok(ActivityLogResult {
        items: rows
            .into_iter()
            .map(|r| ActivityLogEntry {
                id: r.0,
                name: r.1,
                entry_type: r.2,
                user_id: r.3,
                item_id: r.4,
                severity: r.5,
                overview: r.6,
                date: r.7,
            })
            .collect(),
        total_record_count: total.0,
        start_index,
    })
}

/// Deletes the activity log entries older than `days` days; returns how many went.
pub async fn delete_activity_older_than(pool: &SqlitePool, days: u32) -> Result<u64, sqlx::Error> {
    let done =
        sqlx::query("DELETE FROM activity_log WHERE julianday(date) < julianday('now') - ?1")
            .bind(i64::from(days))
            .execute(pool)
            .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_pool;

    #[tokio::test]
    async fn test_activity_log_empty() {
        let pool = test_pool().await;
        let result = get_activity_log(&pool, 0, 10).await.unwrap();
        assert!(result.items.is_empty());
        assert_eq!(result.total_record_count, 0);
    }

    #[tokio::test]
    async fn test_log_and_get_activity() {
        let pool = test_pool().await;
        log_activity(&pool, "User logged in", "Login", None, None, "Info", None)
            .await
            .unwrap();
        let result = get_activity_log(&pool, 0, 10).await.unwrap();
        assert_eq!(result.total_record_count, 1);
        assert_eq!(result.items[0].name, "User logged in");
    }

    #[tokio::test]
    async fn test_activity_log_pagination() {
        let pool = test_pool().await;
        for i in 0..5 {
            log_activity(
                &pool,
                &format!("Entry {}", i),
                "Test",
                None,
                None,
                "Info",
                None,
            )
            .await
            .unwrap();
        }
        let result = get_activity_log(&pool, 0, 2).await.unwrap();
        assert_eq!(result.total_record_count, 5);
        assert_eq!(result.items.len(), 2);
    }
}
