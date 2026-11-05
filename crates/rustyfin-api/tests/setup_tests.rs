//! The setup wizard on servers that already have users. Setup is open only while the server has no
//! users at all. At startup the earliest enabled administrator becomes the owner, whatever the wizard
//! flag says, and `POST /Startup/User` never changes an existing user.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::{RwLock, broadcast, watch};
use tower::ServiceExt;

use rustyfin_api::cache::{ApiCache, ImageCache};
use rustyfin_api::middleware::RateLimiter;
use rustyfin_api::routes::{StartupOwner, build_router, settle_owner_at_startup};
use rustyfin_api::state::AppState;
use rustyfin_db::repo_user::{self, OwnerClaim};
use rustyfin_model::server_info::ServerConfiguration;
use rustyfin_model::ws_event::WsEvent;

const API_KEY: &str = "test-api-key";

/// Removes a test's data directory when the test ends, also when it fails.
struct DataDir(PathBuf);

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds the state on a new data directory. Keep the `DataDir` alive until the test ends.
async fn test_state() -> (AppState, DataDir) {
    let data_dir =
        std::env::temp_dir().join(format!("rustyfin_setup_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(data_dir.join("logs")).unwrap();
    std::fs::create_dir_all(data_dir.join("cache")).unwrap();

    let db_path = data_dir.join("test.db");
    let pool = rustyfin_db::init_db(db_path.to_str().unwrap())
        .await
        .unwrap();

    let (shutdown_tx, _) = watch::channel(false);
    let data_dir_arc = Arc::new(data_dir.clone());

    let transcode_manager = rustyfin_core::transcode::TranscodeManager::new(&data_dir);
    let task_scheduler =
        rustyfin_core::scheduler::TaskScheduler::new(pool.clone(), data_dir_arc.clone());
    let plugin_manager = rustyfin_core::plugin_manager::PluginManager::new(&data_dir_arc);
    let (ws_tx, _) = broadcast::channel::<WsEvent>(100);
    let (log_level_tx, _) = watch::channel("info".to_string());

    let state = AppState {
        db: pool.clone(),
        db_ro: pool,
        server_id: Arc::new("test-server-id".to_string()),
        data_dir: data_dir_arc,
        config: Arc::new(RwLock::new(ServerConfiguration::default())),
        shutdown_tx: Arc::new(shutdown_tx),
        bind_addr: Arc::new("0.0.0.0:8096".to_string()),
        api_key: Some(Arc::new(API_KEY.to_string())),
        quick_connect: Arc::new(RwLock::new(HashMap::new())),
        transcode_manager,
        task_scheduler,
        plugin_manager,
        rate_limiter: RateLimiter::new(1000, 60),
        known_proxies: Arc::new(Vec::new()),
        ws_broadcast: ws_tx,
        log_level_tx,
        cache: ApiCache::new(),
        image_cache: ImageCache::new(),
        http_client: reqwest::Client::new(),
    };
    (state, DataDir(data_dir))
}

fn app(state: AppState) -> axum::Router {
    build_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

/// Sends a request and returns the status and the JSON body (`Null` when the body is not JSON).
async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Builds a request with an optional bearer token and an optional JSON body.
fn request(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    match body {
        Some(body) => builder
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

/// Logs in and returns the access token, or `None` if login fails.
async fn login(app: &axum::Router, name: &str, password: &str) -> Option<String> {
    let body = serde_json::json!({ "Username": name, "Pw": password });
    let req = request("POST", "/Users/AuthenticateByName", None, Some(body));
    let (status, json) = send(app, req).await;
    (status == StatusCode::OK).then(|| json["AccessToken"].as_str().unwrap().to_string())
}

/// Sends the wizard's `POST /Startup/User`.
async fn post_startup_user(
    app: &axum::Router,
    token: Option<&str>,
    name: &str,
    password: &str,
) -> StatusCode {
    let body = serde_json::json!({ "Name": name, "Password": password });
    send(app, request("POST", "/Startup/User", token, Some(body)))
        .await
        .0
}

/// Reads `StartupWizardCompleted` from the public and from the full system info.
async fn wizard_completed(app: &axum::Router) -> (bool, bool) {
    let (status, public) = send(app, request("GET", "/System/Info/Public", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, full) = send(app, request("GET", "/System/Info", Some(API_KEY), None)).await;
    assert_eq!(status, StatusCode::OK);
    (
        public["StartupWizardCompleted"].as_bool().unwrap(),
        full["StartupWizardCompleted"].as_bool().unwrap(),
    )
}

/// Creates a user that is not the owner. `create_initial_user` makes an administrator.
async fn add_user(state: &AppState, name: &str, password: &str, admin: bool) {
    let user = rustyfin_auth::create_initial_user(&state.db, name, password)
        .await
        .unwrap();
    if !admin {
        repo_user::update_policy(&state.db, user.id, false)
            .await
            .unwrap();
    }
}

async fn set_created_at(state: &AppState, name: &str, created_at: &str) {
    sqlx::query("UPDATE users SET created_at = ? WHERE name = ?")
        .bind(created_at)
        .bind(name)
        .execute(&state.db)
        .await
        .unwrap();
}

async fn disable(state: &AppState, name: &str) {
    sqlx::query("UPDATE users SET is_disabled = 1 WHERE name = ?")
        .bind(name)
        .execute(&state.db)
        .await
        .unwrap();
}

/// Returns `(is_administrator, is_owner, is_disabled)` for the named user, or `None` if it is gone.
async fn user_flags(state: &AppState, name: &str) -> Option<(bool, bool, bool)> {
    sqlx::query_as("SELECT is_administrator, is_owner, is_disabled FROM users WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.db)
        .await
        .unwrap()
}

async fn password_hash(state: &AppState, name: &str) -> String {
    sqlx::query_scalar("SELECT password_hash FROM users WHERE name = ?")
        .bind(name)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

/// Returns the names of all owners and the total number of users.
async fn owners_and_count(state: &AppState) -> (Vec<String>, i64) {
    let owners: Vec<String> = sqlx::query_scalar("SELECT name FROM users WHERE is_owner = 1")
        .fetch_all(&state.db)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .unwrap();
    (owners, count)
}

async fn token_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM access_tokens")
        .fetch_one(&state.db)
        .await
        .unwrap()
}

async fn library_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
        .fetch_one(&state.db)
        .await
        .unwrap()
}

/// Runs the server's startup step on the state's database, configuration and data directory.
async fn start(state: &AppState) -> StartupOwner {
    let mut config = state.config.write().await;
    settle_owner_at_startup(&state.db, &mut config, &state.data_dir)
        .await
        .unwrap()
}

/// Reads `IsStartupWizardCompleted` from the saved `config.json`, or `None` if there is none.
fn saved_wizard_flag(state: &AppState) -> Option<bool> {
    let saved = std::fs::read_to_string(state.data_dir.join("config.json")).ok()?;
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    Some(saved["IsStartupWizardCompleted"].as_bool().unwrap())
}

fn media_folder(state: &AppState, name: &str) -> String {
    let dir = state.data_dir.join("media").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_str().unwrap().to_string()
}

// ── Startup ──

#[tokio::test]
async fn g2d_startup_promotes_earliest_enabled_admin_whatever_the_flag() {
    let (state, _dir) = test_state().await;
    // Insertion order differs from creation order, so only created_at can pick "first".
    add_user(&state, "second", "pw", true).await;
    add_user(&state, "first", "pw", true).await;
    add_user(&state, "retired", "pw", true).await;
    add_user(&state, "viewer", "pw", false).await;
    for (name, created_at) in [
        ("viewer", "2019-01-01T00:00:00+00:00"),
        ("retired", "2020-01-01T00:00:00+00:00"),
        ("first", "2021-01-01T00:00:00+00:00"),
        ("second", "2022-01-01T00:00:00+00:00"),
    ] {
        set_created_at(&state, name, created_at).await;
    }
    disable(&state, "retired").await;
    assert!(!state.config.read().await.is_startup_wizard_completed);

    assert_eq!(start(&state).await, StartupOwner::Promoted("first".into()));
    assert_eq!(user_flags(&state, "first").await, Some((true, true, false)));
    assert_eq!(
        user_flags(&state, "retired").await,
        Some((true, false, true)),
        "a disabled administrator is skipped and stays disabled"
    );
    assert_eq!(
        user_flags(&state, "second").await,
        Some((true, false, false))
    );
    assert_eq!(
        user_flags(&state, "viewer").await,
        Some((false, false, false))
    );
    assert!(state.config.read().await.is_startup_wizard_completed);
    assert_eq!(saved_wizard_flag(&state), Some(true));
    assert_eq!(wizard_completed(&app(state.clone())).await, (true, true));

    assert_eq!(start(&state).await, StartupOwner::Existing);
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["first".to_string()], 4)
    );
}

#[tokio::test]
async fn g2d_startup_with_owner_closes_wizard() {
    let (state, _dir) = test_state().await;
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert!(!state.config.read().await.is_startup_wizard_completed);

    assert_eq!(start(&state).await, StartupOwner::Existing);
    assert!(state.config.read().await.is_startup_wizard_completed);
    assert_eq!(saved_wizard_flag(&state), Some(true));
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 1)
    );
}

#[tokio::test]
async fn g2d_startup_without_users_leaves_setup_open() {
    let (state, _dir) = test_state().await;
    state.config.write().await.is_startup_wizard_completed = true;

    assert_eq!(start(&state).await, StartupOwner::NoUsers);
    assert!(
        state.config.read().await.is_startup_wizard_completed,
        "the flag is left alone"
    );
    assert_eq!(
        saved_wizard_flag(&state),
        None,
        "config.json is not written"
    );
    let app = app(state.clone());
    assert_eq!(
        wizard_completed(&app).await,
        (false, false),
        "without users setup is open, whatever the flag says"
    );
    let req = request("GET", "/Startup/Configuration", None, None);
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
}

#[tokio::test]
async fn g2d_startup_without_enabled_admin_keeps_setup_closed() {
    let (state, _dir) = test_state().await;
    add_user(&state, "viewer", "pw", false).await;
    add_user(&state, "retired", "pw", true).await;
    disable(&state, "retired").await;

    assert_eq!(start(&state).await, StartupOwner::NoEnabledAdmin);
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 2));
    assert_eq!(
        user_flags(&state, "retired").await,
        Some((true, false, true))
    );
    assert_eq!(
        user_flags(&state, "viewer").await,
        Some((false, false, false))
    );
    assert!(state.config.read().await.is_startup_wizard_completed);
    assert_eq!(saved_wizard_flag(&state), Some(true));

    let app = app(state.clone());
    assert_eq!(wizard_completed(&app).await, (true, true));
    for name in ["mallory", "viewer", "retired"] {
        assert_eq!(
            post_startup_user(&app, None, name, "taken").await,
            StatusCode::UNAUTHORIZED,
            "anonymous claim of {name}"
        );
    }
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 2));
    assert_eq!(
        user_flags(&state, "retired").await,
        Some((true, false, true))
    );
}

#[tokio::test]
async fn g2d_startup_promotes_default_admin() {
    // The shape of a server from before the setup wizard: only admin/admin, no owner, flag false.
    let (state, _dir) = test_state().await;
    add_user(&state, "admin", "admin", true).await;

    assert_eq!(start(&state).await, StartupOwner::Promoted("admin".into()));
    assert_eq!(user_flags(&state, "admin").await, Some((true, true, false)));
    assert!(state.config.read().await.is_startup_wizard_completed);
    assert_eq!(saved_wizard_flag(&state), Some(true));
    assert_eq!(
        rustyfin_auth::admins_with_default_password(&state.db)
            .await
            .unwrap(),
        vec!["admin".to_string()],
        "the startup warning names admin"
    );
}

// ── The owner claim ──

#[tokio::test]
async fn g2d_claim_never_changes_an_existing_user() {
    let (state, _dir) = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    add_user(&state, "bob", "bobpass", false).await;
    let app = app(state.clone());
    let bob_token = login(&app, "bob", "bobpass").await.unwrap();
    let admin_hash = password_hash(&state, "admin").await;
    let bob_hash = password_hash(&state, "bob").await;
    let tokens = token_count(&state).await;

    for name in ["admin", "bob"] {
        assert_eq!(
            post_startup_user(&app, None, name, "taken").await,
            StatusCode::UNAUTHORIZED,
            "anonymous claim of {name}"
        );
        assert_eq!(
            post_startup_user(&app, Some(&bob_token), name, "taken").await,
            StatusCode::FORBIDDEN,
            "claim of {name} by a user who is not an administrator"
        );
        assert_eq!(
            post_startup_user(&app, Some(API_KEY), name, "taken").await,
            StatusCode::FORBIDDEN,
            "claim of {name} with the API key"
        );
    }

    assert_eq!(password_hash(&state, "admin").await, admin_hash);
    assert_eq!(password_hash(&state, "bob").await, bob_hash);
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, false))
    );
    assert_eq!(user_flags(&state, "bob").await, Some((false, false, false)));
    assert_eq!(
        token_count(&state).await,
        tokens,
        "no token added or removed"
    );
    let (status, me) = send(&app, request("GET", "/Users/Me", Some(&bob_token), None)).await;
    assert_eq!(status, StatusCode::OK, "bob's session survives");
    assert_eq!(me["Name"], "bob");
    assert!(login(&app, "bob", "taken").await.is_none());
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 2));
}

#[tokio::test]
async fn g2d_claim_with_new_name_refused_while_users_exist() {
    let (state, _dir) = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    let app = app(state.clone());

    for name in ["mallory", "Admin"] {
        assert_eq!(
            post_startup_user(&app, None, name, "pw").await,
            StatusCode::UNAUTHORIZED,
            "anonymous claim of {name}"
        );
        assert_eq!(
            post_startup_user(&app, Some(API_KEY), name, "pw").await,
            StatusCode::FORBIDDEN,
            "claim of {name} with the API key"
        );
    }
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 1));
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, false)),
        "admin is neither disabled nor promoted"
    );
}

#[tokio::test]
async fn g2d_claim_owner_refuses_once_any_user_exists() {
    let (state, _dir) = test_state().await;
    add_user(&state, "viewer", "pw", false).await;
    let before = password_hash(&state, "viewer").await;
    let hash = rustyfin_auth::hash_password("new").await.unwrap();

    for name in ["viewer", "mallory"] {
        let claim = repo_user::claim_owner(&state.db, name, &hash)
            .await
            .unwrap();
        assert!(matches!(claim, OwnerClaim::Refused), "claim of {name}");
    }
    assert_eq!(password_hash(&state, "viewer").await, before);
    assert_eq!(
        user_flags(&state, "viewer").await,
        Some((false, false, false))
    );
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 1));
}

// ── Setup routes ──

#[tokio::test]
async fn g2d_setup_routes_need_admin_once_users_exist() {
    let (state, _dir) = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    add_user(&state, "viewer", "viewerpass", false).await;
    let app = app(state.clone());
    let viewer = login(&app, "viewer", "viewerpass").await.unwrap();
    let language_before = state
        .config
        .read()
        .await
        .preferred_metadata_language
        .clone();
    let parent = state.data_dir.to_str().unwrap().to_string();
    let new_library = serde_json::json!({
        "Name": "G2d Refused",
        "ContentType": "Movies",
        "Paths": [media_folder(&state, "g2d-refused")],
    });
    let routes = [
        ("GET", "/Startup/Configuration".to_string(), None),
        (
            "POST",
            "/Startup/Configuration".to_string(),
            Some(serde_json::json!({ "PreferredMetadataLanguage": "fr" })),
        ),
        ("GET", "/Startup/User".to_string(), None),
        (
            "POST",
            "/Startup/User".to_string(),
            Some(serde_json::json!({ "Name": "mallory", "Password": "pw" })),
        ),
        (
            "POST",
            "/Startup/Complete".to_string(),
            Some(serde_json::json!({})),
        ),
        ("GET", "/Startup/FirstUser".to_string(), None),
        (
            "POST",
            "/Startup/RemoteAccess".to_string(),
            Some(serde_json::json!({})),
        ),
        (
            "GET",
            format!("/Library/BrowseDirectories?parent={parent}"),
            None,
        ),
        ("GET", "/Library/VirtualFolders".to_string(), None),
        (
            "POST",
            "/Library/VirtualFolders".to_string(),
            Some(new_library),
        ),
        ("GET", "/Environment/Drives".to_string(), None),
        ("GET", "/Environment/NetworkShares".to_string(), None),
    ];
    for (method, uri, body) in &routes {
        let req = request(method, uri, None, body.clone());
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {uri}"
        );
        let req = request(method, uri, Some(&viewer), body.clone());
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::FORBIDDEN,
            "non-admin {method} {uri}"
        );
    }
    {
        let config = state.config.read().await;
        assert_eq!(config.preferred_metadata_language, language_before);
        assert!(!config.is_startup_wizard_completed);
    }
    assert_eq!(library_count(&state).await, 0);
    assert_eq!(owners_and_count(&state).await, (Vec::<String>::new(), 2));

    for uri in [
        "/Startup/Configuration",
        "/Environment/Drives",
        "/Library/VirtualFolders",
    ] {
        let req = request("GET", uri, Some(API_KEY), None);
        assert_eq!(send(&app, req).await.0, StatusCode::OK, "API key GET {uri}");
    }
}

#[tokio::test]
async fn g2d_setup_routes_stay_open_without_users() {
    let (state, _dir) = test_state().await;
    let app = app(state.clone());
    let parent = state.data_dir.to_str().unwrap().to_string();
    for uri in [
        "/Startup/Configuration".to_string(),
        "/Startup/User".to_string(),
        "/Startup/FirstUser".to_string(),
        format!("/Library/BrowseDirectories?parent={parent}"),
        "/Library/VirtualFolders".to_string(),
        "/Environment/Drives".to_string(),
        "/Environment/NetworkShares".to_string(),
    ] {
        let (status, body) = send(&app, request("GET", &uri, None, None)).await;
        assert_eq!(status, StatusCode::OK, "anonymous GET {uri}: {body}");
    }
    let config = serde_json::json!({ "PreferredMetadataLanguage": "fr" });
    let req = request("POST", "/Startup/Configuration", None, Some(config));
    assert_eq!(send(&app, req).await.0, StatusCode::NO_CONTENT);
    assert_eq!(state.config.read().await.preferred_metadata_language, "fr");
    let body = serde_json::json!({
        "Name": "G2d Setup",
        "ContentType": "Movies",
        "Paths": [media_folder(&state, "g2d-setup")],
    });
    let req = request("POST", "/Library/VirtualFolders", None, Some(body));
    let (status, created) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK, "anonymous create: {created}");
    assert_eq!(library_count(&state).await, 1);
}

// ── A fresh server ──

#[tokio::test]
async fn g2d_fresh_server_wizard_creates_owner_and_completes() {
    let (state, _dir) = test_state().await;
    let app = app(state.clone());
    assert_eq!(wizard_completed(&app).await, (false, false));

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(user_flags(&state, "karl").await, Some((true, true, false)));
    let token = login(&app, "karl", "geheim").await.unwrap();
    let req = request(
        "POST",
        "/Startup/Complete",
        Some(&token),
        Some(serde_json::json!({})),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::NO_CONTENT);
    assert!(state.config.read().await.is_startup_wizard_completed);
    assert_eq!(saved_wizard_flag(&state), Some(true));
    assert_eq!(wizard_completed(&app).await, (true, true));

    assert_eq!(
        post_startup_user(&app, None, "mallory", "pw").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(start(&state).await, StartupOwner::Existing);
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 1)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g2d_concurrent_claims_leave_one_owner() {
    let (state, _dir) = test_state().await;
    let app = app(state.clone());
    let claims = [
        ("karl", "pw-karl-1"),
        ("karl", "pw-karl-2"),
        ("alice", "pw-alice"),
        ("bob", "pw-bob"),
    ];

    let tasks: Vec<_> = claims
        .iter()
        .map(|&(name, password)| {
            let app = app.clone();
            tokio::spawn(async move { post_startup_user(&app, None, name, password).await })
        })
        .collect();
    let mut statuses = Vec::new();
    for task in tasks {
        statuses.push(task.await.unwrap());
    }

    let won = statuses
        .iter()
        .filter(|s| **s == StatusCode::NO_CONTENT)
        .count();
    assert_eq!(won, 1, "{statuses:?}");
    assert!(
        statuses.iter().all(|s| matches!(
            *s,
            StatusCode::NO_CONTENT | StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        )),
        "{statuses:?}"
    );
    let (owners, users) = owners_and_count(&state).await;
    assert_eq!((owners.len(), users), (1, 1));
    let mut signed_in = 0;
    for (name, password) in claims {
        if login(&app, name, password).await.is_some() {
            signed_in += 1;
        }
    }
    assert_eq!(signed_in, 1, "only the winner's password signs in");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g2d_concurrent_owner_claims_in_the_database() {
    let (state, _dir) = test_state().await;
    let hash = rustyfin_auth::hash_password("pw").await.unwrap();

    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let db = state.db.clone();
            let hash = hash.clone();
            tokio::spawn(async move {
                repo_user::claim_owner(&db, &format!("user{}", i % 4), &hash)
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut claimed = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), OwnerClaim::Claimed) {
            claimed += 1;
        }
    }

    assert_eq!(claimed, 1);
    let (owners, users) = owners_and_count(&state).await;
    assert_eq!((owners.len(), users), (1, 1));
}
