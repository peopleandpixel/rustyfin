use chrono::{NaiveDateTime, TimeZone, Utc};
use sqlx::SqlitePool;

use crate::writer::ExecuteWrite;
use uuid::Uuid;

use rustyfin_model::media::MediaItem;
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
    .execute_write(pool)
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
    .execute_write(pool)
    .await?;
    Ok(())
}

pub async fn get_active_sessions(pool: &SqlitePool) -> Result<Vec<SessionInfo>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            i64,
            String,
            String,
            String,
        ),
    >(
        "SELECT s.id, s.user_id, u.name, s.device_id, s.device_name, s.app_name,
                s.now_playing_item_id, s.now_playing_position_ticks, s.is_paused, s.last_activity,
                s.app_version, s.capabilities
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
            app_version: r.10,
            now_playing_item: now_playing,
            last_activity_date: parse_dt(&r.9),
            capabilities: serde_json::from_str(&r.11).unwrap_or_default(),
        });
    }
    Ok(sessions)
}

/// The session of `user_id` on `device_id`, if it has one. A session belongs to a user and a
/// device together, so another user signing in with the same device id gets a session of their own.
pub async fn find_device_session(
    pool: &SqlitePool,
    user_id: Uuid,
    device_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM sessions WHERE user_id = ?1 AND device_id = ?2
         ORDER BY last_activity DESC LIMIT 1",
    )
    .bind(user_id.to_string())
    .bind(device_id)
    .fetch_optional(pool)
    .await
}

/// The ids of the sessions `user_id` has, on `device_id` when one is given, so that what the
/// server keeps about them in memory can be dropped with them.
pub async fn session_ids(
    pool: &SqlitePool,
    user_id: Option<Uuid>,
    device_id: Option<&str>,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM sessions WHERE (?1 IS NULL OR user_id = ?1) AND (?2 IS NULL OR device_id = ?2)",
    )
    .bind(user_id.map(|u| u.to_string()))
    .bind(device_id)
    .fetch_all(pool)
    .await
}

/// The sessions that the token with hash `token_hash` made for other devices than its own, the
/// least recently active first.
pub async fn token_sessions(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM sessions WHERE via_token = ?1 ORDER BY last_activity ASC, created_at ASC, id ASC",
    )
    .bind(token_hash)
    .fetch_all(pool)
    .await
}

/// Records that the token with hash `token_hash` made the session: the session ends with it.
pub async fn set_session_token(
    pool: &SqlitePool,
    session_id: &str,
    token_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET via_token = ?2 WHERE id = ?1")
        .bind(session_id)
        .bind(token_hash)
        .execute_write(pool)
        .await?;
    Ok(())
}

/// The device id of each session `user_id` has, one entry per session.
pub async fn session_device_ids(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(device_id, '') FROM sessions WHERE user_id = ?1")
        .bind(user_id.to_string())
        .fetch_all(pool)
        .await
}

/// Marks the session active now.
pub async fn touch_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET last_activity = datetime('now') WHERE id = ?1")
        .bind(session_id)
        .execute_write(pool)
        .await?;
    Ok(())
}

/// Removes the sessions idle for more than `timeout_minutes`, except `keep` (those with an open
/// socket). Returns the ids removed.
pub async fn expire_sessions(
    pool: &SqlitePool,
    timeout_minutes: i64,
    keep: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    let idle: Vec<String> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE last_activity < datetime('now', ?1)")
            .bind(format!("-{timeout_minutes} minutes"))
            .fetch_all(pool)
            .await?;
    // Each delete checks the idle time again, so a session that became active meanwhile stays.
    let mut removed = Vec::new();
    for id in idle.into_iter().filter(|id| !keep.contains(id)) {
        let gone = sqlx::query(
            "DELETE FROM sessions WHERE id = ?1 AND last_activity < datetime('now', ?2)",
        )
        .bind(&id)
        .bind(format!("-{timeout_minutes} minutes"))
        .execute_write(pool)
        .await?
        .rows_affected();
        if gone > 0 {
            removed.push(id);
        }
    }
    Ok(removed)
}

pub async fn delete_session(pool: &SqlitePool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(session_id)
        .execute_write(pool)
        .await?;
    Ok(())
}

pub async fn delete_sessions_by_device(
    pool: &SqlitePool,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE device_id = ?1")
        .bind(device_id)
        .execute_write(pool)
        .await?;
    Ok(())
}

pub async fn delete_sessions_by_user_device(
    pool: &SqlitePool,
    user_id: Uuid,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ? AND device_id = ?")
        .bind(user_id.to_string())
        .bind(device_id)
        .execute_write(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_expired_sessions(
    pool: &SqlitePool,
    timeout_minutes: i64,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM sessions WHERE last_activity < datetime('now', ?1)")
        .bind(format!("-{timeout_minutes} minutes"))
        .execute_write(pool)
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
    .execute_write(pool)
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
    // Remove the device's sessions and the tokens issued to it, so the device is signed out
    delete_sessions_by_device(pool, device_id).await?;
    super::repo_token::delete_device_tokens(pool, device_id).await?;
    sqlx::query("DELETE FROM devices WHERE id = ?1")
        .bind(device_id)
        .execute_write(pool)
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    pool: &SqlitePool,
    session_id: Uuid,
    user_id: Uuid,
    _user_name: &str,
    client: &str,
    device_name: &str,
    device_id: &str,
    app_version: &str,
    capabilities: &str,
) -> Result<(), sqlx::Error> {
    let sid = session_id.to_string();
    let uid = user_id.to_string();
    // One session per user and device: a sign-in replaces that user's older sessions on the device
    if !device_id.is_empty() {
        sqlx::query("DELETE FROM sessions WHERE user_id = ?1 AND device_id = ?2 AND id != ?3")
            .bind(&uid)
            .bind(device_id)
            .bind(&sid)
            .execute_write(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO sessions (id, user_id, device_id, device_name, app_name, app_version, last_activity, capabilities)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), ?7)
         ON CONFLICT(id) DO UPDATE SET
           last_activity = datetime('now'),
           device_name = excluded.device_name,
           app_name = excluded.app_name,
           app_version = excluded.app_version,
           capabilities = excluded.capabilities",
    )
    .bind(&sid)
    .bind(&uid)
    .bind(device_id)
    .bind(device_name)
    .bind(client)
    .bind(app_version)
    .bind(capabilities)
    .execute_write(pool)
    .await?;

    // Also upsert device
    upsert_device(pool, device_id, device_name, client, app_version, user_id).await?;

    Ok(())
}

/// Update the capabilities for a session.
pub async fn update_session_capabilities(
    pool: &SqlitePool,
    session_id: &str,
    capabilities: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE sessions SET capabilities = ?1, last_activity = datetime('now') WHERE id = ?2",
    )
    .bind(capabilities)
    .bind(session_id)
    .execute_write(pool)
    .await?;
    Ok(())
}

/// Update the capabilities of every session that `user_id` has on `device_id`.
///
/// Sessions on the user's other devices are left alone. Returns the number of
/// sessions changed, which is zero when the device has no session.
pub async fn update_device_capabilities(
    pool: &SqlitePool,
    user_id: Uuid,
    device_id: &str,
    capabilities: &str,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE sessions SET capabilities = ?1, last_activity = datetime('now') WHERE user_id = ?2 AND device_id = ?3",
    )
    .bind(capabilities)
    .bind(user_id.to_string())
    .bind(device_id)
    .execute_write(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Get the capabilities JSON for a session.
pub async fn get_session_capabilities(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT capabilities FROM sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

/// The capabilities JSON of the most recently active session of `user_id` on `device_id`, skipping
/// sessions whose capabilities are empty or `{}`.
pub async fn get_device_capabilities(
    pool: &SqlitePool,
    user_id: Uuid,
    device_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT capabilities FROM sessions \
         WHERE user_id = ?1 AND device_id = ?2 AND capabilities NOT IN ('', '{}') \
         ORDER BY last_activity DESC, rowid DESC LIMIT 1",
    )
    .bind(user_id.to_string())
    .bind(device_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn get_now_playing_item(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let item_id: Option<String> =
        sqlx::query_scalar("SELECT now_playing_item_id FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;

    match item_id {
        Some(id_str) => {
            if let Ok(id) = Uuid::parse_str(&id_str) {
                super::repo_media::get_item_by_id(pool, id).await
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

/// The user a session belongs to, or `None` when no session has this id.
pub async fn get_session_user_id(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = ?1")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    Ok(user_id.and_then(|id| Uuid::parse_str(&id).ok()))
}

/// The user a device belongs to (the last one who signed in on it), or `None` when no device
/// has this id.
pub async fn get_device_user_id(
    pool: &SqlitePool,
    device_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let user_id: Option<String> = sqlx::query_scalar("SELECT user_id FROM devices WHERE id = ?1")
        .bind(device_id)
        .fetch_optional(pool)
        .await?;
    Ok(user_id.and_then(|id| Uuid::parse_str(&id).ok()))
}
