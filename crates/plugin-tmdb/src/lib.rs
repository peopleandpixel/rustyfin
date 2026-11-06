//! TMDB metadata provider plugin for Rustyfin.
//!
//! Fetches metadata, images, trailers, cast/crew, and similar items from The Movie Database.

use moka::sync::Cache;
use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;
use std::pin::Pin;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::future::Future;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

/// L1 in-memory cache for TMDb API responses.
static TMDB_CACHE: LazyLock<Cache<String, String>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(10_000)
        .time_to_live(std::time::Duration::from_secs(24 * 3600))
        .build()
});

/// Fetch JSON from TMDb with caching.
async fn fetch_tmdb_json(
    client: &reqwest::Client,
    cache_key: &str,
    url: &str,
) -> Option<serde_json::Value> {
    if let Some(cached) = TMDB_CACHE.get(cache_key) {
        return serde_json::from_str(&cached).ok();
    }
    let resp = client.get(url).send().await.ok()?;
    let text = resp.text().await.ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    TMDB_CACHE.insert(cache_key.to_string(), text);
    Some(json)
}

// ── Person types ──

#[derive(Debug, Deserialize)]
struct TmdbPersonDetails {
    id: i64,
    name: String,
    profile_path: Option<String>,
    #[allow(dead_code)]
    known_for_department: Option<String>,
    #[allow(dead_code)]
    birthday: Option<String>,
    #[allow(dead_code)]
    deathday: Option<String>,
    #[allow(dead_code)]
    place_of_birth: Option<String>,
    #[allow(dead_code)]
    biography: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbCastMember {
    id: i64,
    name: String,
    #[allow(dead_code)]
    character: Option<String>,
    profile_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbCrewMember {
    id: i64,
    name: String,
    #[allow(dead_code)]
    job: Option<String>,
    department: Option<String>,
    profile_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbDetails {
    credits: Option<TmdbCredits>,
}

#[derive(Debug, Deserialize)]
struct TmdbCredits {
    cast: Option<Vec<TmdbCastMember>>,
    crew: Option<Vec<TmdbCrewMember>>,
}

/// Result of fetching a person's photo from TMDb
#[derive(Debug, Clone)]
pub struct PersonPhoto {
    pub person_id: String,
    pub name: String,
    pub photo_url: String,
}

/// Fetch a person's details from TMDb by TMDb ID, returning photo URL if available
pub async fn get_person_photo(
    client: &reqwest::Client,
    api_key: &str,
    tmdb_person_id: i64,
) -> Option<PersonPhoto> {
    let url = format!(
        "{}/person/{}?api_key={}",
        TMDB_BASE, tmdb_person_id, api_key
    );
    let cache_key = format!("person:{}", tmdb_person_id);

    let details: TmdbPersonDetails = match fetch_tmdb_json(client, &cache_key, &url).await {
        Some(json) => serde_json::from_value(json).ok()?,
        None => return None,
    };

    details.profile_path.map(|path| PersonPhoto {
        person_id: details.id.to_string(),
        name: details.name,
        photo_url: format!("{}{}", TMDB_IMAGE_BASE, path),
    })
}

/// Fetch cast and crew for a movie from TMDb, returning person info with photos
pub async fn get_movie_people(
    client: &reqwest::Client,
    api_key: &str,
    tmdb_movie_id: i64,
) -> Vec<PersonPhoto> {
    let url = format!(
        "{}/movie/{}/credits?api_key={}",
        TMDB_BASE, tmdb_movie_id, api_key
    );
    let cache_key = format!("movie_credits:{}", tmdb_movie_id);

    let mut result = Vec::new();

    let details: TmdbDetails = match fetch_tmdb_json(client, &cache_key, &url).await {
        Some(json) => match serde_json::from_value::<TmdbDetails>(json) {
            Ok(d) => d,
            Err(_) => return result,
        },
        None => return result,
    };

    if let Some(credits) = details.credits {
        if let Some(cast) = credits.cast {
            for member in cast.into_iter().take(20) {
                if let Some(path) = member.profile_path {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: format!("{}{}", TMDB_IMAGE_BASE, path),
                    });
                }
            }
        }
        if let Some(crew) = credits.crew {
            for member in crew {
                if let Some(ref dept) = member.department
                    && dept == "Directing"
                    && let Some(path) = member.profile_path
                {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: format!("{}{}", TMDB_IMAGE_BASE, path),
                    });
                }
            }
        }
    }

    result
}

/// Fetch cast and crew for a TV show season from TMDb, returning person info with photos
pub async fn get_tv_people(
    client: &reqwest::Client,
    api_key: &str,
    tmdb_tv_id: i64,
    season_number: i32,
) -> Vec<PersonPhoto> {
    let url = format!(
        "{}/tv/{}/season/{}/credits?api_key={}",
        TMDB_BASE, tmdb_tv_id, season_number, api_key
    );
    let cache_key = format!("tv_credits:{}:{}", tmdb_tv_id, season_number);

    let mut result = Vec::new();

    let details: TmdbDetails = match fetch_tmdb_json(client, &cache_key, &url).await {
        Some(json) => match serde_json::from_value::<TmdbDetails>(json) {
            Ok(d) => d,
            Err(_) => return result,
        },
        None => return result,
    };

    if let Some(credits) = details.credits {
        if let Some(cast) = credits.cast {
            for member in cast.into_iter().take(20) {
                if let Some(path) = member.profile_path {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: format!("{}{}", TMDB_IMAGE_BASE, path),
                    });
                }
            }
        }
        if let Some(crew) = credits.crew {
            for member in crew {
                if let Some(ref dept) = member.department
                    && dept == "Directing"
                    && let Some(path) = member.profile_path
                {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: format!("{}{}", TMDB_IMAGE_BASE, path),
                    });
                }
            }
        }
    }

    result
}

// ── Search types ──

#[derive(Debug, Deserialize)]
struct TmdbSearchResult {
    results: Vec<TmdbSearchItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbSearchItem {
    id: i64,
    title: Option<String>,
    name: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    vote_average: Option<f64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genre_ids: Option<Vec<i32>>,
}

#[derive(Debug, Deserialize)]
struct TmdbImages {
    posters: Option<Vec<TmdbImage>>,
    backdrops: Option<Vec<TmdbImage>>,
    logos: Option<Vec<TmdbImage>>,
}

#[derive(Debug, Deserialize)]
struct TmdbExternalIds {
    #[serde(rename = "tvdb_id")]
    tvdb_id: Option<i64>,
    #[serde(rename = "imdb_id")]
    imdb_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TmdbProductionCompany {
    name: String,
    #[allow(dead_code)]
    id: i64,
}

#[derive(Debug, Deserialize)]
struct TmdbProductionCountry {
    iso_3166_1: String,
    #[allow(dead_code)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbMovieDetails {
    runtime: Option<i32>,
    status: Option<String>,
    original_language: String,
    tagline: Option<String>,
    budget: Option<i64>,
    revenue: Option<i64>,
    homepage: Option<String>,
    vote_count: Option<i32>,
    popularity: Option<f64>,
    vote_average: Option<f64>,
    overview: Option<String>,
    title: Option<String>,
    release_date: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    production_companies: Option<Vec<TmdbProductionCompany>>,
    production_countries: Option<Vec<TmdbProductionCountry>>,
    #[serde(rename = "spoken_languages")]
    spoken_languages: Option<Vec<TmdbSpokenLanguage>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbTvDetails {
    status: Option<String>,
    original_language: String,
    tagline: Option<String>,
    homepage: Option<String>,
    vote_count: Option<i32>,
    popularity: Option<f64>,
    vote_average: Option<f64>,
    overview: Option<String>,
    name: Option<String>,
    first_air_date: Option<String>,
    last_air_date: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    production_companies: Option<Vec<TmdbProductionCompany>>,
    production_countries: Option<Vec<TmdbProductionCountry>>,
    #[serde(rename = "spoken_languages")]
    spoken_languages: Option<Vec<TmdbSpokenLanguage>>,
    episode_run_time: Option<Vec<i32>>,
    number_of_seasons: Option<i32>,
    number_of_episodes: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct TmdbGenre {
    name: String,
    #[allow(dead_code)]
    id: i32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbSpokenLanguage {
    english_name: Option<String>,
    iso_639_1: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbImage {
    file_path: String,
    width: Option<i32>,
    height: Option<i32>,
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

async fn fetch_images(
    client: &reqwest::Client,
    api_key: &str,
    media_type: &str,
    id: i64,
) -> Vec<MetadataImage> {
    let url = format!(
        "{}/{}/{}/images?api_key={}",
        TMDB_BASE, media_type, id, api_key
    );
    let cache_key = format!("images:{}:{}", media_type, id);
    let mut result = Vec::new();

    if let Some(json) = fetch_tmdb_json(client, &cache_key, &url).await
        && let Ok(imgs) = serde_json::from_value::<TmdbImages>(json)
    {
        if let Some(posters) = imgs.posters {
            for (i, p) in posters.into_iter().take(3).enumerate() {
                result.push(MetadataImage {
                    image_type: if i == 0 {
                        "Primary".to_string()
                    } else {
                        format!("Primary_{}", i)
                    },
                    url: format!("{}{}", TMDB_IMAGE_BASE, p.file_path),
                });
            }
        }
        if let Some(backdrops) = imgs.backdrops {
            for (i, b) in backdrops.into_iter().take(3).enumerate() {
                result.push(MetadataImage {
                    image_type: if i == 0 {
                        "Backdrop".to_string()
                    } else {
                        format!("Backdrop_{}", i)
                    },
                    url: format!("{}{}", TMDB_IMAGE_BASE, b.file_path),
                });
            }
        }
        if let Some(logos) = imgs.logos
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: format!("{}{}", TMDB_IMAGE_BASE, l.file_path),
            });
        }
    }

    result
}

/// Search TMDb for a movie and return full metadata from details endpoint
pub async fn search_movie(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
    let mut url = format!(
        "{}/search/movie?api_key={}&query={}",
        TMDB_BASE,
        api_key,
        urlencoded(name)
    );
    let cache_key_search = format!("search_movie:{}:{}", name, year.unwrap_or(0));
    if let Some(y) = year {
        url.push_str(&format!("&year={}", y));
    }

    let resp: TmdbSearchResult = match fetch_tmdb_json(client, &cache_key_search, &url).await {
        Some(json) => serde_json::from_value(json).ok()?,
        None => return None,
    };
    let search_item = resp.results.into_iter().next()?;
    let tmdb_id = search_item.id;

    let details_url = format!(
        "{}/movie/{}?api_key={}&append_to_response=credits,external_ids",
        TMDB_BASE, tmdb_id, api_key
    );
    let cache_key_details = format!("movie_details:{}", tmdb_id);
    let details: TmdbMovieDetails =
        match fetch_tmdb_json(client, &cache_key_details, &details_url).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    let external_ids: Option<TmdbExternalIds> = {
        let external_ids_url = format!(
            "{}/movie/{}/external_ids?api_key={}",
            TMDB_BASE, tmdb_id, api_key
        );
        let cache_key_ext = format!("movie_external_ids:{}", tmdb_id);
        match fetch_tmdb_json(client, &cache_key_ext, &external_ids_url).await {
            Some(json) => serde_json::from_value(json).ok(),
            None => None,
        }
    };

    let images = fetch_images(client, api_key, "movie", tmdb_id).await;

    let production_year = details
        .release_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    let genres = details
        .genres
        .map(|gs| gs.into_iter().map(|g| g.name).collect());

    let studios = details
        .production_companies
        .as_ref()
        .map(|pcs| pcs.iter().map(|pc| pc.name.clone()).collect());

    let production_countries = details
        .production_countries
        .as_ref()
        .map(|countries| countries.iter().map(|c| c.iso_3166_1.clone()).collect());

    let runtime_ticks = details
        .runtime
        .map(|mins| (mins as i64) * 60_i64 * 10_000_000);

    Some(ExternalMetadata {
        name: details.title,
        overview: details.overview,
        production_year,
        community_rating: details.vote_average,
        official_rating: None,
        genres,
        premiere_date: details.release_date,
        runtime_ticks,
        image_urls: images,
        tmdb_id: Some(tmdb_id),
        tvdb_id: None,
        status: details.status,
        original_language: if details.original_language.is_empty() {
            None
        } else {
            Some(details.original_language)
        },
        tagline: details.tagline,
        production_countries,
        budget: details.budget,
        revenue: details.revenue,
        homepage: details.homepage,
        end_date: None,
        vote_count: details.vote_count,
        popularity: details.popularity,
        studios,
        directors: None,
        writers: None,
        imdb_id: external_ids.and_then(|e| e.imdb_id),
    })
}

/// Search TMDb for a TV show and return full metadata from details endpoint
pub async fn search_tv(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
) -> Option<ExternalMetadata> {
    let url = format!(
        "{}/search/tv?api_key={}&query={}",
        TMDB_BASE,
        api_key,
        urlencoded(name)
    );
    let cache_key_search = format!("search_tv:{}", name);

    let resp: TmdbSearchResult = match fetch_tmdb_json(client, &cache_key_search, &url).await {
        Some(json) => serde_json::from_value(json).ok()?,
        None => return None,
    };
    let search_item = resp.results.into_iter().next()?;
    let tmdb_id = search_item.id;

    let details_url = format!(
        "{}/tv/{}?api_key={}&append_to_response=credits,external_ids",
        TMDB_BASE, tmdb_id, api_key
    );
    let cache_key_details = format!("tv_details:{}", tmdb_id);
    let details: TmdbTvDetails =
        match fetch_tmdb_json(client, &cache_key_details, &details_url).await {
            Some(json) => serde_json::from_value(json).ok()?,
            None => return None,
        };

    let external_ids: Option<TmdbExternalIds> = {
        let external_ids_url = format!(
            "{}/tv/{}/external_ids?api_key={}",
            TMDB_BASE, tmdb_id, api_key
        );
        let cache_key_ext = format!("tv_external_ids:{}", tmdb_id);
        match fetch_tmdb_json(client, &cache_key_ext, &external_ids_url).await {
            Some(json) => serde_json::from_value(json).ok(),
            None => None,
        }
    };

    let images = fetch_images(client, api_key, "tv", tmdb_id).await;

    let production_year = details
        .first_air_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    let genres = details
        .genres
        .map(|gs| gs.into_iter().map(|g| g.name).collect());

    let studios = details
        .production_companies
        .as_ref()
        .map(|pcs| pcs.iter().map(|pc| pc.name.clone()).collect());

    let production_countries = details
        .production_countries
        .as_ref()
        .map(|countries| countries.iter().map(|c| c.iso_3166_1.clone()).collect());

    let runtime_ticks = details
        .episode_run_time
        .as_ref()
        .and_then(|rts| rts.first())
        .map(|mins| (*mins as i64) * 60_i64 * 10_000_000);

    Some(ExternalMetadata {
        name: details.name,
        overview: details.overview,
        production_year,
        community_rating: details.vote_average,
        official_rating: None,
        genres,
        premiere_date: details.first_air_date,
        runtime_ticks,
        image_urls: images,
        tmdb_id: Some(tmdb_id),
        tvdb_id: external_ids.as_ref().and_then(|e| e.tvdb_id),
        status: details.status,
        original_language: if details.original_language.is_empty() {
            None
        } else {
            Some(details.original_language)
        },
        tagline: details.tagline,
        production_countries,
        budget: None,
        revenue: None,
        homepage: details.homepage,
        end_date: details.last_air_date,
        vote_count: details.vote_count,
        popularity: details.popularity,
        studios,
        directors: None,
        writers: None,
        imdb_id: external_ids.and_then(|e| e.imdb_id),
    })
}

/// Fetch TheTVDB ID for a TV show from TMDb external_ids endpoint
pub async fn get_tvdb_id(client: &reqwest::Client, api_key: &str, tmdb_tv_id: i64) -> Option<i64> {
    let url = format!(
        "{}/tv/{}/external_ids?api_key={}",
        TMDB_BASE, tmdb_tv_id, api_key
    );
    let cache_key = format!("tv_external_ids:{}", tmdb_tv_id);
    let ids: TmdbExternalIds = match fetch_tmdb_json(client, &cache_key, &url).await {
        Some(json) => serde_json::from_value(json).ok()?,
        None => return None,
    };
    ids.tvdb_id
}

// ── Trailers ──

#[derive(Debug, Deserialize)]
struct TmdbVideos {
    results: Vec<TmdbVideo>,
}

#[derive(Debug, Deserialize)]
struct TmdbVideo {
    key: String,
    name: String,
    #[serde(rename = "type")]
    video_type: String,
    site: String,
}

/// Fetch trailers (videos) from TMDb for a movie or TV show.
pub async fn get_trailers(
    client: &reqwest::Client,
    api_key: &str,
    media_type: &str,
    tmdb_id: i64,
) -> Vec<(String, String)> {
    let url = format!(
        "{}/{}/{}/videos?api_key={}",
        TMDB_BASE, media_type, tmdb_id, api_key
    );
    let cache_key = format!("videos:{}:{}", media_type, tmdb_id);
    let mut result = Vec::new();

    if let Some(json) = fetch_tmdb_json(client, &cache_key, &url).await
        && let Ok(videos) = serde_json::from_value::<TmdbVideos>(json)
    {
        for video in videos.results {
            if video.site == "YouTube"
                && (video.video_type == "Teaser"
                    || video.video_type == "Trailer"
                    || video.video_type == "Official Trailer")
            {
                result.push((
                    video.name,
                    format!("https://www.youtube.com/watch?v={}", video.key),
                ));
            }
        }
        result.truncate(3);
    }
    result
}

// ── Similar ──

#[derive(Debug, Deserialize)]
struct TmdbSimilarResult {
    results: Vec<TmdbSimilarItem>,
}

#[derive(Debug, Deserialize)]
struct TmdbSimilarItem {
    id: i64,
    title: Option<String>,
    name: Option<String>,
    #[allow(dead_code)]
    overview: Option<String>,
    #[allow(dead_code)]
    poster_path: Option<String>,
}

/// Fetch similar items from TMDb for a movie or TV show.
pub async fn get_similar(
    client: &reqwest::Client,
    api_key: &str,
    media_type: &str,
    tmdb_id: i64,
) -> Vec<(i64, String)> {
    let url = format!(
        "{}/{}/{}/similar?api_key={}",
        TMDB_BASE, media_type, tmdb_id, api_key
    );
    let cache_key = format!("similar:{}:{}", media_type, tmdb_id);
    let mut result = Vec::new();

    if let Some(json) = fetch_tmdb_json(client, &cache_key, &url).await
        && let Ok(similar) = serde_json::from_value::<TmdbSimilarResult>(json)
    {
        for item in similar.results {
            let name = item.title.or(item.name).unwrap_or_default();
            result.push((item.id, name));
        }
        result.truncate(20);
    }
    result
}

// ── Plugin implementation ──

/// TMDB plugin that registers itself as a metadata provider.
pub struct TmdbPlugin {
    api_key: Mutex<Option<String>>,
}

impl Default for TmdbPlugin {
    fn default() -> Self {
        Self {
            api_key: Mutex::new(None),
        }
    }
}

impl Plugin for TmdbPlugin {
    fn id(&self) -> &str {
        "tmdb"
    }

    fn name(&self) -> &str {
        "The Movie Database (TMDB)"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches metadata, images, trailers, and cast/crew from The Movie Database (TMDB)."
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

impl TmdbPlugin {
    fn api_key(&self) -> Option<String> {
        self.api_key.lock().unwrap().clone()
    }
}

/// Create a new TMDB plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> Box<dyn Plugin> {
    Box::new(TmdbPlugin {
        api_key: Mutex::new(None),
    })
}

/// Register the TMDB plugin as a metadata provider with the given PluginManager.
///
/// This is the primary way to integrate the plugin — it registers closures that
/// capture the API key and call the public search/image functions.
pub async fn register_tmdb_provider<P>(
    plugin: &TmdbPlugin,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let api_key = plugin.api_key().ok_or("TMDB API key not configured")?;
    let api_key1 = api_key.clone();
    let api_key2 = api_key.clone();
    let api_key3 = api_key.clone();

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "tmdb".to_string(),
            move |client: &reqwest::Client, name: &str, year: Option<i32>| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let api_key = api_key1.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move { search_movie(&client, &api_key, &name, year).await })
            },
            move |client: &reqwest::Client, name: &str| -> Pin<
                Box<dyn Future<Output = Option<ExternalMetadata>> + Send>,
            > {
                let api_key = api_key2.clone();
                let name = name.to_string();
                let client = client.clone();
                Box::pin(async move { search_tv(&client, &api_key, &name).await })
            },
            move |client: &reqwest::Client, tmdb_id: Option<i64>, tvdb_id: Option<i64>, media_type: MediaType| -> Pin<
                Box<dyn Future<Output = Vec<MetadataImage>> + Send>,
            > {
                let api_key = api_key3.clone();
                let client = client.clone();
                Box::pin(async move {
                    if let Some(id) = tmdb_id {
                        let kind = match media_type {
                            MediaType::Movie => "movie",
                            MediaType::Series | MediaType::Episode => "tv",
                            _ => return Vec::new(),
                        };
                        fetch_images(&client, &api_key, kind, id).await
                    } else if let Some(_tvdb_id) = tvdb_id {
                        Vec::new()
                    } else {
                        Vec::new()
                    }
                })
            },
        )
        .await;

    Ok(())
}

/// Trait for types that can register metadata providers.
/// Implemented by PluginManager in rustyfin-core.
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
