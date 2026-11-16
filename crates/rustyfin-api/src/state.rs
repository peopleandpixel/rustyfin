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
    /// `RUSTYFIN_PUBLISHED_URL`: the URL clients reach the server at, when the operator set one.
    /// UDP discovery's `EndpointAddress` and `LocalAddress` give it.
    pub published_url: Option<Arc<String>>,
    pub api_key: Option<Arc<String>>,
    /// Quick Connect requests keyed by secret. Code→secret lookup done by scanning.
    pub quick_connect: Arc<RwLock<HashMap<String, QuickConnectResult>>>,
    pub transcode_manager: TranscodeManager,
    pub task_scheduler: TaskScheduler,
    pub plugin_manager: PluginManager,
    pub rate_limiter: RateLimiter,
    /// Rate limiter for resource-intensive endpoints (transcoding, streaming start).
    pub resource_limiter: RateLimiter,
    /// Reverse-proxy networks we trust to set `X-Forwarded-For`. Empty = trust no proxy (loopback is
    /// always implicitly trusted). Configured from `RUSTYFIN_KNOWN_PROXIES`.
    pub known_proxies: Arc<Vec<IpNet>>,
    /// Broadcast channel for WebSocket real-time events
    pub ws_broadcast: broadcast::Sender<WsEvent>,
    /// The live log view's level (e.g. "info", "debug", "trace"); it starts at the most verbose
    /// level `RUST_LOG` prints
    pub log_level_tx: watch::Sender<String>,
    /// The server's recent log lines and the channel of new ones, for the live log view
    pub live_log: crate::log_stream::LogHistory,
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
    /// What the file checks keep between requests (see `FileGuardCache`).
    pub file_guard: FileGuardCache,
    /// What is playing on each session (see `sessions::SessionRuntime`).
    pub session_runtime: crate::sessions::SessionRuntime,
}

/// What the stored-path checks keep between requests, so a stream, subtitle or image request
/// runs no database query for them: the files the server owns outside its data dir (its
/// database, TLS key and certificate), read once, and every library's paths, resolved. The
/// roots are read again when a path falls outside the ones held, so a library added or changed
/// since counts at once.
///
/// The owned paths are also kept resolved (`OwnedSet`), with the time they were resolved, so a
/// request compares paths instead of resolving about ninety of them; they are resolved again
/// once they are a minute old.
#[derive(Clone, Default)]
pub struct FileGuardCache {
    pub owned_files: Arc<tokio::sync::OnceCell<Vec<PathBuf>>>,
    pub library_roots: Arc<std::sync::RwLock<Option<Arc<Vec<LibraryRoot>>>>>,
    pub owned_set: Arc<std::sync::RwLock<Option<KeptOwnedSet>>>,
}

/// A resolved `OwnedSet` and when it was resolved.
pub type KeptOwnedSet = (std::time::Instant, Arc<OwnedSet>);

/// The paths the server owns, resolved once (`routes::streaming` builds and reads it): the data
/// dir's forms, each owned data-dir folder that resolves (by name, with its target), and every
/// owned file in each form, with its SQLite siblings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnedSet {
    pub data_forms: Vec<PathBuf>,
    pub linked_dirs: Vec<(String, PathBuf)>,
    pub files: std::collections::HashSet<PathBuf>,
}

/// A library path as the file checks keep it: as written (made absolute, `.` dropped; None when
/// it has a `..` or is relative) and resolved (None when it can't be).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryRoot {
    pub written: Option<PathBuf>,
    pub resolved: Option<PathBuf>,
}
