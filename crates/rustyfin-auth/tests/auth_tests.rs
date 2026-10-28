use chrono::Utc;
use uuid::Uuid;

use rustyfin_model::user::User;

async fn test_pool() -> sqlx::SqlitePool {
    let dir = std::env::temp_dir().join(format!("rustyfin_test_auth_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok();
    let db_path = dir.join("test.db");
    rustyfin_db::init_db(db_path.to_str().unwrap())
        .await
        .unwrap()
}

fn make_user() -> User {
    User {
        id: Uuid::new_v4(),
        name: "testuser".to_string(),
        password_hash: String::new(),
        is_administrator: true,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::json!({}),
    }
}

// ── Password Hashing Tests ──

#[test]
fn test_hash_password_produces_valid_hash() {
    let hash = rustyfin_auth::hash_password("mysecret").unwrap();
    assert!(hash.starts_with("$argon2"));
    assert!(hash.len() > 20);
}

#[test]
fn test_hash_password_different_each_time() {
    let h1 = rustyfin_auth::hash_password("same").unwrap();
    let h2 = rustyfin_auth::hash_password("same").unwrap();
    // Different salts → different hashes
    assert_ne!(h1, h2);
}

#[test]
fn test_verify_password_correct() {
    let hash = rustyfin_auth::hash_password("correct").unwrap();
    assert!(rustyfin_auth::verify_password("correct", &hash).unwrap());
}

#[test]
fn test_verify_password_wrong() {
    let hash = rustyfin_auth::hash_password("correct").unwrap();
    assert!(!rustyfin_auth::verify_password("wrong", &hash).unwrap());
}

#[test]
fn test_verify_password_empty() {
    let hash = rustyfin_auth::hash_password("notempty").unwrap();
    assert!(!rustyfin_auth::verify_password("", &hash).unwrap());
}

// ── JWT Token Tests ──

#[test]
fn test_create_and_validate_token() {
    let user = make_user();
    let secret = "test-secret-key";

    let token = rustyfin_auth::create_token(&user, secret).unwrap();
    assert!(!token.is_empty());

    let claims = rustyfin_auth::validate_token(&token, secret).unwrap();
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.name, "testuser");
    assert!(claims.is_admin);
    assert!(claims.exp > claims.iat);
}

#[test]
fn test_validate_token_wrong_secret() {
    let user = make_user();
    let token = rustyfin_auth::create_token(&user, "secret1").unwrap();

    let result = rustyfin_auth::validate_token(&token, "secret2");
    assert!(result.is_err());
}

#[test]
fn test_validate_token_garbage() {
    let result = rustyfin_auth::validate_token("not.a.token", "secret");
    assert!(result.is_err());
}

#[test]
fn test_token_contains_correct_expiry() {
    let user = make_user();
    let token = rustyfin_auth::create_token(&user, "s").unwrap();
    let claims = rustyfin_auth::validate_token(&token, "s").unwrap();

    // Token should expire ~30 days from now
    let now = Utc::now().timestamp();
    let diff = claims.exp - now;
    assert!(diff > 86400 * 29 && diff <= 86400 * 30);
}

// ── Authenticate (integration with DB) ──

#[tokio::test]
async fn test_authenticate_success() {
    let pool = test_pool().await;
    let secret = "jwt-secret";

    // Create user via the auth module
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&pool, &username, "password123")
        .await
        .unwrap();

    let (user, token) = rustyfin_auth::authenticate(&pool, &username, "password123", secret)
        .await
        .unwrap();

    assert_eq!(user.name, username);
    assert!(user.is_administrator);
    assert!(!token.is_empty());

    // Token should be valid
    let claims = rustyfin_auth::validate_token(&token, secret).unwrap();
    assert_eq!(claims.sub, user.id);
}

#[tokio::test]
async fn test_authenticate_wrong_password() {
    let pool = test_pool().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&pool, &username, "correct")
        .await
        .unwrap();

    let result = rustyfin_auth::authenticate(&pool, &username, "wrong", "secret").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_authenticate_nonexistent_user() {
    let pool = test_pool().await;

    let result = rustyfin_auth::authenticate(&pool, "ghost", "pass", "secret").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_initial_user() {
    let pool = test_pool().await;
    let username = format!("newadmin_{}", uuid::Uuid::new_v4().simple());

    let user = rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();
    assert_eq!(user.name, username);
    assert!(user.is_administrator);

    // Should be retrievable from DB
    let found = rustyfin_db::repo_user::get_user_by_name(&pool, &username)
        .await
        .unwrap();
    assert!(found.is_some());
}
