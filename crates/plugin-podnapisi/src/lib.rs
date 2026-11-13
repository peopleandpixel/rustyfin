//! Podnapisi subtitle provider plugin for Rustyfin.
//!
//! Searches and downloads subtitles from Podnapisi.net.
//! Good European subtitle coverage, alternative to OpenSubtitles.

use rustyfin_model::media::MediaItem;
use rustyfin_model::plugin::{Plugin, PluginType, SubtitleResult};
use serde::Deserialize;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Mutex;

/// Configuration for the Podnapisi plugin.
#[derive(Debug, Clone, Default)]
pub struct PodnapisiConfig {
    /// Optional user agent override.
    #[allow(dead_code)]
    user_agent: Option<String>,
}

/// Podnapisi subtitle provider plugin.
pub struct PodnapisiPlugin {
    #[allow(dead_code)]
    config: Mutex<PodnapisiConfig>,
}

impl Default for PodnapisiPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(PodnapisiConfig::default()),
        }
    }
}

impl Plugin for PodnapisiPlugin {
    fn id(&self) -> &str {
        "podnapisi"
    }

    fn name(&self) -> &str {
        "Podnapisi"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Searches and downloads subtitles from Podnapisi.net. Good European subtitle coverage, alternative to OpenSubtitles."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::SubtitleProvider
    }

    fn set_configuration(&self, _config: serde_json::Value) -> Result<(), String> {
        // Podnapisi doesn't require configuration
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

// ── Podnapisi API types ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PodnapisiSearchResponse {
    status: Option<String>,
    subtitles: Option<Vec<PodnapisiSubtitle>>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case, dead_code)]
struct PodnapisiSubtitle {
    id: String,
    movie_title: String,
    language: String,
    language_name: Option<String>,
    description: Option<String>,
    #[serde(rename = "movieYear")]
    movie_year: Option<String>,
}

/// Search for subtitles on Podnapisi.
pub async fn search_subtitles(
    client: &reqwest::Client,
    name: &str,
    language: &str,
    year: Option<i32>,
) -> Vec<SubtitleResult> {
    let mut query = serde_json::Map::new();
    query.insert("query".to_string(), serde_json::json!(name));
    query.insert("lang".to_string(), serde_json::json!(language));
    if let Some(y) = year {
        query.insert("year".to_string(), serde_json::json!(y));
    }

    let body = serde_json::json!({
        "method": "searchSubtitles",
        "params": [query],
    });

    let resp = client
        .post("https://rest.podnapisi.org/en/reST")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Podnapisi search request failed: {}", e);
            return vec![];
        }
    };

    let pod_resp: PodnapisiSearchResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Podnapisi JSON parse failed: {}", e);
            return vec![];
        }
    };

    let subtitles = pod_resp.subtitles.unwrap_or_default();
    let mut results = Vec::new();

    for sub in subtitles {
        let lang_name = sub.language_name.unwrap_or_else(|| sub.language.clone());

        results.push(SubtitleResult {
            id: sub.id,
            name: sub.movie_title,
            language: lang_name,
            description: sub.description,
        });
    }

    results
}

/// Download a subtitle from Podnapisi by ID.
pub async fn download_subtitle(
    client: &reqwest::Client,
    subtitle_id: &str,
    cache_dir: &Path,
) -> Result<(), String> {
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|e| format!("Failed to create cache directory: {}", e))?;

    let body = serde_json::json!({
        "method": "downloadSubtitle",
        "params": [subtitle_id],
    });

    let resp = client
        .post("https://rest.podnapisi.org/en/reST")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Download failed with status: {}", status));
    }

    // Podnapisi returns a JSON response with a download URL
    let download_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse download response: {}", e))?;

    let download_url = download_resp
        .as_str()
        .or_else(|| download_resp.get("url").and_then(|u| u.as_str()))
        .ok_or_else(|| "No download URL in response".to_string())?;

    // Fetch the actual subtitle file
    let sub_resp = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch subtitle: {}", e))?;

    let sub_bytes = sub_resp
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

/// FFI entry point for dynamic loading.
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
///
/// # Safety
///
/// The caller takes ownership of the returned `Box<dyn Plugin>` and must free it exactly once.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin: Box<dyn Plugin> = Box::new(PodnapisiPlugin::default());
    Box::into_raw(Box::new(plugin)).cast()
}

/// Trait for registering a subtitle provider with a PluginManager.
#[allow(async_fn_in_trait)]
pub trait SubtitleProviderRegistrar {
    async fn register_subtitle_provider<F1, F2>(
        &self,
        plugin_id: String,
        search_subtitles: F1,
        download_subtitle: F2,
    ) where
        F1: Fn(
                &reqwest::Client,
                &MediaItem,
                &str,
            ) -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
                &Path,
            ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
            + Send
            + Sync
            + 'static;
}

/// Register the Podnapisi plugin with a PluginManager.
pub async fn register_podnapisi_provider<P>(plugin: &PodnapisiPlugin, registrar: &P)
where
    P: SubtitleProviderRegistrar,
{
    let plugin_id = plugin.id().to_string();
    registrar
        .register_subtitle_provider(
            plugin_id,
            move |client: &reqwest::Client,
                  item: &MediaItem,
                  lang: &str|
                  -> Pin<Box<dyn Future<Output = Vec<SubtitleResult>> + Send>> {
                let name = item.name.clone();
                let year = item.production_year;
                let lang = lang.to_string();
                let client = client.clone();
                Box::pin(async move { search_subtitles(&client, &name, &lang, year).await })
            },
            move |client: &reqwest::Client,
                  id: &str,
                  path: &Path|
                  -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
                let id = id.to_string();
                let path = path.to_path_buf();
                let client = client.clone();
                Box::pin(async move { download_subtitle(&client, &id, &path).await })
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = PodnapisiPlugin::default();
        assert_eq!(plugin.id(), "podnapisi");
        assert_eq!(plugin.plugin_type(), PluginType::SubtitleProvider);
    }
}
