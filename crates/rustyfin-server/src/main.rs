use std::net::SocketAddr;
use std::sync::Arc;
use tower::Service;

use tokio::sync::{RwLock, broadcast, watch};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use rustyfin_api::middleware::RateLimiter;
use rustyfin_api::routes::build_router;
use rustyfin_api::state::AppState;
use rustyfin_model::server_info::ServerConfiguration;

#[tokio::main]
async fn main() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let use_json = std::env::var("RUSTYFIN_LOG_JSON")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    if use_json {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .json()
            .with_target(true)
            .with_thread_ids(true)
            .with_span_list(true)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    let data_dir = data_dir();
    // Create required data directories at startup
    std::fs::create_dir_all(&data_dir).ok();
    std::fs::create_dir_all(data_dir.join("logs")).ok();
    std::fs::create_dir_all(data_dir.join("cache")).ok();
    std::fs::create_dir_all(data_dir.join("cache/subtitles")).ok();
    std::fs::create_dir_all(data_dir.join("cache/images")).ok();
    std::fs::create_dir_all(data_dir.join("transcodes")).ok();
    std::fs::create_dir_all(data_dir.join("trickplay")).ok();

    // Validate FFmpeg exists and is accessible
    if let Ok(ffmpeg_path) = std::env::var("RUSTYFIN_FFMPEG_PATH") {
        if !std::path::Path::new(&ffmpeg_path).exists() {
            tracing::warn!(
                "FFmpeg path '{}' does not exist — transcoding will fail",
                ffmpeg_path
            );
        } else {
            tracing::info!("FFmpeg found at: {}", ffmpeg_path);
        }
    } else if which::which("ffmpeg").is_ok() {
        tracing::info!("FFmpeg found in PATH");
    } else {
        tracing::warn!(
            "FFmpeg not found — transcoding will not work. Install FFmpeg or set RUSTYFIN_FFMPEG_PATH"
        );
    }

    let db_url = std::env::var("RUSTYFIN_DB")
        .unwrap_or_else(|_| data_dir.join("rustyfin.db").display().to_string());

    let jwt_secret =
        std::env::var("RUSTYFIN_SECRET").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    let server_id =
        std::env::var("RUSTYFIN_SERVER_ID").unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());

    // IPv6 dual-stack: use [::]:8096 to listen on both IPv4 and IPv6
    let bind_addr = std::env::var("RUSTYFIN_BIND").unwrap_or_else(|_| "0.0.0.0:8096".to_string());

    let tls_cert = std::env::var("RUSTYFIN_TLS_CERT").ok();
    let tls_key = std::env::var("RUSTYFIN_TLS_KEY").ok();

    let api_key = std::env::var("RUSTYFIN_API_KEY").ok();

    // Load or create server configuration
    let config = load_config(&data_dir);

    tracing::info!("initializing database at {}", db_url);
    let pool = rustyfin_db::init_db(&db_url)
        .await
        .expect("failed to initialize database");

    // Read-only pool for query-heavy operations (PRAGMA query_only = ON)
    let pool_ro = rustyfin_db::init_read_only_db(&db_url)
        .await
        .expect("failed to initialize read-only database pool");

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

    let transcode_manager = rustyfin_core::transcode::TranscodeManager::new(&data_dir);

    let data_dir_arc = Arc::new(data_dir);
    let task_scheduler =
        rustyfin_core::scheduler::TaskScheduler::new(pool.clone(), data_dir_arc.clone());

    let plugin_manager = rustyfin_core::plugin_manager::PluginManager::new(&data_dir_arc);
    plugin_manager.load_all().await;

    // Rate limiter: 100 requests per 60 seconds per IP (configurable via env)
    let rate_max: usize = std::env::var("RUSTYFIN_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let rate_window: u64 = std::env::var("RUSTYFIN_RATE_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let rate_limiter = RateLimiter::new(rate_max, rate_window);
    rate_limiter.start_cleanup();

    // WebSocket broadcast channel (capacity 32 for buffered events)
    let (ws_broadcast, _) = broadcast::channel::<rustyfin_model::ws_event::WsEvent>(32);

    // In-memory response caches
    let cache = rustyfin_api::cache::ApiCache::new();
    let image_cache = rustyfin_api::cache::ImageCache::new();

    let state = AppState {
        db: pool,
        db_ro: pool_ro,
        jwt_secret: Arc::new(jwt_secret),
        server_id: Arc::new(server_id),
        data_dir: data_dir_arc,
        config: Arc::new(RwLock::new(config)),
        shutdown_tx: Arc::new(shutdown_tx),
        bind_addr: Arc::new(bind_addr.clone()),
        api_key: api_key.map(Arc::new),
        quick_connect: Arc::new(RwLock::new(std::collections::HashMap::new())),
        transcode_manager,
        task_scheduler: task_scheduler.clone(),
        plugin_manager,
        rate_limiter,
        ws_broadcast,
        cache,
        image_cache,
    };

    // Background task: scheduled tasks (library scan, metadata refresh, cache cleanup)
    {
        let shutdown_rx = state.shutdown_tx.subscribe();
        task_scheduler.start(shutdown_rx);
    }

    // Extract HTTP port for discovery services
    let http_port: u16 = bind_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8096);

    // Background task: SSDP discovery (DLNA)
    {
        let dlna_config = std::sync::Arc::new(rustyfin_core::dlna::DlnaConfig {
            server_name: "Rustyfin".to_string(),
            server_id: state.server_id.to_string(),
            http_port,
            bind_ip: std::net::Ipv4Addr::UNSPECIFIED,
        });
        let shutdown_rx = state.shutdown_tx.subscribe();
        let cfg = dlna_config.clone();
        tokio::spawn(async move {
            rustyfin_core::dlna::run_ssdp_responder(cfg, shutdown_rx).await;
        });
        let shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            rustyfin_core::dlna::run_ssdp_advertiser(dlna_config, shutdown_rx).await;
        });
    }

    // Background task: UDP discovery (Jellyfin client auto-discovery on port 7359)
    {
        let udp_config = std::sync::Arc::new(rustyfin_core::udp_discovery::UdpDiscoveryConfig {
            server_name: "Rustyfin".to_string(),
            server_id: state.server_id.to_string(),
            http_port,
            bind_ip: std::net::Ipv4Addr::UNSPECIFIED,
            endpoint_address: std::env::var("RUSTYFIN_PUBLISHED_URL").ok(),
        });
        let shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            rustyfin_core::udp_discovery::run_udp_discovery(udp_config, shutdown_rx).await;
        });
    }

    // Background task: mDNS/Bonjour service discovery
    {
        let mdns_config = std::sync::Arc::new(rustyfin_core::mdns::MdnsConfig {
            server_name: "Rustyfin".to_string(),
            server_id: state.server_id.to_string(),
            http_port,
            bind_ip: std::net::Ipv4Addr::UNSPECIFIED,
        });
        let shutdown_rx = state.shutdown_tx.subscribe();
        tokio::spawn(async move {
            rustyfin_core::mdns::run_mdns_responder(mdns_config, shutdown_rx).await;
        });
    }

    let cleanup_db = state.db.clone();
    let sighup_config = state.config.clone();
    let sighup_data_dir = state.data_dir.clone();
    let app = build_router(state)
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind");

    // Log detected local port (automatic port detection when using port 0)
    let local_addr = listener.local_addr().expect("failed to get local address");
    tracing::info!("rustyfin listening on {}", local_addr);

    // Background task: cleanup expired sessions every 5 minutes
    {
        let db = cleanup_db;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                match rustyfin_db::repo_session::cleanup_expired_sessions(&db, 30).await {
                    Ok(n) if n > 0 => tracing::info!("cleaned up {n} expired sessions"),
                    Err(e) => tracing::warn!("session cleanup error: {e}"),
                    _ => {}
                }
            }
        });
    }

    // Background task: handle SIGHUP for config reload
    {
        let config = sighup_config;
        let data_dir = sighup_data_dir;
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut sig =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                        .expect("failed to install SIGHUP handler");
                loop {
                    if sig.recv().await.is_some() {
                        tracing::info!("SIGHUP received, reloading configuration");
                        let new_config = load_config(&data_dir);
                        let mut cfg = config.write().await;
                        *cfg = new_config;
                        tracing::info!("configuration reloaded");
                    }
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix platforms, just loop forever (SIGHUP not available)
                tokio::signal::ctrl_c().await.ok();
            }
        });
    }

    // TLS support via rustls (set RUSTYFIN_TLS_CERT and RUSTYFIN_TLS_KEY)
    if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
        let tls_config = load_tls_config(&cert_path, &key_path);
        tracing::info!("TLS enabled with cert={}, key={}", cert_path, key_path);
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));

        // Also start a plain HTTP listener that redirects if RUSTYFIN_BIND_HTTP is set
        // For now, serve HTTPS only on the main bind address
        tracing::info!("HTTPS server ready – clients must connect via https://");

        let mut make_svc = app.into_make_service_with_connect_info::<SocketAddr>();
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((tcp_stream, remote_addr)) => {
                            let acceptor = acceptor.clone();
                            let svc = make_svc.call(remote_addr).await.unwrap();
                            tokio::spawn(async move {
                                match acceptor.accept(tcp_stream).await {
                                    Ok(tls_stream) => {
                                        let stream = hyper_util::rt::TokioIo::new(tls_stream);
                                        let hyper_svc = hyper_util::service::TowerToHyperService::new(svc);
                                        let _ = hyper_util::server::conn::auto::Builder::new(
                                            hyper_util::rt::TokioExecutor::new(),
                                        )
                                        .serve_connection(stream, hyper_svc)
                                        .await;
                                    }
                                    Err(e) => tracing::debug!("TLS handshake failed: {}", e),
                                }
                            });
                        }
                        Err(e) => tracing::warn!("accept error: {}", e),
                    }
                }
                _ = shutdown_rx.wait_for(|v| *v) => {
                    tracing::info!("graceful shutdown initiated");
                    break;
                }
            }
        }
    } else {
        // Plain HTTP with ConnectInfo for reverse-proxy header support
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.wait_for(|v| *v).await;
            tracing::info!("graceful shutdown initiated");
        })
        .await
        .expect("server error");
    }

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

fn load_tls_config(cert_path: &str, key_path: &str) -> rustls::ServerConfig {
    let cert_file = std::fs::File::open(cert_path).expect("failed to open TLS certificate file");
    let key_file = std::fs::File::open(key_path).expect("failed to open TLS key file");

    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut std::io::BufReader::new(cert_file))
            .filter_map(|r| r.ok())
            .collect();

    let key = rustls_pemfile::private_key(&mut std::io::BufReader::new(key_file))
        .expect("failed to read TLS private key")
        .expect("no private key found in key file");

    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("failed to build TLS config")
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
