use chrono::{DateTime, Utc};
use rustyfin_model::user::{User, UserPreferences};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn create_user(pool: &SqlitePool, user: &User) -> Result<(), sqlx::Error> {
    let id = user.id.to_string();
    let created = user.created_at.to_rfc3339();
    let is_admin = user.is_administrator as i32;
    let config = user.configuration.to_string();

    sqlx::query(
        "INSERT INTO users (id, name, password_hash, is_administrator, created_at, configuration) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&user.name)
    .bind(&user.password_hash)
    .bind(is_admin)
    .bind(&created)
    .bind(&config)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_user_by_name(pool: &SqlitePool, name: &str) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

pub async fn get_user_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

pub async fn get_all_users(pool: &SqlitePool) -> Result<Vec<User>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UserRow>("SELECT * FROM users")
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn update_last_login(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE users SET last_login_date = ? WHERE id = ?")
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_user(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_password(
    pool: &SqlitePool,
    id: Uuid,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_policy(
    pool: &SqlitePool,
    id: Uuid,
    is_administrator: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET is_administrator = ? WHERE id = ?")
        .bind(is_administrator as i32)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_configuration(
    pool: &SqlitePool,
    id: Uuid,
    configuration: &serde_json::Value,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET configuration = ? WHERE id = ?")
        .bind(configuration.to_string())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: String,
    name: String,
    password_hash: String,
    is_administrator: i32,
    created_at: String,
    last_login_date: Option<String>,
    configuration: String,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_default(),
            name: r.name,
            password_hash: r.password_hash,
            is_administrator: r.is_administrator != 0,
            created_at: r.created_at.parse::<DateTime<Utc>>().unwrap_or_default(),
            last_login_date: r
                .last_login_date
                .and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            configuration: serde_json::from_str(&r.configuration).unwrap_or_default(),
        }
    }
}

pub async fn update_user_name(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn update_display_preference(
    pool: &SqlitePool,
    user_id: Uuid,
    key: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    sqlx::query(
        "INSERT INTO display_preferences (user_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value",
    )
    .bind(&uid)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_display_preference(
    pool: &SqlitePool,
    user_id: Uuid,
    key: &str,
) -> Result<Option<String>, sqlx::Error> {
    let uid = user_id.to_string();
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM display_preferences WHERE user_id = ?1 AND key = ?2")
            .bind(&uid)
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(value)
}

/// Get user images
pub async fn get_user_images(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<(String, String, Option<i32>, Option<i32>)>, sqlx::Error> {
    let uid = user_id.to_string();
    let rows = sqlx::query_as::<_, (String, String, Option<i32>, Option<i32>)>(
        "SELECT id, image_type, width, height FROM user_images WHERE user_id = ?1",
    )
    .bind(&uid)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Save a user image
pub async fn save_user_image(
    pool: &SqlitePool,
    user_id: Uuid,
    image_type: &str,
    path: &str,
    width: Option<i32>,
    height: Option<i32>,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let uid = user_id.to_string();
    sqlx::query(
        "INSERT INTO user_images (id, user_id, image_type, path, width, height) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&id)
    .bind(&uid)
    .bind(image_type)
    .bind(path)
    .bind(width)
    .bind(height)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Delete a user image
pub async fn delete_user_image(
    pool: &SqlitePool,
    user_id: Uuid,
    image_type: &str,
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    sqlx::query("DELETE FROM user_images WHERE user_id = ?1 AND image_type = ?2")
        .bind(&uid)
        .bind(image_type)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get user access schedules (parental control periods)
pub async fn get_user_access_schedules(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<(String, String, String)>, sqlx::Error> {
    let uid = user_id.to_string();
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT day_of_week, start_time, end_time FROM user_access_schedules WHERE user_id = ?1",
    )
    .bind(&uid)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Save user access schedules
pub async fn save_user_access_schedules(
    pool: &SqlitePool,
    user_id: Uuid,
    schedules: &[(&str, &str, &str)],
) -> Result<(), sqlx::Error> {
    let uid = user_id.to_string();
    // Delete existing schedules
    sqlx::query("DELETE FROM user_access_schedules WHERE user_id = ?1")
        .bind(&uid)
        .execute(pool)
        .await?;
    // Insert new schedules
    for (day, start, end) in schedules {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO user_access_schedules (id, user_id, day_of_week, start_time, end_time) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&id)
        .bind(&uid)
        .bind(day)
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn get_user_preferences(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<UserPreferences, sqlx::Error> {
    let uid = id.to_string();

    // Try to read from display_preferences table
    let prefs: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM display_preferences WHERE user_id = ?")
            .bind(&uid)
            .fetch_all(pool)
            .await?;

    let prefs_map: std::collections::HashMap<String, String> = prefs.into_iter().collect();

    // Merge with user configuration
    let user = get_user_by_id(pool, id).await?;
    let config = user
        .as_ref()
        .map(|u| &u.configuration)
        .unwrap_or(&serde_json::Value::Null);

    Ok(UserPreferences {
        chart_missing_episodes: prefs_map
            .get("chartMissingEpisodes")
            .cloned()
            .unwrap_or_default(),
        chart_played_episodes: config
            .get("chartPlayedEpisodes")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        group_episodes_by_season: prefs_map
            .get("groupEpisodesBySeason")
            .and_then(|v| v.parse().ok())
            .unwrap_or(true),
        group_by_collection: prefs_map
            .get("groupByCollection")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        hide_audio_playlists_in_music: prefs_map
            .get("hideAudioPlaylistsInMusic")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        hide_culture_titles: prefs_map
            .get("hideCultureTitles")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        hide_episodes: prefs_map
            .get("hideEpisodes")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        hide_empty_series: prefs_map
            .get("hideEmptySeries")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        hide_seasons: prefs_map
            .get("hideSeasons")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        limit_field: prefs_map.get("limitField").cloned().unwrap_or_default(),
        metadata_country_code: prefs_map
            .get("metadataCountryCode")
            .cloned()
            .unwrap_or_else(|| "US".to_string()),
        preferred_metadata_language: prefs_map
            .get("preferredMetadataLanguage")
            .cloned()
            .unwrap_or_else(|| "en".to_string()),
    })
}

/// Generate a password reset PIN for a user and store it in the database.
/// Returns the 6-digit PIN code.
pub async fn generate_password_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<String, sqlx::Error> {
    let uid = user_id.to_string();
    // Delete any existing unused pins for this user
    sqlx::query("DELETE FROM password_reset_pins WHERE user_id = ?1 AND used = 0")
        .bind(&uid)
        .execute(pool)
        .await?;

    // Generate a random 6-digit PIN (using std::time for seed, no non-Send dependencies)
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pin: String = (0..6)
        .map(|i| {
            let val = ((seed.wrapping_mul(6364136223846793005) >> 16) + i as u64) % 10;
            (b'0' + val as u8) as char
        })
        .collect();

    let id = uuid::Uuid::new_v4().to_string();
    let expires_at = chrono::Utc::now()
        .checked_add_days(chrono::Days::new(1))
        .unwrap()
        .to_rfc3339();

    sqlx::query(
        "INSERT INTO password_reset_pins (id, user_id, pin_code, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&id)
    .bind(&uid)
    .bind(&pin)
    .bind(&expires_at)
    .execute(pool)
    .await?;

    Ok(pin)
}

/// Validate a password reset PIN and mark it as used.
/// Returns the user_id if valid, None otherwise.
pub async fn validate_password_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    pin_code: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let uid = user_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT user_id FROM password_reset_pins WHERE user_id = ?1 AND pin_code = ?2 AND used = 0 AND expires_at > ?3",
    )
    .bind(&uid)
    .bind(pin_code)
    .bind(&now)
    .fetch_optional(pool)
    .await?;

    if let Some((db_user_id,)) = row {
        // Mark the PIN as used
        sqlx::query(
            "UPDATE password_reset_pins SET used = 1 WHERE user_id = ?1 AND pin_code = ?2 AND used = 0",
        )
        .bind(&uid)
        .bind(pin_code)
        .execute(pool)
        .await?;

        // Parse the user ID back
        if let Ok(uid) = db_user_id.parse::<Uuid>() {
            return Ok(Some(uid));
        }
    }

    Ok(None)
}
