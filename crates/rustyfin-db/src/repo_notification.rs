use rustyfin_model::notification::{NotificationDto, NotificationResult};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn get_notifications(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<NotificationResult, sqlx::Error> {
    let uid = user_id.to_string();
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM notifications WHERE user_id = ?1")
        .bind(&uid)
        .fetch_one(pool)
        .await?;

    let rows = sqlx::query_as::<_, (i64, String, String, Option<String>, String, i64, String)>(
        "SELECT id, user_id, name, description, level, is_read, date
         FROM notifications
         WHERE user_id = ?1
         ORDER BY date DESC
         LIMIT 50",
    )
    .bind(&uid)
    .fetch_all(pool)
    .await?;

    Ok(NotificationResult {
        notifications: rows
            .into_iter()
            .map(|r| NotificationDto {
                id: r.0,
                user_id: r.1,
                name: r.2,
                description: r.3,
                level: r.4,
                is_read: r.5 != 0,
                date: r.6,
            })
            .collect(),
        total_record_count: total.0,
    })
}
