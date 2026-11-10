//! Fanart.tv images provider plugin for Rustyfin.
//!
//! Fetches high-quality images (logos, backdrops, posters, etc.) from fanart.tv.

use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

const FANART_BASE: &str = "https://webservice.fanart.tv/v3";

#[derive(Debug, Deserialize)]
struct FanartImage {
    #[allow(dead_code)]
    id: String,
    url: String,
    #[allow(dead_code)]
    lang: Option<String>,
    #[allow(dead_code)]
    likes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FanartMovie {
    hdmovielogo: Option<Vec<FanartImage>>,
    movieposter: Option<Vec<FanartImage>>,
    moviebackground: Option<Vec<FanartImage>>,
    movie4kbackground: Option<Vec<FanartImage>>,
    hdmovieclearart: Option<Vec<FanartImage>>,
    moviebanner: Option<Vec<FanartImage>>,
    moviethumb: Option<Vec<FanartImage>>,
    moviedisc: Option<Vec<FanartImage>>,
    movielogo: Option<Vec<FanartImage>>,
    movieart: Option<Vec<FanartImage>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FanartShow {
    hdtvlogo: Option<Vec<FanartImage>>,
    clearlogo: Option<Vec<FanartImage>>,
    hdclearart: Option<Vec<FanartImage>>,
    showbackground: Option<Vec<FanartImage>>,
    show4kbackground: Option<Vec<FanartImage>>,
    tvthumb: Option<Vec<FanartImage>>,
    seasonposter: Option<Vec<FanartImage>>,
    tvbanner: Option<Vec<FanartImage>>,
    characterart: Option<Vec<FanartImage>>,
}

fn image_type_str(base: &str, index: usize) -> String {
    if index == 0 {
        base.to_string()
    } else {
        format!("{}_{}", base, index)
    }
}

/// Fetch fanart.tv images for a movie by TMDB ID
pub async fn get_movie_images(
    client: &reqwest::Client,
    api_key: &str,
    tmdb_movie_id: i64,
) -> Vec<MetadataImage> {
    let url = format!(
        "{}/movie/{}?api_key={}",
        FANART_BASE, tmdb_movie_id, api_key
    );

    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(movie) = resp.json::<FanartMovie>().await
    {
        if let Some(posters) = movie.movieposter {
            for (i, p) in posters.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Primary", i),
                    url: p.url,
                });
            }
        }

        let backdrops: Vec<_> = movie
            .movie4kbackground
            .unwrap_or_default()
            .into_iter()
            .chain(movie.moviebackground.into_iter().flatten())
            .collect();
        for (i, b) in backdrops.into_iter().enumerate() {
            result.push(MetadataImage {
                image_type: image_type_str("Backdrop", i),
                url: b.url,
            });
        }

        if let Some(logos) = movie.hdmovielogo.or(movie.movielogo)
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: l.url,
            });
        }

        if let Some(arts) = movie.hdmovieclearart
            && let Some(a) = arts.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Art".to_string(),
                url: a.url,
            });
        }

        if let Some(banners) = movie.moviebanner
            && let Some(b) = banners.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Banner".to_string(),
                url: b.url,
            });
        }

        if let Some(discs) = movie.moviedisc
            && let Some(d) = discs.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Disc".to_string(),
                url: d.url,
            });
        }

        if let Some(thumbs) = movie.moviethumb {
            for (i, t) in thumbs.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Thumb", i),
                    url: t.url,
                });
            }
        }
    }

    result
}

/// Fetch fanart.tv images for a TV show by TheTVDB ID
pub async fn get_tv_show_images(
    client: &reqwest::Client,
    api_key: &str,
    tvdb_id: i64,
) -> Vec<MetadataImage> {
    let url = format!("{}/tv/{}?api_key={}", FANART_BASE, tvdb_id, api_key);

    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(show) = resp.json::<FanartShow>().await
    {
        if let Some(posters) = show.seasonposter {
            for (i, p) in posters.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Primary", i),
                    url: p.url,
                });
            }
        }

        let backdrops: Vec<_> = show
            .show4kbackground
            .unwrap_or_default()
            .into_iter()
            .chain(show.showbackground.into_iter().flatten())
            .collect();
        for (i, b) in backdrops.into_iter().enumerate() {
            result.push(MetadataImage {
                image_type: image_type_str("Backdrop", i),
                url: b.url,
            });
        }

        if let Some(logos) = show.hdtvlogo.or(show.clearlogo)
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: l.url,
            });
        }

        if let Some(arts) = show.hdclearart
            && let Some(a) = arts.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Art".to_string(),
                url: a.url,
            });
        }

        if let Some(banners) = show.tvbanner
            && let Some(b) = banners.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Banner".to_string(),
                url: b.url,
            });
        }

        if let Some(thumbs) = show.tvthumb {
            for (i, t) in thumbs.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Thumb", i),
                    url: t.url,
                });
            }
        }

        if let Some(chars) = show.characterart {
            for (i, c) in chars.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Screenshot", i),
                    url: c.url,
                });
            }
        }
    }

    result
}

// ── Plugin implementation ──

/// Fanart.tv plugin that registers itself as a metadata provider (images only).
pub struct FanartPlugin {
    api_key: Mutex<Option<String>>,
}

impl Default for FanartPlugin {
    fn default() -> Self {
        Self {
            api_key: Mutex::new(None),
        }
    }
}

impl Plugin for FanartPlugin {
    fn id(&self) -> &str {
        "fanart"
    }

    fn name(&self) -> &str {
        "Fanart.tv"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches high-quality images (logos, backdrops, posters) from fanart.tv."
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

impl FanartPlugin {
    fn api_key(&self) -> Option<String> {
        self.api_key.lock().unwrap().clone()
    }
}

/// Create a new Fanart plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(FanartPlugin {
        api_key: Mutex::new(None),
    })
}

/// Register the Fanart plugin as a metadata provider with the given PluginManager.
///
/// Fanart.tv only provides images, not metadata search, so search_movie/search_tv
/// return None, and get_images fetches by TMDB/TVDB ID.
pub async fn register_fanart_provider<P>(
    plugin: &FanartPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let api_key = plugin.api_key().ok_or("Fanart.tv API key not configured")?;

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "fanart".to_string(),
            move |_client: &reqwest::Client,
                  _name: &str,
                  _year: Option<i32>|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                Box::pin(async move { None })
            },
            move |_client: &reqwest::Client,
                  _name: &str|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                Box::pin(async move { None })
            },
            move |client: &reqwest::Client,
                  tmdb_id: Option<i64>,
                  tvdb_id: Option<i64>,
                  media_type: MediaType|
                  -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> {
                let api_key = api_key.clone();
                let client = client.clone();
                Box::pin(async move {
                    match media_type {
                        MediaType::Movie => {
                            if let Some(id) = tmdb_id {
                                get_movie_images(&client, &api_key, id).await
                            } else {
                                Vec::new()
                            }
                        }
                        MediaType::Series | MediaType::Episode => {
                            if let Some(id) = tvdb_id {
                                get_tv_show_images(&client, &api_key, id).await
                            } else {
                                Vec::new()
                            }
                        }
                        _ => Vec::new(),
                    }
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
