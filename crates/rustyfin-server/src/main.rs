mod credentials;

use std::net::SocketAddr;
use std::sync::Arc;
use tower::Service;

use tokio::sync::{RwLock, broadcast, watch};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use rustyfin_api::log_stream::LogStreamLayer;
use rustyfin_api::middleware::{
    DEFAULT_LOGIN_LIMIT, DEFAULT_LOGIN_WINDOW_SECS, DEFAULT_RESOURCE_LIMIT,
    DEFAULT_RESOURCE_WINDOW_SECS, RateLimiter,
};
use rustyfin_api::routes::build_router;
use rustyfin_api::state::AppState;
use rustyfin_core::image_proxy::{IMAGE_CACHE_MAX_BYTES, sweep_image_cache};
use rustyfin_model::plugin::Plugin;
use rustyfin_model::server_info::ServerConfiguration;

#[tokio::main]
async fn main() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());

    // Live log level channel — default "info", can be changed at runtime via API
    let (log_level_tx, log_level_rx) = watch::channel("info".to_string());

    // We need ws_broadcast before LogStreamLayer, but ws_broadcast is created later.
    // So we create it here and pass it to both LogStreamLayer and AppState.
    // Broadcast channel size increased to 256 to prevent event loss with many connected clients
    let (ws_broadcast, _) = broadcast::channel::<rustyfin_model::ws_event::WsEvent>(256);

    // Build the tracing subscriber with LogStreamLayer
    let use_json = std::env::var("RUSTYFIN_LOG_JSON")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    let log_layer = LogStreamLayer::new(ws_broadcast.clone(), Arc::new(log_level_rx));

    if use_json {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .json()
            .with_target(true)
            .with_thread_ids(true)
            .with_span_list(true)
            .finish()
            .with(log_layer)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .finish()
            .with(log_layer)
            .init();
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

    if std::env::var("RUSTYFIN_SECRET").is_ok() {
        tracing::info!(
            "RUSTYFIN_SECRET is set but no longer used; access tokens are stored server-side and survive restarts"
        );
    }

    let server_id = load_or_create_server_id(&data_dir);

    // IPv6 dual-stack: use [::]:8096 to listen on both IPv4 and IPv6
    let bind_addr = std::env::var("RUSTYFIN_BIND").unwrap_or_else(|_| "0.0.0.0:8096".to_string());

    let tls_cert = std::env::var("RUSTYFIN_TLS_CERT").ok();
    let tls_key = std::env::var("RUSTYFIN_TLS_KEY").ok();

    let api_key = credentials::api_key(std::env::var("RUSTYFIN_API_KEY").ok());

    let tmdb_settings = match rustyfin_core::tmdb_client::TmdbSettings::from_env() {
        Ok(settings) => {
            tracing::info!(
                "TMDb API at {}, images at {}, key from {}",
                settings.api_base_url,
                settings.image_base_url,
                if settings.env_key.is_some() {
                    "RUSTYFIN_TMDB_API_KEY"
                } else {
                    "the server config, if set"
                }
            );
            settings
        }
        Err(e) => {
            tracing::warn!(
                "TMDb settings not loaded ({}) — TMDb plugin will use defaults; set RUSTYFIN_TMDB_API_KEY to enable",
                e
            );
            rustyfin_core::tmdb_client::TmdbSettings::default()
        }
    };

    // Load or create server configuration
    let mut config = load_config(&data_dir);

    tracing::info!("initializing database at {}", db_url);
    let pool = rustyfin_db::init_db(&db_url)
        .await
        .expect("failed to initialize database");

    // Read-only pool for query-heavy operations (PRAGMA query_only = ON)
    let pool_ro = rustyfin_db::init_read_only_db(&db_url)
        .await
        .expect("failed to initialize read-only database pool");

    // No user is created at startup. A server with users gets an owner, its earliest enabled
    // administrator, and a closed setup wizard. A fresh one waits for the wizard.
    rustyfin_api::routes::settle_owner_at_startup(&pool, &mut config, &data_dir)
        .await
        .expect("failed to settle the server owner");

    // Warn about each enabled administrator who still has the default password "admin".
    match rustyfin_auth::admins_with_default_password(&pool).await {
        Ok(names) => {
            for n in names {
                tracing::warn!(
                    "administrator account '{}' still uses the default password 'admin'; please change it",
                    n
                );
            }
        }
        Err(e) => tracing::warn!("could not check for default admin passwords: {}", e),
    }

    // Makes the setup-code dummy hash now, so the first setup-code check after a start
    // is not slower.
    if let Err(e) = rustyfin_auth::prepare_setup_code_checks().await {
        tracing::warn!("could not prepare setup-code checks: {e}");
    }

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let transcode_manager = rustyfin_core::transcode::TranscodeManager::new(&data_dir);
    // Start periodic cleanup of stale transcode sessions and old cache directories
    transcode_manager.spawn_cleanup_task();

    let data_dir_arc = Arc::new(data_dir);

    // Shared HTTP client for external API calls (TMDB, etc.) - reuse connection pool
    let http_client = reqwest::Client::builder()
        .user_agent(rustyfin_core::tmdb_client::USER_AGENT)
        .pool_max_idle_per_host(10)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let task_scheduler = rustyfin_core::scheduler::TaskScheduler::with_http_client(
        pool.clone(),
        data_dir_arc.clone(),
        http_client.clone(),
    );
    task_scheduler
        .changes()
        .set_delay(std::time::Duration::from_secs(u64::from(
            config.library_monitor_delay,
        )));
    task_scheduler
        .watcher()
        .set_poll_interval(std::time::Duration::from_secs(u64::from(
            config.library_monitor_poll_interval.max(1),
        )));

    let plugin_manager = rustyfin_core::plugin_manager::PluginManager::new(&data_dir_arc);
    plugin_manager.load_all().await;

    // Register built-in plugins with database-backed configuration
    let plugin_manager_with_db = plugin_manager.with_database(pool.clone());

    // Helper: register a built-in plugin, loading config and status from DB if available.
    // Plugins are DISABLED by default — they must be explicitly enabled via the API or by
    // storing an Active status in the database. This ensures fast scans without external API calls.
    async fn register_builtin_plugin<P: rustyfin_model::plugin::Plugin>(
        plugin: &P,
        pm: &rustyfin_core::plugin_manager::PluginManager,
    ) {
        // Try to load saved config from DB
        let mut status = rustyfin_model::plugin::PluginStatus::Disabled;
        if let Some(saved_config) = pm.load_plugin_config_from_db(plugin.id()).await {
            if let Err(e) = plugin.set_configuration(saved_config.clone()) {
                tracing::warn!("failed to apply saved config for {}: {}", plugin.id(), e);
            }
            // Check if status was saved
            if let Some(status_str) = saved_config.get("status").and_then(|v| v.as_str()) {
                status = match status_str {
                    "Active" => rustyfin_model::plugin::PluginStatus::Active,
                    "Disabled" => rustyfin_model::plugin::PluginStatus::Disabled,
                    "Malfunctioned" => rustyfin_model::plugin::PluginStatus::Malfunctioned,
                    _ => rustyfin_model::plugin::PluginStatus::Disabled,
                };
            }
        }
        if status == rustyfin_model::plugin::PluginStatus::Active {
            tracing::info!("plugin {} enabled (from saved config)", plugin.id());
        }
        pm.register_builtin_plugin(rustyfin_core::plugin_manager::BuiltinPluginInfo {
            name: plugin.name().to_string(),
            version: plugin.version().to_string(),
            id: plugin.id().to_string(),
            description: plugin.description().to_string(),
            plugin_type: plugin.plugin_type(),
            status,
        })
        .await;
    }

    // Register TMDb plugin (disabled by default)
    {
        let tmdb_plugin = plugin_tmdb::TmdbPlugin::default();
        // Check env var for API key (configuration only; plugin is still disabled by default)
        if let Ok(key) = std::env::var("RUSTYFIN_TMDB_API_KEY")
            && !key.trim().is_empty()
        {
            let _ = tmdb_plugin.set_configuration(serde_json::json!({ "api_key": key }));
        }
        register_builtin_plugin(&tmdb_plugin, &plugin_manager_with_db).await;
        // Register as metadata provider — will be active only if the plugin is enabled
        if let Err(e) =
            plugin_tmdb::register_tmdb_provider(&tmdb_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("TMDb plugin not registered as provider (no API key): {}", e);
        }
    }

    // Register OMDb plugin (disabled by default)
    {
        let omdb_plugin = plugin_omdb::OmdbPlugin::default();
        register_builtin_plugin(&omdb_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_omdb::register_omdb_provider(&omdb_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("OMDb plugin not registered as provider (no API key): {}", e);
        }
    }

    // Register Fanart.tv plugin (disabled by default)
    {
        let fanart_plugin = plugin_fanart::FanartPlugin::default();
        register_builtin_plugin(&fanart_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_fanart::register_fanart_provider(&fanart_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!(
                "Fanart.tv plugin not registered as provider (no API key): {}",
                e
            );
        }
    }

    // Register Trakt plugin (disabled by default)
    {
        let trakt_plugin = plugin_trakt::TraktPlugin::default();
        register_builtin_plugin(&trakt_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_trakt::register_trakt_provider(&trakt_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Trakt plugin not registered as provider (no API key): {}", e);
        }
    }

    // Register TheTVDB plugin (disabled by default)
    {
        let tvdb_plugin = plugin_tvdb::TvdbPlugin::default();
        register_builtin_plugin(&tvdb_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_tvdb::register_tvdb_provider(&tvdb_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("TheTVDB plugin not registered as provider (no API key): {}", e);
        }
    }

    // Register Local NFO plugin (disabled by default)
    {
        let local_nfo_plugin = plugin_local_nfo::LocalNfoPlugin::default();
        register_builtin_plugin(&local_nfo_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_local_nfo::register_local_nfo_provider(&local_nfo_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Local NFO plugin not registered as provider: {}", e);
        }
    }

    // Register OpenSubtitles plugin (disabled by default)
    {
        let opensubtitles_plugin = plugin_opensubtitles::OpenSubtitlesPlugin::default();
        register_builtin_plugin(&opensubtitles_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_opensubtitles::register_opensubtitles_provider(&opensubtitles_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("OpenSubtitles plugin not registered as provider: {}", e);
        }
    }

    // Register Podnapisi plugin (disabled by default)
    {
        let podnapisi_plugin = plugin_podnapisi::PodnapisiPlugin::default();
        register_builtin_plugin(&podnapisi_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_podnapisi::register_podnapisi_provider(&podnapisi_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Podnapisi plugin not registered as provider: {}", e);
        }
    }

    // Register MQTT plugin (disabled by default)
    {
        let mqtt_plugin = plugin_mqtt::MqttPlugin::default();
        register_builtin_plugin(&mqtt_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_mqtt::register_mqtt_provider(&mqtt_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("MQTT plugin not registered as provider: {}", e);
        }
    }

    // Register Ntfy plugin (disabled by default)
    {
        let ntfy_plugin = plugin_ntfy::NtfyPlugin::default();
        register_builtin_plugin(&ntfy_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_ntfy::register_ntfy_provider(&ntfy_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Ntfy plugin not registered as provider: {}", e);
        }
    }

    // Register Pushover plugin (disabled by default)
    {
        let pushover_plugin = plugin_pushover::PushoverPlugin::default();
        register_builtin_plugin(&pushover_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_pushover::register_pushover_provider(&pushover_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Pushover plugin not registered as provider: {}", e);
        }
    }

    // Register Webhook plugin (disabled by default)
    {
        let webhook_plugin = plugin_webhook::WebhookPlugin::default();
        register_builtin_plugin(&webhook_plugin, &plugin_manager_with_db).await;
        if let Err(e) =
            plugin_webhook::register_webhook_provider(&webhook_plugin, &plugin_manager_with_db).await
        {
            tracing::debug!("Webhook plugin not registered as provider: {}", e);
        }
    }

    // Use the DB-backed plugin manager for the app state
    let plugin_manager = plugin_manager_with_db;

    // Rate limiter: default 10 requests per 60 seconds per resolved client IP, for the login
    // endpoints only (AuthenticateByName, AuthenticateWithSetupCode, ForgotPassword,
    // ForgotPassword/Pin, ResetPassword).
    // Configurable via env; 0 disables.
    let rate_max: usize = std::env::var("RUSTYFIN_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LOGIN_LIMIT);
    let rate_window: u64 = std::env::var("RUSTYFIN_RATE_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_LOGIN_WINDOW_SECS);
    let rate_limiter = RateLimiter::new(rate_max, rate_window);
    rate_limiter.start_cleanup();

    // Resource-intensive endpoint rate limiter (transcoding, streaming start)
    let resource_rate_max: usize = std::env::var("RUSTYFIN_RESOURCE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RESOURCE_LIMIT);
    let resource_rate_window: u64 = std::env::var("RUSTYFIN_RESOURCE_WINDOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_RESOURCE_WINDOW_SECS);
    let resource_limiter = RateLimiter::new(resource_rate_max, resource_rate_window);
    resource_limiter.start_cleanup();

    // Reverse proxies we trust to set X-Forwarded-For (comma-separated CIDRs or IPs). Loopback is
    // trusted only when listed.
    let known_proxies = Arc::new(rustyfin_api::middleware::parse_known_proxies(
        &std::env::var("RUSTYFIN_KNOWN_PROXIES").unwrap_or_default(),
    ));

    // In-memory response caches
    let cache = rustyfin_api::cache::ApiCache::new();
    let image_cache = rustyfin_api::cache::ImageCache::new();

    let tmdb = rustyfin_core::tmdb_client::TmdbClient::new(http_client.clone(), tmdb_settings);

    let state = AppState {
        db: pool,
        db_ro: pool_ro,
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
        resource_limiter,
        known_proxies,
        ws_broadcast,
        log_level_tx,
        cache,
        image_cache,
        http_client,
        tmdb,
        oidc: rustyfin_api::oidc::OidcState::new(),
        jellyfin_import: Default::default(),
        directory_listings: Default::default(),
    };

    // A scheduled library scan clears the same caches as a scan from the API or the monitor.
    {
        let cache = state.cache.clone();
        let image_cache = state.image_cache.clone();
        task_scheduler.set_on_scan(move || {
            cache.invalidate_on_scan();
            image_cache.invalidate_all();
        });
    }

    // Background task: scheduled tasks (library scan, metadata refresh, cache cleanup)
    {
        let shutdown_rx = state.shutdown_tx.subscribe();
        // Test switch: poll every library path, even on a local disk.
        if std::env::var("RUSTYFIN_MONITOR_FORCE_POLL").is_ok_and(|v| v == "1") {
            task_scheduler.watcher().set_force_poll(true);
        }
        task_scheduler.start(shutdown_rx);
    }

    // A wizard import queued before a restart resumes here; the job runs in its own task (B119).
    rustyfin_api::wizard_import::resume(&state).await;

    // Background task: once after an update, dates the items older versions stored by their files
    {
        let state = state.clone();
        tokio::spawn(async move { rustyfin_api::date_backfill::run(&state).await });
    }

    // Extract HTTP port for discovery services
    let http_port: u16 = bind_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8096);

    // Background task: SSDP discovery (DLNA), only when the DLNA server is on
    if state.config.read().await.enable_dlna_server {
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
    let sighup_changes = state.task_scheduler.changes().clone();
    let sighup_watcher = state.task_scheduler.watcher().clone();
    let cache = state.cache.clone();
    // Security headers: prevent clickjacking, MIME-sniffing, and restrict content sources
    use axum::http::header;
    let x_frame_options = header::HeaderValue::from_static("SAMEORIGIN");
    let x_content_type_options = header::HeaderValue::from_static("nosniff");
    let x_xss_protection = header::HeaderValue::from_static("0");
    let referrer_policy = header::HeaderValue::from_static("strict-origin-when-cross-origin");

    // CORS: configurable origins via RUSTYFIN_CORS_ORIGINS (comma-separated).
    // Defaults to a permissive-mirror mode so the web client works from any origin.
    // Set to a comma-separated list to restrict (e.g. "https://app.example.com").
    let cors_origins = std::env::var("RUSTYFIN_CORS_ORIGINS").unwrap_or_default();
    let cors_layer = if cors_origins.is_empty() {
        // Default: mirror the request origin (reflects back whatever Origin the browser sends).
        // This is safer than wildcard (*) when credentials are involved, and still allows
        // any origin. For production, set RUSTYFIN_CORS_ORIGINS to restrict.
        CorsLayer::new()
            .allow_origin(AllowOrigin::mirror_request())
            .allow_methods(AllowMethods::any())
            .allow_headers(AllowHeaders::any())
            .max_age(std::time::Duration::from_secs(86400))
    } else {
        // Parse comma-separated origins into a fixed list
        let origins: Vec<header::HeaderValue> = cors_origins
            .split(',')
            .filter_map(|s| header::HeaderValue::from_str(s.trim()).ok())
            .collect();
        if origins.is_empty() {
            tracing::warn!(
                "RUSTYFIN_CORS_ORIGINS contains no valid origins — falling back to mirror mode"
            );
            CorsLayer::new()
                .allow_origin(AllowOrigin::mirror_request())
                .allow_methods(AllowMethods::any())
                .allow_headers(AllowHeaders::any())
                .max_age(std::time::Duration::from_secs(86400))
        } else {
            tracing::info!("CORS origins restricted to: {:?}", origins);
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods(AllowMethods::any())
                .allow_headers(AllowHeaders::any())
                .max_age(std::time::Duration::from_secs(86400))
        }
    };

    // Request body size limit: 50 MB for API requests (uploads use separate endpoints)
    const MAX_BODY_SIZE: usize = 50 * 1024 * 1024;

    let app = build_router(state)
        .layer(CompressionLayer::new())
        .layer(cors_layer)
        .layer(SetResponseHeaderLayer::<_>::if_not_present(
            header::X_FRAME_OPTIONS,
            x_frame_options,
        ))
        .layer(SetResponseHeaderLayer::<_>::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            x_content_type_options,
        ))
        .layer(SetResponseHeaderLayer::<_>::if_not_present(
            header::HeaderName::from_static("x-xss-protection"),
            x_xss_protection,
        ))
        .layer(SetResponseHeaderLayer::<_>::if_not_present(
            header::REFERRER_POLICY,
            referrer_policy,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .layer(TraceLayer::new_for_http().make_span_with(request_span));

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

    // Background task: cleanup expired L2 cache entries every 6 hours, and trim the image copies
    {
        // `state` has gone into the router by now; this is the same data directory
        let data_dir = sighup_data_dir.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600 * 6));
            loop {
                interval.tick().await;
                cache.cleanup_l2_cache().await;
                let data_dir = data_dir.clone();
                match tokio::task::spawn_blocking(move || {
                    sweep_image_cache(
                        &data_dir.join("cache").join("images"),
                        IMAGE_CACHE_MAX_BYTES,
                    )
                })
                .await
                {
                    Ok(Ok(report)) if report.removed > 0 => {
                        tracing::info!("image cache sweep: {report:?}")
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::warn!("image cache sweep error: {e}"),
                    Err(e) => tracing::warn!("image cache sweep task failed: {e}"),
                }
            }
        });
    }

    // Background task: handle SIGHUP for config reload
    {
        let config = sighup_config;
        let data_dir = sighup_data_dir;
        let changes = sighup_changes;
        let watcher = sighup_watcher;
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
                        changes.set_delay(std::time::Duration::from_secs(u64::from(
                            cfg.library_monitor_delay,
                        )));
                        watcher.set_poll_interval(std::time::Duration::from_secs(u64::from(
                            cfg.library_monitor_poll_interval.max(1),
                        )));
                        watcher.libraries_changed();
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

/// The server id is stable across restarts. `RUSTYFIN_SERVER_ID` wins. Otherwise the id is read
/// from `<data_dir>/server_id`, or on first start generated and written there.
fn load_or_create_server_id(data_dir: &std::path::Path) -> String {
    if let Ok(id) = std::env::var("RUSTYFIN_SERVER_ID")
        && !id.is_empty()
    {
        return id;
    }

    let path = data_dir.join("server_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return existing.to_string();
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    // Write a temp file and rename it into place, so a crash cannot leave a partial id behind.
    let tmp = data_dir.join("server_id.tmp");
    if std::fs::write(&tmp, &id).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    id
}

/// The span of one request: the method, the path without the query string, and the HTTP version.
/// Query strings carry `api_key` tokens and the OIDC callback's `code` and `state` (AC43).
fn request_span<B>(request: &axum::http::Request<B>) -> tracing::Span {
    tracing::debug_span!(
        "request",
        method = %request.method(),
        uri = %request.uri().path(),
        version = ?request.version(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Removes a test's temp dir when dropped, also when an assertion fails.
    struct RemoveDir(std::path::PathBuf);

    impl Drop for RemoveDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn server_id_is_generated_once_then_reloaded() {
        let dir = std::env::temp_dir().join(format!("rustyfin_server_id_{}", uuid::Uuid::new_v4()));
        let _remove = RemoveDir(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();

        let first = load_or_create_server_id(&dir);
        let second = load_or_create_server_id(&dir);

        assert!(!first.is_empty());
        assert_eq!(first, second, "a later start reloads the same id");
        assert_eq!(
            std::fs::read_to_string(dir.join("server_id"))
                .unwrap()
                .trim(),
            first,
            "the id is persisted in server_id"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod request_log_tests;
