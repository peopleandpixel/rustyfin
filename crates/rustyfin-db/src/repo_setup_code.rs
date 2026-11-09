//! One-time setup codes of users the Jellyfin import creates (B20 spec 2.22, 2.24, 3.1, 3.6, 3.7).
//! Only an argon2 hash of a code is stored.

use chrono::{DateTime, Utc};
use rustyfin_model::user::User;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Inserts `user` (the columns `repo_user::create_user` writes) and its code row in one
/// transaction; the code row's `created_at` is the user's, as RFC 3339.
pub async fn create_user_with_code(
    pool: &SqlitePool,
    user: &User,
    code_hash: &str,
) -> Result<(), sqlx::Error> {
    let id = user.id.to_string();
    let created = user.created_at.to_rfc3339();
    let is_admin = user.is_administrator as i32;
    let config = user.configuration.to_string();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO users (id, name, password_hash, is_administrator, created_at, configuration) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&user.name)
    .bind(&user.password_hash)
    .bind(is_admin)
    .bind(&created)
    .bind(&config)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_setup_codes (user_id, code_hash, created_at) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(code_hash)
        .bind(&created)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// A user with a setup code: its id and name, and when the current code was made.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingUser {
    pub user_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// The stored hash of `user`'s code, or `None` when the user has no code row.
pub async fn code_hash(pool: &SqlitePool, user: Uuid) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT code_hash FROM user_setup_codes WHERE user_id = ?")
        .bind(user.to_string())
        .fetch_optional(pool)
        .await
}

/// Every user with a setup code, sorted by `lower(name)`, then name, then id. A row whose id or
/// date does not parse is skipped.
pub async fn pending_users(pool: &SqlitePool) -> Result<Vec<PendingUser>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT c.user_id, u.name, c.created_at FROM user_setup_codes c \
         JOIN users u ON u.id = c.user_id \
         ORDER BY lower(u.name), u.name, c.user_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, name, created)| {
            Some(PendingUser {
                user_id: id.parse().ok()?,
                name,
                created_at: DateTime::parse_from_rfc3339(&created)
                    .ok()?
                    .with_timezone(&Utc),
            })
        })
        .collect())
}

/// Replaces `user`'s code hash and its date. `true` when the user had a code row to replace.
pub async fn replace_code(
    pool: &SqlitePool,
    user: Uuid,
    code_hash: &str,
    created_at: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let result =
        sqlx::query("UPDATE user_setup_codes SET code_hash = ?, created_at = ? WHERE user_id = ?")
            .bind(code_hash)
            .bind(created_at.to_rfc3339())
            .bind(user.to_string())
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Uses a code in one transaction: deletes `user`'s row when it still holds `code_hash`, then sets
/// the password hash. `false`, with nothing written, when the row holds another hash or none (a
/// reissue or another redeem came first).
pub async fn redeem(
    pool: &SqlitePool,
    user: Uuid,
    code_hash: &str,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    let id = user.to_string();
    // The write lock is taken up front, as `repo_user::redeem_reset_pin` does, so two redeems of one
    // code queue instead of racing.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let deleted = sqlx::query("DELETE FROM user_setup_codes WHERE user_id = ? AND code_hash = ?")
        .bind(&id)
        .bind(code_hash)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        // Dropping `tx` rolls the transaction back.
        return Ok(false);
    }
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
