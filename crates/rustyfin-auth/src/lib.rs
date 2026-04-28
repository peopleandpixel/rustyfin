use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::SaltString;
use rand_core::OsRng;
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
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
