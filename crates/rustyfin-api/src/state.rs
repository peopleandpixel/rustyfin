use crate::cache::{ApiCache, ImageCache};
use crate::middleware::RateLimiter;
use ipnet::IpNet;
use reqwest::Client;
use rustyfin_core::plugin_manager::PluginManager;
use rustyfin_core::scheduler::TaskScheduler;
use rustyfin_core::transcode::TranscodeManager;
use rustyfin_model::quick_connect::QuickConnectResult;
use rustyfin_model::server_info::ServerConfiguration;
use rustyfin_model::ws_event::WsEvent;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, watch};

/// Caps the folder listings (GET /Environment/DirectoryContents) that run at once, so a hung mount
/// can't take every blocking thread. Each listing holds one permit until its read ends.
#[derive(Clone)]
pub struct DirectoryListings(pub Arc<tokio::sync::Semaphore>);

impl DirectoryListings {
    pub const MAX: usize = 4;
}

impl Default for DirectoryListings {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(Self::MAX)))
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    /// Read-only connection pool for query-heavy operations (PRAGMA query_only = ON).
    pub db_ro: SqlitePool,
    pub server_id: Arc<String>,
    pub data_dir: Arc<PathBuf>,
    pub config: Arc<RwLock<ServerConfiguration>>,
    pub shutdown_tx: Arc<watch::Sender<bool>>,
    pub bind_addr: Arc<String>,
    pub api_key: Option<Arc<String>>,
    /// Quick Connect requests keyed by secret. Code→secret lookup done by scanning.
    pub quick_connect: Arc<RwLock<HashMap<String, QuickConnectResult>>>,
    pub transcode_manager: TranscodeManager,
    pub task_scheduler: TaskScheduler,
    pub plugin_manager: PluginManager,
    pub rate_limiter: RateLimiter,
    /// Reverse-proxy networks we trust to set `X-Forwarded-For`. Empty = trust no proxy (loopback is
    /// always implicitly trusted). Configured from `RUSTYFIN_KNOWN_PROXIES`.
    pub known_proxies: Arc<Vec<IpNet>>,
    /// Broadcast channel for WebSocket real-time events
    pub ws_broadcast: broadcast::Sender<WsEvent>,
    /// Watch channel to control live log verbosity level (e.g. "info", "debug", "trace")
    pub log_level_tx: watch::Sender<String>,
    /// In-memory response caches for API endpoints
    pub cache: ApiCache,
    /// In-memory cache for image proxy results
    pub image_cache: ImageCache,
    /// Shared HTTP client for external API calls (TMDB, etc.)
    pub http_client: Client,
    /// Shared TMDb client (rate limiter, retries, response cache). Cloning shares them.
    pub tmdb: rustyfin_core::tmdb_client::TmdbClient,
    /// OIDC provider cache and HTTP client (discovery and JWKS are fetched through it).
    pub oidc: crate::oidc::OidcState,
    /// The one import from Jellyfin this server runs at a time, and its status.
    pub jellyfin_import: rustyfin_core::jellyfin_import::ImportJob,
    /// Permits for the folder listings that run at once (see `DirectoryListings`).
    pub directory_listings: DirectoryListings,
}
