use sqlx::SqlitePool;
use uuid::Uuid;

// Tokens are keyed by their hash. The hashing happens in rustyfin-auth; this layer only stores it.

/// Stores a new token row, only while the user exists and is enabled, so a sign-in that races a
/// disable gets no token. `token_hash` is the hex SHA-256 of the bearer token. Returns whether the
/// row was stored.
pub async fn insert_token(
    pool: &SqlitePool,
    token_hash: &str,
    user_id: Uuid,
    device_id: &str,
    device_name: &str,
    app_name: &str,
    app_version: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO access_tokens
            (token_hash, user_id, device_id, device_name, app_name, app_version)
         SELECT ?, ?, ?, ?, ?, ? WHERE EXISTS (SELECT 1 FROM users WHERE id = ? AND is_disabled = 0)",
    )
    .bind(token_hash)
    .bind(user_id.to_string())
    .bind(device_id)
    .bind(device_name)
    .bind(app_name)
    .bind(app_version)
    .bind(user_id.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Looks up a token by hash, returning its user and device id.
/// A row whose user_id does not parse as a UUID is treated as absent.
pub async fn get_token(
    pool: &SqlitePool,
    token_hash: &str,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT user_id, device_id FROM access_tokens WHERE token_hash = ?",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(user_id, device_id)| {
        Uuid::parse_str(&user_id).ok().map(|uid| (uid, device_id))
    }))
}

pub async fn delete_token(pool: &SqlitePool, token_hash: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM access_tokens WHERE token_hash = ?")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes a user's tokens. When `keep_hash` is given, the token with that hash survives.
/// Returns the number of tokens removed.
pub async fn delete_user_tokens(
    pool: &SqlitePool,
    user_id: Uuid,
    keep_hash: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let uid = user_id.to_string();
    let done = match keep_hash {
        Some(keep) => {
            sqlx::query("DELETE FROM access_tokens WHERE user_id = ? AND token_hash != ?")
                .bind(uid)
                .bind(keep)
                .execute(pool)
                .await?
        }
        None => {
            sqlx::query("DELETE FROM access_tokens WHERE user_id = ?")
                .bind(uid)
                .execute(pool)
                .await?
        }
    };
    Ok(done.rows_affected())
}

/// Deletes a user's tokens issued to one device, so a new login on that device replaces them.
pub async fn delete_user_device_tokens(
    pool: &SqlitePool,
    user_id: Uuid,
    device_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM access_tokens WHERE user_id = ? AND device_id = ?")
        .bind(user_id.to_string())
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes every token issued to `device_id`, for every user, so removing a device signs it out.
/// An empty device id matches nothing.
pub async fn delete_device_tokens(pool: &SqlitePool, device_id: &str) -> Result<u64, sqlx::Error> {
    let done = sqlx::query("DELETE FROM access_tokens WHERE device_id = ?1 AND device_id != ''")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}
