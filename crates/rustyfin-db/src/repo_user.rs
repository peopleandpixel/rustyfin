use chrono::{DateTime, Utc};
use rustyfin_model::user::User;
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
            last_login_date: r.last_login_date.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            configuration: serde_json::from_str(&r.configuration).unwrap_or_default(),
        }
    }
}
