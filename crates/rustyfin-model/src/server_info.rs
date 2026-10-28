use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerInfo {
    pub server_name: String,
    pub version: String,
    pub id: String,
    pub startup_wizard_completed: bool,
}

/// Full server info returned by GET /System/Info (authenticated)
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FullServerInfo {
    pub server_name: String,
    pub version: String,
    pub id: String,
    pub startup_wizard_completed: bool,
    pub operating_system: String,
    pub has_pending_restart: bool,
    pub has_update_available: bool,
    pub log_path: String,
    pub cache_path: String,
    pub can_self_restart: bool,
    pub can_launch_web_browser: bool,
    pub program_data_path: String,
    pub web_socket_port_number: u16,
    pub local_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_end_point: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_addresses: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_version: Option<String>,
}

/// Server configuration stored as JSON on disk
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerConfiguration {
    #[serde(default = "default_server_name")]
    pub server_name: String,
    #[serde(default)]
    pub is_startup_wizard_completed: bool,
    #[serde(default = "default_preferred_metadata_language")]
    pub preferred_metadata_language: String,
    #[serde(default = "default_metadata_country_code")]
    pub metadata_country_code: String,
    #[serde(default = "default_log_file_retention_days")]
    pub log_file_retention_days: u32,
    #[serde(default)]
    pub enable_automatic_restart: bool,
    #[serde(default = "default_true")]
    pub enable_metrics: bool,
    #[serde(default = "default_true")]
    pub enable_normalized_item_by_name_ids: bool,
    #[serde(default = "default_true")]
    pub is_port_authorized: bool,
    #[serde(default)]
    pub quick_connect_available: bool,
    #[serde(default = "default_http_port")]
    pub public_https_port: u16,
    #[serde(default = "default_http_port")]
    pub public_port: u16,
    #[serde(default)]
    pub tmdb_api_key: Option<String>,
    #[serde(default)]
    pub omdb_api_key: Option<String>,
    #[serde(default)]
    pub fanart_api_key: Option<String>,
    // SMTP mail server configuration for password reset emails
    #[serde(default)]
    pub smtp_host: Option<String>,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_username: Option<String>,
    #[serde(default)]
    pub smtp_password: Option<String>,
    #[serde(default)]
    pub smtp_enable_ssl: bool,
    #[serde(default)]
    pub smtp_from_address: Option<String>,
}

fn default_smtp_port() -> u16 {
    587
}

fn default_server_name() -> String {
    "Rustyfin".to_string()
}

fn default_preferred_metadata_language() -> String {
    "en".to_string()
}

fn default_metadata_country_code() -> String {
    "US".to_string()
}

fn default_log_file_retention_days() -> u32 {
    3
}

fn default_true() -> bool {
    true
}

fn default_http_port() -> u16 {
    8096
}

impl Default for ServerConfiguration {
    fn default() -> Self {
        Self {
            server_name: default_server_name(),
            is_startup_wizard_completed: false,
            preferred_metadata_language: default_preferred_metadata_language(),
            metadata_country_code: default_metadata_country_code(),
            log_file_retention_days: default_log_file_retention_days(),
            enable_automatic_restart: false,
            enable_metrics: true,
            enable_normalized_item_by_name_ids: true,
            is_port_authorized: true,
            quick_connect_available: false,
            public_https_port: 8096,
            public_port: 8096,
            tmdb_api_key: None,
            omdb_api_key: None,
            fanart_api_key: None,
            smtp_host: None,
            smtp_port: default_smtp_port(),
            smtp_username: None,
            smtp_password: None,
            smtp_enable_ssl: false,
            smtp_from_address: None,
        }
    }
}

/// Network endpoint information returned by GET /System/Endpoint
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointInfo {
    pub is_local: bool,
    pub is_in_network: bool,
    pub local_address: String,
    pub remote_address: String,
}

/// A single log file entry
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogFile {
    pub date_created: String,
    pub date_modified: String,
    pub name: String,
    pub size: u64,
}
