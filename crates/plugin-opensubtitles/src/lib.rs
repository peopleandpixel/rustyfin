//! OpenSubtitles subtitle provider plugin for Rustyfin.
//!
//! Searches and downloads subtitles from the OpenSubtitles API.

use rustyfin_model::media::MediaItem;
use rustyfin_model::plugin::{Plugin, PluginType, SubtitleResult};
use serde::Deserialize;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Mutex;

// ── OpenSubtitles API types ──

#[derive(Debug, Deserialize)]
struct OsSearchResponse {
    data: Option<Vec<OsSubtitleEntry>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct OsSubtitleEntry {
    id: String,
    SubFileName: String,
    SubLanguageID: String,
}

// ── Plugin ──

/// OpenSubtitles subtitle provider plugin.
pub struct OpenSubtitlesPlugin {
    user_agent: Mutex<String>,
}

impl Default for OpenSubtitlesPlugin {
    fn default() -> Self {
        Self {
            user_agent: Mutex::new(format!("rustyfin/{}", env!("CARGO_PKG_VERSION"))),
        }
    }
}

impl Plugin for OpenSubtitlesPlugin {
    fn id(&self) -> &str {
        "opensubtitles"
    }

    fn name(&self) -> &str {
        "OpenSubtitles"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Searches and downloads subtitles from OpenSubtitles"
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::SubtitleProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        if let Some(user_agent) = config.get("user_agent").and_then(|v| v.as_str()) {
            *self.user_agent.lock().unwrap() = user_agent.to_string();
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl OpenSubtitlesPlugin {
    /// Returns the configured user agent string.
    pub fn user_agent(&self) -> String {
        self.user_agent.lock().unwrap().clone()
    }
}

// ── Public API functions ──

fn language_name(code: &str) -> &str {
    match code {
        "eng" => "English",
        "deu" => "German",
        "fra" => "French",
        "spa" => "Spanish",
        "ita" => "Italian",
        "por" => "Portuguese",
        "jpn" => "Japanese",
        "kor" => "Korean",
        "chi" | "zho" => "Chinese",
        "rus" => "Russian",
        "nld" => "Dutch",
        "pol" => "Polish",
        "tur" => "Turkish",
        "ara" => "Arabic",
        other => other,
    }
}

/// Search for subtitles on OpenSubtitles.
pub async fn search_subtitles(
    client: &reqwest::Client,
    user_agent: &str,
    name: &str,
    language: &str,
    year: Option<i32>,
) -> Vec<SubtitleResult> {
    let mut query = serde_json::Map::new();
    query.insert("query".to_string(), serde_json::json!(name));
    if let Some(y) = year {
        query.insert("movie_year".to_string(), serde_json::json!(y));
    }
    query.insert("sublanguageid".to_string(), serde_json::json!(language));

    let body = serde_json::json!({
        "method": "search",
        "params": query,
    });

    let resp = client
        .post("https://api.opensubtitles.com/api/v1/search")
        .header("User-Agent", user_agent)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("OpenSubtitles search request failed: {}", e);
            return vec![];
        }
    };

    let os_resp: OsSearchResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("OpenSubtitles JSON parse failed: {}", e);
            return vec![];
        }
    };

    if let Some(ref error) = os_resp.error {
        tracing::warn!("OpenSubtitles API error: {}", error);
        return vec![];
    }

    let entries = os_resp.data.unwrap_or_default();
    let mut results = Vec::new();

    for entry in entries {
        let lang_name = language_name(&entry.SubLanguageID).to_string();

        results.push(SubtitleResult {
            id: entry.id,
            name: entry.SubFileName,
            language: lang_name,
            description: None,
        });
    }

    results
}

/// Download a subtitle from OpenSubtitles by ID.
pub async fn download_subtitle(
    client: &reqwest::Client,
    user_agent: &str,
    subtitle_id: &str,
    cache_dir: &Path,
) -> Result<(), String> {
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|e| format!("Failed to create cache directory: {}", e))?;

    let body = serde_json::json!({
        "method": "download",
        "params": {
            "subid": subtitle_id,
        },
    });

    let resp = client
        .post("https://api.opensubtitles.com/api/v1/download")
        .header("User-Agent", user_agent)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Download failed with status: {}", status));
    }

    let download_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse download response: {}", e))?;

    let download_url = download_resp
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|d| d.get("data"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| "No download URL in response".to_string())?;

    let sub_bytes = client
        .get(download_url)
        .header("User-Agent", user_agent)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch subtitle: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read subtitle bytes: {}", e))?;

    let filename = format!("{}.srt", subtitle_id);
    let file_path = cache_dir.join(&filename);

    tokio::fs::write(&file_path, &sub_bytes)
        .await
        .map_err(|e| format!("Failed to write subtitle file: {}", e))?;

    tracing::info!("Downloaded subtitle {} to {}", subtitle_id, filename);
    Ok(())
}

// ── Registrar trait for PluginManager integration ──

/// Trait for registering a subtitle provider with a PluginManager.
pub trait SubtitleProviderRegistrar {
    async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(&reqwest::Client, &MediaItem, &str) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>> + Send + Sync + 'static,
        F2: Fn(&reqwest::Client, &str, &Path) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync + 'static;
}

/// Register the OpenSubtitles plugin with a PluginManager.
pub async fn register_opensubtitles_provider<P>(
    plugin: &OpenSubtitlesPlugin,
    registrar: &P,
) where
    P: SubtitleProviderRegistrar,
{
    let user_agent = plugin.user_agent();
    let user_agent1 = user_agent.clone();
    let user_agent2 = user_agent.clone();

    let plugin_id = plugin.id().to_string();
    registrar.register_subtitle_provider(
        plugin_id,
        move |client: &reqwest::Client, item: &MediaItem, lang: &str| -> Pin<
            Box<dyn Future<Output = Vec<SubtitleResult>> + Send>,
        > {
            let user_agent = user_agent1.clone();
            let name = item.name.clone();
            let year = item.production_year;
            let lang = lang.to_string();
            let client = client.clone();
            Box::pin(async move {
                search_subtitles(&client, &user_agent, &name, &lang, year).await
            })
        },
        move |client: &reqwest::Client, id: &str, path: &Path| -> Pin<
            Box<dyn Future<Output = Result<(), String>> + Send>,
        > {
            let user_agent = user_agent2.clone();
            let id = id.to_string();
            let path = path.to_path_buf();
            let client = client.clone();
            Box::pin(async move {
                download_subtitle(&client, &user_agent, &id, &path).await
            })
        },
    ).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = OpenSubtitlesPlugin::default();
        assert_eq!(plugin.id(), "opensubtitles");
        assert_eq!(plugin.plugin_type(), PluginType::SubtitleProvider);
    }

    #[test]
    fn test_language_name() {
        assert_eq!(language_name("eng"), "English");
        assert_eq!(language_name("deu"), "German");
        assert_eq!(language_name("zho"), "Chinese");
        assert_eq!(language_name("xyz"), "xyz");
    }
}
