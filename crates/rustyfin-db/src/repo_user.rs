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
