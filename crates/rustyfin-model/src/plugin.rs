use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub id: String,
    pub description: String,
    pub status: PluginStatus,
    pub has_image: bool,
    pub can_uninstall: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_file_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginStatus {
    Active,
    Disabled,
    Malfunctioned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PluginConfiguration {
    pub plugin_id: String,
    #[serde(flatten)]
    pub values: serde_json::Value,
}

/// Trait that all plugins must implement.
pub trait Plugin: Send + Sync {
    /// Unique identifier for this plugin.
    fn id(&self) -> &str;
    /// Human-readable name.
    fn name(&self) -> &str;
    /// Version string.
    fn version(&self) -> &str;
    /// Description of the plugin.
    fn description(&self) -> &str;
    /// Called when the plugin is loaded.
    fn on_load(&self) {}
    /// Called when the plugin is unloaded.
    fn on_unload(&self) {}
    /// Return the plugin's current configuration as JSON, if any.
    fn get_configuration(&self) -> Option<serde_json::Value> {
        None
    }
    /// Update the plugin's configuration from JSON.
    fn set_configuration(&self, _config: serde_json::Value) -> Result<(), String> {
        Ok(())
    }
}
