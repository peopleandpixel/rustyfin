use serde::{Deserialize, Serialize};

/// The product name Jellyfin clients expect; jellyfin-sdk-kotlin's RecommendedServerDiscovery
/// rejects a server that reports any other.
pub const JELLYFIN_PRODUCT_NAME: &str = "Jellyfin Server";

/// The Jellyfin version Rustyfin reports as `Version`, matching current stable Jellyfin (12.4.0).
/// jellyfin-sdk-kotlin rates lower versions as outdated (minimum 10.10.0 in SDK 1.7, 10.11.0 in
/// 1.8, 12.0.0 in 1.9) and has no upper bound. Rustyfin's own version goes in `RustyfinVersion`.
pub const JELLYFIN_API_VERSION: &str = "12.4.0";

/// Public server info returned by GET /System/Info/Public (no authentication)
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerInfo {
    /// The address clients reach the server at (`routes::system::local_address`); omitted when
    /// unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_address: Option<String>,
    pub server_name: String,
    pub version: String,
    pub product_name: String,
    /// Always empty, as in Jellyfin, which no longer sets it.
    pub operating_system: String,
    pub id: String,
    pub startup_wizard_completed: bool,
    pub rustyfin_version: String,
}

/// Full server info returned by GET /System/Info (authenticated)
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FullServerInfo {
    pub server_name: String,
    pub version: String,
    pub product_name: String,
    pub id: String,
    pub startup_wizard_completed: bool,
    pub rustyfin_version: String,
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
    pub is_shutting_down: bool,
    pub supports_library_monitor: bool,
    pub items_by_name_path: String,
    pub internal_metadata_path: String,
    pub transcoding_temp_path: String,
    pub web_path: String,
    /// `System` for the ffmpeg on the PATH, `Custom` for `RUSTYFIN_FFMPEG_PATH`, as Jellyfin.
    pub encoder_location: String,
    /// .NET's name for the CPU architecture (`X64`, `Arm64`).
    pub system_architecture: String,
    /// From the configuration's `CastReceiverApplications`.
    pub cast_receiver_applications: serde_json::Value,
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
    /// Quick Connect: on by default, as in Jellyfin. Off, its routes answer 401.
    #[serde(default = "default_true")]
    pub quick_connect_available: bool,
    /// The DLNA server: the `/dlna/*` routes and SSDP discovery. Off by default, as in Jellyfin,
    /// where DLNA is an optional plugin.
    #[serde(default)]
    pub enable_dlna_server: bool,
    #[serde(default = "default_http_port")]
    pub public_https_port: u16,
    #[serde(default = "default_http_port")]
    pub public_port: u16,
    // Provider API keys are write-only input and are never returned by the API. A key sent in a
    // settings POST is saved to the provider's plugin config (the plugin_config table) and then
    // cleared here. A key still waiting to be saved (the save failed) stays set, so it is kept in
    // config.json until it is moved. A key left in an old config.json is moved at startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omdb_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// Seconds that a changed folder of a library must stay unchanged before the library monitor
    /// rescans it.
    #[serde(default = "default_library_monitor_delay")]
    pub library_monitor_delay: u32,
    /// How often polled library paths (network shares) are checked for changes, in seconds.
    /// Values below 1 count as 1.
    #[serde(default = "default_library_monitor_poll_interval")]
    pub library_monitor_poll_interval: u32,
    /// Refuse to read a media, subtitle or picture-subtitle file whose links lead out of every
    /// library. Off: a link inside a library may point anywhere but at the server's own files
    /// (link folders from download tools keep playing). On: the resolved file must lie in a
    /// library too. Rustyfin's own setting; Jellyfin has none.
    #[serde(default)]
    pub refuse_links_out_of_libraries: bool,
    /// The language of the server's own messages, as a culture code (Jellyfin's `UICulture`).
    #[serde(rename = "UICulture", default = "default_ui_culture")]
    pub ui_culture: String,
    /// Which Rustyfin wrote the file, as `CONFIG_VERSION`. A file without it (0) comes from a
    /// Rustyfin before the version, and `upgrade` brings it up to date once. Every write sets it.
    #[serde(rename = "RustyfinConfigVersion", default)]
    pub config_version: u32,
    /// The other fields of Jellyfin's `ServerConfiguration` that a client saved. They are kept and
    /// returned, so Jellyfin Web's settings pages round-trip, but Rustyfin doesn't act on them.
    /// `jellyfin_server_defaults` fills in the ones never saved.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

fn default_ui_culture() -> String {
    "en-US".to_string()
}

/// Jellyfin 12.4.0's defaults for the `ServerConfiguration` fields Rustyfin has no field for.
/// `GET /System/Configuration` adds those a client never saved. `PluginRepositories` is empty:
/// Rustyfin can't install Jellyfin's plugins.
pub fn jellyfin_server_defaults() -> serde_json::Map<String, serde_json::Value> {
    // Per item type: the metadata fetchers and the image fetchers Jellyfin disables by default.
    const OMDB: &[&str] = &["The Open Movie Database"];
    const AUDIO_DB: &[&str] = &["TheAudioDB"];
    let metadata_options: Vec<serde_json::Value> = [
        ("Book", &[][..], &[][..]),
        ("Movie", &[], &[]),
        ("MusicVideo", OMDB, OMDB),
        ("Series", &[], &[]),
        ("MusicAlbum", AUDIO_DB, &[]),
        ("MusicArtist", AUDIO_DB, &[]),
        ("BoxSet", &[], &[]),
        ("Season", &[], &[]),
        ("Episode", &[], &[]),
    ]
    .into_iter()
    .map(|(item_type, disabled, disabled_images)| {
        serde_json::json!({
            "ItemType": item_type,
            "DisabledMetadataSavers": [],
            "LocalMetadataReaderOrder": [],
            "DisabledMetadataFetchers": disabled,
            "MetadataFetcherOrder": [],
            "DisabledImageFetchers": disabled_images,
            "ImageFetcherOrder": [],
        })
    })
    .collect();
    let cache_size = std::thread::available_parallelism().map_or(1, |n| n.get()) * 100;
    use serde_json::json;
    [
        ("EnableCaseSensitiveItemIds", json!(true)),
        ("DisableLiveTvChannelUserDataName", json!(true)),
        ("MetadataPath", json!("")),
        ("SortReplaceCharacters", json!([".", "+", "%"])),
        (
            "SortRemoveCharacters",
            json!([",", "&", "-", "{", "}", "'"]),
        ),
        ("SortRemoveWords", json!(["the", "a", "an"])),
        ("MinResumePct", json!(5)),
        ("MaxResumePct", json!(90)),
        ("MinResumeDurationSeconds", json!(300)),
        ("MinAudiobookResume", json!(5)),
        ("MaxAudiobookResume", json!(5)),
        ("InactiveSessionThreshold", json!(0)),
        ("LibraryUpdateDuration", json!(30)),
        ("CacheSize", json!(cache_size)),
        ("ImageSavingConvention", json!("Legacy")),
        ("MetadataOptions", json!(metadata_options)),
        ("SkipDeserializationForBasicTypes", json!(true)),
        ("SaveMetadataHidden", json!(false)),
        ("ContentTypes", json!([])),
        ("RemoteClientBitrateLimit", json!(0)),
        ("EnableFolderView", json!(false)),
        ("EnableGroupingMoviesIntoCollections", json!(false)),
        ("EnableGroupingShowsIntoCollections", json!(false)),
        ("DisplaySpecialsWithinSeasons", json!(true)),
        ("CodecsUsed", json!([])),
        ("PluginRepositories", json!([])),
        ("EnableExternalContentInSuggestions", json!(true)),
        ("ImageExtractionTimeoutMs", json!(0)),
        ("PathSubstitutions", json!([])),
        ("EnableSlowResponseWarning", json!(true)),
        ("SlowResponseThresholdMs", json!(500)),
        ("CorsHosts", json!(["*"])),
        ("ActivityLogRetentionDays", json!(30)),
        ("LibraryScanFanoutConcurrency", json!(0)),
        ("LibraryMetadataRefreshConcurrency", json!(0)),
        ("AllowClientLogUpload", json!(true)),
        ("DummyChapterDuration", json!(0)),
        ("ChapterImageResolution", json!("MatchSource")),
        ("ParallelImageEncodingLimit", json!(0)),
        (
            "CastReceiverApplications",
            json!([
                { "Id": "F007D354", "Name": "Stable" },
                { "Id": "6F511C87", "Name": "Unstable" },
            ]),
        ),
        (
            "TrickplayOptions",
            json!({
                "EnableHwAcceleration": false,
                "EnableHwEncoding": false,
                "EnableKeyFrameOnlyExtraction": false,
                "ScanBehavior": "NonBlocking",
                "ProcessPriority": "BelowNormal",
                "Interval": 10000,
                "WidthResolutions": [320],
                "TileWidth": 10,
                "TileHeight": 10,
                "Qscale": 4,
                "JpegQuality": 90,
                "ProcessThreads": 1,
            }),
        ),
        ("EnableLegacyAuthorization", json!(false)),
    ]
    .into_iter()
    .map(|(field, value)| (field.to_string(), value))
    .collect()
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

fn default_library_monitor_delay() -> u32 {
    60
}

fn default_library_monitor_poll_interval() -> u32 {
    60
}

/// The `RustyfinConfigVersion` this build writes.
/// 1: `QuickConnectAvailable` takes effect. Before it, Quick Connect was always on and the stored
/// value (false, the old default) did nothing.
pub const CONFIG_VERSION: u32 = 1;

impl ServerConfiguration {
    /// Brings a configuration read from an older file up to date; true when it changed. Run once
    /// at load. A later choice the admin saves carries the current version, so it is kept.
    pub fn upgrade(&mut self) -> bool {
        if self.config_version >= CONFIG_VERSION {
            return false;
        }
        if self.config_version < 1 {
            // Quick Connect was on whatever the file said: it stays on after the upgrade
            self.quick_connect_available = true;
        }
        self.config_version = CONFIG_VERSION;
        true
    }

    /// The sort rules: `SortRemoveWords`, `SortRemoveCharacters` and `SortReplaceCharacters`, with
    /// Jellyfin's defaults for the ones no client has saved.
    pub fn sort_rules(&self) -> crate::sort_name::SortRules {
        crate::sort_name::SortRules::from_config(&self.other)
    }
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
            quick_connect_available: true,
            enable_dlna_server: false,
            public_https_port: 8096,
            public_port: 8096,
            smtp_host: None,
            smtp_port: default_smtp_port(),
            smtp_username: None,
            smtp_password: None,
            smtp_enable_ssl: false,
            smtp_from_address: None,
            tmdb_api_key: None,
            omdb_api_key: None,
            fanart_api_key: None,
            library_monitor_delay: 60,
            library_monitor_poll_interval: 60,
            refuse_links_out_of_libraries: false,
            ui_culture: default_ui_culture(),
            config_version: CONFIG_VERSION,
            other: serde_json::Map::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ServerConfiguration {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn r3admin_upgrade_turns_quick_connect_on_once_and_keeps_a_later_choice() {
        // A file from before the version: the stored false never took effect
        let mut old = parse(r#"{"ServerName":"Box","QuickConnectAvailable":false}"#);
        assert_eq!(old.config_version, 0);
        assert!(old.upgrade());
        assert!(old.quick_connect_available);
        assert_eq!(old.config_version, CONFIG_VERSION);
        assert!(!old.upgrade(), "only once");

        // An admin's false saved by this build stays false
        let mut chosen = parse(r#"{"QuickConnectAvailable":false,"RustyfinConfigVersion":1}"#);
        assert!(!chosen.upgrade());
        assert!(!chosen.quick_connect_available);

        // A new install starts current, with Quick Connect on
        let fresh = ServerConfiguration::default();
        assert_eq!(fresh.config_version, CONFIG_VERSION);
        assert!(fresh.quick_connect_available);
        let json = serde_json::to_value(&fresh).unwrap();
        assert_eq!(json["RustyfinConfigVersion"], CONFIG_VERSION);
    }
}
