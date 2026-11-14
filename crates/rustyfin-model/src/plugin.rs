use std::future::Future;
use std::pin::Pin;

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
    /// The category/type of this plugin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_type: Option<PluginType>,
    /// Whether the plugin is a built-in (statically linked) plugin.
    #[serde(default)]
    pub is_builtin: bool,
    /// Plugin priority: lower number = higher priority (processed first). Default is 100.
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    100
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

/// Category of a plugin — determines which trait methods are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PluginType {
    /// Fetches metadata from external sources (TMDB, OMDb, Fanart, TheTVDB, etc.)
    MetadataProvider,
    /// Additional image sources
    ImageProvider,
    /// Searches and downloads subtitles (OpenSubtitles, Podnapisi, etc.)
    SubtitleProvider,
    /// Sends push notifications (Pushover, Telegram, Discord, etc.)
    NotificationProvider,
    /// Authentication backends (LDAP, OAuth, etc.)
    AuthenticationProvider,
    /// Scheduled tasks (cleanup, sync, etc.)
    Task,
}

impl std::fmt::Display for PluginType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginType::MetadataProvider => write!(f, "MetadataProvider"),
            PluginType::ImageProvider => write!(f, "ImageProvider"),
            PluginType::SubtitleProvider => write!(f, "SubtitleProvider"),
            PluginType::NotificationProvider => write!(f, "NotificationProvider"),
            PluginType::AuthenticationProvider => write!(f, "AuthenticationProvider"),
            PluginType::Task => write!(f, "Task"),
        }
    }
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
    /// The category of this plugin.
    fn plugin_type(&self) -> PluginType;
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

    /// Downcast to `Any` for dynamic casting to specific plugin traits.
    fn as_any(&self) -> &dyn std::any::Any;
    /// Mutable downcast to `Any` for dynamic casting to specific plugin traits.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Metadata provider plugin — searches external metadata for movies/series.
///
/// Implement this trait to add a new metadata source (e.g., TMDB, OMDb, TheTVDB).
pub trait MetadataProviderPlugin: Plugin {
    /// Returns the provider name (e.g., "tmdb", "omdb", "fanart").
    fn provider_name(&self) -> &str;

    /// Search for movie metadata.
    fn search_movie(
        &self,
        client: &reqwest::Client,
        name: &str,
        year: Option<i32>,
    ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send + '_>>;

    /// Search for TV series metadata.
    fn search_tv(
        &self,
        client: &reqwest::Client,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send + '_>>;

    /// Fetch images for an item by provider IDs.
    fn get_images(
        &self,
        client: &reqwest::Client,
        tmdb_id: Option<i64>,
        tvdb_id: Option<i64>,
        media_type: MediaType,
    ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send + '_>>;
}

/// Trait for registering a metadata provider with PluginManager.
/// Used by plugin crates to register their provider closures.
#[allow(async_fn_in_trait)]
pub trait MetadataProviderRegistrar {
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
            + 'static;
}

/// Subtitle provider plugin — searches and downloads subtitles.
///
/// Implement this trait to add a new subtitle source (e.g., OpenSubtitles, Podnapisi).
pub trait SubtitleProviderPlugin: Plugin {
    /// Search for subtitles for a given media item.
    fn search_subtitles(
        &self,
        client: &reqwest::Client,
        item: &MediaItem,
        language: &str,
    ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send + '_>>;

    /// Download a subtitle by ID to the given path.
    fn download_subtitle(
        &self,
        client: &reqwest::Client,
        subtitle_id: &str,
        path: &std::path::Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

/// Trait for registering a subtitle provider with PluginManager.
#[allow(async_fn_in_trait)]
pub trait SubtitleProviderRegistrar {
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
            + 'static;
}

/// Notification provider plugin — sends push notifications.
///
/// Implement this trait to add a new notification channel (e.g., Pushover, Telegram, Discord).
pub trait NotificationProviderPlugin: Plugin {
    /// Send a notification.
    fn send_notification(
        &self,
        title: &str,
        message: &str,
        priority: NotificationPriority,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

/// Trait for registering a notification provider with PluginManager.
#[allow(async_fn_in_trait)]
pub trait NotificationProviderRegistrar {
    async fn register_notification_provider<F>(&self, plugin_id: String, send_notification: F)
    where
        F: Fn(
                &str,
                &str,
                NotificationPriority,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static;
}

// ── Supporting types for plugin traits ──

use crate::media::MediaItem;
use crate::media::MediaType;
use crate::metadata::ExternalMetadata;
use crate::metadata::MetadataImage;

/// Result of a subtitle search.
#[derive(Debug, Clone)]
pub struct SubtitleResult {
    /// Unique identifier for downloading.
    pub id: String,
    /// Human-readable name (e.g., "Movie.Name.2023.German.srt").
    pub name: String,
    /// Language code (e.g., "de", "en").
    pub language: String,
    /// Optional description.
    pub description: Option<String>,
}

/// Priority level for notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Emergency,
}

/// Works out a keyed plugin's status (TMDb, OMDb, Fanart.tv) from its saved config and whether
/// the plugin's env key is set.
///
/// A saved `"status"` string wins: "Active", "Disabled" or "Malfunctioned" map to that status and
/// any other string is Disabled. Without one, the plugin is Active when `env_key` is set or the
/// config's `api_key` is a string that is non-blank after trimming, and Disabled otherwise.
pub fn key_status(config: Option<&serde_json::Value>, env_key: bool) -> PluginStatus {
    if let Some(status) = config
        .and_then(|c| c.get("status"))
        .and_then(|v| v.as_str())
    {
        return match status {
            "Active" => PluginStatus::Active,
            "Malfunctioned" => PluginStatus::Malfunctioned,
            _ => PluginStatus::Disabled,
        };
    }
    let saved_key = config
        .and_then(|c| c.get("api_key"))
        .and_then(|v| v.as_str())
        .is_some_and(|k| !k.trim().is_empty());
    if env_key || saved_key {
        PluginStatus::Active
    } else {
        PluginStatus::Disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // B142: one rule for every keyed provider (TMDb, OMDb, Fanart.tv).
    #[test]
    fn b142_key_status_follows_the_key_unless_a_status_is_saved() {
        assert_eq!(key_status(None, false), PluginStatus::Disabled);
        assert_eq!(key_status(None, true), PluginStatus::Active);
        let keyed = json!({ "api_key": " k " });
        assert_eq!(key_status(Some(&keyed), false), PluginStatus::Active);
        for blank in [
            json!({}),
            json!({ "api_key": "  " }),
            json!({ "api_key": 1 }),
        ] {
            assert_eq!(
                key_status(Some(&blank), false),
                PluginStatus::Disabled,
                "{blank}"
            );
        }
        let disabled = json!({ "api_key": "k", "status": "Disabled" });
        assert_eq!(key_status(Some(&disabled), true), PluginStatus::Disabled);
        let active = json!({ "status": "Active" });
        assert_eq!(key_status(Some(&active), false), PluginStatus::Active);
        let broken = json!({ "status": "Malfunctioned" });
        assert_eq!(key_status(Some(&broken), true), PluginStatus::Malfunctioned);
        let odd = json!({ "api_key": "k", "status": "Paused" });
        assert_eq!(key_status(Some(&odd), true), PluginStatus::Disabled);
    }
}
