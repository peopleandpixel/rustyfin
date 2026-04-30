use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing;

use rustyfin_model::plugin::{Plugin, PluginInfo, PluginStatus, PluginConfiguration};

/// A loaded plugin with its dynamic library handle.
struct LoadedPlugin {
    plugin: Box<dyn Plugin>,
    _library: libloading::Library,
    status: PluginStatus,
    config_file: Option<String>,
}

/// Manages loading, unloading, and querying plugins.
#[derive(Clone)]
pub struct PluginManager {
    inner: Arc<RwLock<PluginManagerInner>>,
}

struct PluginManagerInner {
    plugins: HashMap<String, LoadedPlugin>,
    plugins_dir: PathBuf,
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
            })),
        }
    }

    /// Scan the plugins directory and load all shared libraries found.
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
                match Self::load_plugin_from_path(&path) {
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

    fn load_plugin_from_path(path: &Path) -> Result<LoadedPlugin, String> {
        unsafe {
            let library = libloading::Library::new(path)
                .map_err(|e| format!("failed to load library: {}", e))?;

            let create_fn: libloading::Symbol<unsafe extern "C" fn() -> Box<dyn Plugin>> =
                library
                    .get(b"rustyfin_plugin_create")
                    .map_err(|e| format!("missing rustyfin_plugin_create symbol: {}", e))?;

            let plugin = create_fn();

            Ok(LoadedPlugin {
                plugin,
                _library: library,
                status: PluginStatus::Active,
                config_file: None,
            })
        }
    }

    /// List all loaded plugins as `PluginInfo`.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let inner = self.inner.read().await;
        inner
            .plugins
            .values()
            .map(|lp| PluginInfo {
                name: lp.plugin.name().to_string(),
                version: lp.plugin.version().to_string(),
                id: lp.plugin.id().to_string(),
                description: lp.plugin.description().to_string(),
                status: lp.status.clone(),
                has_image: false,
                can_uninstall: true,
                configuration_file_name: lp.config_file.clone(),
            })
            .collect()
    }

    /// Get configuration for a specific plugin.
    pub async fn get_configuration(&self, plugin_id: &str) -> Option<PluginConfiguration> {
        let inner = self.inner.read().await;
        let lp = inner.plugins.get(plugin_id)?;
        let values = lp.plugin.get_configuration()?;
        Some(PluginConfiguration {
            plugin_id: plugin_id.to_string(),
            values,
        })
    }

    /// Update configuration for a specific plugin.
    pub async fn set_configuration(
        &self,
        plugin_id: &str,
        config: serde_json::Value,
    ) -> Result<(), String> {
        let inner = self.inner.read().await;
        let lp = inner
            .plugins
            .get(plugin_id)
            .ok_or_else(|| "plugin not found".to_string())?;
        lp.plugin.set_configuration(config)
    }
}
