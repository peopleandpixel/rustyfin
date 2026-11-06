//! OMDb metadata provider plugin for Rustyfin.
//!
//! Fetches metadata from The Open Movie Database.

use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::pin::Pin;
use std::sync::Mutex;
use std::future::Future;

const OMDB_BASE: &str = "https://www.omdbapi.com";

#[derive(Debug, Deserialize)]
struct OmdbResult {
    #[serde(rename = "Title")]
    title: Option<String>,
    #[serde(rename = "Year")]
    year: Option<String>,
    #[serde(rename = "Rated")]
    rated: Option<String>,
    #[serde(rename = "Runtime")]
    runtime: Option<String>,
    #[serde(rename = "Genre")]
    genre: Option<String>,
    #[serde(rename = "Plot")]
    plot: Option<String>,
    #[serde(rename = "Poster")]
    poster: Option<String>,
    #[serde(rename = "imdbRating")]
    imdb_rating: Option<String>,
    #[serde(rename = "Response")]
    response: Option<String>,
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('?', "%3F")
        .replace('#', "%23")
}

/// Search OMDb for a title and return metadata
pub async fn search(api_key: &str, name: &str, year: Option<i32>) -> Option<ExternalMetadata> {
    let client = reqwest::Client::new();
    let mut url = format!("{}/?apikey={}&t={}", OMDB_BASE, api_key, urlencoded(name));
    if let Some(y) = year {
        url.push_str(&format!("&y={}", y));
    }

    let resp: OmdbResult = client.get(&url).send().await.ok()?.json().await.ok()?;

    if resp.response.as_deref() != Some("True") {
        return None;
    }

    let production_year = resp
        .year
        .as_deref()
        .and_then(|y| y.split('–').next())
        .and_then(|y| y.trim().parse().ok());

    let community_rating = resp
        .imdb_rating
        .as_deref()
        .and_then(|r| r.parse::<f64>().ok());

    let genres = resp
        .genre
        .map(|g| g.split(',').map(|s| s.trim().to_string()).collect());

    let runtime_ticks = resp
        .runtime
        .as_deref()
        .and_then(|r| r.split(' ').next())
        .and_then(|m| m.parse::<i64>().ok())
        .map(|m| m * 600_000_000);

    let mut images = Vec::new();
    if let Some(poster) = &resp.poster
        && poster != "N/A"
    {
        images.push(MetadataImage {
            image_type: "Primary".to_string(),
            url: poster.clone(),
        });
    }

    Some(ExternalMetadata {
        name: resp.title,
        overview: resp.plot,
        production_year,
        community_rating,
        official_rating: resp.rated,
        genres,
        premiere_date: None,
        runtime_ticks,
        image_urls: images,
        tmdb_id: None,
        tvdb_id: None,
        status: None,
        original_language: None,
        tagline: None,
        production_countries: None,
        budget: None,
        revenue: None,
        homepage: None,
        end_date: None,
        vote_count: None,
        popularity: None,
        studios: None,
        directors: None,
        writers: None,
        imdb_id: None,
    })
}

// ── Plugin implementation ──

/// OMDb plugin that registers itself as a metadata provider.
pub struct OmdbPlugin {
    api_key: Mutex<Option<String>>,
}

impl Default for OmdbPlugin {
    fn default() -> Self {
        Self {
            api_key: Mutex::new(None),
        }
    }
}

impl Plugin for OmdbPlugin {
    fn id(&self) -> &str {
        "omdb"
    }

    fn name(&self) -> &str {
        "Open Movie Database (OMDb)"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches metadata from The Open Movie Database (OMDb)."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::MetadataProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        *self.api_key.lock().unwrap() = config
            .get("api_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Ok(())
    }

    fn get_configuration(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "api_key": self.api_key.lock().unwrap().as_deref()
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl OmdbPlugin {
    fn api_key(&self) -> Option<String> {
        self.api_key.lock().unwrap().clone()
    }
}

/// Create a new OMDb plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(OmdbPlugin {
        api_key: Mutex::new(None),
    })
}

/// Register the OMDb plugin as a metadata provider with the given PluginManager.
pub async fn register_omdb_provider<P>(
    plugin: &OmdbPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let api_key = plugin.api_key().ok_or("OMDb API key not configured")?;
    let api_key1 = api_key.clone();
    let api_key2 = api_key.clone();

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "omdb".to_string(),
            move |_client: &reqwest::Client, name: &str, year: Option<i32>| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let api_key = api_key1.clone();
                let name = name.to_string();
                Box::pin(async move { search(&api_key, &name, year).await })
            },
            move |_client: &reqwest::Client, name: &str| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let api_key = api_key2.clone();
                let name = name.to_string();
                Box::pin(async move { search(&api_key, &name, None).await })
            },
            move |_client: &reqwest::Client, _tmdb_id: Option<i64>, _tvdb_id: Option<i64>, _media_type: MediaType| -> Pin<
                Box<dyn Future<Output = Vec<MetadataImage>> + Send>,
            > {
                Box::pin(async move { Vec::new() })
            },
        )
        .await;

    Ok(())
}

/// Trait for types that can register metadata providers.
pub trait MetadataProviderRegistrar {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(&reqwest::Client, &str, Option<i32>) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> + Send + Sync + 'static,
        F2: Fn(&reqwest::Client, &str) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> + Send + Sync + 'static,
        F3: Fn(&reqwest::Client, Option<i64>, Option<i64>, MediaType) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> + Send + Sync + 'static;
}
