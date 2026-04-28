use std::sync::Arc;

use tokio::sync::{watch, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use rustyfin_api::routes::build_router;
use rustyfin_api::state::AppState;
use rustyfin_model::server_info::ServerConfiguration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let data_dir = data_dir();
    std::fs::create_dir_all(&data_dir).ok();
    std::fs::create_dir_all(data_dir.join("logs")).ok();
    std::fs::create_dir_all(data_dir.join("cache")).ok();

    let db_url = std::env::var("RUSTYFIN_DB").unwrap_or_else(|_| {
        format!("sqlite://{}?mode=rwc", data_dir.join("rustyfin.db").display())
    });

    let jwt_secret = std::env::var("RUSTYFIN_SECRET")
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    let server_id = std::env::var("RUSTYFIN_SERVER_ID")
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    let bind_addr = std::env::var("RUSTYFIN_BIND").unwrap_or_else(|_| "0.0.0.0:8096".to_string());

    let api_key = std::env::var("RUSTYFIN_API_KEY").ok();

    // Load or create server configuration
    let config = load_config(&data_dir);

    tracing::info!("initializing database at {}", db_url);
    let pool = rustyfin_db::init_db(&db_url)
        .await
        .expect("failed to initialize database");

    // Create default admin user if no users exist
    let users = rustyfin_db::repo_user::get_all_users(&pool)
        .await
        .expect("failed to query users");

    if users.is_empty() {
        tracing::info!("no users found, creating default admin user (admin / admin)");
        rustyfin_auth::create_initial_user(&pool, "admin", "admin")
            .await
            .expect("failed to create initial user");
    }

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let state = AppState {
        db: pool,
        jwt_secret: Arc::new(jwt_secret),
        server_id: Arc::new(server_id),
        data_dir: Arc::new(data_dir),
        config: Arc::new(RwLock::new(config)),
        shutdown_tx: Arc::new(shutdown_tx),
        bind_addr: Arc::new(bind_addr.clone()),
        api_key: api_key.map(|k| Arc::new(k)),
        quick_connect: Arc::new(RwLock::new(std::collections::HashMap::new())),
    };

    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    tracing::info!("rustyfin listening on {}", bind_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
            tracing::info!("graceful shutdown initiated");
        })
        .await
        .expect("server error");

    tracing::info!("server stopped");
}

fn load_config(data_dir: &std::path::Path) -> ServerConfiguration {
    let config_path = data_dir.join("config.json");
    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(cfg) => {
                    tracing::info!("loaded configuration from {}", config_path.display());
                    return cfg;
                }
                Err(e) => {
                    tracing::warn!("failed to parse config.json, using defaults: {}", e);
                }
            },
            Err(e) => {
                tracing::warn!("failed to read config.json, using defaults: {}", e);
            }
        }
    }
    let cfg = ServerConfiguration::default();
    // Persist default config
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        std::fs::write(&config_path, &json).ok();
    }
    cfg
}

fn data_dir() -> std::path::PathBuf {
    std::env::var("RUSTYFIN_DATA")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".rustyfin"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}
