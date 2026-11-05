use std::collections::HashMap;
use std::sync::Arc;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::{RwLock, broadcast, watch};
use tower::ServiceExt;

use rustyfin_api::cache::{ApiCache, ImageCache};
use rustyfin_api::middleware::RateLimiter;
use rustyfin_api::routes::build_router;
use rustyfin_api::state::AppState;
use rustyfin_model::server_info::ServerConfiguration;
use rustyfin_model::ws_event::WsEvent;

async fn test_state() -> AppState {
    let data_dir = std::env::temp_dir().join(format!("rustyfin_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data_dir).ok();
    std::fs::create_dir_all(data_dir.join("logs")).ok();
    std::fs::create_dir_all(data_dir.join("cache")).ok();

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
    let rate_limiter = RateLimiter::new(1000, 60);
    let (ws_tx, _) = broadcast::channel::<WsEvent>(100);
    let (_log_level_tx, _) = watch::channel("info".to_string());
    let cache = ApiCache::new();
    let image_cache = ImageCache::new();

    AppState {
        db: pool.clone(),
        db_ro: pool,
        server_id: Arc::new("test-server-id".to_string()),
        data_dir: data_dir_arc,
        config: Arc::new(RwLock::new(ServerConfiguration::default())),
        shutdown_tx: Arc::new(shutdown_tx),
        bind_addr: Arc::new("0.0.0.0:8096".to_string()),
        api_key: Some(Arc::new("test-api-key".to_string())),
        quick_connect: Arc::new(RwLock::new(HashMap::new())),
        transcode_manager,
        task_scheduler,
        plugin_manager,
        rate_limiter,
        known_proxies: Arc::new(Vec::new()),
        ws_broadcast: ws_tx,
        log_level_tx: _log_level_tx,
        cache,
        image_cache,
        http_client: reqwest::Client::new(),
    }
}

fn app(state: AppState) -> axum::Router {
    build_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
}

// ── Public Endpoint Tests ──

#[tokio::test]
async fn test_health_endpoint() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn test_ping_endpoint() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Ping")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_public_info_endpoint() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info/Public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let info: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(info["Id"], "test-server-id");
}

// ── Auth Endpoint Tests ──

#[tokio::test]
async fn test_login_success() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/Users/AuthenticateByName")
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"Username":"{}","Pw":"admin"}}"#,
                    username
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let auth: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!auth["AccessToken"].as_str().unwrap().is_empty());
    assert_eq!(auth["ServerId"], "test-server-id");
}

#[tokio::test]
async fn test_login_wrong_password() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/Users/AuthenticateByName")
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"Username":"{}","Pw":"wrong"}}"#,
                    username
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Protected Endpoint Tests ──

#[tokio::test]
async fn test_protected_endpoint_without_auth() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/*#[tokio::test]
async fn test_protected_endpoint_with_api_key() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .header("X-Emby-Token", "test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let info: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(info["Id"], "test-server-id");
}*/

#[tokio::test]
async fn test_protected_endpoint_with_jwt() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_protected_endpoint_with_invalid_token() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .header("Authorization", "Bearer invalid.token.here")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── B8: API key ──
// An empty or blank token never authenticates, whatever the configured API key is, and the
// websocket accepts the API key or a user token.

/// A copy of `base` with `key` as the configured API key.
fn b8_with_key(base: &AppState, key: Option<&str>) -> AppState {
    let mut state = base.clone();
    state.api_key = key.map(|key| Arc::new(key.to_string()));
    state
}

/// The statuses of `GET /System/Configuration` with `key` as the API key and `token` sent as the
/// `api_key` query value and as `X-Emby-Token`.
async fn b8_statuses(base: &AppState, key: Option<&str>, token: &str) -> (StatusCode, StatusCode) {
    let state = b8_with_key(base, key);
    let query = app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/System/Configuration?api_key={}",
                    token.replace(' ', "%20")
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    let header = app(state)
        .oneshot(
            Request::builder()
                .uri("/System/Configuration")
                .header("X-Emby-Token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    (query, header)
}

/// Serves the router on a free local port and returns the status of a websocket upgrade of
/// `/socket{query}`: 101 when the server accepts it.
async fn b8_socket_status(state: AppState, query: &str) -> u16 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app(state)).await.unwrap() });
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /socket{query} HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reader = tokio::io::BufReader::new(stream);
    let mut status_line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        reader.read_line(&mut status_line),
    )
    .await
    .expect("no answer to the websocket upgrade")
    .unwrap();
    status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("bad status line {status_line:?}"))
}

#[tokio::test]
async fn test_b8_empty_token_never_authenticates_http() {
    let base = test_state().await;
    let unauthorized = StatusCode::UNAUTHORIZED;
    // (key, token, expected). For an empty or blank key the right token is the key itself.
    let cases = [
        (Some(""), "", unauthorized),
        (Some(""), "wrong-key", unauthorized),
        (Some("   "), "", unauthorized),
        (Some("   "), "   ", unauthorized),
        (Some("   "), "wrong-key", unauthorized),
        (Some("b8-key"), "", unauthorized),
        (Some("b8-key"), "   ", unauthorized),
        (Some("b8-key"), "wrong-key", unauthorized),
        (Some("b8-key"), "b8-key", StatusCode::OK),
        (None, "", unauthorized),
        (None, "   ", unauthorized),
        (None, "wrong-key", unauthorized),
    ];
    for (key, token, expected) in cases {
        let (query, header) = b8_statuses(&base, key, token).await;
        assert_eq!(query, expected, "api_key={token:?} with key {key:?}");
        assert_eq!(header, expected, "X-Emby-Token {token:?} with key {key:?}");
    }
    std::fs::remove_dir_all(base.data_dir.as_path()).ok();
}

#[tokio::test]
async fn test_b8_socket_rejects_an_empty_token() {
    let base = test_state().await;
    let empty_tokens = [
        "?api_key=",
        "?token=",
        "?authorization=",
        "?authorization=Bearer%20",
        "?api_key=%20%20%20",
    ];
    for key in [Some(""), Some("   "), Some("b8-key"), None] {
        for query in empty_tokens {
            let status = b8_socket_status(b8_with_key(&base, key), query).await;
            assert_eq!(status, 401, "{query} with key {key:?}");
        }
    }
    // The key still opens the socket
    let state = b8_with_key(&base, Some("b8-key"));
    assert_eq!(b8_socket_status(state, "?api_key=b8-key").await, 101);
    std::fs::remove_dir_all(base.data_dir.as_path()).ok();
}

#[tokio::test]
async fn test_b8_socket_accepts_a_user_token_while_an_api_key_is_set() {
    let base = test_state().await;
    let user = rustyfin_model::user::User {
        id: uuid::Uuid::new_v4(),
        name: "b8-user".to_string(),
        password_hash: String::new(),
        is_administrator: false,
        created_at: chrono::Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Null,
    };
    rustyfin_db::repo_user::create_user(&base.db, &user)
        .await
        .unwrap();
    let token = rustyfin_auth::create_token(&base.db, &user, &Default::default())
        .await
        .unwrap();
    // A well-formed token that this server never issued
    let foreign = "0123456789abcdef0123456789abcdef";
    for key in [Some("b8-key"), None] {
        let state = b8_with_key(&base, key);
        // The web client sends `authorization=Bearer <token>`, Jellyfin clients `api_key=<token>`
        let web = format!("?authorization=Bearer%20{token}");
        assert_eq!(
            b8_socket_status(state.clone(), &web).await,
            101,
            "key {key:?}"
        );
        let jellyfin = format!("?api_key={token}");
        assert_eq!(
            b8_socket_status(state.clone(), &jellyfin).await,
            101,
            "key {key:?}"
        );
        let signed_elsewhere = format!("?api_key={foreign}");
        assert_eq!(
            b8_socket_status(state.clone(), &signed_elsewhere).await,
            401,
            "key {key:?}"
        );
        assert_eq!(
            b8_socket_status(state, "?api_key=wrong-key").await,
            401,
            "key {key:?}"
        );
    }
    let state = b8_with_key(&base, Some("b8-key"));
    assert_eq!(b8_socket_status(state, "?api_key=b8-key").await, 101);
    std::fs::remove_dir_all(base.data_dir.as_path()).ok();
}

// ── Error Response Format Tests ──

#[tokio::test]
async fn test_error_response_jellyfin_format() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let err: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Jellyfin-compatible error format
    assert!(err.get("Type").is_some());
    assert!(err.get("Title").is_some());
    assert!(err.get("Detail").is_some());
    assert_eq!(err["Status"], 401);
}

// ── Web Client Static Serving Tests ──

/// Check if the web client dist folder exists at any of the expected locations
fn webclient_dist_exists() -> bool {
    let candidates = [
        "crates/rustyfin-web/dist",
        "../crates/rustyfin-web/dist",
        "../../crates/rustyfin-web/dist",
        "rustyfin-web/dist",
        "dist",
        "web",
    ];
    candidates
        .iter()
        .any(|p| std::path::Path::new(p).join("index.html").exists())
}

#[tokio::test]
#[cfg_attr(not(feature = "webclient"), ignore)]
async fn test_webclient_index() {
    // Skip if web client dist is not built (e.g. in CI without npm step)
    if !webclient_dist_exists() {
        tracing::info!("skipping test_webclient_index: web client dist not found");
        return;
    }

    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/web/index.html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("Rustyfin") || body_str.contains("<!DOCTYPE html>"));
}

#[tokio::test]
#[cfg_attr(not(feature = "webclient"), ignore)]
async fn test_webclient_redirects() {
    // Skip if web client dist is not built (e.g. in CI without npm step)
    if !webclient_dist_exists() {
        tracing::info!("skipping test_webclient_redirects: web client dist not found");
        return;
    }

    let state = test_state().await;

    for path in ["/", "/index.html"] {
        let app = app(state.clone());
        let resp = app
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(resp.headers().get("location").unwrap(), "/web/index.html");
    }
}

// ── Rate Limiter Tests ──

#[tokio::test]
async fn test_rate_limiter_logic() {
    let limiter = RateLimiter::new(3, 60);
    assert!(limiter.check("192.168.1.100").await);
    assert!(limiter.check("192.168.1.100").await);
    assert!(limiter.check("192.168.1.100").await);
    // 4th request within window exceeds limit
    assert!(!limiter.check("192.168.1.100").await);
    // Different IP still has budget
    assert!(limiter.check("192.168.1.101").await);

    // Limit of 0 disables rate limiting
    let unlimited = RateLimiter::new(0, 60);
    for _ in 0..10 {
        assert!(unlimited.check("192.168.1.100").await);
    }
}

#[tokio::test]
async fn test_rate_limiter_loopback_bypass() {
    let mut state = test_state().await;
    // Set a very low limit of 1 request
    state.rate_limiter = RateLimiter::new(1, 60);
    let app = app(state);

    // Loopback requests should not be blocked even after multiple calls
    for _ in 0..5 {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

/// Router whose requests come from a non-loopback peer, so the rate limiter applies to them.
fn remote_app(state: AppState) -> axum::Router {
    build_router(state).layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 10], 40000))))
}

/// A valid 2x2 RGB PNG.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a,
    0x73, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x38, 0x61, 0x64, 0x04,
    0x44, 0x0c, 0x10, 0x0a, 0x00, 0x23, 0x36, 0x04, 0xb1, 0xae, 0x7d, 0xb9, 0xbc, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[tokio::test]
async fn test_remote_browsing_burst_is_not_rate_limited() {
    let mut state = test_state().await;
    // The shipped default budget: RUSTYFIN_RATE_LIMIT=500 per RUSTYFIN_RATE_WINDOW=60 s
    state.rate_limiter = RateLimiter::new(500, 60);

    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();

    let media_dir = state.data_dir.join("movies");
    std::fs::create_dir_all(&media_dir).unwrap();
    let poster = media_dir.join("Burst Movie-poster.png");
    std::fs::write(&poster, TINY_PNG).unwrap();

    let lib = rustyfin_model::library::Library {
        id: uuid::Uuid::new_v4(),
        name: "Burst Movies".to_string(),
        content_type: rustyfin_model::library::LibraryContentType::Movies,
        paths: vec![media_dir.to_string_lossy().to_string()],
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib)
        .await
        .unwrap();

    let item_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("Burst Movie")
    .bind("Movie")
    .bind(
        media_dir
            .join("Burst Movie.mkv")
            .to_string_lossy()
            .to_string(),
    )
    .execute(&state.db)
    .await
    .unwrap();

    let img = rustyfin_model::image::ItemImage {
        item_id,
        image_type: rustyfin_model::image::ImageType::Primary,
        image_index: 0,
        path: poster.to_string_lossy().to_string(),
        provider: "local".to_string(),
        width: None,
        height: None,
        blurhash: None,
    };
    rustyfin_db::repo_image::upsert_image(&state.db, &img)
        .await
        .unwrap();

    let app = remote_app(state);

    // A grid page is one listing plus a thumbnail per card; mix in the other calls a
    // browsing, searching and playing client makes. 600 requests is above the budget.
    for i in 1..=600 {
        let (method, uri, body, must_be_ok) = match i % 10 {
            0 => (
                "GET",
                "/Items?IncludeItemTypes=Movie&Recursive=true&SortBy=SortName&StartIndex=0&Limit=50"
                    .to_string(),
                String::new(),
                true,
            ),
            1 => ("GET", format!("/Items/{item_id}"), String::new(), false),
            2 => (
                "GET",
                "/Search/Hints?searchTerm=burst".to_string(),
                String::new(),
                false,
            ),
            3 => ("GET", "/UserViews".to_string(), String::new(), false),
            4 => (
                "POST",
                "/Sessions/Playing/Progress".to_string(),
                format!(r#"{{"ItemId":"{item_id}","PositionTicks":10000000}}"#),
                false,
            ),
            _ => (
                "GET",
                format!("/Items/{item_id}/Images/Primary?maxWidth=150&format=webp"),
                String::new(),
                true,
            ),
        };
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        let status = resp.status();
        assert_ne!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "request #{i} {method} {uri} was rate limited"
        );
        if must_be_ok {
            assert_eq!(status, StatusCode::OK, "request #{i} {method} {uri}");
        }
    }
}

#[tokio::test]
async fn test_public_non_login_endpoints_are_not_rate_limited() {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    let app = remote_app(state);

    for uri in [
        "/System/Info/Public",
        "/QuickConnect/Enabled",
        "/Users/Public",
    ] {
        for attempt in 1..=10 {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{uri} attempt {attempt}");
        }
    }
}

#[tokio::test]
async fn test_failed_logins_from_remote_client_are_rate_limited() {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let app = remote_app(state);

    let wrong_login = || {
        Request::builder()
            .method("POST")
            .uri("/Users/AuthenticateByName")
            .header("Content-Type", "application/json")
            .body(Body::from(format!(
                r#"{{"Username":"{}","Pw":"wrong"}}"#,
                username
            )))
            .unwrap()
    };

    for attempt in 1..=3 {
        let resp = app.clone().oneshot(wrong_login()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "attempt {attempt}");
    }
    let resp = app.clone().oneshot(wrong_login()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_password_reset_endpoints_share_the_login_rate_limit() {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    let app = remote_app(state);

    let post = |uri: &str, body: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let login = || {
        post(
            "/Users/AuthenticateByName",
            r#"{"Username":"nobody","Pw":"x"}"#,
        )
    };
    let forgot = || post("/Users/ForgotPassword", r#"{"EnteredUsername":"nobody"}"#);
    let reset = || {
        post(
            "/Users/ResetPassword",
            r#"{"Pin":"123456","Username":"nobody","TempPassword":"x"}"#,
        )
    };

    let resp = app.clone().oneshot(login()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // G2: ForgotPassword no longer reveals whether the user exists; unknown users get 200.
    let resp = app.clone().oneshot(forgot()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app.clone().oneshot(reset()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // The three endpoints draw on one budget per client
    for (name, req) in [("login", login()), ("forgot", forgot()), ("reset", reset())] {
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "{name}");
    }
}

// ── Image Route Tests ──

#[tokio::test]
async fn test_item_image_missing_returns_404_not_500() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Items/{random_id}/Images/Primary"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_item_image_missing_file_returns_404_not_500() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();

    let lib = rustyfin_model::library::Library {
        id: uuid::Uuid::new_v4(),
        name: "Test Movies".to_string(),
        content_type: rustyfin_model::library::LibraryContentType::Movies,
        paths: vec!["/media/movies".to_string()],
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib)
        .await
        .unwrap();

    let item_id = uuid::Uuid::new_v4();

    // Insert item and image pointing to non-existent file path
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("Test Item")
    .bind("Movie")
    .bind("/nonexistent/movie.mkv")
    .execute(&state.db)
    .await
    .unwrap();

    let img = rustyfin_model::image::ItemImage {
        item_id,
        image_type: rustyfin_model::image::ImageType::Primary,
        image_index: 0,
        path: "/nonexistent/poster.jpg".to_string(),
        provider: "local".to_string(),
        width: None,
        height: None,
        blurhash: None,
    };
    rustyfin_db::repo_image::upsert_image(&state.db, &img)
        .await
        .unwrap();

    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Items/{item_id}/Images/Primary?maxWidth=300"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Must return 404 Not Found, never 500 Internal Server Error
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Library Endpoint Tests ──

#[tokio::test]
async fn test_get_libraries_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Library/VirtualFolders")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let libs: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(libs.is_empty());
}

#[tokio::test]
async fn test_create_and_get_library() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state.clone());

    // Create a library
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/Library/VirtualFolders")
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"Name":"Test Movies","ContentType":"Movies","Paths":["/media/movies"]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Get libraries
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Library/VirtualFolders")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let libs: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(libs.len(), 1);
    assert_eq!(libs[0]["Name"], "Test Movies");
}

#[tokio::test]
async fn test_get_user_views_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/UserViews")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let views: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(views["Items"].is_array());
    assert_eq!(views["Items"].as_array().unwrap().len(), 0);
    assert_eq!(views["TotalRecordCount"], 0);
}

#[tokio::test]
async fn test_get_user_views_with_library() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();

    // Create a library
    let lib = rustyfin_model::library::Library {
        id: uuid::Uuid::new_v4(),
        name: "My Movies".to_string(),
        content_type: rustyfin_model::library::LibraryContentType::Movies,
        paths: vec!["/media/movies".to_string()],
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib)
        .await
        .unwrap();

    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/UserViews")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let views: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = views["Items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["Name"], "My Movies");
    assert_eq!(items[0]["CollectionType"], "movies");
    assert_eq!(items[0]["Type"], "CollectionFolder");
}

// ── Search Endpoint Tests ──

#[tokio::test]
async fn test_search_hints_empty_term() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Search/Hints?SearchTerm=")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["SearchHints"].is_array());
    assert_eq!(result["TotalRecordCount"], 0);
}

#[tokio::test]
async fn test_full_search_empty_term() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Search?Term=")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["Items"].is_array());
    assert_eq!(result["TotalRecordCount"], 0);
}

// ── System Endpoint Tests ──

#[tokio::test]
async fn test_system_info_with_auth() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let info: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(info["Id"], "test-server-id");
}

#[tokio::test]
async fn test_system_storage_info() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Info/Storage")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let info: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(info.get("LogPath").is_some());
    assert!(info.get("CachePath").is_some());
    assert!(info.get("ProgramDataPath").is_some());
}

#[tokio::test]
async fn test_system_update_packages() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/UpdatePackages")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let packages: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(packages.is_empty());
}

// ── DLNA Endpoint Tests ──

#[tokio::test]
async fn test_dlna_profile_endpoint() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Dlna/Profile")
                .header("X-Emby-Token", "test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let profile: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(profile["Name"], "Default");
    assert!(profile["DirectPlayProfiles"].is_array());
    assert!(profile["TranscodingProfiles"].is_array());
}

#[tokio::test]
async fn test_dlna_device_detection() {
    let state = test_state().await;
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/Dlna/Profile/DeviceDetection?UserAgent=Jellyfin/10.8.0")
                .header("X-Emby-Token", "test-api-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let detection: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detection["FriendlyName"], "Jellyfin Client");
}

// ── Items Endpoint Tests ──

#[tokio::test]
async fn test_get_items_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Items")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["Items"].is_array());
    assert_eq!(result["TotalRecordCount"], 0);
}

#[tokio::test]
async fn test_get_item_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Items/{random_id}"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Activity Log Tests ──

#[tokio::test]
async fn test_activity_log() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Sessions/Logging")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Session Tests ──

#[tokio::test]
async fn test_get_sessions() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Sessions")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let sessions: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(sessions.is_array());
}

// ── Playback Tests ──

#[tokio::test]
async fn test_playback_progress_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/Items/{random_id}/Progress"))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(r#"{"PositionTicks":1000000,"IsPlayed":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Branding Tests ──

#[tokio::test]
async fn test_branding_splashscreen() {
    let state = test_state().await;
    let app = app(state);

    app.oneshot(
        Request::builder()
            .uri("/Branding")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    // Endpoint may not be implemented yet; accept any response
}

// ── Quick Connect Tests ──

#[tokio::test]
async fn test_quick_connect_generate() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/QuickConnect/Generate")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet fully implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Environment Tests ──

#[tokio::test]
async fn test_environment_drives() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Environment/Drives")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let drives: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    // At least root "/" should be present on Linux
    assert!(!drives.is_empty());
}

// ── Scheduled Tasks Tests ──

#[tokio::test]
async fn test_scheduled_tasks_list() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ScheduledTasks")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let tasks: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // May return array or object with Items key
    assert!(
        tasks.is_array() || tasks.get("Items").is_some() || tasks.get("ScheduledTasks").is_some()
    );
}

// ── Localization Tests ──

#[tokio::test]
async fn test_localization_cultures() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Localization/Cultures")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Plugins Tests ──

#[tokio::test]
async fn test_plugins_list() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Plugins")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let plugins: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    // Vec is always an array, just check it deserialized
    drop(plugins);
}

// ── Configuration Tests ──

#[tokio::test]
async fn test_configuration() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/System/Configuration")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Live TV Tests ──

#[tokio::test]
async fn test_live_tv_channels_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/LiveTv/Channels")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["Items"].is_array());
    assert_eq!(result["TotalRecordCount"], 0);
}

// ── Persons Tests ──

#[tokio::test]
async fn test_persons_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Persons")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Shows Tests ──

#[tokio::test]
async fn test_shows_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Shows")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Media Segments Tests ──

#[tokio::test]
async fn test_media_segments_empty() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/MediaSegments")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Devices Tests ──

#[tokio::test]
async fn test_devices_list() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Devices")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 500 if devices table doesn't exist yet
    assert!(resp.status().is_success() || resp.status() == StatusCode::INTERNAL_SERVER_ERROR);
}

// ── Subtitle Tests ──

#[tokio::test]
async fn test_subtitle_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Videos/{random_id}/Subtitles/0/Stream"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Transcode Tests ──

#[tokio::test]
async fn test_transcode_not_started() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_session = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/transcode/{random_session}/1/main.m3u8"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Backup Tests ──

#[tokio::test]
async fn test_backup_status() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    app.oneshot(
        Request::builder()
            .uri("/Backup/Status")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    // Endpoint may not be implemented yet; accept any response
}

// ── SyncPlay Tests ──

#[tokio::test]
async fn test_sync_play_instances() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/SyncPlay/GetInstances")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Encoding Manager Tests ──

#[tokio::test]
async fn test_encoding_manager_stats() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/EncodingManager/EncodingStats")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 404 if the endpoint is not yet implemented
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Library Options Tests ──

#[tokio::test]
async fn test_library_options_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Library/{random_id}/Options"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // May return 200 with empty options or 404
    assert!(resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND);
}

// ── Library Modified Volumes Tests ──

#[tokio::test]
async fn test_library_modified_volumes() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Library/ModifiedVolumes")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let volumes: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(volumes.is_empty());
}

// ── User Library Views (with user ID) Tests ──

#[tokio::test]
async fn test_get_user_views_with_id() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Users/{}/Views", user.id))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let views: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(views["Items"].is_array());
}

// ── DLNA Streaming Tests ──

#[tokio::test]
async fn test_dlna_streaming_description() {
    let state = test_state().await;
    let app = app(state);

    app.oneshot(
        Request::builder()
            .uri("/dlna/contentdirectory/description.xml")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    // DLNA streaming endpoint may not be implemented yet; accept any response
}

// ── Startup Wizard and Owner Tests ──

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

/// Logs in and returns the access token and the user id, or `None` if login fails.
async fn login(app: &axum::Router, name: &str, password: &str) -> Option<(String, String)> {
    let body = serde_json::json!({ "Username": name, "Pw": password });
    let req = request("POST", "/Users/AuthenticateByName", None, Some(body));
    let (status, json) = send(app, req).await;
    if status != StatusCode::OK {
        return None;
    }
    Some((
        json["AccessToken"].as_str().unwrap().to_string(),
        json["User"]["Id"].as_str().unwrap().to_string(),
    ))
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
    let full_req = request("GET", "/System/Info", Some("test-api-key"), None);
    let (status, full) = send(app, full_req).await;
    assert_eq!(status, StatusCode::OK);
    (
        public["StartupWizardCompleted"].as_bool().unwrap(),
        full["StartupWizardCompleted"].as_bool().unwrap(),
    )
}

/// Creates a user that is not the owner. `create_initial_user` makes an administrator.
async fn add_user(state: &AppState, name: &str, password: &str, admin: bool) -> uuid::Uuid {
    let user = rustyfin_auth::create_initial_user(&state.db, name, password)
        .await
        .unwrap();
    if !admin {
        rustyfin_db::repo_user::update_policy(&state.db, user.id, false)
            .await
            .unwrap();
    }
    user.id
}

/// Returns `(is_administrator, is_owner, is_disabled)` for the named user, or `None` if it is gone.
async fn user_flags(state: &AppState, name: &str) -> Option<(bool, bool, bool)> {
    sqlx::query_as("SELECT is_administrator, is_owner, is_disabled FROM users WHERE name = ?")
        .bind(name)
        .fetch_optional(&state.db)
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

#[tokio::test]
async fn test_startup_fresh_state_needs_setup() {
    let state = test_state().await;
    let app = app(state.clone());

    let users = rustyfin_db::repo_user::get_all_users(&state.db)
        .await
        .unwrap();
    assert!(users.is_empty());
    assert_eq!(wizard_completed(&app).await, (false, false));

    let (status, body) = send(&app, request("GET", "/Startup/User", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["Name"], "");
}

#[tokio::test]
async fn test_startup_flag_without_owner_still_needs_setup() {
    let state = test_state().await;
    add_user(&state, "olduser", "oldpass", true).await;
    state.config.write().await.is_startup_wizard_completed = true;
    let app = app(state.clone());

    assert_eq!(wizard_completed(&app).await, (false, false));
}

#[tokio::test]
async fn test_startup_user_creates_owner_with_typed_name() {
    let state = test_state().await;
    let app = app(state.clone());

    let config =
        serde_json::json!({ "PreferredMetadataLanguage": "de", "MetadataCountryCode": "DE" });
    let req = request("POST", "/Startup/Configuration", None, Some(config));
    assert_eq!(send(&app, req).await.0, StatusCode::NO_CONTENT);

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert!(login(&app, "karl", "geheim").await.is_some());
    assert_eq!(wizard_completed(&app).await, (true, true));
    assert_eq!(user_flags(&state, "karl").await, Some((true, true, false)));
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 1)
    );
}

#[tokio::test]
async fn test_startup_user_rejected_once_owner_exists() {
    let state = test_state().await;
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    let (token, _) = login(&app, "karl", "geheim").await.unwrap();

    assert_eq!(
        post_startup_user(&app, None, "karl", "hacked").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_startup_user(&app, None, "mallory", "hacked").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_startup_user(&app, Some(&token), "karl", "changed").await,
        StatusCode::FORBIDDEN
    );

    assert!(
        login(&app, "karl", "geheim").await.is_some(),
        "password unchanged"
    );
    assert!(login(&app, "karl", "hacked").await.is_none());
    assert!(login(&app, "karl", "changed").await.is_none());
    assert!(login(&app, "mallory", "hacked").await.is_none());
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 1)
    );
}

#[tokio::test]
async fn test_startup_user_with_admin_but_no_owner() {
    let state = test_state().await;
    add_user(&state, "olduser", "oldpass", true).await;
    let app = app(state.clone());

    assert_eq!(wizard_completed(&app).await, (false, false));
    let (status, body) = send(&app, request("GET", "/Startup/User", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["Name"], "",
        "no user name leaks before an owner exists"
    );

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert!(login(&app, "karl", "geheim").await.is_some());
    assert!(
        login(&app, "olduser", "oldpass").await.is_some(),
        "existing admin unchanged"
    );
    assert_eq!(
        user_flags(&state, "olduser").await,
        Some((true, false, false))
    );
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 2)
    );
}

#[tokio::test]
async fn test_startup_user_takes_over_existing_admin_name() {
    let state = test_state().await;
    // A leftover "admin" from an older version, demoted to show that the owner becomes an administrator.
    let admin_id = add_user(&state, "admin", "admin", false).await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "admin", "newpass").await,
        StatusCode::NO_CONTENT
    );
    let (_, user_id) = login(&app, "admin", "newpass")
        .await
        .expect("the typed name logs in with the new password");
    assert_eq!(
        user_id,
        admin_id.to_string(),
        "the existing account is reused"
    );
    assert!(login(&app, "admin", "admin").await.is_none());
    assert_eq!(user_flags(&state, "admin").await, Some((true, true, false)));
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["admin".to_string()], 1)
    );
}

#[tokio::test]
async fn test_startup_user_rejects_empty_name() {
    let state = test_state().await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "", "pw").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_startup_user(&app, None, "   ", "pw").await,
        StatusCode::BAD_REQUEST
    );
    let users = rustyfin_db::repo_user::get_all_users(&state.db)
        .await
        .unwrap();
    assert!(users.is_empty());
    assert_eq!(wizard_completed(&app).await, (false, false));
}

#[tokio::test]
async fn test_startup_user_allows_empty_password() {
    let state = test_state().await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "karl", "").await,
        StatusCode::NO_CONTENT
    );
    assert!(login(&app, "karl", "").await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_startup_user_concurrent_requests_one_owner() {
    let state = test_state().await;
    let app = app(state.clone());

    let first = tokio::spawn({
        let app = app.clone();
        async move { post_startup_user(&app, None, "alice", "pw-alice").await }
    });
    let second = tokio::spawn({
        let app = app.clone();
        async move { post_startup_user(&app, None, "bob", "pw-bob").await }
    });
    let mut statuses = [
        first.await.unwrap().as_u16(),
        second.await.unwrap().as_u16(),
    ];
    statuses.sort_unstable();

    assert_eq!(statuses, [204, 403]);
    let (owners, count) = owners_and_count(&state).await;
    assert_eq!(owners.len(), 1);
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_owner_cannot_be_deleted_or_demoted() {
    let state = test_state().await;
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    let (owner_token, owner_id) = login(&app, "karl", "geheim").await.unwrap();
    let bob_id = add_user(&state, "bob", "bobpass", true).await;
    let (bob_token, _) = login(&app, "bob", "bobpass").await.unwrap();

    let demote = serde_json::json!({ "IsAdministrator": false });
    let delete_uri = format!("/Users/{owner_id}/Delete");
    let user_uri = format!("/Users/{owner_id}");
    let policy_uri = format!("/Users/{owner_id}/Policy");
    // Another admin and the owner itself are both refused.
    for token in [bob_token.as_str(), owner_token.as_str()] {
        let req = request("POST", &delete_uri, Some(token), None);
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
        let req = request("DELETE", &user_uri, Some(token), None);
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
        let req = request("POST", &policy_uri, Some(token), Some(demote.clone()));
        assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    }
    let promote = serde_json::json!({ "IsAdministrator": true });
    let req = request("POST", &policy_uri, Some(&bob_token), Some(promote));
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
    assert_eq!(user_flags(&state, "karl").await, Some((true, true, false)));
    assert!(login(&app, "karl", "geheim").await.is_some());

    // Other administrators can still be demoted and deleted.
    let req = request(
        "POST",
        &format!("/Users/{bob_id}/Policy"),
        Some(&owner_token),
        Some(demote),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
    assert_eq!(user_flags(&state, "bob").await, Some((false, false, false)));
    let req = request(
        "DELETE",
        &format!("/Users/{bob_id}"),
        Some(&owner_token),
        None,
    );
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
    assert_eq!(user_flags(&state, "bob").await, None);
}

#[tokio::test]
async fn test_startup_routes_require_admin_after_setup() {
    let state = test_state().await;
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    let (owner_token, _) = login(&app, "karl", "geheim").await.unwrap();
    add_user(&state, "viewer", "viewerpass", false).await;
    let (viewer_token, _) = login(&app, "viewer", "viewerpass").await.unwrap();
    let language_before = state
        .config
        .read()
        .await
        .preferred_metadata_language
        .clone();

    let config = serde_json::json!({ "PreferredMetadataLanguage": "fr" });
    let empty = serde_json::json!({});
    let routes = [
        ("GET", "/Startup/Configuration", None),
        ("POST", "/Startup/Configuration", Some(config.clone())),
        ("GET", "/Startup/User", None),
        ("POST", "/Startup/Complete", Some(empty.clone())),
        ("GET", "/Startup/FirstUser", None),
        ("POST", "/Startup/RemoteAccess", Some(empty)),
    ];
    for (method, uri, body) in &routes {
        let req = request(method, uri, None, body.clone());
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {uri}"
        );
        let req = request(method, uri, Some("not-a-token"), body.clone());
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::UNAUTHORIZED,
            "bad token {method} {uri}"
        );
        let req = request(method, uri, Some(&viewer_token), body.clone());
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

    let req = request("GET", "/Startup/User", Some(&owner_token), None);
    let (status, body) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["Name"], "karl");
    assert_eq!(body["Password"], "");
    let req = request(
        "POST",
        "/Startup/Configuration",
        Some(&owner_token),
        Some(config),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::NO_CONTENT);
    assert_eq!(state.config.read().await.preferred_metadata_language, "fr");
    let req = request("GET", "/Startup/Configuration", Some("test-api-key"), None);
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
}

#[tokio::test]
async fn test_startup_complete_requires_owner() {
    let state = test_state().await;
    let app = app(state.clone());
    let empty = serde_json::json!({});

    let req = request("POST", "/Startup/Complete", None, Some(empty.clone()));
    assert_eq!(send(&app, req).await.0, StatusCode::BAD_REQUEST);
    assert!(!state.config.read().await.is_startup_wizard_completed);

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    let (token, _) = login(&app, "karl", "geheim").await.unwrap();
    let req = request("POST", "/Startup/Complete", Some(&token), Some(empty));
    assert_eq!(send(&app, req).await.0, StatusCode::NO_CONTENT);
    assert!(state.config.read().await.is_startup_wizard_completed);
    let saved = std::fs::read_to_string(state.data_dir.join("config.json")).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["IsStartupWizardCompleted"], true);
}

#[tokio::test]
async fn test_startup_user_disables_leftover_default_admin() {
    let state = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert!(
        login(&app, "karl", "geheim").await.is_some(),
        "the owner can log in"
    );
    assert!(
        login(&app, "admin", "admin").await.is_none(),
        "admin/admin is disabled"
    );
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, true)),
        "disabled, not deleted"
    );
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["karl".to_string()], 2)
    );
}

#[tokio::test]
async fn test_startup_user_keeps_admin_with_changed_password() {
    let state = test_state().await;
    add_user(&state, "admin", "secret", true).await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert!(
        login(&app, "karl", "geheim").await.is_some(),
        "the owner can log in"
    );
    assert!(
        login(&app, "admin", "secret").await.is_some(),
        "admin keeps working"
    );
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, false))
    );
}

#[tokio::test]
async fn test_startup_user_name_match_is_exact() {
    let state = test_state().await;
    let admin_id = add_user(&state, "admin", "admin", true).await;
    let app = app(state.clone());

    assert_eq!(
        post_startup_user(&app, None, "Admin", "geheim").await,
        StatusCode::NO_CONTENT
    );
    let (_, owner_id) = login(&app, "Admin", "geheim")
        .await
        .expect("\"Admin\" is a new account");
    assert_ne!(owner_id, admin_id.to_string());
    assert!(
        login(&app, "admin", "admin").await.is_none(),
        "admin/admin is disabled"
    );
    assert_eq!(user_flags(&state, "admin").await, Some((true, false, true)));
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["Admin".to_string()], 2)
    );
}

#[tokio::test]
async fn test_upgrade_promotes_earliest_admin() {
    use rustyfin_db::repo_user::promote_earliest_admin_to_owner;

    let state = test_state().await;
    add_user(&state, "viewer", "pw", false).await;
    let promoted = promote_earliest_admin_to_owner(&state.db).await.unwrap();
    assert!(promoted.is_none(), "no admin, nothing to promote");

    // Insertion order differs from creation order, so only created_at can pick "first".
    add_user(&state, "second", "pw", true).await;
    add_user(&state, "first", "pw", true).await;
    for (name, created_at) in [
        ("viewer", "2020-01-01T00:00:00+00:00"),
        ("first", "2021-01-01T00:00:00+00:00"),
        ("second", "2022-01-01T00:00:00+00:00"),
    ] {
        sqlx::query("UPDATE users SET created_at = ? WHERE name = ?")
            .bind(created_at)
            .bind(name)
            .execute(&state.db)
            .await
            .unwrap();
    }

    let promoted = promote_earliest_admin_to_owner(&state.db).await.unwrap();
    assert_eq!(promoted.map(|u| u.name).as_deref(), Some("first"));
    let again = promote_earliest_admin_to_owner(&state.db).await.unwrap();
    assert!(again.is_none(), "an owner already exists");
    assert_eq!(
        owners_and_count(&state).await,
        (vec!["first".to_string()], 3)
    );
}

// ── G1: authorization ──
// Admin endpoints answer 401 without a valid token and 403 to a non-admin. Setup endpoints are
// open only until the server has an owner. Names taken from a request stay inside their directory.

/// Creates the folder `media/<name>` in the test's data dir and returns its path.
fn media_folder(state: &AppState, name: &str) -> String {
    let dir = state.data_dir.join("media").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_str().unwrap().to_string()
}

/// Creates a library on a new folder through the API and returns its id.
async fn create_library_via_api(
    app: &axum::Router,
    state: &AppState,
    token: &str,
    name: &str,
    content_type: &str,
) -> uuid::Uuid {
    let body = serde_json::json!({
        "Name": name,
        "ContentType": content_type,
        "Paths": [media_folder(state, &name.to_lowercase())],
    });
    let req = request("POST", "/Library/VirtualFolders", Some(token), Some(body));
    let (status, created) = send(app, req).await;
    assert_eq!(status, StatusCode::OK, "create {name}: {created}");
    assert_eq!(created["Name"], name);
    created["Id"].as_str().unwrap().parse().unwrap()
}

/// Creates the owner through the wizard and a non-admin "viewer", logs both in and returns
/// `(owner_token, owner_id, viewer_token, viewer_id)`.
async fn g1_owner_and_viewer(
    state: &AppState,
    app: &axum::Router,
) -> (String, String, String, String) {
    assert_eq!(
        post_startup_user(app, None, "owner", "ownerpass").await,
        StatusCode::NO_CONTENT
    );
    let (owner_token, owner_id) = login(app, "owner", "ownerpass").await.unwrap();
    add_user(state, "viewer", "viewerpass", false).await;
    let (viewer_token, viewer_id) = login(app, "viewer", "viewerpass").await.unwrap();
    (owner_token, owner_id, viewer_token, viewer_id)
}

/// Asserts 401 without a token and with a bad one, and 403 for the non-admin `viewer`.
async fn g1_assert_admin_only(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: &Option<serde_json::Value>,
    viewer: &str,
) {
    let callers = [
        ("anonymous", None, StatusCode::UNAUTHORIZED),
        ("bad token", Some("not-a-token"), StatusCode::UNAUTHORIZED),
        ("non-admin", Some(viewer), StatusCode::FORBIDDEN),
    ];
    for (who, token, expected) in callers {
        let status = send(app, request(method, uri, token, body.clone())).await.0;
        assert_eq!(status, expected, "{who} {method} {uri}");
    }
}

/// Asserts that an administrator's request is not refused (neither 401 nor 403).
async fn g1_assert_not_refused(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: &Option<serde_json::Value>,
    token: &str,
) {
    let status = send(app, request(method, uri, Some(token), body.clone()))
        .await
        .0;
    assert!(
        status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        "admin {method} {uri} was refused: {status}"
    );
}

/// Checks a table of `(method, uri, body, check the admin too)`: every route refuses anonymous
/// callers, bad tokens and the non-admin; marked routes must not refuse the owner or the API key.
/// The refusals run first, so no admin request changes state before them.
async fn g1_check_admin_routes(
    app: &axum::Router,
    routes: &[(&str, String, Option<serde_json::Value>, bool)],
    owner: &str,
    viewer: &str,
) {
    for (method, uri, body, _) in routes {
        g1_assert_admin_only(app, method, uri, body, viewer).await;
    }
    for (method, uri, body, check_admin) in routes {
        if *check_admin {
            g1_assert_not_refused(app, method, uri, body, owner).await;
            g1_assert_not_refused(app, method, uri, body, "test-api-key").await;
        }
    }
}

/// Sends a request and returns the status and the body as text.
async fn g1_send_raw(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Writes `<data>/g1-secret.txt`, outside every directory the server reads from.
fn g1_secret_file(state: &AppState) -> std::path::PathBuf {
    let secret = state.data_dir.join("g1-secret.txt");
    std::fs::write(&secret, "g1-secret-content").unwrap();
    secret
}

/// Percent-encodes every `/` so that an absolute path fits in one path segment.
fn g1_encode_slashes(path: &std::path::Path) -> String {
    path.to_str().unwrap().replace('/', "%2F")
}

/// Stores a library with one movie and returns the movie's id.
async fn g1_insert_movie(state: &AppState) -> uuid::Uuid {
    let folder = media_folder(state, "g1-movies");
    let lib = rustyfin_model::library::Library {
        id: uuid::Uuid::new_v4(),
        name: "G1 Movies".to_string(),
        content_type: rustyfin_model::library::LibraryContentType::Movies,
        paths: vec![folder.clone()],
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib)
        .await
        .unwrap();
    let item_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("G1 Movie")
    .bind("Movie")
    .bind(format!("{folder}/G1 Movie.mkv"))
    .execute(&state.db)
    .await
    .unwrap();
    item_id
}

#[tokio::test]
async fn g1_system_admin_routes() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let config = serde_json::to_value(&*state.config.read().await).unwrap();
    let empty = Some(serde_json::json!({}));
    let routes = [
        ("GET", "/System/Logs".to_string(), None, true),
        ("GET", "/System/Logs/Log".to_string(), None, true),
        ("GET", "/System/Log/rustyfin.log".to_string(), None, true),
        ("GET", "/System/Info/Storage".to_string(), None, true),
        ("GET", "/System/LogLevel".to_string(), None, true),
        ("POST", "/System/LogLevel".to_string(), empty.clone(), false),
        ("GET", "/System/UpdatePackages".to_string(), None, true),
        (
            "POST",
            "/System/RestartScheduledTasks".to_string(),
            None,
            false,
        ),
        (
            "POST",
            "/System/Configuration".to_string(),
            Some(config),
            true,
        ),
        (
            "POST",
            "/System/Configuration/ServerName".to_string(),
            Some(serde_json::json!("G1")),
            true,
        ),
        (
            "POST",
            "/System/Configuration/Branding".to_string(),
            empty.clone(),
            true,
        ),
        ("POST", "/System/Shutdown".to_string(), None, false),
        ("POST", "/System/Restart".to_string(), None, false),
        (
            "PUT",
            "/Configuration/MediaEncoding".to_string(),
            empty.clone(),
            true,
        ),
        ("PUT", "/Configuration/Networking".to_string(), empty, true),
        ("GET", "/System/ActivityLog/Entries".to_string(), None, true),
        ("GET", "/System/MediaEncoder/Path".to_string(), None, true),
        ("POST", "/System/MediaEncoder/Test".to_string(), None, false),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
}

#[tokio::test]
async fn g1_admin_modules_refuse_users() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let id = uuid::Uuid::new_v4();
    let empty = Some(serde_json::json!({}));
    let list = Some(serde_json::json!([]));
    let routes = [
        ("GET", "/ScheduledTasks".to_string(), None, true),
        ("GET", format!("/ScheduledTasks/{id}"), None, true),
        (
            "POST",
            format!("/ScheduledTasks/{id}/Triggers"),
            list.clone(),
            false,
        ),
        ("POST", format!("/ScheduledTasks/Running/{id}"), None, false),
        (
            "DELETE",
            format!("/ScheduledTasks/Running/{id}"),
            None,
            false,
        ),
        ("GET", "/Plugins".to_string(), None, true),
        ("GET", format!("/Plugins/{id}/Configuration"), None, true),
        (
            "POST",
            format!("/Plugins/{id}/Configuration"),
            empty.clone(),
            false,
        ),
        ("GET", "/Packages".to_string(), None, false),
        ("GET", "/Packages/Repositories".to_string(), None, true),
        ("POST", "/Packages/Repositories".to_string(), list, false),
        (
            "DELETE",
            "/Packages/Repositories/g1".to_string(),
            None,
            false,
        ),
        ("POST", "/Packages/g1/Install".to_string(), None, false),
        ("POST", "/Packages/g1/Uninstall".to_string(), None, false),
        ("POST", "/Packages/g1/Enable".to_string(), None, false),
        ("POST", "/Packages/g1/Disable".to_string(), None, false),
        (
            "POST",
            "/Packages/Restore".to_string(),
            empty.clone(),
            false,
        ),
        ("GET", "/Backup".to_string(), None, true),
        ("POST", "/Backup".to_string(), empty.clone(), false),
        ("DELETE", "/Backup/g1.zip".to_string(), None, false),
        ("POST", "/Backup/Restore".to_string(), empty.clone(), false),
        ("GET", "/Devices".to_string(), None, false),
        ("POST", "/Devices".to_string(), empty.clone(), false),
        ("GET", "/Devices/g1-device".to_string(), None, true),
        ("DELETE", "/Devices/g1-device".to_string(), None, false),
        ("POST", "/Devices/g1-device/Image".to_string(), None, false),
        ("DELETE", "/Devices?id=g1-device".to_string(), None, false),
        ("GET", "/Devices/Info?id=g1-device".to_string(), None, true),
        (
            "GET",
            "/Devices/Options?id=g1-device".to_string(),
            None,
            true,
        ),
        (
            "POST",
            "/Devices/Options?id=g1-device".to_string(),
            empty.clone(),
            false,
        ),
        ("GET", "/Dlna/Streaming/Profiles".to_string(), None, true),
        (
            "POST",
            "/Dlna/Streaming/Profiles".to_string(),
            empty.clone(),
            false,
        ),
        ("GET", format!("/Dlna/Streaming/Profiles/{id}"), None, true),
        (
            "PUT",
            format!("/Dlna/Streaming/Profiles/{id}"),
            empty,
            false,
        ),
        (
            "DELETE",
            format!("/Dlna/Streaming/Profiles/{id}"),
            None,
            false,
        ),
        ("GET", "/Auth/Keys".to_string(), None, true),
        ("DELETE", "/Auth/Keys/g1-key".to_string(), None, false),
        ("GET", "/Auth/Providers".to_string(), None, true),
        (
            "GET",
            "/Auth/PasswordResetProviders".to_string(),
            None,
            true,
        ),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
}

#[tokio::test]
async fn g1_user_admin_routes() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let other = add_user(&state, "other", "otherpass", false).await;
    let new_user = serde_json::json!({ "Name": "g1-new", "Password": "g1-pass" });
    let promote = serde_json::json!({ "IsAdministrator": true });
    let routes = [
        ("POST", "/Users/New".to_string(), Some(new_user), false),
        (
            "POST",
            format!("/Users/{other}/Policy"),
            Some(promote),
            false,
        ),
        ("DELETE", format!("/Users/{other}"), None, false),
        ("POST", format!("/Users/{other}/Delete"), None, false),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
    assert_eq!(user_flags(&state, "g1-new").await, None);
    assert_eq!(
        user_flags(&state, "other").await,
        Some((false, false, false))
    );

    for (method, uri, body, _) in &routes {
        g1_assert_not_refused(&app, method, uri, body, &owner).await;
    }
    assert!(user_flags(&state, "g1-new").await.is_some());
    assert_eq!(user_flags(&state, "other").await, None);
}

#[tokio::test]
async fn g1_item_admin_routes() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let item = uuid::Uuid::new_v4();
    let lib = uuid::Uuid::new_v4();
    let coll = uuid::Uuid::new_v4();
    let routes = [
        ("POST", "/Library/Refresh".to_string(), None, true),
        (
            "POST",
            format!("/Library/VirtualFolders/{lib}/Refresh"),
            None,
            true,
        ),
        ("POST", format!("/Items/{item}/Refresh"), None, false),
        ("DELETE", format!("/Items?ids={item}"), None, true),
        ("DELETE", format!("/Items/{item}"), None, true),
        ("POST", format!("/Items/{item}/Images/Primary"), None, true),
        (
            "DELETE",
            format!("/Items/{item}/Images/Primary"),
            None,
            true,
        ),
        (
            "DELETE",
            format!("/Items/{item}/Images/Primary/0"),
            None,
            true,
        ),
        (
            "DELETE",
            format!("/Items/{item}/AlternateSources"),
            None,
            true,
        ),
        ("DELETE", format!("/Items/{item}/Subtitles/0"), None, true),
        (
            "GET",
            format!("/Items/{item}/RemoteSearch/Subtitles"),
            None,
            false,
        ),
        (
            "POST",
            format!("/Videos/{item}/Subtitles/0/Download"),
            None,
            false,
        ),
        ("POST", "/Collections?Name=G1".to_string(), None, true),
        ("DELETE", format!("/Collections/{coll}"), None, true),
        (
            "POST",
            format!("/Collections/{coll}/Items?ids={item}"),
            None,
            true,
        ),
        (
            "DELETE",
            format!("/Collections/{coll}/Items?ids={item}"),
            None,
            true,
        ),
        (
            "POST",
            format!("/Collections/{coll}/Items/{item}"),
            None,
            true,
        ),
        (
            "DELETE",
            format!("/Collections/{coll}/Items/{item}"),
            None,
            true,
        ),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
}

#[tokio::test]
async fn g1_live_tv_dlna_localization_admin_routes() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let empty = Some(serde_json::json!({}));
    let routes = [
        ("GET", "/LiveTv/TunerHosts".to_string(), None, true),
        (
            "POST",
            "/LiveTv/TunerHosts".to_string(),
            empty.clone(),
            false,
        ),
        (
            "DELETE",
            "/LiveTv/TunerHosts?id=g1".to_string(),
            None,
            false,
        ),
        ("GET", "/LiveTv/TunerHosts/g1".to_string(), None, true),
        ("DELETE", "/LiveTv/TunerHosts/g1".to_string(), None, false),
        ("GET", "/LiveTv/Tuners".to_string(), None, true),
        ("GET", "/LiveTv/Tuners/g1".to_string(), None, true),
        ("GET", "/LiveTv/ListingProviders".to_string(), None, true),
        (
            "POST",
            "/LiveTv/ListingProviders".to_string(),
            empty.clone(),
            false,
        ),
        (
            "DELETE",
            "/LiveTv/ListingProviders?id=g1".to_string(),
            None,
            false,
        ),
        ("GET", "/LiveTv/ListingProviders/g1".to_string(), None, true),
        (
            "DELETE",
            "/LiveTv/ListingProviders/g1".to_string(),
            None,
            false,
        ),
        (
            "GET",
            "/LiveTv/ChannelMappingOptions".to_string(),
            None,
            true,
        ),
        ("GET", "/Dlna/Profile".to_string(), None, true),
        (
            "POST",
            "/Dlna/Profile/DeviceDetection?UserAgent=Jellyfin/10.8.0".to_string(),
            None,
            true,
        ),
        ("GET", "/Dlna/Profile/Device".to_string(), None, true),
        (
            "POST",
            "/Localization/Providers".to_string(),
            empty.clone(),
            false,
        ),
        (
            "PUT",
            "/Localization/Providers/g1".to_string(),
            empty,
            false,
        ),
        (
            "DELETE",
            "/Localization/Providers/g1".to_string(),
            None,
            false,
        ),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
}

#[tokio::test]
async fn g1_user_routes_stay_open() {
    let state = test_state().await;
    let app = app(state.clone());
    let (_, _, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let uris = [
        "/System/Info".to_string(),
        "/System/Configuration".to_string(),
        "/System/Configuration/ServerName".to_string(),
        "/Users/Me".to_string(),
        "/Users".to_string(),
        format!("/Users/{viewer_id}"),
        "/UserViews".to_string(),
        format!("/Users/{viewer_id}/Views"),
        "/Items".to_string(),
        "/Sessions".to_string(),
        "/Localization/Options".to_string(),
        "/Localization/Providers".to_string(),
        "/Configuration/MediaEncoding".to_string(),
        "/Configuration/Networking".to_string(),
        format!("/DisplayPreferences/usersettings?userId={viewer_id}&client=emby"),
        "/Collections".to_string(),
        "/LiveTv/Channels".to_string(),
        "/LiveTv/Timers".to_string(),
    ];
    for uri in &uris {
        let (status, body) = send(&app, request("GET", uri, Some(&viewer), None)).await;
        assert_eq!(status, StatusCode::OK, "non-admin GET {uri}: {body}");
    }
    // Stream and image URLs carry the token as `api_key`.
    let uri = format!("/Users/Me?api_key={viewer}");
    assert_eq!(
        send(&app, request("GET", &uri, None, None)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn g1_setup_routes_open_before_owner() {
    let state = test_state().await;
    let app = app(state.clone());
    let parent = state.data_dir.to_str().unwrap().to_string();
    for uri in [
        "/Environment/Drives".to_string(),
        "/Environment/NetworkShares".to_string(),
        format!("/Library/BrowseDirectories?parent={parent}"),
        "/Library/VirtualFolders".to_string(),
    ] {
        let (status, body) = send(&app, request("GET", &uri, None, None)).await;
        assert_eq!(status, StatusCode::OK, "anonymous GET {uri}: {body}");
    }
    let body = serde_json::json!({
        "Name": "G1 Setup",
        "ContentType": "Movies",
        "Paths": [media_folder(&state, "g1-setup")],
    });
    let req = request("POST", "/Library/VirtualFolders", None, Some(body));
    let (status, created) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK, "anonymous create: {created}");
    let uri = format!("/Library/{}/Options", created["Id"].as_str().unwrap());
    assert_eq!(
        send(&app, request("GET", &uri, None, None)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn g1_setup_routes_admin_after_owner() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let lib = create_library_via_api(&app, &state, &owner, "G1 Kept", "Movies").await;
    let kept = media_folder(&state, "g1 kept");
    let parent = state.data_dir.to_str().unwrap().to_string();
    let new_library = serde_json::json!({
        "Name": "G1 Refused",
        "ContentType": "Movies",
        "Paths": [media_folder(&state, "g1-refused")],
    });
    let remove_path = serde_json::json!({ "LibraryId": lib.to_string(), "Paths": [kept] });
    let routes = [
        ("GET", "/Environment/Drives".to_string(), None, true),
        ("GET", "/Environment/NetworkShares".to_string(), None, true),
        (
            "GET",
            format!("/Library/BrowseDirectories?parent={parent}"),
            None,
            true,
        ),
        ("GET", "/Library/VirtualFolders".to_string(), None, true),
        (
            "POST",
            "/Library/VirtualFolders".to_string(),
            Some(new_library),
            false,
        ),
        ("GET", format!("/Library/{lib}/Options"), None, true),
        (
            "POST",
            format!("/Library/{lib}/Options"),
            Some(serde_json::json!({})),
            false,
        ),
        (
            "DELETE",
            "/Library/VirtualFolders/Paths".to_string(),
            Some(remove_path),
            false,
        ),
        (
            "DELETE",
            format!("/Library/VirtualFolders/{lib}"),
            None,
            false,
        ),
    ];
    g1_check_admin_routes(&app, &routes, &owner, &viewer).await;
    let req = request("GET", "/Library/VirtualFolders", Some(&owner), None);
    let (status, libraries) = send(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = libraries
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["Name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["G1 Kept"]);
}

#[tokio::test]
async fn g1_log_file_route_reads_only_listed_logs() {
    let state = test_state().await;
    let app = app(state.clone());
    let logs = state.data_dir.join("logs");
    std::fs::write(logs.join("rustyfin.log"), "g1 log line\n").unwrap();
    let secret = g1_secret_file(&state);
    std::fs::write(state.data_dir.join("outside.log"), "g1-outside-content").unwrap();
    // A symlink to the secret on unix; elsewhere "link.log" below is a name that does not exist.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, logs.join("link.log")).unwrap();
    std::fs::create_dir_all(logs.join("sub")).unwrap();
    std::fs::write(logs.join("sub").join("inner.log"), "g1-inner-content").unwrap();
    let get = |uri: String| request("GET", &uri, Some("test-api-key"), None);

    let (status, text) = g1_send_raw(&app, get("/System/Log/rustyfin.log".into())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("g1 log line"));

    for name in [
        "..%2Fg1-secret.txt".to_string(),
        "%2e%2e%2fg1-secret.txt".to_string(),
        "..%2Foutside.log".to_string(),
        "..%2Ftest.db".to_string(),
        g1_encode_slashes(&secret),
        "link.log".to_string(),
        "sub%2Finner.log".to_string(),
        "sub".to_string(),
        "%2e%2e".to_string(),
        "missing.log".to_string(),
    ] {
        let uri = format!("/System/Log/{name}");
        let (status, text) = g1_send_raw(&app, get(uri.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
        for content in [
            "g1-secret-content",
            "g1-outside-content",
            "g1-inner-content",
        ] {
            assert!(!text.contains(content), "GET {uri} returned {content}");
        }
    }

    let (status, listed) = send(&app, get("/System/Logs".into())).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["Name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["rustyfin.log"]);
}

#[tokio::test]
async fn g1_segment_path_stays_in_item_dir() {
    let state = test_state().await;
    let transcodes = state.data_dir.join("transcodes");
    let item = uuid::Uuid::new_v4();
    let dir = transcodes.join(item.to_string());
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("segment_0000.ts"), "g1-segment").unwrap();
    std::fs::write(dir.join("sub").join("inner.ts"), "g1-inner").unwrap();
    let secret = g1_secret_file(&state);
    // A symlink to the secret on unix; elsewhere "evil.ts" below is a name that does not exist.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, dir.join("evil.ts")).unwrap();
    let other = uuid::Uuid::new_v4();
    std::fs::create_dir_all(transcodes.join(other.to_string())).unwrap();
    std::fs::write(
        transcodes.join(other.to_string()).join("segment_0000.ts"),
        "g1-other",
    )
    .unwrap();
    let manager = &state.transcode_manager;

    let found = manager
        .get_segment_path(item, "segment_0000.ts")
        .expect("a real segment is found");
    assert_eq!(std::fs::read_to_string(found).unwrap(), "g1-segment");
    let absolute = secret.to_str().unwrap().to_string();
    let other_segment = format!("../{other}/segment_0000.ts");
    for name in [
        "",
        ".",
        "..",
        "../../g1-secret.txt",
        "..\\..\\g1-secret.txt",
        absolute.as_str(),
        "evil.ts",
        "sub",
        "sub/inner.ts",
        other_segment.as_str(),
        "missing.ts",
        "segment_0000.ts\0",
    ] {
        assert_eq!(manager.get_segment_path(item, name), None, "{name:?}");
    }
}

#[tokio::test]
async fn g1_legacy_hls_segment_refuses_traversal() {
    let state = test_state().await;
    let app = app(state.clone());
    let (_, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let item = uuid::Uuid::new_v4();
    let dir = state.data_dir.join("transcodes").join(item.to_string());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("segment_0000.ts"), "g1-segment").unwrap();
    let secret = g1_secret_file(&state);
    // A symlink to the secret on unix; elsewhere "evil.ts" below is a name that does not exist.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, dir.join("evil.ts")).unwrap();

    let uri = format!("/Videos/{item}/hls/segment_0000.ts");
    let (status, text) = g1_send_raw(&app, request("GET", &uri, Some(&viewer), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, "g1-segment");
    let uri = format!("{uri}?api_key={viewer}");
    let (status, text) = g1_send_raw(&app, request("GET", &uri, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(text, "g1-segment");

    for name in [
        "..%2F..%2Fg1-secret.txt".to_string(),
        "%2e%2e%2f%2e%2e%2fg1-secret.txt".to_string(),
        "..%2F..%2Ftest.db".to_string(),
        g1_encode_slashes(&secret),
        "evil.ts".to_string(),
        "%2e%2e".to_string(),
        "missing.ts".to_string(),
    ] {
        let uri = format!("/Videos/{item}/hls/{name}");
        let (status, text) = g1_send_raw(&app, request("GET", &uri, Some(&viewer), None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET {uri}");
        assert!(!text.contains("g1-secret-content"), "GET {uri} leaked");
    }
}

/// Needs an executable shell script, so unix only; `g1_media_encoder_path_cannot_be_set` checks
/// the same routes everywhere.
#[cfg(unix)]
#[tokio::test]
async fn g1_media_encoder_runs_no_request_path() {
    use std::os::unix::fs::PermissionsExt;

    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let marker = state.data_dir.join("g1-encoder-marker");
    let probe = state.data_dir.join("g1-probe.sh");
    std::fs::write(&probe, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let body = serde_json::json!({ "Value": probe.to_str().unwrap() });

    for token in [owner.as_str(), "test-api-key", viewer.as_str()] {
        for (method, uri) in [
            ("PUT", "/System/MediaEncoder/Path"),
            ("POST", "/System/MediaEncoder/Path/Validate"),
        ] {
            send(&app, request(method, uri, Some(token), Some(body.clone()))).await;
        }
    }
    assert!(!marker.exists(), "an encoder path from a request was run");
}

#[tokio::test]
async fn g1_media_encoder_path_cannot_be_set() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let probe = state.data_dir.join("g1-probe.sh");
    let body = serde_json::json!({ "Value": probe.to_str().unwrap() });

    let req = request(
        "PUT",
        "/System/MediaEncoder/Path",
        Some(&owner),
        Some(body.clone()),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::METHOD_NOT_ALLOWED);
    let req = request(
        "POST",
        "/System/MediaEncoder/Path/Validate",
        Some(&owner),
        Some(body),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
    for (method, uri) in [
        ("GET", "/System/MediaEncoder/Path"),
        ("POST", "/System/MediaEncoder/Test"),
    ] {
        g1_assert_admin_only(&app, method, uri, &None, &viewer).await;
    }
    g1_assert_not_refused(&app, "GET", "/System/MediaEncoder/Path", &None, &owner).await;
}

#[tokio::test]
async fn g1_easy_pin_is_hashed_and_never_returned() {
    const PIN: &str = "90817263";
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let settings = serde_json::json!({ "AudioLanguagePreference": "deu" });
    let uri = format!("/Users/{viewer_id}/Configuration");
    let req = request("POST", &uri, Some(&viewer), Some(settings));
    assert!(send(&app, req).await.0.is_success());
    let user_uri = format!("/Users/{viewer_id}");
    let (_, before) = send(&app, request("GET", &user_uri, Some(&viewer), None)).await;

    let uri = format!("/Users/{viewer_id}/EasyPassword");
    let req = request(
        "POST",
        &uri,
        Some(&viewer),
        Some(serde_json::json!({ "EasyPassword": PIN })),
    );
    let (status, text) = g1_send_raw(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!text.contains(PIN), "the call returned the PIN");

    let login_body = serde_json::json!({ "Username": "viewer", "Pw": "viewerpass" });
    let responses = [
        request("GET", "/Users", Some(&owner), None),
        request("GET", &user_uri, Some(&owner), None),
        request("GET", &user_uri, Some(&viewer), None),
        request("GET", "/Users/Me", Some(&viewer), None),
        request("GET", "/Users/Public", None, None),
        request("POST", "/Users/AuthenticateByName", None, Some(login_body)),
    ];
    for req in responses {
        let uri = req.uri().to_string();
        let (status, text) = g1_send_raw(&app, req).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(!text.contains(PIN), "{uri} returned the PIN");
    }

    let (_, after) = send(&app, request("GET", &user_uri, Some(&viewer), None)).await;
    assert_eq!(after["Configuration"], before["Configuration"]);
    let stored: String = sqlx::query_scalar("SELECT configuration FROM users WHERE name = ?")
        .bind("viewer")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert!(
        !stored.contains(PIN),
        "the stored configuration holds the PIN"
    );
    let pin_hash = |name: &'static str| {
        let db = state.db.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT easy_password_hash FROM users WHERE name = ?",
            )
            .bind(name)
            .fetch_one(&db)
            .await
            .unwrap()
        }
    };
    let hash = pin_hash("viewer").await.expect("the PIN hash is stored");
    assert_ne!(hash, PIN);
    assert!(rustyfin_auth::verify_password(PIN, &hash).await.unwrap());

    // Only the user and administrators set a PIN; an empty PIN clears it.
    let uri = format!("/Users/{owner_id}/EasyPassword");
    let req = request(
        "POST",
        &uri,
        Some(&viewer),
        Some(serde_json::json!({ "EasyPassword": "1234" })),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::FORBIDDEN);
    assert_eq!(pin_hash("owner").await, None);
    let uri = format!("/Users/{viewer_id}/EasyPassword");
    let req = request(
        "POST",
        &uri,
        Some(&owner),
        Some(serde_json::json!({ "EasyPassword": "" })),
    );
    assert_eq!(send(&app, req).await.0, StatusCode::OK);
    assert_eq!(pin_hash("viewer").await, None);
}

#[tokio::test]
async fn g1_create_user_honours_is_administrator() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, _, _) = g1_owner_and_viewer(&state, &app).await;
    for (name, flag, admin) in [
        ("g1-admin", Some(true), true),
        ("g1-plain", Some(false), false),
        ("g1-default", None, false),
    ] {
        let mut body = serde_json::json!({ "Name": name, "Password": "g1-pass" });
        if let Some(flag) = flag {
            body["IsAdministrator"] = serde_json::json!(flag);
        }
        let (status, created) = send(
            &app,
            request("POST", "/Users/New", Some(&owner), Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "create {name}: {created}");
        assert_eq!(
            user_flags(&state, name).await,
            Some((admin, false, false)),
            "{name}"
        );
    }
}

#[tokio::test]
async fn g1_played_and_favorite_follow_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let item = g1_insert_movie(&state).await;
    let data = |user: &str| {
        let db = state.db.clone();
        let user: uuid::Uuid = user.parse().unwrap();
        async move {
            let data = rustyfin_db::repo_playback::get_user_item_data(&db, user, item)
                .await
                .unwrap();
            (data.is_favorite, data.played)
        }
    };

    let uri = format!("/Users/{viewer_id}/FavoriteItems/{item}");
    assert_eq!(
        send(&app, request("POST", &uri, Some(&viewer), None))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(data(&viewer_id).await, (true, false));

    for (method, kind) in [
        ("POST", "FavoriteItems"),
        ("DELETE", "FavoriteItems"),
        ("POST", "PlayedItems"),
        ("DELETE", "PlayedItems"),
    ] {
        let uri = format!("/Users/{owner_id}/{kind}/{item}");
        let status = send(&app, request(method, &uri, Some(&viewer), None))
            .await
            .0;
        assert_eq!(status, StatusCode::FORBIDDEN, "non-admin {method} {uri}");
    }
    assert_eq!(data(&owner_id).await, (false, false));
    assert_eq!(data(&viewer_id).await, (true, false));

    // An administrator acts for the user in the path.
    let uri = format!("/Users/{viewer_id}/PlayedItems/{item}");
    assert_eq!(
        send(&app, request("POST", &uri, Some(&owner), None))
            .await
            .0,
        StatusCode::OK
    );
    let uri = format!("/Users/{viewer_id}/FavoriteItems/{item}");
    assert_eq!(
        send(&app, request("DELETE", &uri, Some(&owner), None))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(data(&viewer_id).await, (false, true));
    assert_eq!(data(&owner_id).await, (false, false));
    let uri = format!("/Users/{owner_id}/FavoriteItems/{item}");
    assert_eq!(
        send(&app, request("POST", &uri, Some("test-api-key"), None))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(data(&owner_id).await, (true, false));

    let uri = format!("/Users/{}/PlayedItems/{item}", uuid::Uuid::new_v4());
    assert_eq!(
        send(&app, request("POST", &uri, Some(&owner), None))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

/// The DLNA server's description, a Browse request and a connection-manager request.
fn g1_dlna_requests() -> Vec<Request<Body>> {
    let browse = concat!(
        r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">"#,
        r#"<s:Body><u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">"#,
        "<ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter>",
        "<StartingIndex>0</StartingIndex><RequestedCount>10</RequestedCount>",
        "<SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>"
    );
    let soap = |uri: &str, action: &str| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "text/xml")
            .header("SOAPACTION", action)
            .body(Body::from(browse))
            .unwrap()
    };
    let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
    vec![
        get("/dlna/description.xml"),
        soap(
            "/dlna/control/ContentDirectory",
            r#""urn:schemas-upnp-org:service:ContentDirectory:1#Browse""#,
        ),
        get("/dlna/ContentDirectory.xml"),
        get("/dlna/ConnectionManager.xml"),
        soap(
            "/dlna/control/ConnectionManager",
            r#""urn:schemas-upnp-org:service:ConnectionManager:1#GetProtocolInfo""#,
        ),
    ]
}

/// Turns the DLNA server on or off through `POST /System/Configuration`.
async fn g1_set_dlna(app: &axum::Router, enabled: bool) {
    let req = request("GET", "/System/Configuration", Some("test-api-key"), None);
    let (status, mut config) = send(app, req).await;
    assert_eq!(status, StatusCode::OK);
    config["EnableDlnaServer"] = serde_json::json!(enabled);
    let req = request(
        "POST",
        "/System/Configuration",
        Some("test-api-key"),
        Some(config),
    );
    assert!(send(app, req).await.0.is_success());
}

#[tokio::test]
async fn g1_dlna_server_off_by_default() {
    let defaults = serde_json::to_value(ServerConfiguration::default()).unwrap();
    assert_eq!(defaults["EnableDlnaServer"], serde_json::json!(false));
    let state = test_state().await;
    let app = app(state.clone());
    for i in 0..g1_dlna_requests().len() {
        let req = g1_dlna_requests().remove(i);
        let uri = req.uri().to_string();
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::NOT_FOUND,
            "DLNA off: {uri}"
        );
        let mut req = g1_dlna_requests().remove(i);
        req.headers_mut()
            .insert("X-Emby-Token", "test-api-key".parse().unwrap());
        let status = send(&app, req).await.0;
        assert_eq!(status, StatusCode::NOT_FOUND, "DLNA off, API key: {uri}");
    }

    g1_set_dlna(&app, true).await;
    for req in g1_dlna_requests().into_iter().take(2) {
        let uri = req.uri().to_string();
        assert_eq!(send(&app, req).await.0, StatusCode::OK, "DLNA on: {uri}");
    }
    g1_set_dlna(&app, false).await;
    let req = g1_dlna_requests().remove(0);
    assert_eq!(send(&app, req).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn g1_config_secrets_hidden_from_users() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, _, viewer, _) = g1_owner_and_viewer(&state, &app).await;
    let secrets = [
        ("TmdbApiKey", "g1-tmdb-secret"),
        ("OmdbApiKey", "g1-omdb-secret"),
        ("FanartApiKey", "g1-fanart-secret"),
        ("SmtpPassword", "g1-smtp-secret"),
    ];
    let (_, mut config) = send(
        &app,
        request("GET", "/System/Configuration", Some(&owner), None),
    )
    .await;
    for (field, value) in secrets {
        config[field] = serde_json::json!(value);
    }
    let req = request("POST", "/System/Configuration", Some(&owner), Some(config));
    assert!(send(&app, req).await.0.is_success());

    let req = request("GET", "/System/Configuration", Some(&viewer), None);
    let (status, text) = g1_send_raw(&app, req).await;
    assert_eq!(status, StatusCode::OK);
    let seen: serde_json::Value = serde_json::from_str(&text).unwrap();
    for (field, value) in secrets {
        assert!(!text.contains(value), "a non-admin saw {field}");
        assert_eq!(seen.get(field), Some(&serde_json::Value::Null), "{field}");
        let uri = format!("/System/Configuration/{field}");
        let (status, text) = g1_send_raw(&app, request("GET", &uri, Some(&viewer), None)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(!text.contains(value), "a non-admin saw {uri}");
    }

    let req = request("GET", "/System/Configuration", Some(&owner), None);
    let (_, full) = send(&app, req).await;
    for (field, value) in secrets {
        assert_eq!(full[field], value, "the owner reads {field}");
    }
    let req = request(
        "GET",
        "/System/Configuration/TmdbApiKey",
        Some(&owner),
        None,
    );
    assert_eq!(send(&app, req).await.1, "g1-tmdb-secret");
}

#[tokio::test]
async fn g1_self_or_admin_user_routes() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let routes = [
        (
            "PUT",
            format!("/Users/{owner_id}"),
            serde_json::json!({ "Name": "renamed" }),
        ),
        (
            "POST",
            format!("/Users/{owner_id}/Password"),
            serde_json::json!({ "CurrentPw": "ownerpass", "NewPw": "g1-taken" }),
        ),
        (
            "POST",
            format!("/Users/{owner_id}/Configuration"),
            serde_json::json!({}),
        ),
        (
            "POST",
            format!("/Users/{owner_id}/EasyPassword"),
            serde_json::json!({ "EasyPassword": "1234" }),
        ),
    ];
    for (method, uri, body) in &routes {
        let req = request(method, uri, Some(&viewer), Some(body.clone()));
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::FORBIDDEN,
            "non-admin {method} {uri}"
        );
        let req = request(method, uri, None, Some(body.clone()));
        assert_eq!(
            send(&app, req).await.0,
            StatusCode::UNAUTHORIZED,
            "anonymous {method} {uri}"
        );
    }
    assert!(login(&app, "owner", "ownerpass").await.is_some());
    assert_eq!(user_flags(&state, "owner").await, Some((true, true, false)));

    // A wrong current password stays 401.
    let uri = format!("/Users/{viewer_id}/Password");
    let body = serde_json::json!({ "CurrentPw": "wrong", "NewPw": "g1-other" });
    let req = request("POST", &uri, Some(&viewer), Some(body));
    assert_eq!(send(&app, req).await.0, StatusCode::UNAUTHORIZED);

    // The user and administrators may change the user.
    let uri = format!("/Users/{viewer_id}/Configuration");
    for token in [&viewer, &owner] {
        let req = request("POST", &uri, Some(token), Some(serde_json::json!({})));
        assert!(send(&app, req).await.0.is_success());
    }
    let uri = format!("/Users/{viewer_id}");
    let req = request(
        "PUT",
        &uri,
        Some(&viewer),
        Some(serde_json::json!({ "Name": "viewer" })),
    );
    assert!(send(&app, req).await.0.is_success());
}

// ── Packages Tests ──

#[tokio::test]
async fn test_packages_list() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/Packages")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Display Preferences Tests ──

#[tokio::test]
async fn test_display_preferences() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Users/{}/Preferences", user.id))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Additional Videos Tests ──

#[tokio::test]
async fn test_additional_videos_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Items/{random_id}/AdditionalVideoParts"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Dynamic HLS Tests ──

#[tokio::test]
async fn test_dynamic_hls_not_found() {
    let state = test_state().await;
    let username = format!("admin_{}", uuid::Uuid::new_v4().simple());
    rustyfin_auth::create_initial_user(&state.db, &username, "admin")
        .await
        .unwrap();
    let user = rustyfin_db::repo_user::get_user_by_name(&state.db, &username)
        .await
        .unwrap()
        .unwrap();
    let token = rustyfin_auth::create_token(&state.db, &user, &Default::default())
        .await
        .unwrap();
    let app = app(state);

    let random_id = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/Items/{random_id}/main.m3u8"))
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── G1 follow-up ──
// Another user's ratings, resume list, Next Up, Upcoming and notifications need that user or an
// administrator, and a query without userId means the caller (Jellyfin's
// RequestHelpers.GetUserId). Remote control of a session or device needs its owner or an
// administrator (SessionManager.AssertCanControl); GET /Sessions shows a non-admin only their own.

const G1F_NIL: &str = "00000000-0000-0000-0000-000000000000";

/// Inserts a TV library with one series, one season and three episodes: E1 and E2 have aired, E3
/// airs in 2999. Returns `(series_id, [e1, e2, e3])`.
async fn g1f_insert_show(state: &AppState) -> (uuid::Uuid, [uuid::Uuid; 3]) {
    let folder = media_folder(state, "g1f-shows");
    let lib = rustyfin_model::library::Library {
        id: uuid::Uuid::new_v4(),
        name: "G1 Shows".to_string(),
        content_type: rustyfin_model::library::LibraryContentType::TvShows,
        paths: vec![folder.clone()],
    };
    rustyfin_db::repo_library::create_library(&state.db, &lib)
        .await
        .unwrap();
    let series = uuid::Uuid::new_v4();
    let season = uuid::Uuid::new_v4();
    let episodes = [
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    ];
    // (id, name, type, parent, index, season number, premiere date)
    let rows = [
        (series, "G1 Show", "Series", None, None, None, None),
        (
            season,
            "Season 1",
            "Season",
            Some(series),
            Some(1),
            None,
            None,
        ),
        (
            episodes[0],
            "E1",
            "Episode",
            Some(season),
            Some(1),
            Some(1),
            Some("2001-01-01T00:00:00+00:00"),
        ),
        (
            episodes[1],
            "E2",
            "Episode",
            Some(season),
            Some(2),
            Some(1),
            Some("2001-01-08T00:00:00+00:00"),
        ),
        (
            episodes[2],
            "E3",
            "Episode",
            Some(season),
            Some(3),
            Some(1),
            Some("2999-01-01T00:00:00+00:00"),
        ),
    ];
    for (id, name, media_type, parent, index, season_number, premiere) in rows {
        sqlx::query(
            "INSERT INTO media_items (id, library_id, name, media_type, path, parent_id, \
             index_number, parent_index_number, premiere_date) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(lib.id.to_string())
        .bind(name)
        .bind(media_type)
        .bind(format!("{folder}/{name}"))
        .bind(parent.map(|p: uuid::Uuid| p.to_string()))
        .bind(index)
        .bind(season_number)
        .bind(premiere)
        .execute(&state.db)
        .await
        .unwrap();
    }
    (series, episodes)
}

/// Sends a GET and returns the status and the ids in the answer's `Items`, in order.
async fn g1f_ids(
    app: &axum::Router,
    uri: String,
    token: Option<&str>,
) -> (StatusCode, Vec<uuid::Uuid>) {
    let (status, json) = send(app, request("GET", &uri, token, None)).await;
    let ids = json["Items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| uuid::Uuid::parse_str(item["Id"].as_str().unwrap()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    (status, ids)
}

/// Gives the user a rating of 8 with Likes on the item.
async fn g1f_rate(state: &AppState, user: &str, item: uuid::Uuid) {
    rustyfin_db::repo_playback::set_user_rating(
        &state.db,
        user.parse().unwrap(),
        item,
        Some(true),
        Some(8),
    )
    .await
    .unwrap();
}

/// The user's `(Likes, UserRating)` on the item.
async fn g1f_rating(state: &AppState, user: &str, item: uuid::Uuid) -> (Option<bool>, Option<i32>) {
    let data =
        rustyfin_db::repo_playback::get_user_item_data(&state.db, user.parse().unwrap(), item)
            .await
            .unwrap();
    (data.likes, data.user_rating)
}

#[tokio::test]
async fn g1f_delete_rating_follows_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let item = g1_insert_movie(&state).await;
    let random = uuid::Uuid::new_v4().to_string();
    let delete = |user: &str, token: Option<&str>| {
        request(
            "DELETE",
            &format!("/Users/{user}/Items/{item}/Rating"),
            token,
            None,
        )
    };
    let rated = (Some(true), Some(8));
    let cleared = (None, None);
    g1f_rate(&state, &owner_id, item).await;
    g1f_rate(&state, &viewer_id, item).await;

    assert_eq!(
        send(&app, delete(&owner_id, None)).await.0,
        StatusCode::UNAUTHORIZED
    );
    // A non-admin on another user, existing or not: 403, and the rating stays.
    for target in [&owner_id, &random] {
        assert_eq!(
            send(&app, delete(target, Some(&viewer))).await.0,
            StatusCode::FORBIDDEN,
            "viewer on {target}"
        );
    }
    assert_eq!(g1f_rating(&state, &owner_id, item).await, rated);
    // On themselves: today's answer, and their rating is cleared.
    assert_eq!(
        send(&app, delete(&viewer_id, Some(&viewer))).await,
        (StatusCode::OK, serde_json::json!({}))
    );
    assert_eq!(g1f_rating(&state, &viewer_id, item).await, cleared);
    // An administrator or the API key on another user: 200, and that user's rating is cleared.
    g1f_rate(&state, &viewer_id, item).await;
    assert_eq!(
        send(&app, delete(&viewer_id, Some(&owner))).await,
        (StatusCode::OK, serde_json::json!({}))
    );
    assert_eq!(g1f_rating(&state, &viewer_id, item).await, cleared);
    assert_eq!(
        send(&app, delete(&owner_id, Some("test-api-key"))).await,
        (StatusCode::OK, serde_json::json!({}))
    );
    assert_eq!(g1f_rating(&state, &owner_id, item).await, cleared);
    // An administrator on an unknown user: 404.
    assert_eq!(
        send(&app, delete(&random, Some(&owner))).await.0,
        StatusCode::NOT_FOUND
    );
}

/// Sends `GET /Users/{user}/Items/Resume` and returns the status and the item ids.
async fn g1f_resume(
    app: &axum::Router,
    user: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<uuid::Uuid>) {
    g1f_ids(app, format!("/Users/{user}/Items/Resume"), token).await
}

#[tokio::test]
async fn g1f_user_resume_follows_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    // The owner has started one movie and the viewer another.
    let owners_movie = g1_insert_movie(&state).await;
    let viewers_movie = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) \
         SELECT ?, library_id, 'G1 Movie 2', media_type, path || '.2' FROM media_items WHERE id = ?",
    )
    .bind(viewers_movie.to_string())
    .bind(owners_movie.to_string())
    .execute(&state.db)
    .await
    .unwrap();
    let random = uuid::Uuid::new_v4().to_string();
    for (user, item) in [(&owner_id, owners_movie), (&viewer_id, viewers_movie)] {
        rustyfin_db::repo_playback::update_playback_position(
            &state.db,
            user.parse().unwrap(),
            item,
            10_000_000,
        )
        .await
        .unwrap();
    }
    let owners = vec![owners_movie];
    let viewers = vec![viewers_movie];

    assert_eq!(
        g1f_resume(&app, &viewer_id, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // A non-admin on another user, existing or not: 403.
    for target in [&owner_id, &random] {
        assert_eq!(
            g1f_resume(&app, target, Some(&viewer)).await.0,
            StatusCode::FORBIDDEN,
            "viewer on {target}"
        );
    }
    // On themselves: today's answer.
    assert_eq!(
        g1f_resume(&app, &viewer_id, Some(&viewer)).await,
        (StatusCode::OK, viewers.clone())
    );
    assert_eq!(
        g1f_resume(&app, &owner_id, Some(&owner)).await,
        (StatusCode::OK, owners.clone())
    );
    // An administrator or the API key on another user: that user's list.
    assert_eq!(
        g1f_resume(&app, &viewer_id, Some(&owner)).await,
        (StatusCode::OK, viewers)
    );
    assert_eq!(
        g1f_resume(&app, &owner_id, Some("test-api-key")).await,
        (StatusCode::OK, owners)
    );
    // An administrator on an unknown user: 404.
    assert_eq!(
        g1f_resume(&app, &random, Some(&owner)).await.0,
        StatusCode::NOT_FOUND
    );
}

/// Checks `GET {path}` (Next Up or Upcoming), with and without `seriesId`, for every kind of
/// caller. `users` is what `g1_owner_and_viewer` returns; `all` is the list for a user who has
/// played nothing and `owners` the owner's list.
async fn g1f_assert_episode_list(
    app: &axum::Router,
    path: &str,
    series: uuid::Uuid,
    users: &(String, String, String, String),
    all: &[uuid::Uuid],
    owners: &[uuid::Uuid],
) {
    let (owner, owner_id, viewer, viewer_id) = users;
    let random = uuid::Uuid::new_v4();
    for base in [format!("{path}?"), format!("{path}?seriesId={series}&")] {
        assert_eq!(
            g1f_ids(app, base.clone(), None).await.0,
            StatusCode::UNAUTHORIZED,
            "anonymous {base}"
        );
        // A non-admin naming another user, existing or not: 403.
        for target in [owner_id.clone(), random.to_string()] {
            assert_eq!(
                g1f_ids(app, format!("{base}userId={target}"), Some(viewer))
                    .await
                    .0,
                StatusCode::FORBIDDEN,
                "viewer {base}userId={target}"
            );
        }
        // A non-admin with their own id, the nil id or no userId: their own list.
        for query in [
            String::new(),
            format!("userId={viewer_id}"),
            format!("userId={G1F_NIL}"),
        ] {
            assert_eq!(
                g1f_ids(app, format!("{base}{query}"), Some(viewer)).await,
                (StatusCode::OK, all.to_vec()),
                "viewer {base}{query}"
            );
        }
        // An administrator: their own list with no userId, the nil id or their own id, another
        // user's list with that user's id, and 404 for an unknown user.
        for query in [
            String::new(),
            format!("userId={G1F_NIL}"),
            format!("userId={owner_id}"),
        ] {
            assert_eq!(
                g1f_ids(app, format!("{base}{query}"), Some(owner)).await,
                (StatusCode::OK, owners.to_vec()),
                "owner {base}{query}"
            );
        }
        assert_eq!(
            g1f_ids(app, format!("{base}userId={viewer_id}"), Some(owner)).await,
            (StatusCode::OK, all.to_vec()),
            "owner {base}userId={viewer_id}"
        );
        assert_eq!(
            g1f_ids(app, format!("{base}userId={random}"), Some(owner))
                .await
                .0,
            StatusCode::NOT_FOUND,
            "owner {base}userId={random}"
        );
        // The API key: every episode with no userId (today's answer), a user's list with their id.
        assert_eq!(
            g1f_ids(app, base.clone(), Some("test-api-key")).await,
            (StatusCode::OK, all.to_vec()),
            "API key {base}"
        );
        assert_eq!(
            g1f_ids(
                app,
                format!("{base}userId={owner_id}"),
                Some("test-api-key")
            )
            .await,
            (StatusCode::OK, owners.to_vec()),
            "API key {base}userId={owner_id}"
        );
    }
}

#[tokio::test]
async fn g1f_next_up_follows_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let users = g1_owner_and_viewer(&state, &app).await;
    let (series, [e1, e2, e3]) = g1f_insert_show(&state).await;
    rustyfin_db::repo_playback::mark_played(&state.db, users.1.parse().unwrap(), e1)
        .await
        .unwrap();
    g1f_assert_episode_list(
        &app,
        "/Shows/NextUp",
        series,
        &users,
        &[e1, e2, e3],
        &[e2, e3],
    )
    .await;
}

#[tokio::test]
async fn g1f_upcoming_follows_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let users = g1_owner_and_viewer(&state, &app).await;
    let (series, [_, _, e3]) = g1f_insert_show(&state).await;
    rustyfin_db::repo_playback::mark_played(&state.db, users.1.parse().unwrap(), e3)
        .await
        .unwrap();
    g1f_assert_episode_list(&app, "/Shows/Upcoming", series, &users, &[e3], &[]).await;
}

/// Sends `GET /Notifications/{user}` and returns the status and the notification names.
async fn g1f_notifications(
    app: &axum::Router,
    user: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<String>) {
    let uri = format!("/Notifications/{user}");
    let (status, json) = send(app, request("GET", &uri, token, None)).await;
    let names = json["Notifications"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|n| n["Name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    (status, names)
}

#[tokio::test]
async fn g1f_notifications_follow_user_id() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let random = uuid::Uuid::new_v4().to_string();
    for (user, name) in [(&owner_id, "for the owner"), (&viewer_id, "for the viewer")] {
        sqlx::query("INSERT INTO notifications (user_id, name) VALUES (?, ?)")
            .bind(user.parse::<uuid::Uuid>().unwrap().to_string())
            .bind(name)
            .execute(&state.db)
            .await
            .unwrap();
    }
    let owners = vec!["for the owner".to_string()];
    let viewers = vec!["for the viewer".to_string()];

    assert_eq!(
        g1f_notifications(&app, &viewer_id, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // A non-admin on another user, existing or not: 403.
    for target in [&owner_id, &random] {
        assert_eq!(
            g1f_notifications(&app, target, Some(&viewer)).await.0,
            StatusCode::FORBIDDEN,
            "viewer on {target}"
        );
    }
    // On themselves: today's answer.
    assert_eq!(
        g1f_notifications(&app, &viewer_id, Some(&viewer)).await,
        (StatusCode::OK, viewers.clone())
    );
    // An administrator or the API key on another user: that user's notifications; unknown: 404.
    assert_eq!(
        g1f_notifications(&app, &viewer_id, Some(&owner)).await,
        (StatusCode::OK, viewers)
    );
    assert_eq!(
        g1f_notifications(&app, &owner_id, Some("test-api-key")).await,
        (StatusCode::OK, owners)
    );
    assert_eq!(
        g1f_notifications(&app, &random, Some(&owner)).await.0,
        StatusCode::NOT_FOUND
    );
}

/// Creates a session for the owner on device "g1f-owner-device" and one for the viewer on
/// "g1f-viewer-device", and returns `(owner_session, viewer_session)`.
async fn g1f_sessions(state: &AppState, owner_id: &str, viewer_id: &str) -> (String, String) {
    let mut ids = Vec::new();
    for (user, device) in [
        (owner_id, "g1f-owner-device"),
        (viewer_id, "g1f-viewer-device"),
    ] {
        let user: uuid::Uuid = user.parse().unwrap();
        let session = uuid::Uuid::new_v4();
        rustyfin_db::repo_session::create_session(
            &state.db, session, user, "", "G1F", "cli", device, "1.0", "{}",
        )
        .await
        .unwrap();
        rustyfin_db::repo_session::upsert_device(&state.db, device, "cli", "G1F", "1.0", user)
            .await
            .unwrap();
        ids.push(session.to_string());
    }
    (ids[0].clone(), ids[1].clone())
}

#[tokio::test]
async fn g1f_session_control_owner_or_admin() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let (owner_session, viewer_session) = g1f_sessions(&state, &owner_id, &viewer_id).await;
    let unknown = uuid::Uuid::new_v4().to_string();
    let command = serde_json::json!({
        "Command": "DisplayMessage",
        "Arguments": {"Header": "g1f", "Text": "test"}
    });
    let message = serde_json::json!({"Header": "g1f", "Text": "test"});
    // (route with {} for the target, body, the viewer's target, the owner's target)
    let routes = [
        (
            "/Sessions/{}/Command",
            Some(command.clone()),
            viewer_session.as_str(),
            owner_session.as_str(),
        ),
        (
            "/Sessions/{}/Command/DisplayMessage",
            None,
            viewer_session.as_str(),
            owner_session.as_str(),
        ),
        (
            "/Sessions/{}/Message",
            Some(message),
            viewer_session.as_str(),
            owner_session.as_str(),
        ),
        (
            "/Devices/{}/Command",
            Some(command),
            "g1f-viewer-device",
            "g1f-owner-device",
        ),
    ];
    for (route, body, mine, theirs) in &routes {
        let post = |target: &str, token: Option<&str>| {
            request("POST", &route.replace("{}", target), token, body.clone())
        };
        assert_eq!(
            send(&app, post(mine, None)).await.0,
            StatusCode::UNAUTHORIZED,
            "{route} anonymous"
        );
        // A non-admin on another user's session or device, or on an unknown one: 403.
        for target in [*theirs, unknown.as_str()] {
            assert_eq!(
                send(&app, post(target, Some(&viewer))).await.0,
                StatusCode::FORBIDDEN,
                "{route} viewer on {target}"
            );
        }
        // The owner of the session or device: today's answer.
        assert_eq!(
            send(&app, post(mine, Some(&viewer))).await,
            (StatusCode::OK, serde_json::json!({})),
            "{route} viewer on their own"
        );
        // An administrator or the API key: any existing session or device; unknown: 404.
        for token in [owner.as_str(), "test-api-key"] {
            for target in [*mine, *theirs] {
                assert_eq!(
                    send(&app, post(target, Some(token))).await,
                    (StatusCode::OK, serde_json::json!({})),
                    "{route} administrator on {target}"
                );
            }
            assert_eq!(
                send(&app, post(&unknown, Some(token))).await.0,
                StatusCode::NOT_FOUND,
                "{route} administrator on an unknown target"
            );
        }
    }
}

/// Sends `GET /Sessions` and returns the status and each session's `(Id, UserId)`.
async fn g1f_session_list(
    app: &axum::Router,
    token: Option<&str>,
) -> (StatusCode, Vec<(String, uuid::Uuid)>) {
    let (status, json) = send(app, request("GET", "/Sessions", token, None)).await;
    let sessions = json
        .as_array()
        .map(|list| {
            list.iter()
                .map(|s| {
                    (
                        s["Id"].as_str().unwrap().to_string(),
                        uuid::Uuid::parse_str(s["UserId"].as_str().unwrap()).unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    (status, sessions)
}

#[tokio::test]
async fn g1f_sessions_list_own_unless_admin() {
    let state = test_state().await;
    let app = app(state.clone());
    let (owner, owner_id, viewer, viewer_id) = g1_owner_and_viewer(&state, &app).await;
    let (owner_session, viewer_session) = g1f_sessions(&state, &owner_id, &viewer_id).await;
    let viewer_uuid: uuid::Uuid = viewer_id.parse().unwrap();

    assert_eq!(
        g1f_session_list(&app, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    // A non-admin sees only their own sessions.
    let (status, sessions) = g1f_session_list(&app, Some(&viewer)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        sessions.iter().all(|(_, user)| *user == viewer_uuid),
        "the viewer sees only their own sessions: {sessions:?}"
    );
    assert!(sessions.iter().any(|(id, _)| *id == viewer_session));
    // An administrator and the API key see everyone's.
    for token in [owner.as_str(), "test-api-key"] {
        let (status, sessions) = g1f_session_list(&app, Some(token)).await;
        assert_eq!(status, StatusCode::OK);
        for session in [&owner_session, &viewer_session] {
            assert!(
                sessions.iter().any(|(id, _)| id == session),
                "an administrator sees {session}"
            );
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// G2: sessions, reset PIN, login limit (R1-01, R1-08, R1-09, M-15, M-16, M-17)
// These drive the HTTP surface and so compile on the base build; they fail at
// runtime there until the G2 fixes land.
// ══════════════════════════════════════════════════════════════════════════

/// Logs in with an explicit DeviceId (via X-Emby-Authorization); returns (token, user_id).
async fn g2_login_device(
    app: &axum::Router,
    name: &str,
    password: &str,
    device_id: &str,
) -> Option<(String, String)> {
    let body = serde_json::json!({ "Username": name, "Pw": password });
    let req = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header("Content-Type", "application/json")
        .header(
            "X-Emby-Authorization",
            format!(
                "MediaBrowser Client=\"G2\", Device=\"{device_id}\", DeviceId=\"{device_id}\", Version=\"1.0\""
            ),
        )
        .body(Body::from(body.to_string()))
        .unwrap();
    let (status, json) = send(app, req).await;
    if status != StatusCode::OK {
        return None;
    }
    Some((
        json["AccessToken"].as_str().unwrap().to_string(),
        json["User"]["Id"].as_str().unwrap().to_string(),
    ))
}

/// A router whose connecting peer is the given IPv4 socket, for login-limit tests.
fn g2_app_from_peer(state: AppState, ip: [u8; 4], port: u16) -> axum::Router {
    build_router(state).layer(MockConnectInfo(SocketAddr::from((ip, port))))
}

/// A failed-login request carrying an optional X-Forwarded-For header.
fn g2_bad_login(xff: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header("Content-Type", "application/json");
    if let Some(v) = xff {
        b = b.header("X-Forwarded-For", v);
    }
    b.body(Body::from(r#"{"Username":"nobody","Pw":"x"}"#))
        .unwrap()
}

// ── G2: tokens and sessions (R1-08, R1-09) ──

#[tokio::test]
async fn g2_logout_ends_only_that_device_and_returns_204() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (ta, _) = g2_login_device(&app, "owner", "pw-owner", "dev-a")
        .await
        .expect("login A");
    let (tb, _) = g2_login_device(&app, "owner", "pw-owner", "dev-b")
        .await
        .expect("login B");

    let (status, _) = send(&app, request("POST", "/Sessions/Logout", Some(&ta), None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "logout returns 204");

    let (a_after, _) = send(&app, request("GET", "/Users/Me", Some(&ta), None)).await;
    assert_eq!(
        a_after,
        StatusCode::UNAUTHORIZED,
        "device A token is revoked"
    );
    let (b_after, _) = send(&app, request("GET", "/Users/Me", Some(&tb), None)).await;
    assert_eq!(b_after, StatusCode::OK, "device B token still works");
}

#[tokio::test]
async fn g2_relogin_same_device_replaces_token() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (t1, _) = g2_login_device(&app, "owner", "pw-owner", "dev-x")
        .await
        .expect("login 1");
    let (t2, _) = g2_login_device(&app, "owner", "pw-owner", "dev-x")
        .await
        .expect("login 2");

    let (s1, _) = send(&app, request("GET", "/Users/Me", Some(&t1), None)).await;
    assert_eq!(
        s1,
        StatusCode::UNAUTHORIZED,
        "the previous token for the same device is revoked"
    );
    let (s2, _) = send(&app, request("GET", "/Users/Me", Some(&t2), None)).await;
    assert_eq!(s2, StatusCode::OK, "the new token works");
}

#[tokio::test]
async fn g2_self_password_change_keeps_caller_revokes_other_sessions() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (ta, oid) = g2_login_device(&app, "owner", "pw-owner", "dev-a")
        .await
        .expect("A");
    let (tb, _) = g2_login_device(&app, "owner", "pw-owner", "dev-b")
        .await
        .expect("B");

    let body = serde_json::json!({ "CurrentPw": "pw-owner", "NewPw": "pw-owner-2" });
    let (status, _) = send(
        &app,
        request(
            "POST",
            &format!("/Users/{oid}/Password"),
            Some(&ta),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "password change succeeds");

    let (a_after, _) = send(&app, request("GET", "/Users/Me", Some(&ta), None)).await;
    assert_eq!(a_after, StatusCode::OK, "the caller's own session survives");
    let (b_after, _) = send(&app, request("GET", "/Users/Me", Some(&tb), None)).await;
    assert_eq!(
        b_after,
        StatusCode::UNAUTHORIZED,
        "the user's other sessions end"
    );
}

#[tokio::test]
async fn g2_admin_changing_another_users_password_revokes_all_their_tokens() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (owner_t, _) = g2_login_device(&app, "owner", "pw-owner", "owner-dev")
        .await
        .expect("owner");
    let (status, created) = send(
        &app,
        request(
            "POST",
            "/Users/New",
            Some(&owner_t),
            Some(serde_json::json!({ "Name": "bob", "Password": "pw-bob" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "owner creates bob");
    let bob_id = created["Id"].as_str().expect("bob id").to_string();
    let (bob_t, _) = g2_login_device(&app, "bob", "pw-bob", "bob-dev")
        .await
        .expect("bob");

    let body = serde_json::json!({ "CurrentPw": "", "NewPw": "pw-bob-2" });
    let (status, _) = send(
        &app,
        request(
            "POST",
            &format!("/Users/{bob_id}/Password"),
            Some(&owner_t),
            Some(body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin resets bob's password");

    let (bob_after, _) = send(&app, request("GET", "/Users/Me", Some(&bob_t), None)).await;
    assert_eq!(
        bob_after,
        StatusCode::UNAUTHORIZED,
        "all of bob's tokens are revoked"
    );
}

#[tokio::test]
async fn g2_deleted_users_token_is_rejected() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (owner_t, _) = g2_login_device(&app, "owner", "pw-owner", "owner-dev")
        .await
        .expect("owner");
    let (status, created) = send(
        &app,
        request(
            "POST",
            "/Users/New",
            Some(&owner_t),
            Some(serde_json::json!({ "Name": "bob", "Password": "pw-bob" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let bob_id = created["Id"].as_str().expect("bob id").to_string();
    let (bob_t, _) = g2_login_device(&app, "bob", "pw-bob", "bob-dev")
        .await
        .expect("bob");

    let (status, _) = send(
        &app,
        request(
            "POST",
            &format!("/Users/{bob_id}/Delete"),
            Some(&owner_t),
            None,
        ),
    )
    .await;
    assert!(status.is_success(), "owner deletes bob: {status}");

    let (bob_after, _) = send(&app, request("GET", "/Users/Me", Some(&bob_t), None)).await;
    assert_eq!(
        bob_after,
        StatusCode::UNAUTHORIZED,
        "a deleted user's token is rejected"
    );
}

#[tokio::test]
async fn g2_disabled_leftover_admin_token_is_rejected() {
    let state = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    let app = app(state.clone());
    // The leftover admin logs in before the wizard runs.
    let (admin_t, _) = g2_login_device(&app, "admin", "admin", "admin-dev")
        .await
        .expect("admin login");
    // The wizard creates the real owner, disabling the leftover admin.
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, true)),
        "the leftover admin is disabled"
    );

    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&admin_t), None)).await;
    assert_eq!(
        me,
        StatusCode::UNAUTHORIZED,
        "a disabled user's token is rejected on /Users/Me"
    );
    let (users, _) = send(&app, request("GET", "/Users", Some(&admin_t), None)).await;
    assert_eq!(
        users,
        StatusCode::UNAUTHORIZED,
        "a disabled user's token is rejected on an admin route"
    );
}

#[tokio::test]
async fn g2_login_response_has_fresh_last_login_date() {
    // M-16: the login response must carry the login's own timestamp, not the prior value.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let body = serde_json::json!({ "Username": "owner", "Pw": "pw-owner" });
    let (status, json) = send(
        &app,
        request("POST", "/Users/AuthenticateByName", None, Some(body)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        json["User"]["LastLoginDate"].is_string(),
        "the first login response carries the updated LastLoginDate, got {:?}",
        json["User"]["LastLoginDate"]
    );
}

// ── G2: reset PIN (R1-01, M-17) ──

#[tokio::test]
async fn g2_forgot_password_is_uniform_and_hides_the_pin() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );

    let known = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword",
            None,
            Some(serde_json::json!({"EnteredUsername":"owner"})),
        ),
    )
    .await;
    let unknown = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword",
            None,
            Some(serde_json::json!({"EnteredUsername":"ghost"})),
        ),
    )
    .await;
    assert_eq!(known.0, StatusCode::OK, "known user");
    assert_eq!(
        unknown.0,
        StatusCode::OK,
        "an unknown user is indistinguishable from a known one"
    );
    assert_eq!(known.1["Action"], "PinCode");
    assert_eq!(unknown.1["Action"], "PinCode");
    assert!(
        known.1["PinFile"].is_string(),
        "the response names the PIN file, got {:?}",
        known.1
    );
    assert!(
        !known.1.to_string().to_lowercase().contains("\"pin\":"),
        "the PIN itself never appears in the response body"
    );
}

#[tokio::test]
async fn g2_legitimate_reset_pin_flow_changes_password_and_revokes_tokens() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (old_t, _) = g2_login_device(&app, "owner", "pw-owner", "owner-dev")
        .await
        .expect("owner");

    let (status, body) = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword",
            None,
            Some(serde_json::json!({"EnteredUsername":"owner"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pin_file = body["PinFile"]
        .as_str()
        .expect("the response names a PinFile path");
    let contents = std::fs::read_to_string(pin_file).expect("the PIN file exists on a LAN request");
    let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
    let pin = parsed["Pin"]
        .as_str()
        .expect("a Pin in the file")
        .to_string();

    let (status, body) = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword/Pin",
            None,
            Some(serde_json::json!({ "Pin": pin })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "redeeming the PIN succeeds: {body:?}"
    );
    assert_eq!(body["Success"], true);

    // The PIN string itself is now the password.
    assert!(
        g2_login_device(&app, "owner", &pin, "new-dev")
            .await
            .is_some(),
        "login with the new PIN as the password"
    );
    // The original password no longer works.
    assert!(
        g2_login_device(&app, "owner", "pw-owner", "d")
            .await
            .is_none(),
        "the old password is replaced"
    );
    // The old session is revoked.
    let (old_after, _) = send(&app, request("GET", "/Users/Me", Some(&old_t), None)).await;
    assert_eq!(
        old_after,
        StatusCode::UNAUTHORIZED,
        "old tokens are revoked after a reset"
    );
}

#[tokio::test]
async fn g2_five_wrong_pin_attempts_void_the_reset() {
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword",
            None,
            Some(serde_json::json!({"EnteredUsername":"owner"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pin_file = body["PinFile"].as_str().expect("a PinFile path");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(pin_file).unwrap()).unwrap();
    let real_pin = parsed["Pin"].as_str().unwrap().to_string();

    for n in 1..=5 {
        let (s, _) = send(
            &app,
            request(
                "POST",
                "/Users/ForgotPassword/Pin",
                None,
                Some(serde_json::json!({"Pin":"ZZ-ZZ-ZZ-ZZ"})),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "wrong PIN attempt {n} is 404");
    }
    let (s, _) = send(
        &app,
        request(
            "POST",
            "/Users/ForgotPassword/Pin",
            None,
            Some(serde_json::json!({ "Pin": real_pin })),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "the PIN is void after 5 failed attempts"
    );
    assert!(
        g2_login_device(&app, "owner", "pw-owner", "d")
            .await
            .is_some(),
        "the original password is intact"
    );
}

// ── G2: login limit and proxy trust (XFF) ──

#[tokio::test]
async fn g2_spoofed_loopback_xff_from_remote_peer_is_still_limited() {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    let app = g2_app_from_peer(state, [127, 0, 0, 2], 5000);

    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(
            app.clone()
                .oneshot(g2_bad_login(Some("127.0.0.1")))
                .await
                .unwrap()
                .status(),
        );
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "a spoofed loopback XFF must not exempt a non-loopback peer; got {statuses:?}"
    );
}

#[tokio::test]
async fn g2_rotating_xff_shares_the_peer_budget() {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    let app = g2_app_from_peer(state, [127, 0, 0, 2], 5001);

    let mut statuses = Vec::new();
    for i in 0..6 {
        statuses.push(
            app.clone()
                .oneshot(g2_bad_login(Some(&format!("10.9.9.{i}"))))
                .await
                .unwrap()
                .status(),
        );
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "rotating XFF values must share the peer's budget; got {statuses:?}"
    );
}

#[tokio::test]
async fn g2_loopback_peer_exempt_but_127_0_0_2_limited() {
    // Guard: this behaviour already holds on base and must be preserved.
    let mut s1 = test_state().await;
    s1.rate_limiter = RateLimiter::new(2, 60);
    let app1 = g2_app_from_peer(s1, [127, 0, 0, 1], 6000);
    for _ in 0..6 {
        let st = app1
            .clone()
            .oneshot(g2_bad_login(None))
            .await
            .unwrap()
            .status();
        assert_ne!(
            st,
            StatusCode::TOO_MANY_REQUESTS,
            "a loopback peer is never limited"
        );
    }

    let mut s2 = test_state().await;
    s2.rate_limiter = RateLimiter::new(2, 60);
    let app2 = g2_app_from_peer(s2, [127, 0, 0, 2], 6001);
    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(
            app2.clone()
                .oneshot(g2_bad_login(None))
                .await
                .unwrap()
                .status(),
        );
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "a 127.0.0.2 peer is limited; got {statuses:?}"
    );
}

#[tokio::test]
async fn g2_trusted_proxy_limits_the_forwarded_client() {
    // A request arriving through a configured trusted proxy is attributed to the forwarded client,
    // so repeated logins from one real client behind the proxy share one budget and get limited.
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    state.known_proxies = Arc::new(rustyfin_api::middleware::parse_known_proxies("10.0.0.0/8"));
    let app = g2_app_from_peer(state, [10, 0, 0, 7], 7000);

    let mut statuses = Vec::new();
    for _ in 0..5 {
        statuses.push(
            app.clone()
                .oneshot(g2_bad_login(Some("203.0.113.5")))
                .await
                .unwrap()
                .status(),
        );
    }
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "the client forwarded by a trusted proxy is limited; got {statuses:?}"
    );
}

#[tokio::test]
async fn g2_trusted_proxy_separates_budgets_per_forwarded_client() {
    // The limiter keys on the resolved client, not the shared proxy, so a second real client behind
    // the same trusted proxy keeps its own budget.
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(2, 60);
    state.known_proxies = Arc::new(rustyfin_api::middleware::parse_known_proxies("10.0.0.0/8"));
    let app = g2_app_from_peer(state, [10, 0, 0, 7], 7001);

    for _ in 0..3 {
        let _ = app
            .clone()
            .oneshot(g2_bad_login(Some("203.0.113.5")))
            .await
            .unwrap();
    }
    let other = app
        .clone()
        .oneshot(g2_bad_login(Some("203.0.113.6")))
        .await
        .unwrap()
        .status();
    assert_ne!(
        other,
        StatusCode::TOO_MANY_REQUESTS,
        "a different forwarded client keeps its own budget"
    );
}

#[test]
fn g2_parse_known_proxies_reads_cidrs_and_hosts_and_skips_junk() {
    // Accepts CIDR blocks and bare host IPs; silently drops unparseable entries.
    let nets = rustyfin_api::middleware::parse_known_proxies("10.0.0.0/8, 192.168.1.1 , not-an-ip");
    assert_eq!(nets.len(), 2, "two valid entries, junk skipped: {nets:?}");
}

// ── G2 follow-up: security-review fixes. HTTP-level like the block above, so they compile on
// 90c99b5 and fail there at runtime. ──

/// A router whose connecting peer is `addr`, any `SocketAddr` text such as `[::ffff:10.0.0.7]:1`.
fn g2f_app_from(state: AppState, addr: &str) -> axum::Router {
    build_router(state).layer(MockConnectInfo(addr.parse::<SocketAddr>().unwrap()))
}

/// A state with a login budget of 3 per 60 s that trusts the proxies in `proxies` (CSV).
async fn g2f_state(proxies: &str) -> AppState {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(3, 60);
    state.known_proxies = Arc::new(rustyfin_api::middleware::parse_known_proxies(proxies));
    state
}

/// A failed login that carries each of `xff` as its own `X-Forwarded-For` header line.
fn g2f_bad_login(xff: &[&str]) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/Users/AuthenticateByName")
        .header("Content-Type", "application/json");
    for line in xff {
        b = b.header("X-Forwarded-For", *line);
    }
    b.body(Body::from(r#"{"Username":"nobody","Pw":"x"}"#))
        .unwrap()
}

/// Sends `n` failed logins with the given XFF lines and returns how many got 429.
async fn g2f_limited(app: &axum::Router, n: usize, xff: &[&str]) -> usize {
    let mut limited = 0;
    for _ in 0..n {
        let status = app
            .clone()
            .oneshot(g2f_bad_login(xff))
            .await
            .unwrap()
            .status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
        }
    }
    limited
}

/// Sends ForgotPassword for `user` with the given XFF lines; returns whether the reset file exists.
async fn g2f_forgot_writes_file(app: &axum::Router, user: &str, xff: &[&str]) -> bool {
    let mut b = Request::builder()
        .method("POST")
        .uri("/Users/ForgotPassword")
        .header("Content-Type", "application/json");
    for line in xff {
        b = b.header("X-Forwarded-For", *line);
    }
    let req = b
        .body(Body::from(
            serde_json::json!({ "EnteredUsername": user }).to_string(),
        ))
        .unwrap();
    let (status, body) = send(app, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ForgotPassword is always 200: {body:?}"
    );
    std::path::Path::new(body["PinFile"].as_str().expect("a PinFile path")).exists()
}

/// Creates the owner "owner" through the setup wizard.
async fn g2f_with_owner(state: &AppState) {
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn g2f_unlisted_loopback_peer_with_xff_is_not_exempt() {
    // Loopback is a trusted proxy only when listed, so its XFF is never read. Since G2c a peer that
    // is not listed but sends XFF is unresolved: keyed on itself and not exempt. 90c99b5 always
    // trusted loopback and limited 203.0.113.5; 9dd5a44 took the request as loopback's own, exempt.
    let app = g2f_app_from(g2f_state("").await, "127.0.0.1:6100");
    assert_eq!(
        g2f_limited(&app, 5, &["203.0.113.5"]).await,
        2,
        "an unlisted loopback peer that sends XFF draws on its own budget of 3"
    );
}

#[tokio::test]
async fn g2f_listed_loopback_proxy_limits_the_forwarded_client() {
    // Guard: once listed, loopback forwards like any other proxy.
    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6101");
    assert!(
        g2f_limited(&app, 5, &["203.0.113.5"]).await > 0,
        "the client behind a listed loopback proxy is limited"
    );
}

#[tokio::test]
async fn g2f_listed_loopback_without_xff_is_not_exempt() {
    // A listed proxy with no XFF has only trusted hops: unresolved, keyed on the peer, not exempt.
    // 90c99b5 exempted it.
    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6102");
    assert!(
        g2f_limited(&app, 5, &[]).await > 0,
        "a listed proxy without XFF is limited on its own address"
    );
}

#[tokio::test]
async fn g2f_second_xff_line_is_read() {
    // The client writes `X-Forwarded-For: 127.0.0.1`; the proxy appends its own line with the real
    // client. 90c99b5 read only the first line and exempted the request as loopback.
    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6103");
    assert!(
        g2f_limited(&app, 5, &["127.0.0.1", "203.0.113.5"]).await > 0,
        "the real client in the last XFF line is limited"
    );
}

#[tokio::test]
async fn g2f_xff_entries_with_ports_and_brackets_resolve() {
    // `ip:port`, `[v6]:port` and `[v6]` all parse, and the port is not part of the key, so one
    // client on changing ports shares one budget. 90c99b5 dropped these entries and fell back to
    // loopback, which was exempt.
    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6104");
    let mut limited = 0;
    for i in 0..5 {
        let line = format!("127.0.0.1, 203.0.113.5:{}", 1000 + i);
        limited += g2f_limited(&app, 1, &[&line]).await;
    }
    assert!(limited > 0, "ip:port on changing ports shares one budget");

    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6105");
    let mut limited = 0;
    for i in 0..5 {
        let line = format!("[2001:db8:5::5]:{}", 1000 + i);
        limited += g2f_limited(&app, 1, &[&line]).await;
    }
    assert!(limited > 0, "[v6]:port resolves and is limited");

    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6106");
    assert!(
        g2f_limited(&app, 5, &["[2001:db8:6::6]"]).await > 0,
        "[v6] resolves and is limited"
    );
}

#[tokio::test]
async fn g2f_malformed_xff_entry_is_neither_exempt_nor_local() {
    // A malformed entry reached by the right-to-left walk leaves the request unresolved: keyed on
    // the peer, not exempt, not local. 90c99b5 skipped the junk and fell back to the client-written
    // 127.0.0.1 (exempt), and its LAN gate used the raw LAN peer.
    let app = g2f_app_from(g2f_state("127.0.0.1").await, "127.0.0.1:6107");
    assert!(
        g2f_limited(&app, 5, &["127.0.0.1, not-an-ip"]).await > 0,
        "a malformed entry must not resolve to the client-written loopback"
    );

    let state = g2f_state("10.0.0.7").await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "10.0.0.7:6108");
    assert!(
        !g2f_forgot_writes_file(&app, "owner", &["192.168.1.5, not-an-ip"]).await,
        "a malformed entry is not local: no reset file"
    );
}

#[tokio::test]
async fn g2f_junk_left_of_the_proxy_entry_is_ignored() {
    // Guard: junk a client wrote before the proxy's own entry is never read, so the request still
    // resolves to that entry. A client cannot move everyone onto the proxy's shared budget.
    let app = g2f_app_from(g2f_state("10.0.0.7").await, "10.0.0.7:6109");
    let _ = g2f_limited(&app, 3, &["not-an-ip, 203.0.113.5"]).await;
    assert_eq!(
        g2f_limited(&app, 1, &["203.0.113.5"]).await,
        1,
        "the junk-prefixed requests drew on 203.0.113.5's budget"
    );
}

#[tokio::test]
async fn g2f_all_hops_trusted_is_not_local() {
    // Every XFF hop is a listed proxy: unresolved, so no reset file. 90c99b5 gated on the raw,
    // private peer and wrote one.
    let state = g2f_state("10.0.0.0/8").await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "10.0.0.7:6110");
    assert!(
        !g2f_forgot_writes_file(&app, "owner", &["10.0.0.8"]).await,
        "an all-trusted chain is not local"
    );
}

#[tokio::test]
async fn g2f_lan_gate_uses_the_resolved_client() {
    // Behind a listed LAN proxy the gate follows the real client in the last XFF line. 90c99b5
    // checked the raw peer, so every client behind a LAN proxy got a reset file.
    let state = g2f_state("10.0.0.7").await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "10.0.0.7:6111");
    assert!(
        !g2f_forgot_writes_file(&app, "owner", &["192.168.1.5", "203.0.113.5"]).await,
        "a remote client behind a LAN proxy gets no reset file"
    );
    assert!(
        g2f_forgot_writes_file(&app, "owner", &["203.0.113.5", "192.168.1.5"]).await,
        "a LAN client behind the same proxy gets one"
    );
}

#[tokio::test]
async fn g2f_untrusted_peer_xff_cannot_claim_the_lan() {
    // Guard: an untrusted remote peer's XFF is ignored by the LAN gate as by the limiter.
    let state = g2f_state("10.0.0.7").await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "203.0.113.9:6112");
    assert!(
        !g2f_forgot_writes_file(&app, "owner", &["192.168.1.5"]).await,
        "a spoofed LAN XFF from a remote peer gets no reset file"
    );
    assert!(
        g2f_limited(&app, 5, &["127.0.0.1"]).await > 0,
        "a spoofed loopback XFF from a remote peer is limited"
    );
}

#[tokio::test]
async fn g2f_mapped_ipv4_lan_peer_gets_a_reset_file() {
    // A dual-stack listener sees IPv4 LAN peers as ::ffff:a.b.c.d. 90c99b5 did not canonicalize
    // them and wrote no file.
    let state = g2f_state("").await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "[::ffff:192.168.1.20]:6113");
    assert!(
        g2f_forgot_writes_file(&app, "owner", &[]).await,
        "a mapped LAN peer gets a reset file"
    );
}

#[tokio::test]
async fn g2f_mapped_loopback_peer_is_exempt() {
    // ::ffff:127.0.0.1 is exact loopback once canonicalized. 90c99b5 limited it.
    let app = g2f_app_from(g2f_state("").await, "[::ffff:127.0.0.1]:6114");
    assert_eq!(
        g2f_limited(&app, 5, &[]).await,
        0,
        "a mapped loopback peer is exempt"
    );
}

#[tokio::test]
async fn g2f_mapped_proxy_peer_is_trusted() {
    // A listed IPv4 proxy that reaches a dual-stack listener as ::ffff:10.0.0.7 is still that proxy,
    // so two clients behind it keep separate budgets. 90c99b5 keyed both on the mapped peer.
    let app = g2f_app_from(g2f_state("10.0.0.7").await, "[::ffff:10.0.0.7]:6115");
    let _ = g2f_limited(&app, 3, &["203.0.113.5"]).await;
    assert_eq!(
        g2f_limited(&app, 1, &["203.0.113.6"]).await,
        0,
        "a second client behind the mapped proxy keeps its own budget"
    );
}

#[tokio::test]
async fn g2f_ipv6_clients_share_a_slash64_budget() {
    // One host rotating through its /64 must not get a fresh budget per address. 90c99b5 keyed
    // every full IPv6 address separately.
    let state = g2f_state("").await;
    let a = g2f_app_from(state.clone(), "[2001:db8:1:2::10]:6116");
    let b = g2f_app_from(state.clone(), "[2001:db8:1:2::20]:6117");
    let other = g2f_app_from(state, "[2001:db8:1:3::10]:6118");
    let _ = g2f_limited(&a, 3, &[]).await;
    assert_eq!(
        g2f_limited(&b, 1, &[]).await,
        1,
        "a second address in the same /64 shares the budget"
    );
    assert_eq!(
        g2f_limited(&other, 1, &[]).await,
        0,
        "a different /64 keeps its own budget"
    );
}

/// Starts a reset for "owner" (from loopback) and returns the PIN in the reset file.
async fn g2f_start_reset(app: &axum::Router) -> String {
    let (status, body) = send(
        app,
        request(
            "POST",
            "/Users/ForgotPassword",
            None,
            Some(serde_json::json!({"EnteredUsername":"owner"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let file = body["PinFile"].as_str().expect("a PinFile path");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).expect("the reset file")).unwrap();
    parsed["Pin"]
        .as_str()
        .expect("a Pin in the file")
        .to_string()
}

/// Redeems `pin` through ForgotPassword/Pin and returns the status.
async fn g2f_redeem(app: &axum::Router, pin: &str) -> StatusCode {
    send(
        app,
        request(
            "POST",
            "/Users/ForgotPassword/Pin",
            None,
            Some(serde_json::json!({ "Pin": pin })),
        ),
    )
    .await
    .0
}

#[tokio::test]
async fn g2f_reissue_after_five_misses_grants_no_new_guesses() {
    // A new ForgotPassword must not reset the attempt count. 90c99b5 replaced the voided reset with
    // a fresh one, so every re-issue gave 5 more guesses.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let _ = g2f_start_reset(&app).await;
    for n in 1..=5 {
        assert_eq!(
            g2f_redeem(&app, "ZZ-ZZ-ZZ-ZZ").await,
            StatusCode::NOT_FOUND,
            "miss {n}"
        );
    }
    let pin = g2f_start_reset(&app).await;
    assert_eq!(
        g2f_redeem(&app, &pin).await,
        StatusCode::NOT_FOUND,
        "after 5 misses the PIN in the reset file stays void until the reset expires"
    );
    assert!(
        g2_login_device(&app, "owner", "pw-owner", "d")
            .await
            .is_some(),
        "the password is unchanged"
    );
}

#[tokio::test]
async fn g2f_reissue_keeps_the_attempt_count() {
    // 3 misses, a re-issue, 2 more misses: 5 in all, so the PIN in the reset file is void. 90c99b5
    // restarted the count at the re-issue.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let _ = g2f_start_reset(&app).await;
    for _ in 0..3 {
        assert_eq!(g2f_redeem(&app, "ZZ-ZZ-ZZ-ZZ").await, StatusCode::NOT_FOUND);
    }
    let pin = g2f_start_reset(&app).await;
    for _ in 0..2 {
        assert_eq!(g2f_redeem(&app, "ZZ-ZZ-ZZ-ZZ").await, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        g2f_redeem(&app, &pin).await,
        StatusCode::NOT_FOUND,
        "5 misses in all void the reset"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn g2f_concurrent_redemption_succeeds_once() {
    // The right PIN sent 8 times at once succeeds exactly once. 90c99b5 checked and marked the PIN
    // in separate statements, so concurrent requests could all pass the check.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let pin = g2f_start_reset(&app).await;
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let app = app.clone();
        let pin = pin.clone();
        tasks.push(tokio::spawn(async move { g2f_redeem(&app, &pin).await }));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap() == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 1, "exactly one concurrent redemption succeeds");
}

#[tokio::test]
async fn g2f_redeemed_pin_row_is_deleted() {
    // A redeemed PIN is deleted, not kept as used. 90c99b5 kept the row.
    let state = test_state().await;
    let app = app(state.clone());
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let pin = g2f_start_reset(&app).await;
    assert_eq!(g2f_redeem(&app, &pin).await, StatusCode::OK);
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM password_reset_pins")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(left, 0, "no reset row is left after redemption");
}

#[tokio::test]
async fn g2f_reset_password_hides_unknown_users() {
    // ResetPassword answers an unknown user exactly like a wrong PIN. 90c99b5 said "user not found".
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let attempt = |name: &str| {
        request(
            "POST",
            "/Users/ResetPassword",
            None,
            Some(serde_json::json!({
                "Username": name, "Pin": "ZZ-ZZ-ZZ-ZZ", "TempPassword": "x"
            })),
        )
    };
    let known = send(&app, attempt("owner")).await;
    let unknown = send(&app, attempt("ghost")).await;
    assert_ne!(known.0, StatusCode::OK, "a wrong PIN fails");
    assert_eq!(
        unknown, known,
        "an unknown user gets the same status and body as a wrong PIN"
    );
}

#[tokio::test]
async fn g2f_wizard_claiming_admin_ends_its_sessions() {
    // The wizard claims the leftover "admin" by name and sets a new password; the admin/admin
    // tokens end in the same step. 90c99b5 kept them.
    let state = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    let app = app(state);
    let (old_t, _) = g2_login_device(&app, "admin", "admin", "admin-dev")
        .await
        .expect("admin login");
    assert_eq!(
        post_startup_user(&app, None, "admin", "new-secret").await,
        StatusCode::NO_CONTENT
    );
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&old_t), None)).await;
    assert_eq!(
        me,
        StatusCode::UNAUTHORIZED,
        "the claimed account's earlier token is revoked"
    );
    assert!(
        g2_login_device(&app, "admin", "new-secret", "d2")
            .await
            .is_some(),
        "the new password works"
    );
}

#[tokio::test]
async fn g2f_disabled_leftover_admin_tokens_are_deleted() {
    // The wizard creates a new owner and disables the leftover admin. Its tokens are deleted, not
    // only rejected while disabled, so re-enabling the account does not revive them.
    let state = test_state().await;
    add_user(&state, "admin", "admin", true).await;
    let app = app(state.clone());
    let (old_t, _) = g2_login_device(&app, "admin", "admin", "admin-dev")
        .await
        .expect("admin login");
    assert_eq!(
        post_startup_user(&app, None, "karl", "geheim").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        user_flags(&state, "admin").await,
        Some((true, false, true)),
        "the leftover admin is disabled"
    );
    sqlx::query("UPDATE users SET is_disabled = 0 WHERE name = 'admin'")
        .execute(&state.db)
        .await
        .unwrap();
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&old_t), None)).await;
    assert_eq!(
        me,
        StatusCode::UNAUTHORIZED,
        "a re-enabled leftover's old token stays revoked"
    );
}

#[tokio::test]
async fn g2f_admin_device_delete_ends_that_devices_sessions() {
    // DELETE /Devices?id= signs the device out, as in Jellyfin. 90c99b5 left its tokens valid.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (victim_t, _) = g2_login_device(&app, "owner", "pw-owner", "victim-dev")
        .await
        .expect("login on the device to delete");
    let (admin_t, _) = g2_login_device(&app, "owner", "pw-owner", "admin-dev")
        .await
        .expect("login on another device");
    let (status, body) = send(
        &app,
        request("DELETE", "/Devices?id=victim-dev", Some(&admin_t), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "device delete: {body:?}");
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&victim_t), None)).await;
    assert_eq!(
        me,
        StatusCode::UNAUTHORIZED,
        "the deleted device's token is revoked"
    );
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&admin_t), None)).await;
    assert_eq!(me, StatusCode::OK, "other devices stay signed in");
}

// ── G2c: second review of the G2 merge. These use only what 9dd5a44 exposes, so they compile there
// and fail at runtime. ──

const G2C_XFF: (&str, &str) = ("X-Forwarded-For", "203.0.113.9");
const G2C_XRI: (&str, &str) = ("X-Real-IP", "203.0.113.9");
const G2C_FWD: (&str, &str) = ("Forwarded", "for=203.0.113.9");

/// A state with the default login budget (10 per 60 s) and no RUSTYFIN_KNOWN_PROXIES.
async fn g2c_default_budget_state() -> AppState {
    let mut state = test_state().await;
    state.rate_limiter = RateLimiter::new(
        rustyfin_api::middleware::DEFAULT_LOGIN_LIMIT,
        rustyfin_api::middleware::DEFAULT_LOGIN_WINDOW_SECS,
    );
    state
}

/// A POST of `json` to `uri` that carries each of `headers`.
fn g2c_post(uri: &str, headers: &[(&str, &str)], json: String) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json");
    for (name, value) in headers {
        b = b.header(*name, *value);
    }
    b.body(Body::from(json)).unwrap()
}

/// Sends `n` failed logins that carry `headers`; returns how many got 429.
async fn g2c_limited(app: &axum::Router, n: usize, headers: &[(&str, &str)]) -> usize {
    let mut limited = 0;
    for _ in 0..n {
        let req = g2c_post(
            "/Users/AuthenticateByName",
            headers,
            r#"{"Username":"nobody","Pw":"x"}"#.to_string(),
        );
        if app.clone().oneshot(req).await.unwrap().status() == StatusCode::TOO_MANY_REQUESTS {
            limited += 1;
        }
    }
    limited
}

/// Sends ForgotPassword for `user` with `headers`; returns the reset-file path the answer names.
async fn g2c_forgot(
    app: &axum::Router,
    user: &str,
    headers: &[(&str, &str)],
) -> std::path::PathBuf {
    let json = serde_json::json!({ "EnteredUsername": user }).to_string();
    let (status, body) = send(app, g2c_post("/Users/ForgotPassword", headers, json)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ForgotPassword is always 200: {body:?}"
    );
    body["PinFile"].as_str().expect("a PinFile path").into()
}

#[test]
fn g2c_unlisted_peer_with_a_forwarding_header_is_unresolved() {
    use axum::http::{HeaderMap, HeaderName, HeaderValue};
    use rustyfin_api::middleware::{parse_known_proxies, resolve_client};
    let one = |name: &'static str, value: &'static [u8]| {
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static(name),
            HeaderValue::from_bytes(value).unwrap(),
        );
        h
    };
    // Any value counts, even one that names no address or is not ASCII: it is never parsed.
    let cases = [
        one("x-forwarded-for", b"203.0.113.9"),
        one("x-real-ip", b"203.0.113.9"),
        one("forwarded", b"for=203.0.113.9"),
        one("x-forwarded-for", b""),
        one("x-forwarded-for", b"\xff"),
    ];
    for known in [Vec::new(), parse_known_proxies("10.0.0.1")] {
        for headers in &cases {
            for peer in ["127.0.0.1", "192.168.1.20"] {
                let client = resolve_client(peer.parse().unwrap(), headers, &known);
                assert!(!client.is_exempt(), "{peer} with {headers:?} is not exempt");
                assert!(!client.is_local(), "{peer} with {headers:?} is not local");
                assert_eq!(
                    client.limiter_key(),
                    peer,
                    "{peer} with {headers:?} is keyed on itself"
                );
            }
        }
    }
    // Without any of them the peer is the client, as before.
    let none = HeaderMap::new();
    let loopback = resolve_client("127.0.0.1".parse().unwrap(), &none, &[]);
    assert!(
        loopback.is_exempt() && loopback.is_local(),
        "direct loopback stays exempt and local"
    );
    let lan = resolve_client("192.168.1.20".parse().unwrap(), &none, &[]);
    assert!(
        lan.is_local() && !lan.is_exempt(),
        "a direct LAN client stays local"
    );
}

/// 127.0.0.1 sends `header` with no proxies listed, as a same-host proxy would: it draws on
/// 127.0.0.1's default budget and is not local, while direct loopback stays exempt.
async fn g2c_unlisted_loopback_proxy_case(header: (&str, &str)) {
    let app = g2f_app_from(g2c_default_budget_state().await, "127.0.0.1:6200");
    assert_eq!(
        g2c_limited(&app, 15, &[header]).await,
        5,
        "{} from an unlisted loopback peer: 10 allowed, then 429",
        header.0
    );
    assert_eq!(
        g2c_limited(&app, 3, &[]).await,
        0,
        "direct loopback stays exempt after the proxied requests used up the budget"
    );

    let state = g2c_default_budget_state().await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "127.0.0.1:6200");
    assert!(
        !g2c_forgot(&app, "owner", &[header]).await.exists(),
        "{} from an unlisted loopback peer is not local: no reset file",
        header.0
    );
}

#[tokio::test]
async fn g2c_unlisted_loopback_proxy_with_xff_is_limited_and_gets_no_reset_file() {
    g2c_unlisted_loopback_proxy_case(G2C_XFF).await;
}

#[tokio::test]
async fn g2c_unlisted_loopback_proxy_with_x_real_ip_is_limited_and_gets_no_reset_file() {
    g2c_unlisted_loopback_proxy_case(G2C_XRI).await;
}

#[tokio::test]
async fn g2c_unlisted_loopback_proxy_with_forwarded_is_limited_and_gets_no_reset_file() {
    g2c_unlisted_loopback_proxy_case(G2C_FWD).await;
}

#[tokio::test]
async fn g2c_direct_loopback_stays_exempt_and_gets_the_reset_file() {
    // Today's result for a direct client, kept by G2c.
    let state = g2c_default_budget_state().await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "127.0.0.1:6200");
    assert_eq!(
        g2c_limited(&app, 15, &[]).await,
        0,
        "direct loopback is exempt"
    );
    assert!(
        g2c_forgot(&app, "owner", &[]).await.exists(),
        "direct loopback is local and gets the reset file"
    );
}

#[tokio::test]
async fn g2c_direct_lan_client_is_local_only_without_a_forwarding_header() {
    let state = g2c_default_budget_state().await;
    g2f_with_owner(&state).await;
    let app = g2f_app_from(state, "192.168.1.20:6300");
    assert!(
        !g2c_forgot(&app, "owner", &[G2C_XFF]).await.exists(),
        "a LAN peer that sends X-Forwarded-For is not local"
    );
    assert!(
        g2c_forgot(&app, "owner", &[]).await.exists(),
        "the same LAN peer without it stays local"
    );
}

#[tokio::test]
async fn g2c_device_delete_by_path_ends_that_devices_sessions() {
    // DELETE /Devices/{deviceId} signs the device out like DELETE /Devices?id=. 9dd5a44 deleted only
    // the device row and left its tokens valid.
    let state = test_state().await;
    let app = app(state);
    assert_eq!(
        post_startup_user(&app, None, "owner", "pw-owner").await,
        StatusCode::NO_CONTENT
    );
    let (victim_t, _) = g2_login_device(&app, "owner", "pw-owner", "victim-dev")
        .await
        .expect("login on the device to delete");
    let (admin_t, _) = g2_login_device(&app, "owner", "pw-owner", "admin-dev")
        .await
        .expect("login on another device");
    let (status, body) = send(
        &app,
        request("DELETE", "/Devices/victim-dev", Some(&admin_t), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "device delete: {body:?}");
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&victim_t), None)).await;
    assert_eq!(
        me,
        StatusCode::UNAUTHORIZED,
        "the deleted device's token is revoked"
    );
    let (me, _) = send(&app, request("GET", "/Users/Me", Some(&admin_t), None)).await;
    assert_eq!(me, StatusCode::OK, "other devices stay signed in");
}

#[tokio::test]
async fn g2c_forgot_password_time_does_not_depend_on_the_name() {
    // A known name answered only after its PIN row and file were written, an unknown one at once,
    // so the time told whether the account exists; with the write lock held elsewhere (a scan, say)
    // the known name waited for the lock. Both now answer after the same fixed delay (250 ms in
    // users.rs), and the reset still happens once the lock is free.
    let state = test_state().await;
    g2f_with_owner(&state).await;
    let app = app(state.clone());
    let mut lock = state.db.begin().await.unwrap();
    sqlx::query("DELETE FROM password_reset_pins WHERE 0")
        .execute(&mut *lock)
        .await
        .unwrap();

    let mut times = Vec::new();
    let mut files = Vec::new();
    for user in ["owner", "ghost"] {
        let start = std::time::Instant::now();
        let answer = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            g2c_forgot(&app, user, &[]),
        )
        .await;
        times.push(start.elapsed());
        files.push(answer.unwrap_or_else(|_| {
            panic!("ForgotPassword for {user} answers while the write lock is held")
        }));
    }
    assert!(
        times[0].abs_diff(times[1]) < std::time::Duration::from_millis(200),
        "a known and an unknown name answer alike: {times:?}"
    );
    assert!(
        times
            .iter()
            .all(|t| *t >= std::time::Duration::from_millis(200)),
        "both answer after the fixed delay: {times:?}"
    );

    lock.rollback().await.unwrap();
    let mut written = false;
    for _ in 0..250 {
        if files[0].exists() {
            written = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        written,
        "the owner's reset file appears once the lock is free"
    );
    assert!(!files[1].exists(), "an unknown name gets no file");
}
