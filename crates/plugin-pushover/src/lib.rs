//! Pushover notification provider plugin for Rustyfin.
//!
//! Sends push notifications via Pushover.net to iOS and Android devices.

use rustyfin_model::plugin::{NotificationPriority, Plugin, PluginType};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// Configuration for the Pushover plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushoverConfig {
    /// Your Pushover API token/key (from your app on pushover.net).
    pub api_token: String,
    /// Your user key (or group/device key).
    pub user_key: String,
    /// Optional device name to target.
    #[serde(default)]
    pub device: Option<String>,
    /// Optional sound name (e.g. "pushover", "bike", "cashregister").
    #[serde(default)]
    pub sound: Option<String>,
    /// Optional URL to attach to the notification.
    #[serde(default)]
    pub url: Option<String>,
    /// Optional URL title.
    #[serde(default)]
    pub url_title: Option<String>,
}

/// Pushover notification provider plugin.
pub struct PushoverPlugin {
    config: Mutex<Option<PushoverConfig>>,
}

impl Default for PushoverPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(None),
        }
    }
}

impl Plugin for PushoverPlugin {
    fn id(&self) -> &str {
        "pushover"
    }

    fn name(&self) -> &str {
        "Pushover Notifications"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Sends push notifications via Pushover.net to iOS and Android devices. Supports priority, sounds, and URLs."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::NotificationProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        let plugin_config: PushoverConfig = serde_json::from_value(config)
            .map_err(|e| format!("Invalid Pushover config: {}", e))?;
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

impl PushoverPlugin {
    fn config(&self) -> Option<PushoverConfig> {
        self.config.lock().unwrap().clone()
    }
}

/// Simple URL encoding for form data.
fn url_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                encoded.push(c);
            }
            ' ' => {
                encoded.push('+');
            }
            _ => {
                for byte in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}

/// Map Rustyfin priority to Pushover priority (-2 to 2).
fn to_pushover_priority(priority: NotificationPriority) -> i32 {
    match priority {
        NotificationPriority::Low => -2,
        NotificationPriority::Normal => 0,
        NotificationPriority::High => 1,
        NotificationPriority::Emergency => 2,
    }
}

/// Send a notification via Pushover.
pub async fn send_notification(
    config: &PushoverConfig,
    title: &str,
    message: &str,
    priority: NotificationPriority,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut body = format!(
        "token={}&user={}&title={}&message={}&priority={}",
        url_encode(&config.api_token),
        url_encode(&config.user_key),
        url_encode(title),
        url_encode(message),
        to_pushover_priority(priority)
    );

    if let Some(ref device) = config.device {
        body.push_str(&format!("&device={}", url_encode(device)));
    }
    if let Some(ref sound) = config.sound {
        body.push_str(&format!("&sound={}", url_encode(sound)));
    }
    if let Some(ref url) = config.url {
        body.push_str(&format!("&url={}", url_encode(url)));
    }
    if let Some(ref url_title) = config.url_title {
        body.push_str(&format!("&url_title={}", url_encode(url_title)));
    }

    let resp = client
        .post("https://api.pushover.net/1/messages.json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await;

    match resp {
        Ok(r) => {
            if r.status().is_success() {
                tracing::debug!("Pushover notification sent successfully");
                Ok(())
            } else {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                Err(format!("Pushover request failed: {} - {}", status, text))
            }
        }
        Err(e) => Err(format!("Pushover request error: {}", e)),
    }
}

/// FFI entry point for dynamic loading.
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
///
/// # Safety
///
/// The caller takes ownership of the returned `Box<dyn Plugin>` and must free it exactly once.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin: Box<dyn Plugin> = Box::new(PushoverPlugin {
        config: Mutex::new(None),
    });
    Box::into_raw(Box::new(plugin)).cast()
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

/// Register the Pushover plugin as a notification provider.
pub async fn register_pushover_provider<P>(
    plugin: &PushoverPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: NotificationProviderRegistrar,
{
    let config = plugin.config().ok_or("Pushover plugin not configured")?;
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
        let plugin = PushoverPlugin::default();
        assert_eq!(plugin.id(), "pushover");
        assert_eq!(plugin.plugin_type(), PluginType::NotificationProvider);
    }

    #[test]
    fn test_priority_mapping() {
        assert_eq!(to_pushover_priority(NotificationPriority::Low), -2);
        assert_eq!(to_pushover_priority(NotificationPriority::Normal), 0);
        assert_eq!(to_pushover_priority(NotificationPriority::High), 1);
        assert_eq!(to_pushover_priority(NotificationPriority::Emergency), 2);
    }
}
