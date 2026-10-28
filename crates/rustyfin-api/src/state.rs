use crate::cache::{ApiCache, ImageCache};
use crate::middleware::RateLimiter;
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

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    /// Read-only connection pool for query-heavy operations (PRAGMA query_only = ON).
    pub db_ro: SqlitePool,
    pub jwt_secret: Arc<String>,
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
}
