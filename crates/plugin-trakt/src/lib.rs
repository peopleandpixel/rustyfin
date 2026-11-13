//! Trakt.tv metadata provider plugin for Rustyfin.
//!
//! Fetches metadata, images, and ratings from Trakt.tv API.
//! Good for watchlists, ratings, and social features.

use moka::sync::Cache;
use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::sync::Mutex;

const TRAKT_BASE: &str = "https://api.trakt.tv";
#[allow(dead_code)]
const TRAKT_IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

/// L1 in-memory cache for Trakt API responses.
static TRAKT_CACHE: LazyLock<Cache<String, String>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(10_000)
        .time_to_live(std::time::Duration::from_secs(24 * 3600))
        .build()
});

/// Fetch JSON from Trakt with caching.
async fn fetch_trakt_json(
    client: &reqwest::Client,
    cache_key: &str,
    url: &str,
    api_key: &str,
) -> Option<serde_json::Value> {
    if let Some(cached) = TRAKT_CACHE.get(cache_key) {
        return serde_json::from_str(&cached).ok();
    }
    let resp = client
        .get(url)
        .header("trakt-api-key", api_key)
        .header("trakt-api-version", "2")
        .send()
        .await
        .ok()?;
    let text = resp.text().await.ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    TRAKT_CACHE.insert(cache_key.to_string(), text.clone());
    Some(json)
}

// ── Search types ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktSearchResult {
    #[serde(rename = "type")]
    result_type: String,
    movie: Option<TraktSearchMovie>,
    show: Option<TraktSearchShow>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktSearchMovie {
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    ids: TraktIds,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktSearchShow {
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    ids: TraktIds,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct TraktIds {
    trakt: i64,
    slug: String,
    #[serde(rename = "tmdb")]
    tmdb_id: Option<i64>,
    #[serde(rename = "tvdb")]
    tvdb_id: Option<i64>,
    #[serde(rename = "imdb")]
    imdb_id: Option<String>,
}

// ── Movie details ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktMovieDetails {
    title: String,
    year: Option<i32>,
    tagline: Option<String>,
    overview: Option<String>,
    #[serde(rename = "runtime")]
    runtime_minutes: Option<i32>,
    certificates: Option<TraktCertificates>,
    #[serde(rename = "released")]
    released: Option<String>,
    #[serde(rename = "dates")]
    dates: Option<TraktDates>,
    trailer: Option<String>,
    status: Option<String>,
    rating: Option<f64>,
    votes: Option<i32>,
    popularity: Option<f64>,
    genre: Option<Vec<String>>,
    countries: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    ids: TraktIds,
    available_translations: Option<Vec<String>>,
    produced_by: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TraktCertificates {
    us: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktDates {
    desktop: Option<String>,
}

// ── Show details ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktShowDetails {
    title: String,
    year: Option<i32>,
    tagline: Option<String>,
    overview: Option<String>,
    #[serde(rename = "runtime")]
    runtime_minutes: Option<i32>,
    certificates: Option<TraktCertificates>,
    #[serde(rename = "aired_episodes")]
    aired_episodes: Option<i32>,
    #[serde(rename = "number_of_seasons")]
    number_of_seasons: Option<i32>,
    first_aired: Option<String>,
    trailer: Option<String>,
    status: Option<String>,
    rating: Option<f64>,
    votes: Option<i32>,
    popularity: Option<f64>,
    genre: Option<Vec<String>>,
    countries: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    network: Option<String>,
    ids: TraktIds,
}

// ── Images ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktImages {
    fanart: Option<Vec<String>>,
    logo: Option<String>,
    clearlogo: Option<String>,
    banner: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktMovieWithImages {
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    ids: TraktIds,
    images: Option<TraktImages>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TraktShowWithImages {
    title: String,
    year: Option<i32>,
    overview: Option<String>,
    ids: TraktIds,
    images: Option<TraktImages>,
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

/// Search Trakt for a movie and return full metadata
pub async fn search_movie(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
    let query = urlencoded(name);
    let url = format!("{}/search/movie?query={}", TRAKT_BASE, query);
    let cache_key = format!("trakt_search_movie:{}:{}", name, year.unwrap_or(0));

    let results: Vec<TraktSearchResult> =
        match fetch_trakt_json(client, &cache_key, &url, api_key).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    // Find the best match (prefer year match)
    let search_result = if let Some(y) = year {
        results.into_iter().find(|r| {
            if let Some(ref movie) = r.movie {
                movie.year == Some(y)
            } else {
                false
            }
        })
    } else {
        results.into_iter().next()
    }?;

    let movie = search_result.movie?;
    let trakt_id = movie.ids.trakt;

    // Fetch full details
    let details_url = format!(
        "{}/movies/{}/extensions?extended=full,images",
        TRAKT_BASE, trakt_id
    );
    let cache_key_details = format!("trakt_movie_details:{}", trakt_id);

    let details: TraktMovieDetails =
        match fetch_trakt_json(client, &cache_key_details, &details_url, api_key).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    // Fetch images
    let images_url = format!("{}/movies/{}/images?extended=images", TRAKT_BASE, trakt_id);
    let cache_key_images = format!("trakt_movie_images:{}", trakt_id);
    let images = fetch_images_for_movie(client, api_key, &cache_key_images, &images_url).await;

    let production_year = details.year;
    let runtime_ticks = details
        .runtime_minutes
        .map(|mins| (mins as i64) * 60_i64 * 10_000_000);

    Some(ExternalMetadata {
        name: Some(details.title),
        overview: details.overview,
        production_year,
        community_rating: details.rating,
        official_rating: details.certificates.and_then(|c| c.us),
        genres: details.genre,
        premiere_date: details.released,
        runtime_ticks,
        image_urls: images,
        tmdb_id: details.ids.tmdb_id,
        tvdb_id: None,
        status: details.status,
        original_language: details.languages.and_then(|l| l.first().cloned()),
        tagline: details.tagline,
        production_countries: details.countries,
        budget: None,
        revenue: None,
        homepage: None,
        end_date: None,
        vote_count: details.votes,
        popularity: details.popularity,
        studios: details.produced_by,
        directors: None,
        writers: None,
        imdb_id: details.ids.imdb_id,
    })
}

/// Search Trakt for a TV show and return full metadata
pub async fn search_tv(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
) -> Option<ExternalMetadata> {
    let query = urlencoded(name);
    let url = format!("{}/search/show?query={}", TRAKT_BASE, query);
    let cache_key = format!("trakt_search_tv:{}", name);

    let results: Vec<TraktSearchResult> =
        match fetch_trakt_json(client, &cache_key, &url, api_key).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    let search_result = results.into_iter().next()?;
    let show = search_result.show?;
    let trakt_id = show.ids.trakt;

    // Fetch full details
    let details_url = format!(
        "{}/shows/{}/extensions?extended=full,images",
        TRAKT_BASE, trakt_id
    );
    let cache_key_details = format!("trakt_show_details:{}", trakt_id);

    let details: TraktShowDetails =
        match fetch_trakt_json(client, &cache_key_details, &details_url, api_key).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    // Fetch images
    let images_url = format!("{}/shows/{}/images?extended=images", TRAKT_BASE, trakt_id);
    let cache_key_images = format!("trakt_show_images:{}", trakt_id);
    let images = fetch_images_for_show(client, api_key, &cache_key_images, &images_url).await;

    let production_year = details.year;
    let runtime_ticks = details
        .runtime_minutes
        .map(|mins| (mins as i64) * 60_i64 * 10_000_000);

    Some(ExternalMetadata {
        name: Some(details.title),
        overview: details.overview,
        production_year,
        community_rating: details.rating,
        official_rating: details.certificates.and_then(|c| c.us),
        genres: details.genre,
        premiere_date: details.first_aired,
        runtime_ticks,
        image_urls: images,
        tmdb_id: details.ids.tmdb_id,
        tvdb_id: details.ids.tvdb_id,
        status: details.status,
        original_language: details.languages.and_then(|l| l.first().cloned()),
        tagline: details.tagline,
        production_countries: details.countries,
        budget: None,
        revenue: None,
        homepage: None,
        end_date: None,
        vote_count: details.votes,
        popularity: details.popularity,
        studios: None,
        directors: None,
        writers: None,
        imdb_id: details.ids.imdb_id,
    })
}

/// Fetch images for a movie from Trakt
async fn fetch_images_for_movie(
    client: &reqwest::Client,
    api_key: &str,
    cache_key: &str,
    url: &str,
) -> Vec<MetadataImage> {
    let mut result = Vec::new();

    let movie: TraktMovieWithImages = match fetch_trakt_json(client, cache_key, url, api_key).await
    {
        Some(json) => match serde_json::from_value(json) {
            Ok(m) => m,
            Err(_) => return result,
        },
        None => return result,
    };

    if let Some(images) = movie.images {
        // Fanart as backdrop
        if let Some(fanarts) = images.fanart {
            for (i, fanart) in fanarts.into_iter().take(3).enumerate() {
                result.push(MetadataImage {
                    image_type: if i == 0 {
                        "Backdrop".to_string()
                    } else {
                        format!("Backdrop_{}", i)
                    },
                    url: fanart,
                });
            }
        }
        // Logo
        if let Some(logo) = images.logo {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: logo,
            });
        }
        // Clear logo as secondary
        if let Some(clearlogo) = images.clearlogo {
            result.push(MetadataImage {
                image_type: "ClearLogo".to_string(),
                url: clearlogo,
            });
        }
    }

    result
}

/// Fetch images for a TV show from Trakt
async fn fetch_images_for_show(
    client: &reqwest::Client,
    api_key: &str,
    cache_key: &str,
    url: &str,
) -> Vec<MetadataImage> {
    let mut result = Vec::new();

    let show: TraktShowWithImages = match fetch_trakt_json(client, cache_key, url, api_key).await {
        Some(json) => match serde_json::from_value(json) {
            Ok(s) => s,
            Err(_) => return result,
        },
        None => return result,
    };

    if let Some(images) = show.images {
        // Fanart as backdrop
        if let Some(fanarts) = images.fanart {
            for (i, fanart) in fanarts.into_iter().take(3).enumerate() {
                result.push(MetadataImage {
                    image_type: if i == 0 {
                        "Backdrop".to_string()
                    } else {
                        format!("Backdrop_{}", i)
                    },
                    url: fanart,
                });
            }
        }
        // Logo
        if let Some(logo) = images.logo {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: logo,
            });
        }
        // Clear logo as secondary
        if let Some(clearlogo) = images.clearlogo {
            result.push(MetadataImage {
                image_type: "ClearLogo".to_string(),
                url: clearlogo,
            });
        }
    }

    result
}

/// Fetch images by Trakt ID (generic for both movies and shows)
pub async fn get_images(
    client: &reqwest::Client,
    api_key: &str,
    trakt_id: i64,
    media_type: MediaType,
) -> Vec<MetadataImage> {
    let (kind, cache_key, url) = match media_type {
        MediaType::Movie => (
            "movie",
            format!("trakt_movie_images:{}", trakt_id),
            format!("{}/movies/{}/images?extended=images", TRAKT_BASE, trakt_id),
        ),
        MediaType::Series | MediaType::Episode => (
            "show",
            format!("trakt_show_images:{}", trakt_id),
            format!("{}/shows/{}/images?extended=images", TRAKT_BASE, trakt_id),
        ),
        _ => return Vec::new(),
    };

    let mut result = Vec::new();
    let images: serde_json::Value = match fetch_trakt_json(client, &cache_key, &url, api_key).await
    {
        Some(json) => json,
        None => return result,
    };

    if let Some(imgs) = images.get("images") {
        if let Some(fanarts) = imgs.get("fanart").and_then(|a| a.as_array()) {
            for (i, fanart) in fanarts.iter().take(3).enumerate() {
                if let Some(url) = fanart.as_str() {
                    result.push(MetadataImage {
                        image_type: if i == 0 {
                            "Backdrop".to_string()
                        } else {
                            format!("Backdrop_{}", i)
                        },
                        url: url.to_string(),
                    });
                }
            }
        }
        if let Some(logo) = imgs.get("logo").and_then(|l| l.as_str()) {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: logo.to_string(),
            });
        }
    }

    tracing::debug!(
        "fetched {} images for {} {} from Trakt",
        result.len(),
        kind,
        trakt_id
    );
    result
}

// ── Plugin implementation ──

/// Trakt.tv plugin that registers itself as a metadata provider.
pub struct TraktPlugin {
    api_key: Mutex<Option<String>>,
}

impl Default for TraktPlugin {
    fn default() -> Self {
        Self {
            api_key: Mutex::new(None),
        }
    }
}

impl Plugin for TraktPlugin {
    fn id(&self) -> &str {
        "trakt"
    }

    fn name(&self) -> &str {
        "Trakt.tv"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches metadata, images, and ratings from Trakt.tv. Good for watchlists, ratings, and social features."
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

impl TraktPlugin {
    fn api_key(&self) -> Option<String> {
        self.api_key.lock().unwrap().clone()
    }
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
    let plugin: Box<dyn Plugin> = Box::new(TraktPlugin {
        api_key: Mutex::new(None),
    });
    Box::into_raw(Box::new(plugin)).cast()
}

/// Register the Trakt plugin as a metadata provider.
pub async fn register_trakt_provider<P>(
    plugin: &TraktPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let api_key = plugin.api_key().ok_or("Trakt API key not configured")?;
    let api_key1 = api_key.clone();
    let api_key2 = api_key.clone();
    let api_key3 = api_key.clone();

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "trakt".to_string(),
            move |client: &reqwest::Client,
                  name: &str,
                  year: Option<i32>|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let api_key = api_key1.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move { search_movie(&client, &api_key, &name, year).await })
            },
            move |client: &reqwest::Client,
                  name: &str|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let api_key = api_key2.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move { search_tv(&client, &api_key, &name).await })
            },
            move |client: &reqwest::Client,
                  tmdb_id: Option<i64>,
                  tvdb_id: Option<i64>,
                  media_type: MediaType|
                  -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> {
                let api_key = api_key3.clone();
                let client = client.clone();
                Box::pin(async move {
                    // Trakt uses its own IDs, so we can't directly look up by TMDB/TVDB ID.
                    // This is a limitation — Trakt images require Trakt IDs.
                    // We return empty for now since we don't have a Trakt ID mapping.
                    let _ = (client, api_key, tmdb_id, tvdb_id, media_type);
                    Vec::new()
                })
            },
        )
        .await;

    Ok(())
}

/// Trait for types that can register metadata providers.
#[allow(async_fn_in_trait)]
pub trait MetadataProviderRegistrar {
    async fn register_metadata_provider<F1, F2, F3>(
        &self,
        plugin_id: String,
        provider_name: String,
        search_movie: F1,
        search_tv: F2,
        get_images: F3,
    ) where
        F1: Fn(
                &reqwest::Client,
                &str,
                Option<i32>,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F2: Fn(
                &reqwest::Client,
                &str,
            ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>>
            + Send
            + Sync
            + 'static,
        F3: Fn(
                &reqwest::Client,
                Option<i64>,
                Option<i64>,
                MediaType,
            ) -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>>
            + Send
            + Sync
            + 'static;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = TraktPlugin::default();
        assert_eq!(plugin.id(), "trakt");
        assert_eq!(plugin.plugin_type(), PluginType::MetadataProvider);
    }

    #[test]
    fn test_urlencoded() {
        assert_eq!(urlencoded("The Dark Knight"), "The+Dark+Knight");
        assert_eq!(
            urlencoded("Türkisch für Anfänger"),
            "T%C3%BCrkisch+f%C3%BCr+Anf%C3%A4nger"
        );
    }
}
