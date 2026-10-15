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
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM activity_log")
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
        "SELECT id, name, type, user_id, item_id, severity, overview, date
         FROM activity_log
         ORDER BY date DESC
         LIMIT ?1 OFFSET ?2",
    )
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
    })
}
