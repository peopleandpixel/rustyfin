async fn test_pool() -> sqlx::SqlitePool {
    let dir = std::env::temp_dir().join(format!("rustyfin_test_auth_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).ok();
    let db_path = dir.join("test.db");
    rustyfin_db::init_db(db_path.to_str().unwrap())
        .await
        .unwrap()
}

// ── Password Hashing Tests (now async: argon2 runs on spawn_blocking, M-15) ──

#[tokio::test]
async fn test_hash_password_produces_valid_hash() {
    let hash = rustyfin_auth::hash_password("mysecret").await.unwrap();
    assert!(hash.starts_with("$argon"));
}

#[tokio::test]
async fn test_hash_password_different_each_time() {
    let h1 = rustyfin_auth::hash_password("same").await.unwrap();
    let h2 = rustyfin_auth::hash_password("same").await.unwrap();
    assert_ne!(h1, h2, "a fresh salt each time");
}

#[tokio::test]
async fn test_verify_password_correct() {
    let hash = rustyfin_auth::hash_password("correct").await.unwrap();
    assert!(
        rustyfin_auth::verify_password("correct", &hash)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_verify_password_wrong() {
    let hash = rustyfin_auth::hash_password("correct").await.unwrap();
    assert!(
        !rustyfin_auth::verify_password("wrong", &hash)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn test_verify_password_empty() {
    let hash = rustyfin_auth::hash_password("notempty").await.unwrap();
    assert!(!rustyfin_auth::verify_password("", &hash).await.unwrap());
}

// ── Opaque DB-backed tokens (R1-08, R1-09) ──

#[tokio::test]
async fn test_create_and_validate_token() {
    let pool = test_pool().await;
    let username = format!("u_{}", uuid::Uuid::new_v4().simple());
    let user = rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();

    let token = rustyfin_auth::create_token(&pool, &user, &Default::default())
        .await
        .unwrap();
    assert!(!token.is_empty());

    let claims = rustyfin_auth::validate_token(&pool, &token).await.unwrap();
    assert_eq!(claims.sub, user.id);
    assert_eq!(claims.name, username);
    assert!(claims.is_admin);
}

#[tokio::test]
async fn test_validate_unknown_token_fails() {
    let pool = test_pool().await;
    assert!(
        rustyfin_auth::validate_token(&pool, "deadbeefdeadbeef")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn test_revoke_token_invalidates_it() {
    let pool = test_pool().await;
    let username = format!("u_{}", uuid::Uuid::new_v4().simple());
    let user = rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();
    let token = rustyfin_auth::create_token(&pool, &user, &Default::default())
        .await
        .unwrap();
    assert!(rustyfin_auth::validate_token(&pool, &token).await.is_ok());

    rustyfin_auth::revoke_token(&pool, &token).await.unwrap();
    assert!(rustyfin_auth::validate_token(&pool, &token).await.is_err());
}

#[tokio::test]
async fn test_revoke_user_tokens_keeps_one() {
    let pool = test_pool().await;
    let username = format!("u_{}", uuid::Uuid::new_v4().simple());
    let user = rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();
    let keep = rustyfin_auth::create_token(&pool, &user, &Default::default())
        .await
        .unwrap();
    let other = rustyfin_auth::create_token(&pool, &user, &Default::default())
        .await
        .unwrap();

    let removed = rustyfin_auth::revoke_user_tokens(&pool, user.id, Some(&keep))
        .await
        .unwrap();
    assert_eq!(removed, 1, "only the other token is revoked");
    assert!(rustyfin_auth::validate_token(&pool, &keep).await.is_ok());
    assert!(rustyfin_auth::validate_token(&pool, &other).await.is_err());
}

#[tokio::test]
async fn test_disabled_user_token_is_rejected() {
    let pool = test_pool().await;
    let username = format!("u_{}", uuid::Uuid::new_v4().simple());
    let user = rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();
    let token = rustyfin_auth::create_token(&pool, &user, &Default::default())
        .await
        .unwrap();

    sqlx::query("UPDATE users SET is_disabled = 1 WHERE id = ?")
        .bind(user.id.to_string())
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        rustyfin_auth::validate_token(&pool, &token).await.is_err(),
        "a disabled user's token no longer validates"
    );
}

// ── admins_with_default_password (admin/admin startup warning) ──

#[tokio::test]
async fn test_admins_with_default_password() {
    let pool = test_pool().await;
    rustyfin_auth::create_initial_user(&pool, "admin", "admin")
        .await
        .unwrap();
    rustyfin_auth::create_initial_user(&pool, "karl", "geheim")
        .await
        .unwrap();

    let names = rustyfin_auth::admins_with_default_password(&pool)
        .await
        .unwrap();
    assert!(names.contains(&"admin".to_string()));
    assert!(!names.contains(&"karl".to_string()));
}

// ── authenticate (integration with DB) ──

#[tokio::test]
async fn test_authenticate_success() {
    let pool = test_pool().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&pool, &username, "password123")
        .await
        .unwrap();

    let (user, token) =
        rustyfin_auth::authenticate(&pool, &username, "password123", &Default::default())
            .await
            .unwrap();

    assert_eq!(user.name, username);
    assert!(user.is_administrator);
    assert!(!token.is_empty());

    let claims = rustyfin_auth::validate_token(&pool, &token).await.unwrap();
    assert_eq!(claims.sub, user.id);
}

#[tokio::test]
async fn test_authenticate_returns_updated_last_login_date() {
    // M-16: authenticate returns the user as it stands after the login, with the new timestamp.
    let pool = test_pool().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&pool, &username, "pw")
        .await
        .unwrap();

    let (user, _) = rustyfin_auth::authenticate(&pool, &username, "pw", &Default::default())
        .await
        .unwrap();
    assert!(
        user.last_login_date.is_some(),
        "the returned user carries the refreshed LastLoginDate"
    );
}

#[tokio::test]
async fn test_authenticate_wrong_password() {
    let pool = test_pool().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&pool, &username, "correct")
        .await
        .unwrap();

    let result = rustyfin_auth::authenticate(&pool, &username, "wrong", &Default::default()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_authenticate_nonexistent_user() {
    let pool = test_pool().await;
    let result = rustyfin_auth::authenticate(&pool, "ghost", "pass", &Default::default()).await;
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

    let found = rustyfin_db::repo_user::get_user_by_name(&pool, &username)
        .await
        .unwrap();
    assert!(found.is_some());
}
