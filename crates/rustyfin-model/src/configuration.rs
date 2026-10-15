use serde::{Deserialize, Serialize};

/// Media encoding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaEncodingConfiguration {
    #[serde(default = "default_encode_path")]
    pub encoding_scope: String,
    #[serde(default = "default_down_mix_audio")]
    pub down_mix_audio_scale: f64,
    #[serde(default)]
    pub down_mix_audio_mode: String,
    #[serde(default)]
    pub enable_audio_cache_repair: bool,
    #[serde(default = "default_true")]
    pub enable_thunder_labs: bool,
    #[serde(default = "default_video_bitrate")]
    pub video_bitrate: u32,
    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate: u32,
    #[serde(default = "default_bitrate_depth")]
    pub bitrate_depth: u32,
    #[serde(default = "default_min_default_bitrate")]
    pub min_default_bitrate: u32,
    #[serde(default = "default_deinterlace")]
    pub deinterlace_filter: String,
    #[serde(default)]
    pub enable_non_anamorphic: bool,
    #[serde(default)]
    pub video_hardware_acceleration: String,
    #[serde(default)]
    pub video_codecs: Vec<String>,
    #[serde(default)]
    pub audio_codecs: Vec<String>,
    #[serde(default)]
    pub subtitle_codecs: Vec<String>,
    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,
    #[serde(default = "default_ffmpeg_live_path")]
    pub ffmpeg_live_path: String,
    #[serde(default)]
    pub ffmpeg_extra_hardware_args: String,
    #[serde(default)]
    pub ffmpeg_extra_args: String,
    #[serde(default = "default_enable_fallback_ffmpeg_path")]
    pub enable_fallback_ffmpeg_path: String,
    #[serde(default)]
    pub allow_breaking_the_bitrate_limit: bool,
    #[serde(default)]
    pub allow_high_bitrate: bool,
    #[serde(default)]
    pub allow_map_filename: bool,
    #[serde(default)]
    pub allow_map_auto_channels: bool,
    #[serde(default)]
    pub allow_copy_subtitles_in_hls: bool,
    #[serde(default)]
    pub allow_copy_with_chapters_in_hls: bool,
    #[serde(default)]
    pub allow_copy_with_pts_correction_in_hls: bool,
    #[serde(default)]
    pub allow_dts_in_hls: bool,
    #[serde(default)]
    pub allow_mkv: bool,
    #[serde(default)]
    pub allow_soft_rename: bool,
    #[serde(default)]
    pub enable_mpegts_m2ts: bool,
    #[serde(default)]
    pub enable_ac3_transcode: bool,
    #[serde(default = "default_preferred_transcode_codec_audio")]
    pub preferred_transcode_codec_audio: String,
    #[serde(default = "default_preferred_transcode_codec_video")]
    pub preferred_transcode_codec_video: String,
    #[serde(default)]
    pub tone_mapping_enabled: bool,
    #[serde(default)]
    pub tone_mapping_algorithm: String,
    #[serde(default)]
    pub tone_mapping_range_type: String,
    #[serde(default)]
    pub tone_mapping_peak: f64,
    #[serde(default)]
    pub tone_mapping_max_truncate: f64,
    #[serde(default)]
    pub tone_mapping_linear: bool,
    #[serde(default)]
    pub tone_mapping_clamp: bool,
    #[serde(default)]
    pub tone_mapping_threshold: f64,
    #[serde(default)]
    pub tone_mapping_desaturate: f64,
    #[serde(default)]
    pub tone_mapping_peak_fallback: f64,
    #[serde(default)]
    pub tone_mapping_offset: f64,
}

fn default_encode_path() -> String {
    "All".to_string()
}
fn default_down_mix_audio() -> f64 {
    1.0
}
fn default_true() -> bool {
    true
}
fn default_video_bitrate() -> u32 {
    8000000
}
fn default_audio_bitrate() -> u32 {
    384000
}
fn default_bitrate_depth() -> u32 {
    3
}
fn default_min_default_bitrate() -> u32 {
    2000000
}
fn default_deinterlace() -> String {
    "Yadif".to_string()
}
fn default_ffmpeg_path() -> String {
    "ffmpeg".to_string()
}
fn default_ffmpeg_live_path() -> String {
    "ffmpeg".to_string()
}
fn default_enable_fallback_ffmpeg_path() -> String {
    String::new()
}
fn default_preferred_transcode_codec_audio() -> String {
    "aac".to_string()
}
fn default_preferred_transcode_codec_video() -> String {
    "libx264".to_string()
}

impl Default for MediaEncodingConfiguration {
    fn default() -> Self {
        Self {
            encoding_scope: default_encode_path(),
            down_mix_audio_scale: default_down_mix_audio(),
            down_mix_audio_mode: String::new(),
            enable_audio_cache_repair: false,
            enable_thunder_labs: true,
            video_bitrate: default_video_bitrate(),
            audio_bitrate: default_audio_bitrate(),
            bitrate_depth: default_bitrate_depth(),
            min_default_bitrate: default_min_default_bitrate(),
            deinterlace_filter: default_deinterlace(),
            enable_non_anamorphic: false,
            video_hardware_acceleration: String::new(),
            video_codecs: Vec::new(),
            audio_codecs: Vec::new(),
            subtitle_codecs: Vec::new(),
            ffmpeg_path: default_ffmpeg_path(),
            ffmpeg_live_path: default_ffmpeg_live_path(),
            ffmpeg_extra_hardware_args: String::new(),
            ffmpeg_extra_args: String::new(),
            enable_fallback_ffmpeg_path: default_enable_fallback_ffmpeg_path(),
            allow_breaking_the_bitrate_limit: false,
            allow_high_bitrate: false,
            allow_map_filename: false,
            allow_map_auto_channels: false,
            allow_copy_subtitles_in_hls: false,
            allow_copy_with_chapters_in_hls: false,
            allow_copy_with_pts_correction_in_hls: false,
            allow_dts_in_hls: false,
            allow_mkv: false,
            allow_soft_rename: false,
            enable_mpegts_m2ts: false,
            enable_ac3_transcode: false,
            preferred_transcode_codec_audio: default_preferred_transcode_codec_audio(),
            preferred_transcode_codec_video: default_preferred_transcode_codec_video(),
            tone_mapping_enabled: false,
            tone_mapping_algorithm: String::new(),
            tone_mapping_range_type: String::new(),
            tone_mapping_peak: 0.0,
            tone_mapping_max_truncate: 0.0,
            tone_mapping_linear: false,
            tone_mapping_clamp: false,
            tone_mapping_threshold: 0.0,
            tone_mapping_desaturate: 0.0,
            tone_mapping_peak_fallback: 0.0,
            tone_mapping_offset: 0.0,
        }
    }
}

/// Networking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkingConfiguration {
    #[serde(default)]
    pub enable_https: bool,
    #[serde(default = "default_http_port")]
    pub http_server_port: u16,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    #[serde(default)]
    pub url_base: String,
    #[serde(default)]
    pub enable_preferred_router_access: bool,
    #[serde(default)]
    pub published_server_uri: String,
    #[serde(default)]
    pub local_network_addresses: Vec<String>,
    #[serde(default)]
    pub local_network_interfaces: Vec<String>,
    #[serde(default)]
    pub certificate_path: String,
    #[serde(default)]
    pub certificate_password: String,
}

fn default_http_port() -> u16 {
    8096
}
fn default_https_port() -> u16 {
    8920
}

impl Default for NetworkingConfiguration {
    fn default() -> Self {
        Self {
            enable_https: false,
            http_server_port: default_http_port(),
            https_port: default_https_port(),
            url_base: String::new(),
            enable_preferred_router_access: false,
            published_server_uri: String::new(),
            local_network_addresses: Vec::new(),
            local_network_interfaces: Vec::new(),
            certificate_path: String::new(),
            certificate_password: String::new(),
        }
    }
}

/// Server recording configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ServerRecordingConfiguration {
    #[serde(default)]
    pub enable_video_metadata: bool,
    #[serde(default)]
    pub enable_all_metadata: bool,
    #[serde(default)]
    pub enable_base_metadata: bool,
    #[serde(default)]
    pub enable_optical_disc_metadata: bool,
    #[serde(default)]
    pub enable_last_shutdown_metadata: bool,
    #[serde(default)]
    pub enable_startup_metadata: bool,
    #[serde(default)]
    pub enable_visual_metadata: bool,
}

/// Display configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DisplayConfiguration {
    #[serde(default)]
    pub enable_grouped_albums: bool,
    #[serde(default)]
    pub enable_all_album_artists: bool,
    #[serde(default)]
    pub group_episodes_by_season: bool,
    #[serde(default)]
    pub group_by_collection: bool,
    #[serde(default)]
    pub hide_audio_playlists_in_music: bool,
    #[serde(default)]
    pub hide_culture_titles: bool,
    #[serde(default)]
    pub hide_episodes: bool,
    #[serde(default)]
    pub hide_empty_series: bool,
    #[serde(default)]
    pub hide_seasons: bool,
    #[serde(default)]
    pub home_screen_show_collapsed_sections: bool,
    #[serde(default)]
    pub home_screen_show_languages: bool,
    #[serde(default)]
    pub home_screen_show_playlists: bool,
    #[serde(default)]
    pub home_screen_show_recently_viewed: bool,
    #[serde(default)]
    pub home_screen_show_resume_anywhere: bool,
    #[serde(default)]
    pub home_screen_show_resume_books: bool,
    #[serde(default)]
    pub home_screen_show_resume_movies: bool,
    #[serde(default)]
    pub home_screen_show_resume_shows: bool,
    #[serde(default)]
    pub home_screen_show_upcoming: bool,
    #[serde(default)]
    pub home_screen_show_favorites: bool,
    #[serde(default)]
    pub limit_field: bool,
    #[serde(default)]
    pub memory_cache_enabled: bool,
    #[serde(default)]
    pub memory_cache_size: u32,
    #[serde(default)]
    pub memory_cache_base_path: String,
    #[serde(default)]
    pub memory_cache_max_size: u32,
    #[serde(default)]
    pub memory_cache_max_age: u32,
    #[serde(default)]
    pub memory_cache_max_items: u32,
}

/// Parental controls configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ParentalControlsConfiguration {
    #[serde(default)]
    pub blocked_tags: Vec<String>,
    #[serde(default)]
    pub blocked_channels: Vec<String>,
    #[serde(default)]
    pub blocked_items: Vec<String>,
    #[serde(default)]
    pub blocked_media_types: Vec<String>,
    #[serde(default)]
    pub blocked_genres: Vec<String>,
    #[serde(default)]
    pub blocked_studios: Vec<String>,
    #[serde(default)]
    pub blocked_people: Vec<String>,
    #[serde(default)]
    pub blocked_collections: Vec<String>,
    #[serde(default)]
    pub blocked_playlists: Vec<String>,
    #[serde(default)]
    pub blocked_categories: Vec<String>,
    #[serde(default)]
    pub blocked_official_ratings: Vec<String>,
    #[serde(default)]
    pub blocked_words: Vec<String>,
    #[serde(default)]
    pub enable_blocked_tags: bool,
    #[serde(default)]
    pub enable_blocked_channels: bool,
    #[serde(default)]
    pub enable_blocked_items: bool,
    #[serde(default)]
    pub enable_blocked_media_types: bool,
    #[serde(default)]
    pub enable_blocked_genres: bool,
    #[serde(default)]
    pub enable_blocked_studios: bool,
    #[serde(default)]
    pub enable_blocked_people: bool,
    #[serde(default)]
    pub enable_blocked_collections: bool,
    #[serde(default)]
    pub enable_blocked_playlists: bool,
    #[serde(default)]
    pub enable_blocked_categories: bool,
    #[serde(default)]
    pub enable_blocked_official_ratings: bool,
    #[serde(default)]
    pub enable_blocked_words: bool,
}
