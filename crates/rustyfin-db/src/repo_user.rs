use chrono::{DateTime, Utc};
use rustyfin_model::user::{NameMatch, User, UserFlags, UserPreferences, name_too_long};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Writes a user row. The executor is a pool, or a transaction the caller holds.
pub async fn create_user<'e, E>(executor: E, user: &User) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
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
    .execute(executor)
    .await?;

    Ok(())
}

/// Writes a new user unless a user's name matches the new one (`NameMatch`, AC56, AC62). The check
/// and the insert run in one IMMEDIATE transaction, so two callers cannot both take a name.
/// Returns false when the name is taken.
pub async fn create_user_unless_taken(pool: &SqlitePool, user: &User) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM users")
        .fetch_all(&mut *tx)
        .await?;
    let matcher = NameMatch::new(&user.name);
    if names.iter().any(|name| matcher.matches(name)) {
        tx.rollback().await?;
        return Ok(false);
    }
    create_user(&mut *tx, user).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn get_user_by_name(pool: &SqlitePool, name: &str) -> Result<Option<User>, sqlx::Error> {
    let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

/// The account a sign-in or password reset names (AC57, AC61): the user whose name is exactly
/// `name`, else the one user whose name matches it (`NameMatch`). A name over the limit matches
/// only exactly, so it reads no other users. Several matches with no exact one give None.
pub async fn find_user_for_sign_in(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<User>, sqlx::Error> {
    if let Some(user) = get_user_by_name(pool, name).await? {
        return Ok(Some(user));
    }
    if name_too_long(name) {
        return Ok(None);
    }
    let matcher = NameMatch::new(name);
    let mut matches: Vec<User> = get_all_users(pool)
        .await?
        .into_iter()
        .filter(|user| matcher.matches(&user.name))
        .collect();
    if matches.len() == 1 {
        return Ok(matches.pop());
    }
    Ok(None)
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

/// Users a sign-in screen may list: the enabled ones, as Jellyfin's GetPublicUsers.
pub async fn get_public_users(pool: &SqlitePool) -> Result<Vec<User>, sqlx::Error> {
    let rows = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE is_disabled = 0")
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

/// Reads the owner and disabled flags for one user. An unknown id gives the default flags.
pub async fn user_flags(pool: &SqlitePool, id: Uuid) -> Result<UserFlags, sqlx::Error> {
    let row: Option<(bool, bool)> =
        sqlx::query_as("SELECT is_owner, is_disabled FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?;

    Ok(row
        .map(|(is_owner, is_disabled)| UserFlags {
            is_owner,
            is_disabled,
        })
        .unwrap_or_default())
}

/// Reads the owner and disabled flags for every user, keyed by id. Rows whose id doesn't parse are skipped.
pub async fn all_user_flags(
    pool: &SqlitePool,
) -> Result<std::collections::HashMap<Uuid, UserFlags>, sqlx::Error> {
    let rows: Vec<(String, bool, bool)> =
        sqlx::query_as("SELECT id, is_owner, is_disabled FROM users")
            .fetch_all(pool)
            .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id, is_owner, is_disabled)| {
            let id = Uuid::parse_str(&id).ok()?;
            Some((
                id,
                UserFlags {
                    is_owner,
                    is_disabled,
                },
            ))
        })
        .collect())
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

/// Sets a user's password hash and returns whether the user exists. A password write also ends the
/// user's one-time setup code, in the same transaction (B20 3.6).
pub async fn update_password(
    pool: &SqlitePool,
    id: Uuid,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    let id = id.to_string();
    let mut tx = pool.begin().await?;
    let result = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let updated = result.rows_affected() > 0;
    if updated {
        sqlx::query("DELETE FROM user_setup_codes WHERE user_id = ?")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(updated)
}

/// Sets the administrator flag by hand.
/// Unguarded: no owner, self, disabled or last-administrator check. Only tests call it; the Policy
/// endpoint uses `set_policy`.
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

/// Outcome of `set_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyChange {
    /// The policy was changed.
    Done,
    /// No user has that id. Nothing changed.
    NotFound,
    /// The change would disable an account that is, or would become, an administrator. Nothing changed.
    AdminDisabled,
    /// The change would make a disabled account an administrator. Nothing changed.
    PromoteDisabled,
    /// The change would leave no enabled administrator. Nothing changed.
    LastAdmin,
}

/// Sets the administrator and disabled flags. `None` leaves a flag as it is. The checks and the update
/// run in one IMMEDIATE transaction, so two callers cannot both demote the last enabled administrator.
/// Disabling deletes the user's tokens, sessions and devices in the same transaction, as Jellyfin's
/// RevokeUserTokens logs each device out.
pub async fn set_policy(
    pool: &SqlitePool,
    id: Uuid,
    is_administrator: Option<bool>,
    is_disabled: Option<bool>,
) -> Result<PolicyChange, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let current: Option<(bool, bool)> =
        sqlx::query_as("SELECT is_administrator, is_disabled FROM users WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    let Some((cur_admin, cur_disabled)) = current else {
        tx.rollback().await?;
        return Ok(PolicyChange::NotFound);
    };

    let new_admin = is_administrator.unwrap_or(cur_admin);
    let new_disabled = is_disabled.unwrap_or(cur_disabled);

    // As in Jellyfin, which checks the current administrator flag, so demote and disable in one
    // request is refused.
    if new_disabled && !cur_disabled && (cur_admin || new_admin) {
        tx.rollback().await?;
        return Ok(PolicyChange::AdminDisabled);
    }
    if new_admin && !cur_admin && new_disabled {
        tx.rollback().await?;
        return Ok(PolicyChange::PromoteDisabled);
    }

    if cur_admin && !cur_disabled && !(new_admin && !new_disabled) {
        let others: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE is_administrator = 1 AND is_disabled = 0 AND id <> ?)",
        )
        .bind(id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        if !others {
            tx.rollback().await?;
            return Ok(PolicyChange::LastAdmin);
        }
    }

    sqlx::query("UPDATE users SET is_administrator = ?1, is_disabled = ?2 WHERE id = ?3")
        .bind(new_admin as i32)
        .bind(new_disabled as i32)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;

    if is_disabled == Some(true) {
        sqlx::query("DELETE FROM access_tokens WHERE user_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM devices WHERE user_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(PolicyChange::Done)
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

/// Outcome of `rename_user_unless_taken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renamed {
    /// The user has the new name.
    Done,
    /// Another user's name matches the new name. Nothing changed.
    Taken,
    /// No user has this id. Nothing changed.
    NotFound,
}

/// Gives a user a new name unless another user's name matches it (`NameMatch`, AC56, AC62). The
/// user's own name never counts, so a user may change its case. The check and the update run in one
/// IMMEDIATE transaction.
pub async fn rename_user_unless_taken(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
) -> Result<Renamed, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, name FROM users")
        .fetch_all(&mut *tx)
        .await?;
    let own_id = id.to_string();
    if !rows.iter().any(|(row_id, _)| *row_id == own_id) {
        tx.rollback().await?;
        return Ok(Renamed::NotFound);
    }
    let matcher = NameMatch::new(name);
    if rows
        .iter()
        .any(|(row_id, row_name)| *row_id != own_id && matcher.matches(row_name))
    {
        tx.rollback().await?;
        return Ok(Renamed::Taken);
    }
    sqlx::query("UPDATE users SET name = ? WHERE id = ?")
        .bind(name)
        .bind(&own_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Renamed::Done)
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
