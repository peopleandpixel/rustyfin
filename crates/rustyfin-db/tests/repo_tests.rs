use chrono::Utc;
use sqlx::SqlitePool;
use std::path::PathBuf;
use uuid::Uuid;

use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::user::User;

/// Create a temporary file-based SQLite database for proper test isolation.
/// Returns (pool, temp_dir) — the temp dir is deleted when dropped.
struct TempDb {
    pool: SqlitePool,
    _dir: PathBuf,
}

impl TempDb {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.db");
        let db_url = db_path.to_str().unwrap().to_string();
        let pool = rustyfin_db::init_db(&db_url).await.unwrap();
        Self { pool, _dir: dir }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

/// Create an in-memory SQLite database with all migrations applied.
async fn test_pool() -> TempDb {
    TempDb::new().await
}

fn make_user(name: &str, is_admin: bool) -> User {
    User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: "$argon2id$v=19$m=19456,t=2,p=1$fake$fakehash".to_string(),
        is_administrator: is_admin,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::json!({}),
    }
}

fn make_library(name: &str, ct: LibraryContentType) -> Library {
    Library {
        id: Uuid::new_v4(),
        name: name.to_string(),
        content_type: ct,
        paths: vec!["/media/movies".to_string()],
    }
}

// ── User Repository Tests ──

#[tokio::test]
async fn test_create_and_get_user_by_name() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("alice", true);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_name(&pool, "alice")
        .await
        .unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "alice");
    assert!(found.is_administrator);
    assert_eq!(found.id, user.id);
}

#[tokio::test]
async fn test_get_user_by_id() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("bob", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "bob");
}

#[tokio::test]
async fn test_get_user_not_found() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let found = rustyfin_db::repo_user::get_user_by_name(&pool, "nonexistent")
        .await
        .unwrap();
    assert!(found.is_none());

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, Uuid::new_v4())
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_get_all_users() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    rustyfin_db::repo_user::create_user(&pool, &make_user("u1", false))
        .await
        .unwrap();
    rustyfin_db::repo_user::create_user(&pool, &make_user("u2", true))
        .await
        .unwrap();

    let users = rustyfin_db::repo_user::get_all_users(&pool).await.unwrap();
    assert_eq!(users.len(), 2);
}

#[tokio::test]
async fn test_delete_user() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("deleteme", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let deleted = rustyfin_db::repo_user::delete_user(&pool, user.id)
        .await
        .unwrap();
    assert!(deleted);

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap();
    assert!(found.is_none());

    // Deleting again returns false
    let deleted = rustyfin_db::repo_user::delete_user(&pool, user.id)
        .await
        .unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_update_password() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("pwuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let updated = rustyfin_db::repo_user::update_password(&pool, user.id, "newhash")
        .await
        .unwrap();
    assert!(updated);

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.password_hash, "newhash");
}

#[tokio::test]
async fn test_update_policy() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("policyuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();
    assert!(!user.is_administrator);

    rustyfin_db::repo_user::update_policy(&pool, user.id, true)
        .await
        .unwrap();
    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(found.is_administrator);
}

#[tokio::test]
async fn test_update_last_login() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("loginuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_user::update_last_login(&pool, user.id)
        .await
        .unwrap();
    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(found.last_login_date.is_some());
}

#[tokio::test]
async fn test_update_configuration() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("confuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let new_config = serde_json::json!({"theme": "dark"});
    rustyfin_db::repo_user::update_configuration(&pool, user.id, &new_config)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.configuration["theme"], "dark");
}

// ── Library Repository Tests ──

#[tokio::test]
async fn test_create_and_get_library() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let found = rustyfin_db::repo_library::get_library_by_id(&pool, lib.id)
        .await
        .unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "Movies");
    assert_eq!(found.content_type, LibraryContentType::Movies);
    assert_eq!(found.paths, vec!["/media/movies".to_string()]);
}

#[tokio::test]
async fn test_get_all_libraries() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    rustyfin_db::repo_library::create_library(
        &pool,
        &make_library("Movies", LibraryContentType::Movies),
    )
    .await
    .unwrap();
    rustyfin_db::repo_library::create_library(
        &pool,
        &make_library("TV", LibraryContentType::TvShows),
    )
    .await
    .unwrap();

    let libs = rustyfin_db::repo_library::get_all_libraries(&pool)
        .await
        .unwrap();
    assert_eq!(libs.len(), 2);
}

#[tokio::test]
async fn test_delete_library() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("ToDelete", LibraryContentType::Mixed);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    rustyfin_db::repo_library::delete_library(&pool, lib.id)
        .await
        .unwrap();
    let found = rustyfin_db::repo_library::get_library_by_id(&pool, lib.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

// ── Session Repository Tests ──

#[tokio::test]
async fn test_upsert_and_get_sessions() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("sessuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(
        &pool, "sess1", user.id, "dev1", "MyDevice", "TestApp", "1.0",
    )
    .await
    .unwrap();

    let sessions = rustyfin_db::repo_session::get_active_sessions(&pool)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].device_name, "MyDevice");
}

#[tokio::test]
async fn test_delete_session() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("sessuser2", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(
        &pool, "sess2", user.id, "dev2", "Device", "App", "1.0",
    )
    .await
    .unwrap();

    rustyfin_db::repo_session::delete_session(&pool, "sess2")
        .await
        .unwrap();
    let sessions = rustyfin_db::repo_session::get_active_sessions(&pool)
        .await
        .unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_cleanup_expired_sessions() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("expuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(&pool, "old_sess", user.id, "dev", "D", "A", "1")
        .await
        .unwrap();

    // Manually set last_activity to the past so cleanup finds it
    sqlx::query("UPDATE sessions SET last_activity = datetime('now', '-10 minutes')")
        .execute(&pool)
        .await
        .unwrap();

    let cleaned = rustyfin_db::repo_session::cleanup_expired_sessions(&pool, 5)
        .await
        .unwrap();
    assert!(cleaned >= 1);
}

// ── Device Repository Tests ──

#[tokio::test]
async fn test_upsert_and_get_devices() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("devuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_device(
        &pool,
        "device1",
        "Living Room TV",
        "Jellyfin",
        "10.8",
        user.id,
    )
    .await
    .unwrap();

    let devices = rustyfin_db::repo_session::get_all_devices(&pool)
        .await
        .unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Living Room TV");
}

#[tokio::test]
async fn test_delete_device() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("devuser2", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_device(&pool, "device2", "Phone", "App", "1.0", user.id)
        .await
        .unwrap();

    rustyfin_db::repo_session::delete_device(&pool, "device2")
        .await
        .unwrap();
    let devices = rustyfin_db::repo_session::get_all_devices(&pool)
        .await
        .unwrap();
    assert!(devices.is_empty());
}
