//! Webhook notification provider plugin for Rustyfin.
//!
//! Sends notifications via HTTP webhooks to any endpoint.
//! Supports configurable URLs, headers, and JSON payloads.
//! Useful for integration with Home Assistant, n8n, Discord, Slack, and more.

use rustyfin_model::plugin::{NotificationPriority, Plugin, PluginType};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// A single webhook endpoint configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEndpoint {
    /// The URL to send the webhook to.
    pub url: String,
    /// Optional HTTP method (defaults to POST).
    #[serde(default = "default_method")]
    pub method: String,
    /// Optional custom headers to include in the request.
    #[serde(default)]
    pub headers: Vec<WebhookHeader>,
    /// Optional template for the JSON payload body.
    /// Supports placeholders: {title}, {message}, {priority}, {timestamp}
    #[serde(default)]
    pub template: Option<String>,
}

fn default_method() -> String {
    "POST".to_string()
}

/// A custom header for webhook requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookHeader {
    pub name: String,
    pub value: String,
}

/// Webhook notification provider plugin.
pub struct WebhookPlugin {
    config: Mutex<Option<WebhookPluginConfig>>,
}

/// Configuration for the webhook plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPluginConfig {
    /// List of webhook endpoints to notify.
    #[serde(default)]
    pub endpoints: Vec<WebhookEndpoint>,
    /// Optional default template for all endpoints.
    #[serde(default)]
    pub default_template: Option<String>,
    /// Timeout in seconds for webhook requests (default: 10).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    10
}

impl Default for WebhookPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(None),
        }
    }
}

impl Plugin for WebhookPlugin {
    fn id(&self) -> &str {
        "webhook"
    }

    fn name(&self) -> &str {
        "Webhook Notifications"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Sends notifications via HTTP webhooks. Supports multiple endpoints, custom headers, and JSON templates. Integrates with Home Assistant, n8n, Discord, Slack, and more."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::NotificationProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        let plugin_config: WebhookPluginConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid webhook config: {}", e))?;
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

impl WebhookPlugin {
    fn config(&self) -> Option<WebhookPluginConfig> {
        self.config.lock().unwrap().clone()
    }
}

/// Default JSON payload template.
const DEFAULT_TEMPLATE: &str = r#"{"title":"{title}","message":"{message}","priority":"{priority}","timestamp":"{timestamp}"}"#;

/// Replace placeholders in a template string.
fn fill_template(template: &str, title: &str, message: &str, priority: &str) -> String {
    let timestamp = chrono::Utc::now().to_rfc3339();
    template
        .replace("{title}", title)
        .replace("{message}", message)
        .replace("{priority}", priority)
        .replace("{timestamp}", &timestamp)
}

/// Send a notification to a single webhook endpoint.
async fn send_to_endpoint(
    client: &reqwest::Client,
    endpoint: &WebhookEndpoint,
    title: &str,
    message: &str,
    priority: &str,
    timeout_secs: u64,
) -> Result<(), String> {
    let template = endpoint.template.as_deref().unwrap_or(DEFAULT_TEMPLATE);
    let body = fill_template(template, title, message, priority);

    let mut request = match endpoint.method.as_str() {
        "GET" => client.get(&endpoint.url),
        "PUT" => client.put(&endpoint.url),
        "PATCH" => client.patch(&endpoint.url),
        "DELETE" => client.delete(&endpoint.url),
        _ => client.post(&endpoint.url),
    };

    request = request
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(timeout_secs.max(1)));

    // Add custom headers
    for header in &endpoint.headers {
        request = request.header(&header.name, &header.value);
    }

    let resp = request.body(body).send().await;

    match resp {
        Ok(r) => {
            if r.status().is_success() {
                tracing::debug!("webhook notification sent successfully to {}", endpoint.url);
                Ok(())
            } else {
                Err(format!(
                    "Webhook request failed with status: {}",
                    r.status()
                ))
            }
        }
        Err(e) => Err(format!("Webhook request error: {}", e)),
    }
}

/// Send a notification to all configured webhook endpoints.
pub async fn send_notification(
    config: &WebhookPluginConfig,
    title: &str,
    message: &str,
    priority: NotificationPriority,
) -> Result<(), String> {
    let priority_str = match priority {
        NotificationPriority::Low => "low",
        NotificationPriority::Normal => "normal",
        NotificationPriority::High => "high",
        NotificationPriority::Emergency => "emergency",
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut last_error = None;
    for endpoint in &config.endpoints {
        if let Err(e) = send_to_endpoint(
            &client,
            endpoint,
            title,
            message,
            priority_str,
            config.timeout_secs,
        )
        .await
        {
            tracing::warn!("failed to send webhook to {}: {}", endpoint.url, e);
            last_error = Some(e);
        }
    }

    match last_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Create a new Webhook plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin = Box::new(WebhookPlugin {
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

/// Register the Webhook plugin as a notification provider with the given PluginManager.
pub async fn register_webhook_provider<P>(
    plugin: &WebhookPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: NotificationProviderRegistrar,
{
    let config = plugin.config().ok_or("Webhook plugin not configured")?;
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
        let plugin = WebhookPlugin::default();
        assert_eq!(plugin.id(), "webhook");
        assert_eq!(plugin.plugin_type(), PluginType::NotificationProvider);
    }

    #[test]
    fn test_fill_template() {
        let template = r#"{"title":"{title}","message":"{message}","priority":"{priority}"}"#;
        let result = fill_template(template, "Test", "Hello", "high");
        assert!(result.contains("\"title\":\"Test\""));
        assert!(result.contains("\"message\":\"Hello\""));
        assert!(result.contains("\"priority\":\"high\""));
    }

    #[test]
    fn test_default_template() {
        let result = fill_template(DEFAULT_TEMPLATE, "Title", "Message", "normal");
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["title"], "Title");
        assert_eq!(json["message"], "Message");
        assert_eq!(json["priority"], "normal");
    }

    #[test]
    fn test_webhook_config_serialization() {
        let config = WebhookPluginConfig {
            endpoints: vec![WebhookEndpoint {
                url: "http://localhost:8080/webhook".to_string(),
                method: "POST".to_string(),
                headers: vec![WebhookHeader {
                    name: "Authorization".to_string(),
                    value: "Bearer token123".to_string(),
                }],
                template: None,
            }],
            default_template: None,
            timeout_secs: 10,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WebhookPluginConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.endpoints.len(), 1);
        assert_eq!(
            deserialized.endpoints[0].url,
            "http://localhost:8080/webhook"
        );
    }
}
