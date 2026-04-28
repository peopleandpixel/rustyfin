use rustyfin_model::quick_connect::QuickConnectResult;
use rustyfin_model::server_info::ServerConfiguration;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub jwt_secret: Arc<String>,
    pub server_id: Arc<String>,
    pub data_dir: Arc<PathBuf>,
    pub config: Arc<RwLock<ServerConfiguration>>,
    pub shutdown_tx: Arc<watch::Sender<bool>>,
    pub bind_addr: Arc<String>,
    pub api_key: Option<Arc<String>>,
    /// Quick Connect requests keyed by secret. Code→secret lookup done by scanning.
    pub quick_connect: Arc<RwLock<HashMap<String, QuickConnectResult>>>,
}
