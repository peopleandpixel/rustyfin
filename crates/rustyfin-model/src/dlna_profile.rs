use serde::{Deserialize, Serialize};

/// DLNA profile for a device, describing its media capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaProfile {
    /// Profile name
    pub name: String,
    /// Device description URL
    #[serde(rename = "DeviceProfileXml")]
    pub device_profile_xml: Option<String>,
    /// Direct play profiles
    #[serde(rename = "DirectPlayProfiles")]
    pub direct_play_profiles: Vec<DlnaStreamProfile>,
    /// Transcode profiles
    #[serde(rename = "TranscodingProfiles")]
    pub transcoding_profiles: Vec<DlnaStreamProfile>,
    /// Container profiles
    #[serde(rename = "ContainerProfiles")]
    pub container_profiles: Vec<DlnaContainerProfile>,
    /// Response profiles
    #[serde(rename = "ResponseProfiles")]
    pub response_profiles: Vec<DlnaResponseProfile>,
    /// Codec profiles
    #[serde(rename = "CodecProfiles")]
    pub codec_profiles: Vec<DlnaCodecProfile>,
    /// Subtitle profiles
    #[serde(rename = "SubtitleProfiles")]
    pub subtitle_profiles: Vec<DlnaSubtitleProfile>,
}

/// Stream profile for direct play or transcoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaStreamProfile {
    pub container: Option<String>,
    #[serde(rename = "Type")]
    pub profile_type: Option<String>,
    pub protocol: Option<String>,
    #[serde(rename = "EstimateContentLength")]
    pub estimate_content_length: Option<bool>,
    #[serde(rename = "EnableMpegtsM2TsMode")]
    pub enable_mpegts_m2ts_mode: Option<bool>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

/// Container profile for format-specific handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaContainerProfile {
    #[serde(rename = "Container")]
    pub container: String,
    #[serde(rename = "Action")]
    pub action: String,
    #[serde(rename = "Condition")]
    pub condition: Option<String>,
}

/// Response profile for content modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaResponseProfile {
    #[serde(rename = "MimeType")]
    pub mime_type: String,
    #[serde(rename = "Container")]
    pub container: Option<String>,
    #[serde(rename = "Type")]
    pub profile_type: String,
    #[serde(rename = "OrgPath")]
    pub org_path: Option<String>,
}

/// Codec profile for codec-specific restrictions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaCodecProfile {
    #[serde(rename = "Type")]
    pub profile_type: String,
    #[serde(rename = "Codec")]
    pub codec: Option<String>,
    #[serde(rename = "Filename")]
    pub filename: Option<bool>,
    #[serde(rename = "Conditions")]
    pub conditions: Vec<DlnaCondition>,
}

/// Condition for codec profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaCondition {
    #[serde(rename = "Condition")]
    pub condition_type: String,
    #[serde(rename = "Property")]
    pub property: String,
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "IsRequired")]
    pub is_required: Option<bool>,
}

/// Subtitle profile for subtitle handling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaSubtitleProfile {
    #[serde(rename = "Format")]
    pub format: Option<String>,
    #[serde(rename = "Method")]
    pub method: String,
    #[serde(rename = "DidlMode")]
    pub didl_mode: Option<String>,
}

/// Device detection result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DlnaDeviceDetection {
    /// User agent string
    #[serde(rename = "UserAgent")]
    pub user_agent: String,
    /// Friendly device name
    #[serde(rename = "FriendlyName")]
    pub friendly_name: String,
    /// Model name
    #[serde(rename = "ModelName")]
    pub model_name: String,
    /// Model number
    #[serde(rename = "ModelNumber")]
    pub model_number: String,
    /// Serial number
    #[serde(rename = "SerialNumber")]
    pub serial_number: String,
    /// Manufacturer
    pub manufacturer: String,
    /// Manufacturer URL
    #[serde(rename = "ManufacturerUrl")]
    pub manufacturer_url: String,
    /// Device ID
    #[serde(rename = "DeviceId")]
    pub device_id: String,
    /// Icon URL
    #[serde(rename = "IconUrl")]
    pub icon_url: Option<String>,
}
