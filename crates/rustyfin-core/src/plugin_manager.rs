use sqlx::SqlitePool;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing;

use plugin_fanart::MetadataProviderRegistrar as FanartRegistrar;
use plugin_local_nfo::MetadataProviderRegistrar as LocalNfoRegistrar;
use plugin_mqtt::NotificationProviderRegistrar as MqttRegistrar;
use plugin_ntfy::NotificationProviderRegistrar as NtfyRegistrar;
use plugin_omdb::MetadataProviderRegistrar as OmdbRegistrar;
use plugin_opensubtitles::SubtitleProviderRegistrar as OpenSubtitlesRegistrar;
use plugin_podnapisi::SubtitleProviderRegistrar as PodnapisiRegistrar;
use plugin_pushover::NotificationProviderRegistrar as PushoverRegistrar;
use plugin_tmdb::MetadataProviderRegistrar as TmdbRegistrar;
use plugin_trakt::MetadataProviderRegistrar as TraktRegistrar;
use plugin_tvdb::MetadataProviderRegistrar as TvdbRegistrar;
use plugin_webhook::NotificationProviderRegistrar as WebhookRegistrar;
use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{
    NotificationPriority, Plugin, PluginConfiguration, PluginInfo, PluginStatus, PluginType,
    SubtitleResult,
};

/// A loaded plugin with its dynamic library handle.
struct LoadedPlugin {
    plugin: Box<dyn Plugin>,
    _library: Option<libloading::Library>,
    status: PluginStatus,
    config_file: Option<String>,
    /// True if this is a built-in (statically registered) plugin.
    is_builtin: bool,
}

// ── FFI callbacks for dynamic plugin registration ──

/// Opaque handle passed across FFI so a dynamically loaded plugin can call back into
/// the host to register itself as a provider.
#[repr(C)]
pub struct PluginRegistrarCallbacks {
    /// Register this plugin as a metadata provider.
    /// The plugin passes function pointers for search_movie, search_tv, and get_images.
    register_metadata_provider: Option<
        unsafe extern "C" fn(
            *mut std::ffi::c_void,   // manager handle (reserved)
            *const std::ffi::c_char, // plugin_id
            *const std::ffi::c_char, // provider_name
            MetadataSearchMovieFn,
            MetadataSearchTvFn,
            MetadataGetImagesFn,
        ),
    >,
    /// Register this plugin as a subtitle provider.
    register_subtitle_provider: Option<
        unsafe extern "C" fn(
            *mut std::ffi::c_void,   // manager handle (reserved)
            *const std::ffi::c_char, // plugin_id
            SubtitleSearchFn,
            SubtitleDownloadFn,
        ),
    >,
    /// Register this plugin as a notification provider.
    register_notification_provider: Option<
        unsafe extern "C" fn(
            *mut std::ffi::c_void,   // manager handle (reserved)
            *const std::ffi::c_char, // plugin_id
            NotificationSendFn,
        ),
    >,
}

// FFI function pointer types for metadata providers
type MetadataSearchMovieFn = Option<
    unsafe extern "C" fn(
        *const std::ffi::c_void,
        *const std::ffi::c_char,
        i32,
        i8,
    ) -> *mut std::ffi::c_void,
>;
type MetadataSearchTvFn = Option<
    unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_char) -> *mut std::ffi::c_void,
>;
type MetadataGetImagesFn = Option<
    unsafe extern "C" fn(
        *const std::ffi::c_void,
        i64,
        i8,
        i64,
        i8,
        i32, // media_type: 0=movie, 1=tv
    ) -> *mut std::ffi::c_void,
>;

// FFI function pointer types for subtitle providers
type SubtitleSearchFn = Option<
    unsafe extern "C" fn(
        *const std::ffi::c_void,
        *const std::ffi::c_char, // item json
        *const std::ffi::c_char, // language
    ) -> *mut std::ffi::c_void,
>;
type SubtitleDownloadFn = Option<
    unsafe extern "C" fn(
        *const std::ffi::c_void,
        *const std::ffi::c_char, // subtitle_id
        *const std::ffi::c_char, // path
    ) -> *mut std::ffi::c_void,
>;

// FFI function pointer type for notification providers
type NotificationSendFn = Option<
    unsafe extern "C" fn(
        *const std::ffi::c_void,
        *const std::ffi::c_char, // title
        *const std::ffi::c_char, // message
        i32,                     // priority: 0=Low, 1=Normal, 2=High, 3=Emergency
    ) -> *mut std::ffi::c_void,
>;

impl PluginRegistrarCallbacks {
    /// Create a callbacks struct wired to the given PluginManager.
    /// For now this returns a no-op struct — the async registration bridge is a future enhancement.
    /// The struct is FFI-safe and can be extended when async FFI support is added.
    pub fn new(_manager: &PluginManager) -> Self {
        Self {
            register_metadata_provider: None,
            register_subtitle_provider: None,
            register_notification_provider: None,
        }
    }
}

// Ensure FFI safety
unsafe impl Send for PluginRegistrarCallbacks {}
unsafe impl Sync for PluginRegistrarCallbacks {}

// Type aliases for complex async closure types (avoids clippy type_complexity)
type SearchMovieFn = Arc<
    dyn Fn(
            &reqwest::Client,
            &str,
            Option<i32>,
        ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
        + Send
        + Sync,
>;
type SearchTvFn = Arc<
    dyn Fn(&reqwest::Client, &str) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
        + Send
        + Sync,
>;
type GetImagesFn = Arc<
    dyn Fn(
            &reqwest::Client,
            Option<i64>,
            Option<i64>,
            MediaType,
        ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
        + Send
        + Sync,
>;
type SearchSubtitlesFn = Arc<
    dyn Fn(
            &reqwest::Client,
            &MediaItem,
            &str,
        ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>>
        + Send
        + Sync,
>;
type DownloadSubtitleFn = Arc<
    dyn Fn(
            &reqwest::Client,
            &str,
            &std::path::Path,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;
type SendNotificationFn = Arc<
    dyn Fn(
            &str,
            &str,
            NotificationPriority,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Runs after `set_configuration` saves a built-in plugin's config. `Some(status)` updates the
/// plugin's in-memory status (the DB is not written); `None` leaves it alone.
pub type ConfigHook = Arc<dyn Fn(&serde_json::Value) -> Option<PluginStatus> + Send + Sync>;

/// A built-in plugin registered via registrar traits (no dynamic library).
#[derive(Clone)]
pub struct BuiltinPluginInfo {
    pub name: String,
    pub version: String,
    pub id: String,
    pub description: String,
    pub plugin_type: PluginType,
    pub status: PluginStatus,
}

/// Metadata provider handle stored in the plugin manager.
#[derive(Clone)]
pub struct MetadataProviderHandle {
    pub plugin_id: String,
    pub provider_name: String,
    pub priority: i32,
    pub status: PluginStatus,
    pub search_movie: SearchMovieFn,
    pub search_tv: SearchTvFn,
    pub get_images: GetImagesFn,
}

/// Subtitle provider handle stored in the plugin manager.
#[derive(Clone)]
pub struct SubtitleProviderHandle {
    pub plugin_id: String,
    pub priority: i32,
    pub status: PluginStatus,
    pub search_subtitles: SearchSubtitlesFn,
    pub download_subtitle: DownloadSubtitleFn,
}

/// Notification provider handle stored in the plugin manager.
#[derive(Clone)]
pub struct NotificationProviderHandle {
    pub plugin_id: String,
    pub priority: i32,
    pub status: PluginStatus,
    #[allow(dead_code)]
    send_notification: SendNotificationFn,
}

/// Install the built-in TMDb plugin. It is enabled by a key (`RUSTYFIN_TMDB_API_KEY` or a saved
/// `api_key`) unless a saved status says otherwise. A key saved later through the plugin's
/// configuration enables it at once, without a restart. The env key is never written to the
/// plugin config or the DB.
pub async fn install_tmdb_plugin(pm: &PluginManager, client: crate::tmdb_client::TmdbClient) {
    let plugin = Arc::new(plugin_tmdb::TmdbPlugin::default());
    let saved = pm.load_plugin_config_from_db("tmdb").await;
    if let Some(config) = &saved
        && let Err(e) = Plugin::set_configuration(plugin.as_ref(), config.clone())
    {
        tracing::warn!("failed to apply saved config for tmdb: {}", e);
    }

    let env_key = client.has_env_key();
    let status = plugin_tmdb::status_for(saved.as_ref(), env_key);
    let saved_status = saved
        .as_ref()
        .and_then(|c| c.get("status"))
        .and_then(|v| v.as_str());
    let saved_key = saved
        .as_ref()
        .and_then(|c| c.get("api_key"))
        .and_then(|v| v.as_str())
        .is_some_and(|k| !k.trim().is_empty());
    let reason = match (saved_status, env_key, saved_key) {
        (Some(s), _, _) => format!("saved status {s}"),
        (None, true, _) => "RUSTYFIN_TMDB_API_KEY is set".to_string(),
        (None, false, true) => "saved API key".to_string(),
        (None, false, false) => "no API key (set RUSTYFIN_TMDB_API_KEY or save one)".to_string(),
    };
    tracing::info!("TMDb plugin status {:?}: {}", status, reason);

    pm.register_builtin_plugin(BuiltinPluginInfo {
        name: plugin.name().to_string(),
        version: plugin.version().to_string(),
        id: plugin.id().to_string(),
        description: plugin.description().to_string(),
        plugin_type: plugin.plugin_type(),
        status,
    })
    .await;

    // Always registers; the provider reads the plugin's current key on each call.
    if let Err(e) = plugin_tmdb::register_tmdb_provider(&plugin, client, pm).await {
        tracing::debug!("TMDb plugin not registered as provider: {}", e);
    }

    let hook_plugin = plugin.clone();
    pm.register_config_hook(
        "tmdb",
        Arc::new(move |config: &serde_json::Value| {
            if let Err(e) = Plugin::set_configuration(hook_plugin.as_ref(), config.clone()) {
                tracing::warn!("failed to apply tmdb config: {}", e);
            }
            Some(plugin_tmdb::status_for(Some(config), env_key))
        }),
    )
    .await;
}

/// Manages loading, unloading, and querying plugins.
#[derive(Clone)]
pub struct PluginManager {
    inner: Arc<RwLock<PluginManagerInner>>,
    /// Database pool for persistent plugin configuration storage.
    db: Option<SqlitePool>,
}

struct PluginManagerInner {
    plugins: HashMap<String, LoadedPlugin>,
    plugins_dir: PathBuf,
    /// Built-in plugins registered via registrar traits
    builtin_plugins: HashMap<String, BuiltinPluginInfo>,
    /// Registered metadata providers from plugins
    metadata_providers: Vec<MetadataProviderHandle>,
    /// Registered subtitle providers from plugins
    subtitle_providers: Vec<SubtitleProviderHandle>,
    /// Registered notification providers from plugins
    notification_providers: Vec<NotificationProviderHandle>,
    /// Per-plugin hooks that run after `set_configuration`
    config_hooks: HashMap<String, ConfigHook>,
}

impl PluginManager {
    /// Create a new plugin manager. `plugins_dir` is the directory to scan for `.so`/`.dylib`/`.dll` files.
    pub fn new(data_dir: &Path) -> Self {
        let plugins_dir = data_dir.join("plugins");
        std::fs::create_dir_all(&plugins_dir).ok();
        Self {
            inner: Arc::new(RwLock::new(PluginManagerInner {
                plugins: HashMap::new(),
                plugins_dir,
                builtin_plugins: HashMap::new(),
                metadata_providers: Vec::new(),
                subtitle_providers: Vec::new(),
                notification_providers: Vec::new(),
                config_hooks: HashMap::new(),
            })),
            db: None,
        }
    }

    /// Set the database pool for persistent plugin configuration storage.
    pub fn with_database(mut self, pool: SqlitePool) -> Self {
        self.db = Some(pool);
        self
    }

    /// Load plugin configuration from the database and apply it to the in-memory plugin.
    pub async fn load_plugin_config_from_db(&self, plugin_id: &str) -> Option<serde_json::Value> {
        let db = self.db.as_ref()?;
        match rustyfin_db::repo_plugin_config::get_plugin_config(db, plugin_id).await {
            Ok(Some(config)) => {
                tracing::debug!("loaded plugin config from DB for {}", plugin_id);
                // Apply to in-memory plugin if loaded
                {
                    let inner = self.inner.read().await;
                    if let Some(lp) = inner.plugins.get(plugin_id) {
                        let _ = lp.plugin.set_configuration(config.clone());
                    }
                }
                Some(config)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(
                    "failed to load plugin config from DB for {}: {}",
                    plugin_id,
                    e
                );
                None
            }
        }
    }

    /// Save plugin configuration to the database.
    pub async fn save_plugin_config_to_db(
        &self,
        plugin_id: &str,
        config: &serde_json::Value,
    ) -> Result<(), String> {
        let db = self.db.as_ref().ok_or("no database configured")?;
        rustyfin_db::repo_plugin_config::set_plugin_config(db, plugin_id, config)
            .await
            .map_err(|e| format!("failed to save plugin config: {}", e))
    }

    /// Scan the plugins directory and load all shared libraries found.
    /// Each library must export a `rustyfin_plugin_create` symbol that returns a `Box<dyn Plugin>`.
    ///
    /// For provider registration, the library can optionally export a `rustyfin_plugin_register`
    /// symbol.  If present, it is called with a set of function pointers so the plugin can
    /// register itself as a metadata / subtitle / notification provider without the host needing
    /// to know about it at compile time.
    ///
    /// See `PluginRegistrarCallbacks` for the full FFI interface.
    pub async fn load_all(&self) {
        let mut inner = self.inner.write().await;
        let dir = inner.plugins_dir.clone();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::debug!("no plugins directory or cannot read: {}", e);
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "so" | "dylib" | "dll") {
                match Self::load_plugin_from_path(&path, self) {
                    Ok(loaded) => {
                        let id = loaded.plugin.id().to_string();
                        tracing::info!("loaded plugin: {} ({})", loaded.plugin.name(), id);
                        loaded.plugin.on_load();
                        inner.plugins.insert(id, loaded);
                    }
                    Err(e) => {
                        tracing::warn!("failed to load plugin {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    fn load_plugin_from_path(path: &Path, manager: &Self) -> Result<LoadedPlugin, String> {
        unsafe {
            let library = libloading::Library::new(path)
                .map_err(|e| format!("failed to load library: {}", e))?;

            let create_fn: libloading::Symbol<unsafe extern "C" fn() -> *mut std::ffi::c_void> =
                library
                    .get(b"rustyfin_plugin_create")
                    .map_err(|e| format!("missing rustyfin_plugin_create symbol: {}", e))?;

            // The FFI returns a raw pointer; we transmute it to Box<dyn Plugin>.
            // SAFETY: The plugin crate guarantees the pointer is a valid Box<dyn Plugin>
            // allocated on the heap.
            let raw = create_fn();
            let plugin: Box<dyn Plugin> =
                std::ptr::read::<Box<dyn Plugin>>(raw as *mut Box<dyn Plugin>);

            // Optionally call rustyfin_plugin_register so the plugin can register itself
            // as a provider (metadata / subtitle / notification).
            if let Ok(register_fn_sym) = library
                .get::<unsafe extern "C" fn(*const std::ffi::c_void, PluginRegistrarCallbacks)>(
                    b"rustyfin_plugin_register",
                )
            {
                let callbacks = PluginRegistrarCallbacks::new(manager);
                register_fn_sym(std::ptr::null(), callbacks);
                tracing::info!(
                    "called rustyfin_plugin_register for dynamic plugin from {:?}",
                    path.file_name()
                );
            }

            Ok(LoadedPlugin {
                plugin,
                _library: Some(library),
                status: PluginStatus::Active,
                config_file: None,
                is_builtin: false,
            })
        }
    }

    /// Register a built-in plugin (statically linked, no dynamic library).
    /// This is called during startup for TMDB, OMDb, Fanart, etc.
    pub async fn register_builtin_plugin(&self, info: BuiltinPluginInfo) {
        let name = info.name.clone();
        let mut inner = self.inner.write().await;
        inner.builtin_plugins.insert(info.id.clone(), info);
        tracing::info!("registered built-in plugin: {}", name);
    }

    /// Register a hook that runs after `set_configuration` saves this plugin's config.
    pub async fn register_config_hook(&self, plugin_id: &str, hook: ConfigHook) {
        let mut inner = self.inner.write().await;
        inner.config_hooks.insert(plugin_id.to_string(), hook);
    }

    /// Register a metadata provider plugin.
    /// Call this from a plugin's `on_load` or during initialization to make the provider available.
    /// `plugin_id` is used to look up the priority from the database.
    /// `status` determines whether the provider is active; only active providers are returned by `metadata_providers()`.
    pub async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        status: PluginStatus,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let priority = self.get_plugin_priority_from_db(&plugin_id).await;
        let mut inner = self.inner.write().await;
        inner.metadata_providers.push(MetadataProviderHandle {
            plugin_id: plugin_id.clone(),
            provider_name: provider_name.clone(),
            priority,
            status: status.clone(),
            search_movie: Arc::new(search_movie),
            search_tv: Arc::new(search_tv),
            get_images: Arc::new(get_images),
        });
        inner.metadata_providers.sort_by_key(|h| h.priority);
        tracing::info!(
            "registered metadata provider: {} (priority: {}, status: {:?})",
            provider_name,
            priority,
            status
        );
    }

    /// Register a subtitle provider plugin.
    /// `plugin_id` is used to look up the priority from the database.
    /// `status` determines whether the provider is active; only active providers are returned by `subtitle_providers()`.
    pub async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        status: PluginStatus,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(
                &reqwest::Client,
                &MediaItem,
                &str,
            ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
                &std::path::Path,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let priority = self.get_plugin_priority_from_db(&plugin_id).await;
        let mut inner = self.inner.write().await;
        inner.subtitle_providers.push(SubtitleProviderHandle {
            plugin_id: plugin_id.clone(),
            priority,
            status: status.clone(),
            search_subtitles: Arc::new(search_subtitles),
            download_subtitle: Arc::new(download_subtitle),
        });
        inner.subtitle_providers.sort_by_key(|h| h.priority);
        tracing::info!(
            "registered subtitle provider: {} (priority: {}, status: {:?})",
            plugin_id,
            priority,
            status
        );
    }

    /// Register a notification provider plugin.
    /// `plugin_id` is used to look up the priority from the database.
    /// `status` determines whether the provider is active; only active providers are returned by `notification_providers()`.
    pub async fn register_notification_provider<F>(
        &self,
        plugin_id: String,
        status: PluginStatus,
        send_notification: F,
    ) where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let priority = self.get_plugin_priority_from_db(&plugin_id).await;
        let mut inner = self.inner.write().await;
        inner
            .notification_providers
            .push(NotificationProviderHandle {
                plugin_id: plugin_id.clone(),
                priority,
                status: status.clone(),
                send_notification: Arc::new(send_notification),
            });
        inner.notification_providers.sort_by_key(|h| h.priority);
        tracing::info!(
            "registered notification provider: {} (priority: {}, status: {:?})",
            plugin_id,
            priority,
            status
        );
    }

    // ── Priority management ──

    /// Get the priority for a plugin from the database. Returns 100 (default) if not set.
    pub async fn get_plugin_priority_from_db(&self, plugin_id: &str) -> i32 {
        if let Some(db) = &self.db {
            match rustyfin_db::repo_plugin_priority::get_plugin_priority(db, plugin_id).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("failed to load priority for plugin {}: {}", plugin_id, e);
                    100
                }
            }
        } else {
            100
        }
    }

    /// Get the current priority for a plugin.
    pub async fn get_plugin_priority(&self, plugin_id: &str) -> i32 {
        self.get_plugin_priority_from_db(plugin_id).await
    }

    /// Set the priority for a plugin. Lower number = higher priority (processed first).
    pub async fn set_plugin_priority(&self, plugin_id: &str, priority: i32) -> Result<(), String> {
        let db = self.db.as_ref().ok_or("no database configured")?;
        rustyfin_db::repo_plugin_priority::set_plugin_priority(db, plugin_id, priority)
            .await
            .map_err(|e| format!("failed to save plugin priority: {}", e))?;

        // Re-sort the provider lists
        {
            let mut inner = self.inner.write().await;
            inner.metadata_providers.sort_by_key(|h| h.priority);
            inner.subtitle_providers.sort_by_key(|h| h.priority);
            inner.notification_providers.sort_by_key(|h| h.priority);

            // Update in-memory priority values
            for handle in &mut inner.metadata_providers {
                if handle.plugin_id == plugin_id {
                    handle.priority = priority;
                }
            }
            for handle in &mut inner.subtitle_providers {
                if handle.plugin_id == plugin_id {
                    handle.priority = priority;
                }
            }
            for handle in &mut inner.notification_providers {
                if handle.plugin_id == plugin_id {
                    handle.priority = priority;
                }
            }
        }

        // Priority is stored in DB, not in BuiltinPluginInfo directly

        tracing::info!("updated plugin priority: {} -> {}", plugin_id, priority);
        Ok(())
    }

    /// Get priorities for all plugins, ordered by priority.
    pub async fn get_all_plugin_priorities(&self) -> Vec<(String, i32)> {
        if let Some(db) = &self.db {
            match rustyfin_db::repo_plugin_priority::list_plugin_priorities(db).await {
                Ok(priorities) => priorities,
                Err(e) => {
                    tracing::warn!("failed to load plugin priorities: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    }

    /// List all loaded plugins as `PluginInfo` (includes both dynamic and built-in plugins).
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let inner = self.inner.read().await;

        // Collect all plugin IDs to load priorities
        let mut all_ids: Vec<String> = inner.plugins.keys().cloned().collect();
        all_ids.extend(inner.builtin_plugins.keys().cloned());
        drop(inner);

        // Load priorities for all plugins
        let priorities: std::collections::HashMap<String, i32> = if let Some(_db) = &self.db {
            let mut map = std::collections::HashMap::new();
            for id in &all_ids {
                map.insert(id.clone(), self.get_plugin_priority_from_db(id).await);
            }
            map
        } else {
            all_ids.iter().map(|id| (id.clone(), 100)).collect()
        };

        let inner = self.inner.read().await;

        // Dynamic plugins from .so/.dylib/.dll files
        let dynamic: Vec<PluginInfo> = inner
            .plugins
            .values()
            .map(|lp| {
                let pid = lp.plugin.id().to_string();
                PluginInfo {
                    name: lp.plugin.name().to_string(),
                    version: lp.plugin.version().to_string(),
                    id: pid.clone(),
                    description: lp.plugin.description().to_string(),
                    status: lp.status.clone(),
                    has_image: false,
                    can_uninstall: true,
                    configuration_file_name: lp.config_file.clone(),
                    plugin_type: Some(lp.plugin.plugin_type()),
                    is_builtin: lp.is_builtin,
                    priority: *priorities.get(&pid).unwrap_or(&100),
                }
            })
            .collect();

        // Built-in plugins registered via registrar traits
        let builtin: Vec<PluginInfo> = inner
            .builtin_plugins
            .values()
            .map(|bp| PluginInfo {
                name: bp.name.clone(),
                version: bp.version.clone(),
                id: bp.id.clone(),
                description: bp.description.clone(),
                status: bp.status.clone(),
                has_image: false,
                can_uninstall: false,
                configuration_file_name: None,
                plugin_type: Some(bp.plugin_type),
                is_builtin: true,
                priority: *priorities.get(&bp.id).unwrap_or(&100),
            })
            .collect();

        let mut all = dynamic;
        all.extend(builtin);
        // Sort by plugin_type (as string), then by priority, then by id
        all.sort_by(|a, b| {
            let type_cmp = a
                .plugin_type
                .map(|t| t.to_string())
                .cmp(&b.plugin_type.map(|t| t.to_string()));
            type_cmp
                .then(a.priority.cmp(&b.priority))
                .then(a.id.cmp(&b.id))
        });
        all
    }

    /// Get configuration for a specific plugin.
    /// First checks the database, then falls back to the in-memory plugin.
    pub async fn get_configuration(&self, plugin_id: &str) -> Option<PluginConfiguration> {
        // Try DB first (works for both built-in and dynamic plugins)
        if let Some(config) = self.load_plugin_config_from_db(plugin_id).await {
            return Some(PluginConfiguration {
                plugin_id: plugin_id.to_string(),
                values: config,
            });
        }
        // Fall back to in-memory dynamic plugin
        let inner = self.inner.read().await;
        if let Some(lp) = inner.plugins.get(plugin_id)
            && let Some(values) = lp.plugin.get_configuration()
        {
            return Some(PluginConfiguration {
                plugin_id: plugin_id.to_string(),
                values,
            });
        }
        // Built-in plugins without DB config return empty config
        if inner.builtin_plugins.contains_key(plugin_id) {
            return Some(PluginConfiguration {
                plugin_id: plugin_id.to_string(),
                values: serde_json::Value::Object(serde_json::Map::new()),
            });
        }
        None
    }

    /// Update configuration for a specific plugin.
    /// Updates the in-memory plugin (if dynamic) and persists to the database.
    pub async fn set_configuration(
        &self,
        plugin_id: &str,
        config: serde_json::Value,
    ) -> Result<(), String> {
        // Check if plugin exists (dynamic or built-in)
        {
            let inner = self.inner.read().await;
            let is_dynamic = inner.plugins.contains_key(plugin_id);
            let is_builtin = inner.builtin_plugins.contains_key(plugin_id);

            if !is_dynamic && !is_builtin {
                return Err("plugin not found".to_string());
            }

            // Update in-memory dynamic plugin if it exists
            if is_dynamic && let Some(lp) = inner.plugins.get(plugin_id) {
                lp.plugin.set_configuration(config.clone())?;
            }
        }

        // Persist to database (built-in plugins rely solely on DB). The DB merges the patch
        // into the stored config, so the hook below gets the stored result, not just the patch.
        if let Err(e) = self.save_plugin_config_to_db(plugin_id, &config).await {
            tracing::warn!("failed to persist plugin config to DB: {}", e);
        }
        let merged = self
            .load_plugin_config_from_db(plugin_id)
            .await
            .unwrap_or_else(|| config.clone());

        // Let the plugin's hook set its status from the merged config (e.g. a TMDb key enables it).
        // The lock is released before the hook runs.
        let hook = self.inner.read().await.config_hooks.get(plugin_id).cloned();
        if let Some(hook) = hook
            && let Some(status) = hook(&merged)
        {
            self.set_status_in_memory(plugin_id, status).await;
        }

        Ok(())
    }

    /// Set a built-in plugin's status, and its provider handles' status, in memory only.
    async fn set_status_in_memory(&self, plugin_id: &str, status: PluginStatus) {
        let mut inner = self.inner.write().await;
        if let Some(info) = inner.builtin_plugins.get_mut(plugin_id) {
            info.status = status.clone();
        }
        // Update all provider handles for this plugin
        for handle in &mut inner.metadata_providers {
            if handle.plugin_id == plugin_id {
                handle.status = status.clone();
            }
        }
        for handle in &mut inner.subtitle_providers {
            if handle.plugin_id == plugin_id {
                handle.status = status.clone();
            }
        }
        for handle in &mut inner.notification_providers {
            if handle.plugin_id == plugin_id {
                handle.status = status.clone();
            }
        }
    }

    // ── Plugin type queries ──

    /// Returns the count of plugins of a specific type.
    pub async fn plugin_count_by_type(&self, plugin_type: PluginType) -> usize {
        let inner = self.inner.read().await;
        inner
            .plugins
            .values()
            .filter(|lp| lp.plugin.plugin_type() == plugin_type)
            .count()
    }

    /// Returns the list of registered metadata providers that are currently active.
    /// Disabled providers are excluded so scans run fast without external API calls.
    pub async fn metadata_providers(&self) -> Vec<MetadataProviderHandle> {
        let inner = self.inner.read().await;
        inner
            .metadata_providers
            .iter()
            .filter(|h| h.status == PluginStatus::Active)
            .cloned()
            .collect()
    }

    /// Returns the list of registered subtitle providers that are currently active.
    pub async fn subtitle_providers(&self) -> Vec<SubtitleProviderHandle> {
        let inner = self.inner.read().await;
        inner
            .subtitle_providers
            .iter()
            .filter(|h| h.status == PluginStatus::Active)
            .cloned()
            .collect()
    }

    /// Returns the list of registered notification providers that are currently active.
    pub async fn notification_providers(&self) -> Vec<NotificationProviderHandle> {
        let inner = self.inner.read().await;
        inner
            .notification_providers
            .iter()
            .filter(|h| h.status == PluginStatus::Active)
            .cloned()
            .collect()
    }

    // ── Plugin status management ──

    /// Enable or disable a plugin by ID. Updates both the built-in plugin info and all associated
    /// provider handles, then persists the status to the database so it survives restarts.
    pub async fn set_plugin_status(
        &self,
        plugin_id: &str,
        status: PluginStatus,
    ) -> Result<(), String> {
        // Update built-in plugin info and provider handles
        self.set_status_in_memory(plugin_id, status.clone()).await;

        // Persist to database. Only the "status" key is written; the stored merge keeps other keys.
        if let Some(_db) = &self.db {
            let status_str = match status {
                PluginStatus::Active => "Active",
                PluginStatus::Disabled => "Disabled",
                PluginStatus::Malfunctioned => "Malfunctioned",
            };
            let config = serde_json::json!({ "status": status_str });
            if let Err(e) = self.save_plugin_config_to_db(plugin_id, &config).await {
                tracing::warn!("failed to persist plugin status to DB: {}", e);
            }
        }

        tracing::info!("plugin {} status changed to {:?}", plugin_id, status);
        Ok(())
    }

    /// Get the current status of a plugin.
    pub async fn get_plugin_status(&self, plugin_id: &str) -> Option<PluginStatus> {
        let inner = self.inner.read().await;
        inner
            .builtin_plugins
            .get(plugin_id)
            .map(|info| info.status.clone())
    }
}

// ── MetadataProviderRegistrar trait implementations ──

impl TmdbRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}

impl OmdbRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}

impl FanartRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}

impl OpenSubtitlesRegistrar for PluginManager {
    async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(
                &reqwest::Client,
                &MediaItem,
                &str,
            ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
                &std::path::Path,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_subtitle_provider(plugin_id, status, search_subtitles, download_subtitle)
            .await;
    }
}

impl TvdbRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}

impl WebhookRegistrar for PluginManager {
    async fn register_notification_provider<F>(&self, plugin_id: String, send_notification: F)
    where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_notification_provider(plugin_id, status, send_notification)
            .await;
    }
}

impl NtfyRegistrar for PluginManager {
    async fn register_notification_provider<F>(&self, plugin_id: String, send_notification: F)
    where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_notification_provider(plugin_id, status, send_notification)
            .await;
    }
}

impl MqttRegistrar for PluginManager {
    async fn register_notification_provider<F>(&self, plugin_id: String, send_notification: F)
    where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_notification_provider(plugin_id, status, send_notification)
            .await;
    }
}

impl PushoverRegistrar for PluginManager {
    async fn register_notification_provider<F>(&self, plugin_id: String, send_notification: F)
    where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_notification_provider(plugin_id, status, send_notification)
            .await;
    }
}

impl TraktRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}

impl PodnapisiRegistrar for PluginManager {
    async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(
                &reqwest::Client,
                &MediaItem,
                &str,
            ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
                &std::path::Path,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_subtitle_provider(plugin_id, status, search_subtitles, download_subtitle)
            .await;
    }
}

impl LocalNfoRegistrar for PluginManager {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let status = self
            .get_plugin_status(&plugin_id)
            .await
            .unwrap_or(PluginStatus::Disabled);
        self.register_metadata_provider(
            plugin_id,
            provider_name,
            status,
            search_movie,
            search_tv,
            get_images,
        )
        .await;
    }
}
