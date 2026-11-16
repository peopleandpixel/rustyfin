//! OMDb metadata provider plugin for Rustyfin.
//!
//! Fetches metadata from The Open Movie Database.

use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{
    ExternalMetadata, ImdbLookups, LookupFailed, LookupResult, MetadataImage, SeasonMetadata,
};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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
pub async fn search(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
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
        sort_title: None,
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
        people: None,
    })
}

// ── Lookups by IMDb id ──

/// OMDb's "no value".
fn available(value: Option<String>) -> Option<String> {
    value.filter(|v| {
        let v = v.trim();
        !v.is_empty() && v != "N/A"
    })
}

/// An OMDb title or episode as Jellyfin's OMDb provider reads it: the name, the year, the US
/// rating (`Rated`), the genres, the plot and the IMDb rating. "N/A" is no value.
fn omdb_meta(value: &serde_json::Value) -> ExternalMetadata {
    let text = |key: &str| available(value.get(key).and_then(|v| v.as_str()).map(String::from));
    ExternalMetadata {
        name: text("Title"),
        production_year: text("Year").and_then(|y| y.get(..4).and_then(|y| y.parse().ok())),
        official_rating: text("Rated"),
        genres: text("Genre").map(|g| {
            g.split(',')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(String::from)
                .collect()
        }),
        overview: text("Plot"),
        community_rating: text("imdbRating").and_then(|r| r.parse().ok()),
        imdb_id: text("imdbID"),
        ..Default::default()
    }
}

/// OMDb's lookups by IMDb id, with the plugin's current key. Every final answer (a title, or
/// OMDb's "not found") is kept for the life of the process, so OMDb is asked about each title and
/// season once; a failure is not kept.
pub struct OmdbLookups {
    http: reqwest::Client,
    base_url: String,
    key: Arc<Mutex<Option<String>>>,
    /// By the query without the key
    answers: moka::future::Cache<String, Option<serde_json::Value>>,
}

impl OmdbLookups {
    /// Lookups against `base_url` (OMDb's, or a stub's), with the key `key` holds at each call.
    pub fn new(http: reqwest::Client, base_url: &str, key: Arc<Mutex<Option<String>>>) -> Self {
        OmdbLookups {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
            answers: moka::future::Cache::builder().max_capacity(10_000).build(),
        }
    }

    /// `GET {base}/?{query}&apikey=…`: the answer, `None` for OMDb's "Response": "False". One
    /// request for concurrent callers of the same query.
    async fn get(&self, query: String) -> LookupResult<serde_json::Value> {
        let key = usable_key(&self.key).ok_or(LookupFailed::Unauthorized)?;
        let url = format!("{}/?{query}&apikey={}", self.base_url, urlencoded(&key));
        let http = self.http.clone();
        self.answers
            .try_get_with(query, async move {
                let resp = http.get(&url).send().await.map_err(|e| {
                    tracing::warn!("OMDb lookup failed: {}", e.without_url());
                    LookupFailed::Transient
                })?;
                match resp.status().as_u16() {
                    200 => {}
                    401 | 403 => return Err(LookupFailed::Unauthorized),
                    status => {
                        tracing::warn!("OMDb lookup failed: status {status}");
                        return Err(LookupFailed::Transient);
                    }
                }
                let value: serde_json::Value =
                    resp.json().await.map_err(|_| LookupFailed::Transient)?;
                Ok(value
                    .get("Response")
                    .and_then(|r| r.as_str())
                    .filter(|r| r.eq_ignore_ascii_case("true"))
                    .map(|_| value.clone()))
            })
            .await
            .map_err(|e: Arc<LookupFailed>| *e)
    }
}

type Lookup<'a, T> = Pin<Box<dyn Future<Output = LookupResult<T>> + Send + 'a>>;

impl ImdbLookups for OmdbLookups {
    fn by_imdb_id(&self, imdb_id: &str) -> Lookup<'_, ExternalMetadata> {
        let query = format!("i={}&plot=short&r=json", urlencoded(imdb_id));
        Box::pin(async move { Ok(self.get(query).await?.as_ref().map(omdb_meta)) })
    }

    fn season(&self, series_imdb_id: &str, season_number: i32) -> Lookup<'_, SeasonMetadata> {
        let query = format!(
            "i={}&season={season_number}&detail=full",
            urlencoded(series_imdb_id)
        );
        Box::pin(async move {
            let Some(season) = self.get(query).await? else {
                return Ok(None);
            };
            let episodes = season
                .get("Episodes")
                .and_then(|e| e.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|e| {
                            let number = e.get("Episode")?.as_str()?.trim().parse().ok()?;
                            Some((number, omdb_meta(e)))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Some(SeasonMetadata {
                meta: ExternalMetadata::default(),
                episodes,
            }))
        })
    }
}

// ── Plugin implementation ──

/// OMDb plugin that registers itself as a metadata provider.
pub struct OmdbPlugin {
    api_key: Arc<Mutex<Option<String>>>,
}

impl Default for OmdbPlugin {
    fn default() -> Self {
        Self {
            api_key: Arc::new(Mutex::new(None)),
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

/// Create a new OMDb plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
///
/// # Safety
///
/// The caller takes ownership of the returned `Box<dyn Plugin>` and must free it exactly once.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin: Box<dyn Plugin> = Box::new(OmdbPlugin {
        api_key: Arc::new(Mutex::new(None)),
    });
    Box::into_raw(Box::new(plugin)).cast()
}

/// The key to call OMDb with: the current key, or `None` when it is missing or blank.
fn usable_key(key: &Mutex<Option<String>>) -> Option<String> {
    key.lock().unwrap().clone().filter(|k| !k.trim().is_empty())
}

/// Register the OMDb plugin as a metadata provider with the given PluginManager.
pub async fn register_omdb_provider<P>(
    plugin: &OmdbPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    // Always registers; each call reads the plugin's current key and makes no request without one.
    let key1 = plugin.api_key.clone();
    let key2 = plugin.api_key.clone();
    let lookups = Arc::new(OmdbLookups::new(
        reqwest::Client::new(),
        OMDB_BASE,
        plugin.api_key.clone(),
    ));

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "omdb".to_string(),
            move |client: &reqwest::Client,
                  name: &str,
                  year: Option<i32>|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let key = key1.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move {
                    let api_key = usable_key(&key)?;
                    search(&client, &api_key, &name, year).await
                })
            },
            move |client: &reqwest::Client,
                  name: &str|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let key = key2.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move {
                    let api_key = usable_key(&key)?;
                    search(&client, &api_key, &name, None).await
                })
            },
            move |_client: &reqwest::Client,
                  _tmdb_id: Option<i64>,
                  _tvdb_id: Option<i64>,
                  _media_type: MediaType|
                  -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> {
                Box::pin(async move { Vec::new() })
            },
        )
        .await;
    plugin_manager
        .register_imdb_lookups("omdb".to_string(), lookups)
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

    /// Attaches OMDb's lookups by IMDb id to the provider registered as `provider_name`.
    async fn register_imdb_lookups(&self, _provider_name: String, _lookups: Arc<dyn ImdbLookups>) {}
}
