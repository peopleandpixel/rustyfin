//! Local NFO metadata provider plugin for Rustyfin.
//!
//! Parses local .nfo files to extract metadata for movies and TV shows.
//! This is the same NFO parsing logic that was previously in the core,
//! now exposed as a configurable plugin.

use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;

/// Configuration for the Local NFO plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalNfoConfig {
    /// NFO file names to search for (in order of priority).
    #[serde(default = "default_nfo_names")]
    pub nfo_file_names: Vec<String>,
    /// Whether to parse poster URLs from NFO files.
    #[serde(default = "default_true")]
    pub parse_poster_urls: bool,
    /// Whether to parse fanart URLs from NFO files.
    #[serde(default = "default_true")]
    pub parse_fanart_urls: bool,
}

fn default_nfo_names() -> Vec<String> {
    vec![
        "movie.nfo".to_string(),
        "tvshow.nfo".to_string(),
    ]
}

fn default_true() -> bool {
    true
}

impl Default for LocalNfoConfig {
    fn default() -> Self {
        Self {
            nfo_file_names: default_nfo_names(),
            parse_poster_urls: true,
            parse_fanart_urls: true,
        }
    }
}

/// Local NFO metadata provider plugin.
pub struct LocalNfoPlugin {
    config: Mutex<LocalNfoConfig>,
}

impl Default for LocalNfoPlugin {
    fn default() -> Self {
        Self {
            config: Mutex::new(LocalNfoConfig::default()),
        }
    }
}

impl Plugin for LocalNfoPlugin {
    fn id(&self) -> &str {
        "local-nfo"
    }

    fn name(&self) -> &str {
        "Local NFO Parser"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Parses local .nfo files to extract metadata. Configurable file names and parsing options. Works offline without any API keys."
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::MetadataProvider
    }

    fn set_configuration(&self, config: serde_json::Value) -> Result<(), String> {
        let plugin_config: LocalNfoConfig =
            serde_json::from_value(config).map_err(|e| format!("Invalid NFO config: {}", e))?;
        *self.config.lock().unwrap() = plugin_config;
        Ok(())
    }

    fn get_configuration(&self) -> Option<serde_json::Value> {
        let config = self.config.lock().unwrap().clone();
        Some(serde_json::to_value(config).unwrap())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl LocalNfoPlugin {
    fn config(&self) -> LocalNfoConfig {
        self.config.lock().unwrap().clone()
    }
}

/// Find an NFO file in a directory.
fn find_nfo_file(directory: &Path, nfo_names: &[String]) -> Option<PathBuf> {
    for name in nfo_names {
        let path = directory.join(name);
        if path.exists() {
            return Some(path);
        }
        // Try with the directory's basename
        if let Some(base) = directory.file_name() {
            let stem = base.to_string_lossy();
            let nfo_path = directory.join(format!("{}.nfo", stem));
            if nfo_path.exists() {
                return Some(nfo_path);
            }
        }
    }
    None
}

/// Parse an NFO file and extract metadata.
pub async fn parse_nfo_file(nfo_path: &Path) -> Option<ExternalMetadata> {
    let content = tokio::fs::read_to_string(nfo_path)
        .await
        .ok()?;

    parse_nfo_content(&content)
}

/// Parse NFO content string and extract metadata using quick-xml.
fn parse_nfo_content(content: &str) -> Option<ExternalMetadata> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(content);
    
    let mut nfo = NfoFile::default();
    let mut in_tag = String::new();
    let mut in_ids = false;
    let mut in_genre = false;
    let mut in_studio = false;
    let mut in_country = false;

    loop {
        let event = match reader.read_event() {
            Ok(e) => e,
            Err(_) => return None,
        };
        
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = match std::str::from_utf8(e.name().as_ref()) {
                    Ok(s) => s.to_string(),
                    Err(_) => return None,
                };
                in_tag = name.clone();
                
                if name == "id" {
                    in_ids = true;
                } else if name == "genre" {
                    in_genre = true;
                } else if name == "studio" {
                    in_studio = true;
                } else if name == "country" {
                    in_country = true;
                }
            }
            Event::Text(ref e) => {
                let text = match e.unescape() {
                    Ok(t) => t.to_string(),
                    Err(_) => return None,
                };
                if text.trim().is_empty() {
                    continue;
                }
                
                if in_ids {
                    match in_tag.as_str() {
                        "imdb" => nfo.id.imdb = Some(text),
                        "tmdb" => nfo.id.tmdb = text.parse().ok(),
                        "tvdb" => nfo.id.tvdb = text.parse().ok(),
                        "original_language" => nfo.id.original_language = Some(text),
                        _ => {}
                    }
                } else if in_genre {
                    nfo.genre.push(text);
                } else if in_studio {
                    nfo.studio.push(text);
                } else if in_country {
                    if nfo.country.is_none() {
                        nfo.country = Some(Vec::new());
                    }
                    nfo.country.as_mut().unwrap().push(text);
                } else {
                    match in_tag.as_str() {
                        "title" => nfo.title = Some(text),
                        "originaltitle" => nfo.original_title = Some(text),
                        "plot" => nfo.plot = Some(text),
                        "year" => nfo.year = Some(text),
                        "rating" => nfo.rating = Some(text),
                        "runtime" => nfo.runtime = Some(text),
                        "mpaa" => nfo.mpaa = Some(text),
                        "tagline" => nfo.tagline = Some(text),
                        "released" => nfo.released = Some(text),
                        "poster" => nfo.poster = text,
                        "fanart" => nfo.fanart = text,
                        "thumb" => nfo.thumb = text,
                        _ => {}
                    }
                }
            }
            Event::End(ref e) => {
                let name = match std::str::from_utf8(e.name().as_ref()) {
                    Ok(s) => s.to_string(),
                    Err(_) => return None,
                };
                if name == "id" {
                    in_ids = false;
                } else if name == "genre" {
                    in_genre = false;
                } else if name == "studio" {
                    in_studio = false;
                } else if name == "country" {
                    in_country = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let title = nfo.title.or(nfo.original_title);
    let overview = nfo.plot;
    let year = nfo.year.and_then(|y| y.parse::<i32>().ok());
    let rating = nfo.rating.and_then(|r| r.parse::<f64>().ok());
    let runtime_ticks = nfo.runtime.and_then(|r| {
        r.parse::<i64>().ok().map(|mins| mins * 60 * 10_000_000)
    });

    let genres = if nfo.genre.is_empty() {
        None
    } else {
        Some(nfo.genre)
    };

    let studios = if nfo.studio.is_empty() {
        None
    } else {
        Some(nfo.studio)
    };

    let mut image_urls = Vec::new();
    if !nfo.poster.is_empty() {
        image_urls.push(MetadataImage {
            image_type: "Primary".to_string(),
            url: nfo.poster.clone(),
        });
    }
    if !nfo.fanart.is_empty() {
        image_urls.push(MetadataImage {
            image_type: "Backdrop".to_string(),
            url: nfo.fanart.clone(),
        });
    }
    if !nfo.thumb.is_empty() {
        image_urls.push(MetadataImage {
            image_type: "Thumb".to_string(),
            url: nfo.thumb.clone(),
        });
    }

    Some(ExternalMetadata {
        name: title,
        overview,
        production_year: year,
        community_rating: rating,
        official_rating: nfo.mpaa,
        genres,
        premiere_date: nfo.released,
        runtime_ticks,
        image_urls,
        tmdb_id: nfo.id.tmdb,
        tvdb_id: nfo.id.tvdb,
        status: None,
        original_language: nfo.id.original_language,
        tagline: nfo.tagline,
        production_countries: nfo.country,
        budget: None,
        revenue: None,
        homepage: None,
        end_date: None,
        vote_count: None,
        popularity: None,
        studios,
        directors: nfo.director,
        writers: nfo.writer,
        imdb_id: nfo.id.imdb,
    })
}

/// NFO file structure (simplified).
#[derive(Debug, Default)]
struct NfoFile {
    title: Option<String>,
    original_title: Option<String>,
    plot: Option<String>,
    year: Option<String>,
    rating: Option<String>,
    runtime: Option<String>,
    mpaa: Option<String>,
    tagline: Option<String>,
    released: Option<String>,
    genre: Vec<String>,
    studio: Vec<String>,
    director: Option<Vec<String>>,
    writer: Option<Vec<String>>,
    poster: String,
    fanart: String,
    thumb: String,
    country: Option<Vec<String>>,
    id: NfoIds,
}

#[derive(Debug, Default)]
struct NfoIds {
    imdb: Option<String>,
    tmdb: Option<i64>,
    tvdb: Option<i64>,
    original_language: Option<String>,
}

/// Search for metadata by looking for NFO files in the given directory.
/// This is a local-only provider — it doesn't use the HTTP client.
pub async fn search_movie(
    _client: &reqwest::Client,
    config: &LocalNfoConfig,
    media_path: &str,
) -> Option<ExternalMetadata> {
    let directory = Path::new(media_path);
    if let Some(nfo_path) = find_nfo_file(directory, &config.nfo_file_names) {
        tracing::debug!("Found NFO file: {:?}", nfo_path);
        parse_nfo_file(&nfo_path).await
    } else {
        tracing::debug!("No NFO file found in: {:?}", directory);
        None
    }
}

/// Search for TV show metadata by looking for NFO files.
pub async fn search_tv(
    _client: &reqwest::Client,
    config: &LocalNfoConfig,
    media_path: &str,
) -> Option<ExternalMetadata> {
    search_movie(_client, config, media_path).await
}

/// Get images from NFO files (local only).
pub async fn get_images_from_nfo(
    config: &LocalNfoConfig,
    media_path: &str,
) -> Vec<MetadataImage> {
    let directory = Path::new(media_path);
    if let Some(nfo_path) = find_nfo_file(directory, &config.nfo_file_names) {
        if let Some(meta) = parse_nfo_file(&nfo_path).await {
            meta.image_urls
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    }
}

/// FFI entry point for dynamic loading.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(LocalNfoPlugin::default())
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

/// Register the Local NFO plugin as a metadata provider.
pub async fn register_local_nfo_provider<P>(
    plugin: &LocalNfoPlugin,
    plugin_manager: &P,
) where
    P: MetadataProviderRegistrar,
{
    let config = plugin.config();
    let config1 = config.clone();
    let config2 = config.clone();

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "local-nfo".to_string(),
            move |client: &reqwest::Client, name: &str, _year: Option<i32>| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let config = config1.clone();
                let media_path = name.to_string();
                let client = client.clone();
                Box::pin(async move {
                    search_movie(&client, &config, &media_path).await
                })
            },
            move |client: &reqwest::Client, name: &str| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let config = config2.clone();
                let media_path = name.to_string();
                let client = client.clone();
                Box::pin(async move {
                    search_tv(&client, &config, &media_path).await
                })
            },
            move |_client: &reqwest::Client, _tmdb_id: Option<i64>, _tvdb_id: Option<i64>, _media_type: MediaType| -> Pin<
                Box<dyn Future<Output = Vec<MetadataImage>> + Send>,
            > {
                Box::pin(async move {
                    // NFO images require a file path, not TMDB/TVDB IDs.
                    // This closure can't be used for image lookup by ID.
                    Vec::new()
                })
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_metadata() {
        let plugin = LocalNfoPlugin::default();
        assert_eq!(plugin.id(), "local-nfo");
        assert_eq!(plugin.plugin_type(), PluginType::MetadataProvider);
    }

    #[test]
    fn test_config_defaults() {
        let config = LocalNfoConfig::default();
        assert!(config.nfo_file_names.contains(&"movie.nfo".to_string()));
        assert!(config.nfo_file_names.contains(&"tvshow.nfo".to_string()));
        assert!(config.parse_poster_urls);
        assert!(config.parse_fanart_urls);
    }

    #[test]
    fn test_parse_simple_nfo() {
        let nfo_content = r#"<?xml version="1.0"?>
<movie>
    <title>Test Movie</title>
    <plot>A test plot.</plot>
    <year>2024</year>
    <rating>8.5</rating>
    <runtime>120</runtime>
    <genre>Action</genre>
    <poster>https://example.com/poster.jpg</poster>
</movie>"#;

        let meta = parse_nfo_content(nfo_content);
        assert!(meta.is_some());
        let meta = meta.unwrap();
        assert_eq!(meta.name, Some("Test Movie".to_string()));
        assert_eq!(meta.production_year, Some(2024));
        assert_eq!(meta.community_rating, Some(8.5));
    }
}
