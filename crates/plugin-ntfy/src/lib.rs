//! ntfy.sh notification provider plugin for Rustyfin.
//!
//! Sends push notifications via ntfy.sh (self-hosted or public).
//! Supports topics, priority, tags, actions, and email subscriptions.

use rustyfin_model::plugin::{NotificationPriority, Plugin, PluginType};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// Configuration for the ntfy plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtfyConfig {
    /// The ntfy server URL, e.g. "https://ntfy.sh" or "http://localhost:8080".
    #[serde(default = "default_url")]
    pub url: String,
    /// The topic name to publish to, e.g. "rustyfin".
    #[serde(default = "default_topic")]
    pub topic: String,
    /// Optional authentication token.
    #[serde(default)]
    pub token: Option<String>,
    /// Optional email for subscriptions.
    #[serde(default)]
    pub email: Option<String>,
    /// Click action URL (optional).
    #[serde(default)]
    pub click_url: Option<String>,
}

fn default_url() -> String {
    "https://ntfy.sh".to_string()
}

fn default_topic() -> String {
    "rustyfin".to_string()
}

/// ntfy notification provider plugin.
pub struct NtfyPlugin {
    config: Mutex<Option<NtfyConfig>>,
}

impl Default for NtfyPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(None),
        }
    }
}

impl Plugin for NtfyPlugin {
    fn id(&self) -> &str {
        "ntfy"
    }

    fn name(&self) -> &str {
        "ntfy.sh Notifications"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Sends push notifications via ntfy.sh. Supports self-hosted servers, topics, priority, tags, and email subscriptions."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::NotificationProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        let plugin_config: NtfyConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid ntfy config: {}", e))?;
        *self.config.lock().unwrap() = Some(plugin_config);
        Ok(())
    }

    fn get_configuration(&self) -> Option<serde_json::Value> {
        self.config
            .lock()
            .unwrap()
            .clone()
            .map(|c| serde_json::to_value(c).unwrap())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl NtfyPlugin {
    fn config(&self) -> Option<NtfyConfig> {
        self.config.lock().unwrap().clone()
    }
}

/// Map Rustyfin priority to ntfy priority (1-5).
fn to_ntfy_priority(priority: NotificationPriority) -> i32 {
    match priority {
        NotificationPriority::Low => 2,
        NotificationPriority::Normal => 3,
        NotificationPriority::High => 4,
        NotificationPriority::Emergency => 5,
    }
}

/// Send a notification via ntfy.
pub async fn send_notification(
    config: &NtfyConfig,
    title: &str,
    message: &str,
    priority: NotificationPriority,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let publish_url = format!("{}/{}", config.url.trim_end_matches('/'), config.topic);

    let mut request = client.post(&publish_url);

    // ntfy uses form-encoded body for publish API
    request = request.form(&serde_json::json!({
        "title": title,
        "message": message,
        "priority": to_ntfy_priority(priority),
        "tags": match priority {
            NotificationPriority::Emergency => "red_circle,warning",
            NotificationPriority::High => "orange_circle",
            NotificationPriority::Normal => "white_circle",
            NotificationPriority::Low => "blue_circle",
        },
    }));

    if let Some(ref token) = config.token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    if let Some(ref email) = config.email {
        request = request.header("X-Email", email.as_str());
    }

    if let Some(ref click_url) = config.click_url {
        request = request.form(&serde_json::json!({ "click": click_url }));
    }

    let resp = request.send().await;

    match resp {
        Ok(r) => {
            if r.status().is_success() {
                tracing::debug!("ntfy notification sent to topic '{}'", config.topic);
                Ok(())
            } else {
                Err(format!("ntfy request failed with status: {}", r.status()))
            }
        }
        Err(e) => Err(format!("ntfy request error: {}", e)),
    }
}

/// FFI entry point for dynamic loading.
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin = Box::new(NtfyPlugin {
        config: Mutex::new(None),
    });
    Box::into_raw(plugin)
}

/// Trait for registering a notification provider with a PluginManager.
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

/// Register the ntfy plugin as a notification provider.
pub async fn register_ntfy_provider<P>(
    plugin: &NtfyPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: NotificationProviderRegistrar,
{
    let config = plugin.config().ok_or("ntfy plugin not configured")?;
    let config1 = config.clone();
    let plugin_id = plugin.id().to_string();

    plugin_manager
        .register_notification_provider(
            plugin_id,
            move |title: &str,
                  message: &str,
                  priority: NotificationPriority|
                  -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
                let config = config1.clone();
                let title = title.to_string();
                let message = message.to_string();
                Box::pin(
                    async move { send_notification(&config, &title, &message, priority).await },
                )
            },
        )
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = NtfyPlugin::default();
        assert_eq!(plugin.id(), "ntfy");
        assert_eq!(plugin.plugin_type(), PluginType::NotificationProvider);
    }

    #[test]
    fn test_priority_mapping() {
        assert_eq!(to_ntfy_priority(NotificationPriority::Low), 2);
        assert_eq!(to_ntfy_priority(NotificationPriority::Normal), 3);
        assert_eq!(to_ntfy_priority(NotificationPriority::High), 4);
        assert_eq!(to_ntfy_priority(NotificationPriority::Emergency), 5);
    }

    #[test]
    fn test_config_defaults() {
        let config = NtfyConfig {
            url: default_url(),
            topic: default_topic(),
            token: None,
            email: None,
            click_url: None,
        };
        assert_eq!(config.url, "https://ntfy.sh");
        assert_eq!(config.topic, "rustyfin");
    }
}
