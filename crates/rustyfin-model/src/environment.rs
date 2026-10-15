use serde::{Deserialize, Serialize};

/// Drive information returned by the Environment controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DriveInfo {
    /// Drive name/label
    pub name: String,
    /// Drive type
    #[serde(rename = "Type")]
    pub drive_type: String,
    /// Total size in bytes
    #[serde(rename = "TotalSize")]
    pub total_size: i64,
    /// Available free space in bytes
    #[serde(rename = "AvailableFreeSpace")]
    pub available_free_space: i64,
    /// Drive path
    pub path: String,
    /// Whether the drive is ready
    #[serde(rename = "IsReady")]
    pub is_ready: bool,
}

/// Network share information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkShare {
    /// Share name
    pub name: String,
    /// Share path (UNC or local)
    pub path: String,
    /// Server/host name
    pub server: String,
    /// Share type
    #[serde(rename = "Type")]
    pub share_type: String,
}
