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

/// Returns true if a server owner exists.
pub async fn owner_exists(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE is_owner = 1)")
        .fetch_one(pool)
        .await?;

    Ok(exists)
}

/// Returns true if the server has at least one user.
pub async fn users_exist(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(pool)
        .await?;

    Ok(exists)
}

/// Returns the server owner, or None if no owner exists yet.
pub async fn get_owner(pool: &SqlitePool) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE is_owner = 1")
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

/// Returns true if the user with this id is the server owner. An unknown id is not an owner.
pub async fn is_owner(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let owner: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = ? AND is_owner = 1)")
            .bind(id.to_string())
            .fetch_one(pool)
            .await?;

    Ok(owner)
}

/// Returns true if the user with this id is disabled. An unknown id is not disabled.
pub async fn is_disabled(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let disabled: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = ? AND is_disabled = 1)")
            .bind(id.to_string())
            .fetch_one(pool)
            .await?;

    Ok(disabled)
}

/// Makes the earliest-created enabled administrator the server owner, if no owner exists.
/// Returns the promoted user, or None if an owner already exists or no enabled admin does.
/// The check and the update run in one IMMEDIATE transaction, so two callers cannot both promote.
pub async fn promote_earliest_admin_to_owner(
    pool: &SqlitePool,
) -> Result<Option<User>, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let has_owner: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE is_owner = 1)")
            .fetch_one(&mut *tx)
            .await?;
    if has_owner {
        return Ok(None);
    }

    let candidate = sqlx::query_as::<_, UserRow>(
        "SELECT * FROM users WHERE is_administrator = 1 AND is_disabled = 0 ORDER BY datetime(created_at), rowid LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = candidate else {
        return Ok(None);
    };

    sqlx::query("UPDATE users SET is_owner = 1 WHERE id = ?")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Some(row.into()))
}

/// Outcome of `claim_owner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerClaim {
    /// The named user was created as administrator and owner.
    Claimed,
    /// The server already has users, or a concurrent claim won. Nothing changed.
    Refused,
}

/// Creates `name` as an administrator and the owner, but only while the server has no users. It never
/// changes an existing user. The check and the insert run in one IMMEDIATE transaction, so only one
/// caller can claim ownership.
pub async fn claim_owner(
    pool: &SqlitePool,
    name: &str,
    password_hash: &str,
) -> Result<OwnerClaim, sqlx::Error> {
    match claim_owner_in_tx(pool, name, password_hash).await {
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Ok(OwnerClaim::Refused),
        result => result,
    }
}

async fn claim_owner_in_tx(
    pool: &SqlitePool,
    name: &str,
    password_hash: &str,
) -> Result<OwnerClaim, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let has_users: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(&mut *tx)
        .await?;
    if has_users {
        return Ok(OwnerClaim::Refused);
    }

    sqlx::query(
        "INSERT INTO users (id, name, password_hash, is_administrator, created_at, configuration, is_owner) VALUES (?, ?, ?, 1, ?, ?, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(name)
    .bind(password_hash)
    .bind(Utc::now().to_rfc3339())
    .bind("{}")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(OwnerClaim::Claimed)
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

/// Sets the user's Easy PIN hash, or clears it with `None`. Returns false when the user does not exist.
pub async fn set_easy_password_hash(
    pool: &SqlitePool,
    id: Uuid,
    hash: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE users SET easy_password_hash = ? WHERE id = ?")
        .bind(hash)
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

/// Wrong PINs a reset allows before it is void. Every query that checks the cap binds this.
pub const MAX_RESET_PIN_ATTEMPTS: i64 = 5;

/// Store the hash of a freshly generated reset PIN for `user_id`. Expired resets of every user are
/// deleted first, then a row is inserted only if the user has none (pending or voided), so a new
/// request never replaces an open reset. `pin_hash` is the SHA-256 hex of the normalized PIN;
/// `expires_at` is RFC3339. Returns `true` when a row was inserted.
pub async fn store_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    pin_hash: &str,
    expires_at: &str,
) -> Result<bool, sqlx::Error> {
    let uid = user_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("DELETE FROM password_reset_pins WHERE expires_at <= ?1")
        .bind(&now)
        .execute(pool)
        .await?;
    let id = uuid::Uuid::new_v4().to_string();
    let inserted = sqlx::query(
        "INSERT INTO password_reset_pins (id, user_id, pin_code, expires_at, failed_attempts) \
         VALUES (?1, ?2, ?3, ?4, 0) ON CONFLICT(user_id) DO NOTHING",
    )
    .bind(&id)
    .bind(&uid)
    .bind(pin_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(inserted.rows_affected() == 1)
}

/// Validate a reset PIN for one named user (the `/Users/ResetPassword` flow). `pin_hash` is the
/// SHA-256 hex of the normalized PIN. A PIN is valid only while unexpired and under
/// `MAX_RESET_PIN_ATTEMPTS` failed attempts. On success the PIN row is deleted and the user id is
/// returned. On failure this user's failed-attempt counter goes up and `None` is returned. The
/// check, the delete and the counter update run in one IMMEDIATE transaction, so concurrent
/// requests cannot both succeed.
pub async fn validate_password_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    pin_hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let uid = user_id.to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let deleted: Option<(String,)> = sqlx::query_as(
        "DELETE FROM password_reset_pins WHERE id = (\
         SELECT id FROM password_reset_pins \
         WHERE user_id = ?1 AND pin_code = ?2 AND expires_at > ?3 AND failed_attempts < ?4 LIMIT 1) \
         RETURNING user_id",
    )
    .bind(&uid)
    .bind(pin_hash)
    .bind(&now)
    .bind(MAX_RESET_PIN_ATTEMPTS)
    .fetch_optional(&mut *tx)
    .await?;
    if deleted.is_none() {
        sqlx::query(
            "UPDATE password_reset_pins SET failed_attempts = failed_attempts + 1 \
             WHERE user_id = ?1 AND expires_at > ?2 AND failed_attempts < ?3",
        )
        .bind(&uid)
        .bind(&now)
        .bind(MAX_RESET_PIN_ATTEMPTS)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(deleted.and_then(|(db_user_id,)| db_user_id.parse::<Uuid>().ok()))
}

/// Redeem a reset PIN with no username (the `/Users/ForgotPassword/Pin` flow). Searches every
/// active reset for a matching hash. On success deletes that reset and returns the user id. On no
/// match increments the failed-attempt counter of every active reset still under
/// `MAX_RESET_PIN_ATTEMPTS` and returns `None`. The delete and the counter update run in one
/// IMMEDIATE transaction, so concurrent requests cannot both redeem the same PIN.
pub async fn redeem_reset_pin(
    pool: &SqlitePool,
    pin_hash: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let deleted: Option<(String,)> = sqlx::query_as(
        "DELETE FROM password_reset_pins WHERE id = (\
         SELECT id FROM password_reset_pins \
         WHERE pin_code = ?1 AND expires_at > ?2 AND failed_attempts < ?3 LIMIT 1) \
         RETURNING user_id",
    )
    .bind(pin_hash)
    .bind(&now)
    .bind(MAX_RESET_PIN_ATTEMPTS)
    .fetch_optional(&mut *tx)
    .await?;
    if deleted.is_none() {
        sqlx::query(
            "UPDATE password_reset_pins SET failed_attempts = failed_attempts + 1 \
             WHERE expires_at > ?1 AND failed_attempts < ?2",
        )
        .bind(&now)
        .bind(MAX_RESET_PIN_ATTEMPTS)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(deleted.and_then(|(db_user_id,)| db_user_id.parse::<Uuid>().ok()))
}
