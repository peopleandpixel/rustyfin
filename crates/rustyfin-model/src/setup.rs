use serde::{Deserialize, Serialize};

/// Response for GET /Startup/Configuration
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartupConfiguration {
    pub ui_culture: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
}

/// Request body for POST /Startup/Configuration
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartupConfigurationUpdate {
    pub ui_culture: Option<String>,
    pub metadata_country_code: Option<String>,
    pub preferred_metadata_language: Option<String>,
}

/// Response for GET /Startup/User
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartupUser {
    pub name: String,
    pub password: String,
}

/// Request body for POST /Startup/User
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartupUserCreate {
    pub name: String,
    pub password: String,
}
