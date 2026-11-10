//! MQTT notification provider plugin for Rustyfin.
//!
//! Sends notifications via MQTT to any broker (Mosquitto, EMQX, Home Assistant, etc.).
//! Ideal for IoT and home automation integration.

use rustyfin_model::plugin::{NotificationPriority, Plugin, PluginType};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// Configuration for the MQTT plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    /// MQTT broker URL, e.g. "localhost" or "mqtt.example.com".
    #[serde(default = "default_broker")]
    pub broker: String,
    /// MQTT broker port (default: 1883 for plain, 8883 for TLS).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional username for authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// Optional password for authentication.
    #[serde(default)]
    pub password: Option<String>,
    /// MQTT topic to publish to, e.g. "rustyfin/notifications".
    #[serde(default = "default_topic")]
    pub topic: String,
    /// Quality of Service (0, 1, or 2; default: 1).
    #[serde(default = "default_qos")]
    pub qos: u8,
    /// Enable TLS/SSL (default: false).
    #[serde(default)]
    pub use_tls: bool,
    /// Client ID (optional, auto-generated if not set).
    #[serde(default)]
    pub client_id: Option<String>,
    /// Keep-alive interval in seconds (default: 60).
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u64,
}

fn default_broker() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    1883
}

fn default_topic() -> String {
    "rustyfin/notifications".to_string()
}

fn default_qos() -> u8 {
    1
}

fn default_keepalive() -> u64 {
    60
}

/// MQTT notification provider plugin.
pub struct MqttPlugin {
    config: Mutex<Option<MqttConfig>>,
}

impl Default for MqttPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(None),
        }
    }
}

impl Plugin for MqttPlugin {
    fn id(&self) -> &str {
        "mqtt"
    }

    fn name(&self) -> &str {
        "MQTT Notifications"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Sends notifications via MQTT. Integrates with Home Assistant, Mosquitto, EMQX, and any MQTT broker for IoT automation."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::NotificationProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        let plugin_config: MqttConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid MQTT config: {}", e))?;
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

impl MqttPlugin {
    fn config(&self) -> Option<MqttConfig> {
        self.config.lock().unwrap().clone()
    }
}

/// Map Rustyfin priority to MQTT payload string.
fn priority_to_string(priority: NotificationPriority) -> &'static str {
    match priority {
        NotificationPriority::Low => "low",
        NotificationPriority::Normal => "normal",
        NotificationPriority::High => "high",
        NotificationPriority::Emergency => "emergency",
    }
}

/// Send a notification via MQTT.
/// Creates a new connection for each notification (fire-and-forget pattern).
pub async fn send_notification(
    config: &MqttConfig,
    title: &str,
    message: &str,
    priority: NotificationPriority,
) -> Result<(), String> {
    let client_id = config
        .client_id
        .clone()
        .unwrap_or_else(|| format!("rustyfin-notify-{}", std::process::id()));

    let mut mqtt_options = rumqttc::MqttOptions::new(client_id, &config.broker, config.port);
    mqtt_options.set_keep_alive(std::time::Duration::from_secs(config.keepalive_secs));

    if let (Some(username), Some(password)) = (&config.username, &config.password) {
        mqtt_options.set_credentials(username, password);
    }

    // Build JSON payload
    let payload = serde_json::json!({
        "title": title,
        "message": message,
        "priority": priority_to_string(priority),
        "timestamp": format!("{}", chrono::Utc::now().timestamp())
    });
    let payload_str = payload.to_string();

    // Connect and publish using async client
    let (client, _eventloop) = rumqttc::AsyncClient::new(mqtt_options, 10);
    client
        .publish(
            &config.topic,
            rumqttc::QoS::AtLeastOnce,
            false,
            payload_str.as_bytes(),
        )
        .await
        .map_err(|e| format!("MQTT publish failed: {}", e))?;

    // Wait a short time for the message to be sent
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    tracing::debug!(
        "MQTT notification sent to topic '{}' on {}",
        config.topic,
        config.broker
    );
    Ok(())
}

/// FFI entry point for dynamic loading.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(MqttPlugin {
        config: Mutex::new(None),
    })
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

/// Register the MQTT plugin as a notification provider.
pub async fn register_mqtt_provider<P>(
    plugin: &MqttPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: NotificationProviderRegistrar,
{
    let config = plugin.config().ok_or("MQTT plugin not configured")?;
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
        let plugin = MqttPlugin::default();
        assert_eq!(plugin.id(), "mqtt");
        assert_eq!(plugin.plugin_type(), PluginType::NotificationProvider);
    }

    #[test]
    fn test_priority_to_string() {
        assert_eq!(priority_to_string(NotificationPriority::Low), "low");
        assert_eq!(priority_to_string(NotificationPriority::Normal), "normal");
        assert_eq!(priority_to_string(NotificationPriority::High), "high");
        assert_eq!(
            priority_to_string(NotificationPriority::Emergency),
            "emergency"
        );
    }

    #[test]
    fn test_config_defaults() {
        let config = MqttConfig {
            broker: default_broker(),
            port: default_port(),
            username: None,
            password: None,
            topic: default_topic(),
            qos: default_qos(),
            use_tls: false,
            client_id: None,
            keepalive_secs: default_keepalive(),
        };
        assert_eq!(config.broker, "localhost");
        assert_eq!(config.port, 1883);
        assert_eq!(config.topic, "rustyfin/notifications");
        assert_eq!(config.qos, 1);
    }
}
