use chrono::{NaiveDateTime, TimeZone, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use rustyfin_model::session::{DeviceInfo, SessionInfo};

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .unwrap_or_else(|_| Utc::now())
}

// ── Sessions ──

pub async fn upsert_session(
    pool: &SqlitePool,
    session_id: &str,
    user_id: Uuid,
    device_id: &str,
    device_name: &str,
    app_name: &str,
    app_version: &str,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, device_id, device_name, app_name, app_version, last_activity)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           last_activity = datetime('now'),
           device_name = excluded.device_name,
           app_name = excluded.app_name,
           app_version = excluded.app_version",
    )
    .bind(session_id)
    .bind(&uid)
    .bind(device_id)
    .bind(device_name)
    .bind(app_name)
    .bind(app_version)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_session_now_playing(
    pool: &SqlitePool,
    session_id: &str,
    item_id: Option<Uuid>,
    position_ticks: Option<i64>,
    is_paused: bool,
) -> Result<(), sqlx::Error> {
    let iid = item_id.map(|i| i.to_string());
    sqlx::query(
        "UPDATE sessions SET now_playing_item_id = ?1, now_playing_position_ticks = ?2, is_paused = ?3, last_activity = datetime('now') WHERE id = ?4",
    )
    .bind(&iid)
    .bind(position_ticks)
    .bind(is_paused)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_active_sessions(pool: &SqlitePool) -> Result<Vec<SessionInfo>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>, Option<i64>, i64, String)>(
        "SELECT s.id, s.user_id, u.name, s.device_id, s.device_name, s.app_name,
                s.now_playing_item_id, s.now_playing_position_ticks, s.is_paused, s.last_activity
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         ORDER BY s.last_activity DESC",
    )
    .fetch_all(pool)
    .await?;

    let mut sessions = Vec::with_capacity(rows.len());
    for r in rows {
        let now_playing = if let Some(ref item_id_str) = r.6 {
            if let Ok(iid) = Uuid::parse_str(item_id_str) {
                // Try to get item name
                let name: String = sqlx::query_scalar("SELECT name FROM media_items WHERE id = ?1")
                    .bind(item_id_str)
                    .fetch_optional(pool)
                    .await?
                    .unwrap_or_else(|| "Unknown".to_string());
                Some(rustyfin_model::session::NowPlayingItem {
                    id: iid,
                    name,
                    position_ticks: r.7,
                    is_paused: r.8 != 0,
                })
            } else {
                None
            }
        } else {
            None
        };

        sessions.push(SessionInfo {
            id: r.0,
            user_id: Uuid::parse_str(&r.1).unwrap_or_default(),
            user_name: r.2,
            device_id: r.3,
            device_name: r.4,
            app_name: r.5,
            app_version: String::new(),
            now_playing_item: now_playing,
            last_activity_date: parse_dt(&r.9),
        });
    }
    Ok(sessions)
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_sessions_by_device(pool: &SqlitePool, device_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE device_id = ?1")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_expired_sessions(pool: &SqlitePool, timeout_minutes: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM sessions WHERE last_activity < datetime('now', ?1)",
    )
    .bind(format!("-{timeout_minutes} minutes"))
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

// ── Devices ──

pub async fn upsert_device(
    pool: &SqlitePool,
    device_id: &str,
    device_name: &str,
    app_name: &str,
    app_version: &str,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    sqlx::query(
        "INSERT INTO devices (id, name, app_name, app_version, user_id, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           app_name = excluded.app_name,
           app_version = excluded.app_version,
           user_id = excluded.user_id,
           last_seen = datetime('now')",
    )
    .bind(device_id)
    .bind(device_name)
    .bind(app_name)
    .bind(app_version)
    .bind(&uid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_all_devices(pool: &SqlitePool) -> Result<Vec<DeviceInfo>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
        "SELECT d.id, d.name, d.app_name, d.app_version, d.user_id, u.name, d.last_seen
         FROM devices d
         JOIN users u ON u.id = d.user_id
         ORDER BY d.last_seen DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DeviceInfo {
            id: r.0,
            name: r.1,
            app_name: r.2,
            app_version: r.3,
            last_user_id: Uuid::parse_str(&r.4).unwrap_or_default(),
            last_user_name: r.5,
            date_last_activity: parse_dt(&r.6),
        })
        .collect())
}

pub async fn delete_device(pool: &SqlitePool, device_id: &str) -> Result<(), sqlx::Error> {
    // Also remove sessions for this device
    delete_sessions_by_device(pool, device_id).await?;
    sqlx::query("DELETE FROM devices WHERE id = ?1")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}
