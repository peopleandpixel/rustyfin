use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use rand_core::{OsRng, RngCore};
use rustyfin_db::{repo_session, repo_token, repo_user};
use rustyfin_model::auth::Claims;
use rustyfin_model::user::{User, name_key, name_too_long};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user not found")]
    UserNotFound,
    #[error("invalid token")]
    InvalidToken,
    #[error("database error: {0}")]
    DbError(#[from] sqlx::Error),
    #[error("password hash error")]
    HashError,
}

// ── Password and PIN hashing ──

fn argon2_hash(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::HashError)?;
    Ok(hash.to_string())
}

fn argon2_verify(password: &str, hash: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash).map_err(|_| AuthError::HashError)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Hashes a password. Argon2 is slow, so it runs on the blocking pool (M-15).
pub async fn hash_password(password: &str) -> Result<String, AuthError> {
    let p = password.to_owned();
    tokio::task::spawn_blocking(move || argon2_hash(&p))
        .await
        .map_err(|_| AuthError::HashError)?
}

/// Verifies a password against its hash. Runs on the blocking pool (M-15).
pub async fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
    let p = password.to_owned();
    let h = hash.to_owned();
    tokio::task::spawn_blocking(move || argon2_verify(&p, &h))
        .await
        .map_err(|_| AuthError::HashError)?
}

/// Hash a PIN for storage.
pub fn hash_pin(pin: &str) -> Result<String, AuthError> {
    argon2_hash(pin)
}

/// Verify a PIN against its hash.
pub fn verify_pin(pin: &str, hash: &str) -> Result<bool, AuthError> {
    argon2_verify(pin, hash)
}

// ── Password-reset PINs (R1-01, M-17) ──

/// A reset PIN: 4 OsRng bytes as dash-grouped uppercase hex, e.g. `A1-B2-C3-D4`.
fn generate_reset_pin() -> String {
    let mut b = [0u8; 4];
    OsRng.fill_bytes(&mut b);
    format!("{:02X}-{:02X}-{:02X}-{:02X}", b[0], b[1], b[2], b[3])
}

/// Normalize a reset PIN for comparison and for use as the new password: trimmed and uppercased.
pub fn normalize_reset_pin(pin: &str) -> String {
    pin.trim().to_uppercase()
}

/// SHA-256 hex of the normalized PIN. Only this is stored, so a database leak exposes no live PIN.
fn reset_pin_hash(pin: &str) -> String {
    Sha256::digest(normalize_reset_pin(pin).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// File name that holds the plaintext reset PIN for `username`,
/// `passwordreset-<hex sha256(t)>.json` (AC65). `t` is `key:` and the name key for a name within
/// the limit, so names that match share one file. A longer name gives `name:` and its exact
/// spelling, so it matches only itself. The name is hashed so the file name leaks no username.
/// ForgotPassword names the file from the name it was asked with; the redeem removes the file
/// named from the stored name.
pub fn reset_pin_filename(username: &str) -> String {
    let t = if name_too_long(username) {
        format!("name:{username}")
    } else {
        format!("key:{}", name_key(username))
    };
    pin_file_named(&t)
}

/// The file name older servers used: `passwordreset-<hex sha256(UPPERCASE name)>.json`. Nothing
/// writes it now; the redeem also removes it, so a reset asked before the upgrade leaves no file.
pub fn legacy_reset_pin_filename(username: &str) -> String {
    pin_file_named(&username.to_uppercase())
}

/// `passwordreset-<hex sha256(t)>.json`, the PIN file name for the text `t`.
fn pin_file_named(t: &str) -> String {
    let h: String = Sha256::digest(t.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("passwordreset-{h}.json")
}

/// Generate a reset PIN for `user_id`, store only its hash (with the given RFC3339 `expires_at`).
/// Returns `Some(pin)`, the plaintext PIN once, so the caller can write it to the on-disk reset
/// file, when the user had no pending or voided reset. Returns `None` when one exists: that reset
/// stays in force and the generated PIN is discarded. NEVER log the return value.
pub async fn create_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    expires_at: &str,
) -> Result<Option<String>, AuthError> {
    let pin = generate_reset_pin();
    let stored =
        repo_user::store_reset_pin(pool, user_id, &reset_pin_hash(&pin), expires_at).await?;
    Ok(stored.then_some(pin))
}

/// Validate a reset PIN for one named user (`/Users/ResetPassword`). Returns the user id on success.
pub async fn validate_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    pin: &str,
) -> Result<Option<Uuid>, AuthError> {
    Ok(repo_user::validate_password_reset_pin(pool, user_id, &reset_pin_hash(pin)).await?)
}

/// Redeem a reset PIN given only the PIN (`/Users/ForgotPassword/Pin`). Returns the user id on success.
pub async fn redeem_reset_pin(pool: &SqlitePool, pin: &str) -> Result<Option<Uuid>, AuthError> {
    Ok(repo_user::redeem_reset_pin(pool, &reset_pin_hash(pin)).await?)
}

// ── Opaque, server-side access tokens ──

/// A bearer token: 16 random bytes as 32 lowercase hex characters.
fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex SHA-256 of a token. Only this hash is stored, so a database leak exposes no live token.
fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Device details recorded with a token. A non-empty `device_id` lets a new login on the same
/// device replace the old token, and lets logout remove that device's sessions.
#[derive(Debug, Clone, Default)]
pub struct TokenDevice {
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
}

/// The bearer token of the current request, inserted as a request extension so that handlers
/// can revoke it or keep it.
#[derive(Debug, Clone)]
pub struct RequestToken(pub String);

/// Issues a token for `user` and returns the plaintext once. Only its hash is stored.
/// A disabled or deleted user gets no token, with the answer of a wrong password.
pub async fn create_token(
    pool: &SqlitePool,
    user: &User,
    device: &TokenDevice,
) -> Result<String, AuthError> {
    let token = generate_token();
    let stored = repo_token::insert_token(
        pool,
        &token_hash(&token),
        user.id,
        &device.device_id,
        &device.device_name,
        &device.app_name,
        &device.app_version,
    )
    .await?;
    if !stored {
        return Err(AuthError::InvalidCredentials);
    }
    Ok(token)
}

/// Resolves a bearer token to its claims. Unknown tokens, and tokens of disabled or deleted
/// users, are rejected. Tokens do not expire, so `iat` is not tracked.
pub async fn validate_token(pool: &SqlitePool, token: &str) -> Result<Claims, AuthError> {
    let (uid, _) = repo_token::get_token(pool, &token_hash(token))
        .await?
        .ok_or(AuthError::InvalidToken)?;
    let user = repo_user::get_user_by_id(pool, uid)
        .await?
        .ok_or(AuthError::InvalidToken)?;
    if repo_user::is_disabled(pool, uid).await? {
        return Err(AuthError::InvalidToken);
    }
    Ok(Claims {
        sub: user.id,
        name: user.name,
        is_admin: user.is_administrator,
        iat: 0,
        exp: i64::MAX,
    })
}

/// Revokes a single token.
pub async fn revoke_token(pool: &SqlitePool, token: &str) -> Result<(), AuthError> {
    repo_token::delete_token(pool, &token_hash(token)).await?;
    Ok(())
}

/// Revokes a user's tokens, except `keep` when given. Returns how many were revoked.
pub async fn revoke_user_tokens(
    pool: &SqlitePool,
    user_id: Uuid,
    keep: Option<&str>,
) -> Result<u64, AuthError> {
    let keep_hash = keep.map(token_hash);
    Ok(repo_token::delete_user_tokens(pool, user_id, keep_hash.as_deref()).await?)
}

/// Logs out a token: revokes it and removes the sessions of its device.
pub async fn logout(pool: &SqlitePool, token: &str) -> Result<(), AuthError> {
    let hash = token_hash(token);
    if let Some((uid, device_id)) = repo_token::get_token(pool, &hash).await? {
        repo_token::delete_token(pool, &hash).await?;
        if !device_id.is_empty() {
            repo_session::delete_sessions_by_user_device(pool, uid, &device_id).await?;
        }
    }
    Ok(())
}

/// Checks a login and, if it succeeds, issues a token for `device`. The name is found exactly, else
/// by the one name that matches it caselessly (`repo_user::find_user_for_sign_in`). The returned
/// user is re-read after the login, so its LastLoginDate is the new one (M-16).
pub async fn authenticate(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    let user = repo_user::find_user_for_sign_in(pool, username)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    if !verify_password(password, &user.password_hash).await? {
        return Err(AuthError::InvalidCredentials);
    }

    // A disabled account gets the same answer as a wrong password, and its last login is not updated.
    if repo_user::is_disabled(pool, user.id).await? {
        return Err(AuthError::InvalidCredentials);
    }

    repo_user::update_last_login(pool, user.id).await?;
    let user = repo_user::get_user_by_id(pool, user.id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    // A new login on a device replaces that device's previous token.
    if !device.device_id.is_empty() {
        repo_token::delete_user_device_tokens(pool, user.id, &device.device_id).await?;
    }

    let token = create_token(pool, &user, device).await?;
    Ok((user, token))
}

/// Names of enabled administrators whose password is still the default "admin".
pub async fn admins_with_default_password(pool: &SqlitePool) -> Result<Vec<String>, AuthError> {
    let mut names = Vec::new();
    for u in repo_user::get_all_users(pool).await? {
        if u.is_administrator
            && !repo_user::is_disabled(pool, u.id).await?
            && verify_password("admin", &u.password_hash)
                .await
                .unwrap_or(false)
        {
            names.push(u.name);
        }
    }
    Ok(names)
}

pub async fn create_initial_user(
    pool: &SqlitePool,
    name: &str,
    password: &str,
) -> Result<User, AuthError> {
    let hash = hash_password(password).await?;
    let user = User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: hash,
        is_administrator: true,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    };

    repo_user::create_user(pool, &user).await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hash_password / verify_password ──

    #[tokio::test]
    async fn test_hash_password_returns_hash() {
        let hash = hash_password("secret123").await.unwrap();
        assert!(!hash.is_empty());
        assert!(hash.starts_with("$argon"));
    }

    #[tokio::test]
    async fn test_verify_password_correct() {
        let hash = hash_password("mypassword").await.unwrap();
        assert!(verify_password("mypassword", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_password_incorrect() {
        let hash = hash_password("mypassword").await.unwrap();
        assert!(!verify_password("wrongpassword", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_password_empty_hash() {
        let result = verify_password("test", "").await;
        assert!(result.is_err());
    }

    // Token lifecycle (opaque DB-backed), authenticate, and admin-default-password
    // checks live in tests/auth_tests.rs now that they need a live pool.

    // ── hash_pin / verify_pin ──

    #[test]
    fn test_hash_and_verify_pin() {
        let pin = "123456";
        let hash = hash_pin(pin).unwrap();
        assert!(verify_pin(pin, &hash).unwrap());
    }

    #[test]
    fn test_verify_wrong_pin() {
        let pin = "123456";
        let hash = hash_pin(pin).unwrap();
        assert!(!verify_pin("654321", &hash).unwrap());
    }
}
