use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand_core::OsRng;
use rustyfin_model::auth::Claims;
use rustyfin_model::user::User;
use sqlx::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user not found")]
    UserNotFound,
    #[error("token error: {0}")]
    TokenError(#[from] jsonwebtoken::errors::Error),
    #[error("database error: {0}")]
    DbError(#[from] sqlx::Error),
    #[error("password hash error")]
    HashError,
}

pub fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::HashError)?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash).map_err(|_| AuthError::HashError)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn create_token(user: &User, secret: &str) -> Result<String, AuthError> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user.id,
        name: user.name.clone(),
        is_admin: user.is_administrator,
        iat: now,
        exp: now + 86400 * 30, // 30 days
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

pub fn validate_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )?;

    Ok(data.claims)
}

pub async fn authenticate(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    secret: &str,
) -> Result<(User, String), AuthError> {
    let user = rustyfin_db::repo_user::get_user_by_name(pool, username)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    if !verify_password(password, &user.password_hash)? {
        return Err(AuthError::InvalidCredentials);
    }

    rustyfin_db::repo_user::update_last_login(pool, user.id).await?;

    let token = create_token(&user, secret)?;
    Ok((user, token))
}

pub async fn create_initial_user(
    pool: &SqlitePool,
    name: &str,
    password: &str,
) -> Result<User, AuthError> {
    let hash = hash_password(password)?;
    let user = User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: hash,
        is_administrator: true,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    };

    rustyfin_db::repo_user::create_user(pool, &user).await?;
    Ok(user)
}

/// Refresh a JWT token, returning a new token with extended expiry.
pub fn refresh_token(token: &str, secret: &str) -> Result<String, AuthError> {
    let claims = validate_token(token, secret)?;
    let now = Utc::now().timestamp();
    let new_claims = Claims {
        sub: claims.sub,
        name: claims.name,
        is_admin: claims.is_admin,
        iat: now,
        exp: now + 86400 * 30, // 30 days
    };

    let new_token = encode(
        &Header::default(),
        &new_claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(new_token)
}

/// Generate a random PIN for password reset.
pub fn generate_reset_pin() -> String {
    let mut pin = String::with_capacity(6);
    for _ in 0..6 {
        let digit = rand::random::<u8>() % 10;
        pin.push(char::from(digit as u8 + b'0'));
    }
    pin
}

/// Hash a PIN for storage.
pub fn hash_pin(pin: &str) -> Result<String, AuthError> {
    hash_password(pin)
}

/// Verify a PIN against its hash.
pub fn verify_pin(pin: &str, hash: &str) -> Result<bool, AuthError> {
    verify_password(pin, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hash_password / verify_password ──

    #[test]
    fn test_hash_password_returns_hash() {
        let hash = hash_password("secret123").unwrap();
        assert!(!hash.is_empty());
        assert!(hash.starts_with("$argon"));
    }

    #[test]
    fn test_verify_password_correct() {
        let hash = hash_password("mypassword").unwrap();
        assert!(verify_password("mypassword", &hash).unwrap());
    }

    #[test]
    fn test_verify_password_incorrect() {
        let hash = hash_password("mypassword").unwrap();
        assert!(!verify_password("wrongpassword", &hash).unwrap());
    }

    #[test]
    fn test_verify_password_empty_hash() {
        let result = verify_password("test", "");
        assert!(result.is_err());
    }

    // ── create_token / validate_token ──

    #[test]
    fn test_create_token() {
        let user = User {
            id: Uuid::new_v4(),
            name: "testuser".to_string(),
            password_hash: "hash".to_string(),
            is_administrator: true,
            created_at: Utc::now(),
            last_login_date: None,
            configuration: serde_json::Value::Object(Default::default()),
        };
        let token = create_token(&user, "secret").unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_validate_token_round_trip() {
        let user = User {
            id: Uuid::new_v4(),
            name: "testuser".to_string(),
            password_hash: "hash".to_string(),
            is_administrator: true,
            created_at: Utc::now(),
            last_login_date: None,
            configuration: serde_json::Value::Object(Default::default()),
        };
        let token = create_token(&user, "secret").unwrap();
        let claims = validate_token(&token, "secret").unwrap();
        assert_eq!(claims.name, "testuser");
        assert!(claims.is_admin);
    }

    #[test]
    fn test_validate_token_wrong_secret() {
        let user = User {
            id: Uuid::new_v4(),
            name: "testuser".to_string(),
            password_hash: "hash".to_string(),
            is_administrator: false,
            created_at: Utc::now(),
            last_login_date: None,
            configuration: serde_json::Value::Object(Default::default()),
        };
        let token = create_token(&user, "secret").unwrap();
        let result = validate_token(&token, "wrong_secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_token_invalid() {
        let result = validate_token("not.a.token", "secret");
        assert!(result.is_err());
    }

    // ── refresh_token ──

    #[test]
    fn test_refresh_token() {
        let user = User {
            id: Uuid::new_v4(),
            name: "testuser".to_string(),
            password_hash: "hash".to_string(),
            is_administrator: true,
            created_at: Utc::now(),
            last_login_date: None,
            configuration: serde_json::Value::Object(Default::default()),
        };
        let token = create_token(&user, "secret").unwrap();
        let old_claims = validate_token(&token, "secret").unwrap();
        let new_token = refresh_token(&token, "secret").unwrap();
        let new_claims = validate_token(&new_token, "secret").unwrap();
        // New token should have equal or later expiry (same second is possible)
        assert!(new_claims.exp >= old_claims.exp);
        assert_eq!(new_claims.name, "testuser");
        // The issued-at time should be the same or newer
        assert!(new_claims.iat >= old_claims.iat);
    }

    // ── generate_reset_pin ──

    #[test]
    fn test_generate_reset_pin_length() {
        let pin = generate_reset_pin();
        assert_eq!(pin.len(), 6);
    }

    #[test]
    fn test_generate_reset_pin_is_numeric() {
        let pin = generate_reset_pin();
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_generate_reset_pin_randomness() {
        // Generate multiple PINs and verify they're not all identical
        let pins: Vec<String> = (0..10).map(|_| generate_reset_pin()).collect();
        let unique = pins.iter().collect::<std::collections::HashSet<_>>();
        assert!(unique.len() > 1, "PINs should be random");
    }

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
