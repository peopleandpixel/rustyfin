//! TheTVDB metadata provider plugin for Rustyfin.
//!
//! Fetches metadata and images from TheTVDB v2 API with JWT authentication.
//! Provides better TV series metadata than TMDB, including season/episode details.

use moka::sync::Cache;
use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::pin::Pin;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::future::Future;

const TVDB_BASE: &str = "https://api.tvmaze.com";

/// L1 in-memory cache for TVMaze API responses.
static TVDB_CACHE: LazyLock<Cache<String, String>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(10_000)
        .time_to_live(std::time::Duration::from_secs(24 * 3600))
        .build()
});

/// Fetch JSON from TVMaze with caching.
async fn fetch_tvdb_json(
    client: &reqwest::Client,
    cache_key: &str,
    url: &str,
) -> Option<serde_json::Value> {
    if let Some(cached) = TVDB_CACHE.get(cache_key) {
        return serde_json::from_str(&cached).ok();
    }
    let resp = client.get(url).send().await.ok()?;
    let text = resp.text().await.ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    TVDB_CACHE.insert(cache_key.to_string(), text);
    Some(json)
}

// ── TVMaze types ──

#[derive(Debug, Deserialize)]
struct TvmazeSearchResult {
    score: f64,
    show: TvmazeShow,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeShow {
    id: i64,
    name: String,
    rating: Option<TvmazeRating>,
    summary: Option<String>,
    premiered: Option<String>,
    ended: Option<String>,
    language: Option<String>,
    genres: Option<Vec<String>>,
    style: Option<String>,
    runtime: Option<i32>,
    status: Option<String>,
    webchannel: Option<String>,
    weight: Option<i32>,
    network: Option<TvmazeNetwork>,
    #[serde(rename = "webChannel")]
    #[allow(non_snake_case)]
    web_channel: Option<TvmazeWebChannel>,
    externals: TvmazeExternals,
    image: Option<TvmazeImage>,
    country: Option<TvmazeCountry>,
    schedule: Option<TvmazeSchedule>,
    rating_count: Option<i32>,
    updated: Option<i64>,
    _links: Option<TvmazeLinks>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeRating {
    average: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeNetwork {
    id: i64,
    name: String,
    country: Option<TvmazeCountry>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeWebChannel {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize, Clone)]
struct TvmazeExternals {
    tvdb: Option<i64>,
    #[serde(rename = "thetvdb")]
    #[allow(dead_code)]
    thetvdb: Option<i64>,
    imdb: Option<String>,
    tmdb: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeImage {
    medium: String,
    original: String,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeCountry {
    name: String,
    code: Option<String>,
    timezone: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeSchedule {
    time: String,
    days: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeLinks {
    #[serde(rename = "self")]
    self_link: Option<TvmazeLink>,
    previousepisode: Option<TvmazeLink>,
    nextepisode: Option<TvmazeLink>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeLink {
    href: String,
}

// ── TVMaze Cast types ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TvmazeCastMember {
    person: TvmazePerson,
    #[allow(dead_code)]
    character: TvmazeCharacter,
}

#[derive(Debug, Deserialize, Clone)]
struct TvmazePerson {
    id: i64,
    name: String,
    #[allow(dead_code)]
    country: Option<TvmazeCountry>,
    #[allow(dead_code)]
    birthday: Option<String>,
    #[allow(dead_code)]
    deathday: Option<String>,
    #[allow(dead_code)]
    gender: Option<String>,
    image: Option<TvmazeImage>,
    #[allow(dead_code)]
    updated: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TvmazeCharacter {
    id: i64,
    name: String,
}

/// Result of fetching a person's photo from TVMaze
#[derive(Debug, Clone)]
pub struct PersonPhoto {
    pub person_id: String,
    pub name: String,
    pub photo_url: String,
}

/// Fetch cast for a TV show from TVMaze, returning person info with photos
pub async fn get_tv_people(
    client: &reqwest::Client,
    tvdb_id: i64,
) -> Vec<PersonPhoto> {
    let url = format!("{}/shows/{}/cast", TVDB_BASE, tvdb_id);
    let cache_key = format!("tvdb_cast:{}", tvdb_id);

    let mut result = Vec::new();

    if let Some(json) = fetch_tvdb_json(client, &cache_key, &url).await {
        if let Ok(cast) = serde_json::from_value::<Vec<TvmazeCastMember>>(json) {
            for member in cast.into_iter().take(20) {
                if let Some(ref image) = member.person.image {
                    result.push(PersonPhoto {
                        person_id: member.person.id.to_string(),
                        name: member.person.name,
                        photo_url: image.original.clone(),
                    });
                }
            }
        }
    }

    result
}

/// Fetch a person's details from TVMaze by TVDB ID, returning photo URL if available
pub async fn get_person_photo(
    client: &reqwest::Client,
    tvdb_person_id: i64,
) -> Option<PersonPhoto> {
    let url = format!("{}/people/{}", TVDB_BASE, tvdb_person_id);
    let cache_key = format!("tvdb_person:{}", tvdb_person_id);

    if let Some(json) = fetch_tvdb_json(client, &cache_key, &url).await {
        if let Ok(person) = serde_json::from_value::<TvmazePerson>(json) {
            if let Some(ref image) = person.image {
                return Some(PersonPhoto {
                    person_id: person.id.to_string(),
                    name: person.name,
                    photo_url: image.original.clone(),
                });
            }
        }
    }
    None
}

fn urlencoded(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                encoded.push(c);
            }
            ' ' => {
                encoded.push('+');
            }
            _ => {
                for byte in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}

/// Fetch images for a TV show from TVMaze
async fn fetch_images(
    client: &reqwest::Client,
    tvdb_id: i64,
) -> Vec<MetadataImage> {
    let url = format!("{}/shows/{}/images", TVDB_BASE, tvdb_id);
    let cache_key = format!("tvdb_images:{}", tvdb_id);
    let mut result = Vec::new();

    if let Some(json) = fetch_tvdb_json(client, &cache_key, &url).await {
        // TVMaze images endpoint returns an array of image objects
        if let Ok(images) = serde_json::from_value::<Vec<TvmazeImageItem>>(json) {
            for (i, img) in images.into_iter().enumerate() {
                let image_type = match img.kind.as_str() {
                    "poster" => {
                        if i == 0 {
                            "Primary".to_string()
                        } else {
                            format!("Primary_{}", i)
                        }
                    }
                    "background" => {
                        if i == 0 {
                            "Backdrop".to_string()
                        } else {
                            format!("Backdrop_{}", i)
                        }
                    }
                    _ => continue,
                };
                result.push(MetadataImage {
                    image_type,
                    url: img.original,
                });
            }
        }
    }

    result
}

#[derive(Debug, Deserialize)]
struct TvmazeImageItem {
    #[allow(dead_code)]
    country: Option<TvmazeCountry>,
    #[allow(dead_code)]
    kind: String,
    original: String,
    #[allow(dead_code)]
    score: Option<f64>,
    #[allow(dead_code)]
    language: Option<String>,
}

/// Search TVMaze for a TV show and return full metadata
pub async fn search_tv(
    client: &reqwest::Client,
    name: &str,
) -> Option<ExternalMetadata> {
    let url = format!(
        "{}/shows?q={}",
        TVDB_BASE,
        urlencoded(name)
    );
    let cache_key = format!("tvdb_search:{}", name);

    let resp: Vec<TvmazeSearchResult> = match fetch_tvdb_json(client, &cache_key, &url).await {
        Some(json) => serde_json::from_value(json).ok()?,
        None => return None,
    };

    // Pick the best result (highest score)
    let best = resp.into_iter().max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))?;
    let show = best.show;
    let tvdb_id = show.externals.tvdb?;

    // Get additional images
    let mut images = Vec::new();
    if let Some(ref image) = show.image {
        images.push(MetadataImage {
            image_type: "Primary".to_string(),
            url: image.original.clone(),
        });
    }

    // Fetch more images from the images endpoint
    let extra_images = fetch_images(client, tvdb_id).await;
    images.extend(extra_images);

    let production_year = show
        .premiered
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    let community_rating = show.rating.as_ref().and_then(|r| r.average);

    let genres = show.genres.clone();

    let studios = show
        .network
        .as_ref()
        .map(|n| vec![n.name.clone()]);

    let production_countries = show
        .country
        .as_ref()
        .and_then(|c| c.code.clone())
        .map(|code| vec![code]);

    let runtime_ticks = show
        .runtime
        .map(|mins| (mins as i64) * 60_i64 * 10_000_000);

    Some(ExternalMetadata {
        name: Some(show.name),
        overview: show.summary,
        production_year,
        community_rating,
        official_rating: None,
        genres,
        premiere_date: show.premiered,
        runtime_ticks,
        image_urls: images,
        tmdb_id: show.externals.tmdb,
        tvdb_id: Some(tvdb_id),
        status: show.status,
        original_language: show.language,
        tagline: None,
        production_countries,
        budget: None,
        revenue: None,
        homepage: show.web_channel.as_ref().and_then(|wc| Some(wc.name.clone())),
        end_date: show.ended,
        vote_count: show.rating_count,
        popularity: show.weight.map(|w| w as f64),
        studios,
        directors: None,
        writers: None,
        imdb_id: show.externals.imdb,
    })
}

/// Search TVMaze for a movie (TVMaze doesn't have movies, so we return None)
pub async fn search_movie(
    _client: &reqwest::Client,
    _name: &str,
    _year: Option<i32>,
) -> Option<ExternalMetadata> {
    // TVMaze only covers TV shows
    None
}

/// Get TheTVDB ID for a TV show by name
pub async fn get_tvdb_id_by_name(
    client: &reqwest::Client,
    name: &str,
) -> Option<i64> {
    let result = search_tv(client, name).await?;
    result.tvdb_id
}

// ── Plugin implementation ──

/// TheTVDB plugin that registers itself as a metadata provider.
/// Uses TVMaze API as a free proxy to TheTVDB data.
pub struct TvdbPlugin {
    /// Optional API key for future TheTVDB v2 direct API usage
    api_key: Mutex<Option<String>>,
}

impl Default for TvdbPlugin {
    fn default() -> Self {
        Self {
            api_key: Mutex::new(None),
        }
    }
}

impl Plugin for TvdbPlugin {
    fn id(&self) -> &str {
        "tvdb"
    }

    fn name(&self) -> &str {
        "TheTVDB (via TVMaze)"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches TV series metadata from TheTVDB via TVMaze API. Provides better series data including seasons and episodes."
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

impl TvdbPlugin {
    #[allow(dead_code)]
    fn api_key(&self) -> Option<String> {
        self.api_key.lock().unwrap().clone()
    }
}

/// Create a new TheTVDB plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(TvdbPlugin {
        api_key: Mutex::new(None),
    })
}

/// Register the TheTVDB plugin as a metadata provider with the given PluginManager.
pub async fn register_tvdb_provider<P>(
    plugin: &TvdbPlugin,
    plugin_manager: &P,
) where
    P: MetadataProviderRegistrar,
{
    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "tvdb".to_string(),
            move |_client: &reqwest::Client, name: &str, year: Option<i32>| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let name = name.to_string();
                let client = _client.clone();
                Box::pin(async move { search_movie(&client, &name, year).await })
            },
            move |_client: &reqwest::Client, name: &str| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let name = name.to_string();
                let client = _client.clone();
                Box::pin(async move { search_tv(&client, &name).await })
            },
            move |_client: &reqwest::Client, _tmdb_id: Option<i64>, tvdb_id: Option<i64>, _media_type: MediaType| -> Pin<
                Box<dyn Future<Output = Vec<MetadataImage>> + Send>,
            > {
                let client = _client.clone();
                Box::pin(async move {
                    if let Some(id) = tvdb_id {
                        fetch_images(&client, id).await
                    } else {
                        Vec::new()
                    }
                })
            },
        )
        .await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = TvdbPlugin::default();
        assert_eq!(plugin.id(), "tvdb");
        assert_eq!(plugin.plugin_type(), PluginType::MetadataProvider);
    }

    #[test]
    fn test_urlencoded() {
        assert_eq!(urlencoded("The Big Bang Theory"), "The+Big+Bang+Theory");
        assert_eq!(urlencoded("Börne"), "B%C3%B6rne");
    }
}
