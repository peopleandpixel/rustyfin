pub mod tmdb_client;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::tmdb_client::{TmdbApi, TmdbClient};
use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use rustyfin_model::plugin::{Plugin, PluginType};
use serde::Deserialize;

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

// ── Plugin implementation ──

/// TMDb plugin that registers itself as a metadata provider.
pub struct TmdbPlugin {
    api_key: Arc<Mutex<Option<String>>>,
}

impl Default for TmdbPlugin {
    fn default() -> Self {
        Self {
            api_key: Arc::new(Mutex::new(None)),
        }
    }
}

impl Plugin for TmdbPlugin {
    fn id(&self) -> &str {
        "tmdb"
    }

    fn name(&self) -> &str {
        "The Movie Database (TMDb)"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &str {
        "Fetches metadata, images, and cast information from The Movie Database (TMDb)."
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

/// Create a new TMDb plugin instance (FFI entry point for dynamic loading).
/// Only compiled when building as a dynamic library (`cdylib`).
/// Returns a raw pointer to a `Box<dyn Plugin>` for FFI safety.
///
/// # Safety
///
/// The caller takes ownership of the returned `Box<dyn Plugin>` and must free it exactly once.
#[cfg(feature = "dynamic")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyfin_plugin_create() -> *mut std::ffi::c_void {
    let plugin: Box<dyn Plugin> = Box::new(TmdbPlugin::default());
    Box::into_raw(Box::new(plugin)).cast()
}

/// Register the TMDb plugin as a metadata provider with the given PluginManager.
///
/// Always registers. Each call reads the plugin's current key and uses the shared `client`; with
/// no key the call returns nothing and makes no request.
pub async fn register_tmdb_provider<P>(
    plugin: &TmdbPlugin,
    client: TmdbClient,
    plugin_manager: &P,
) -> Result<(), String>
where
    P: MetadataProviderRegistrar,
{
    let key1 = plugin.api_key.clone();
    let key2 = plugin.api_key.clone();
    let key3 = plugin.api_key.clone();
    let client1 = client.clone();
    let client2 = client.clone();
    let client3 = client;

    let plugin_id = plugin.id().to_string();
    plugin_manager
        .register_metadata_provider(
            plugin_id,
            "tmdb".to_string(),
            move |_client: &reqwest::Client,
                  name: &str,
                  year: Option<i32>|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let key = key1.clone();
                let client = client1.clone();
                let name = name.to_string();
                Box::pin(async move {
                    let current = key.lock().unwrap().clone();
                    let tmdb = client.api(current.as_deref())?;
                    search_movie(&tmdb, &name, year).await
                })
            },
            move |_client: &reqwest::Client,
                  name: &str|
                  -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send>> {
                let key = key2.clone();
                let client = client2.clone();
                let name = name.to_string();
                Box::pin(async move {
                    let current = key.lock().unwrap().clone();
                    let tmdb = client.api(current.as_deref())?;
                    search_tv(&tmdb, &name).await
                })
            },
            move |_client: &reqwest::Client,
                  tmdb_id: Option<i64>,
                  _tvdb_id: Option<i64>,
                  _media_type: MediaType|
                  -> Pin<Box<dyn Future<Output = Vec<MetadataImage>> + Send>> {
                let key = key3.clone();
                let client = client3.clone();
                Box::pin(async move {
                    let Some(id) = tmdb_id else {
                        return Vec::new();
                    };
                    let current = key.lock().unwrap().clone();
                    let Some(tmdb) = client.api(current.as_deref()) else {
                        return Vec::new();
                    };
                    fetch_images_for_tmdb(&tmdb, id).await
                })
            },
        )
        .await;

    Ok(())
}

/// Fetch images for a TMDb ID (used by the registration closure).
async fn fetch_images_for_tmdb(tmdb: &TmdbApi, tmdb_id: i64) -> Vec<MetadataImage> {
    // Try movie first, then TV
    let movie_images = fetch_images(tmdb, "movie", tmdb_id).await;
    if !movie_images.is_empty() {
        return movie_images;
    }
    fetch_images(tmdb, "tv", tmdb_id).await
}

/// TMDb person details with photo
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

/// TMDb crew/cast member
#[derive(Debug, Deserialize)]
struct TmdbCastMember {
    id: i64,
    name: String,
    #[allow(dead_code)]
    character: Option<String>,
    profile_path: Option<String>,
}

/// TMDb crew member
#[derive(Debug, Deserialize)]
struct TmdbCrewMember {
    id: i64,
    name: String,
    #[allow(dead_code)]
    job: Option<String>,
    department: Option<String>,
    profile_path: Option<String>,
}

/// TMDb movie/TV details with cast and crew
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
pub async fn get_person_photo(tmdb: &TmdbApi, tmdb_person_id: i64) -> Option<PersonPhoto> {
    let endpoint = format!("/person/{}", tmdb_person_id);

    let details: TmdbPersonDetails = match tmdb.get_json(&endpoint, &[]).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    details.profile_path.map(|path| PersonPhoto {
        person_id: details.id.to_string(),
        name: details.name,
        photo_url: tmdb.image_url(&path),
    })
}

/// Fetch cast and crew for a movie from TMDb, returning person info with photos
pub async fn get_movie_people(tmdb: &TmdbApi, tmdb_movie_id: i64) -> Vec<PersonPhoto> {
    let endpoint = format!("/movie/{}/credits", tmdb_movie_id);

    let mut result = Vec::new();

    let details: TmdbDetails = match tmdb.get_json(&endpoint, &[]).await {
        Ok(json) => match serde_json::from_value::<TmdbDetails>(json) {
            Ok(d) => d,
            Err(_) => return result,
        },
        Err(_) => return result,
    };

    if let Some(credits) = details.credits {
        if let Some(cast) = credits.cast {
            for member in cast.into_iter().take(20) {
                if let Some(path) = member.profile_path {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: tmdb.image_url(&path),
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
                        photo_url: tmdb.image_url(&path),
                    });
                }
            }
        }
    }

    result
}

/// Fetch cast and crew for a TV show season from TMDb, returning person info with photos
pub async fn get_tv_people(
    tmdb: &TmdbApi,
    tmdb_tv_id: i64,
    season_number: i32,
) -> Vec<PersonPhoto> {
    let endpoint = format!("/tv/{}/season/{}/credits", tmdb_tv_id, season_number);

    let mut result = Vec::new();

    let details: TmdbDetails = match tmdb.get_json(&endpoint, &[]).await {
        Ok(json) => match serde_json::from_value::<TmdbDetails>(json) {
            Ok(d) => d,
            Err(_) => return result,
        },
        Err(_) => return result,
    };

    if let Some(credits) = details.credits {
        if let Some(cast) = credits.cast {
            for member in cast.into_iter().take(20) {
                if let Some(path) = member.profile_path {
                    result.push(PersonPhoto {
                        person_id: member.id.to_string(),
                        name: member.name,
                        photo_url: tmdb.image_url(&path),
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
                        photo_url: tmdb.image_url(&path),
                    });
                }
            }
        }
    }

    result
}

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

/// TMDb production company (studio)
#[derive(Debug, Deserialize)]
struct TmdbProductionCompany {
    name: String,
    #[allow(dead_code)]
    id: i64,
}

/// TMDb country
#[derive(Debug, Deserialize)]
struct TmdbProductionCountry {
    iso_3166_1: String,
    #[allow(dead_code)]
    name: String,
}

/// Full TMDb movie details response
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

/// Full TMDb TV details response
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

/// TMDb genre
#[derive(Debug, Deserialize)]
struct TmdbGenre {
    name: String,
    #[allow(dead_code)]
    id: i32,
}

/// TMDb spoken language
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

/// Search TMDb for a movie and return full metadata from details endpoint
pub async fn search_movie(
    tmdb: &TmdbApi,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
    // Step 1: Search to find the TMDb ID
    let year_text = year.map(|y| y.to_string());
    let mut query = vec![("query", name)];
    if let Some(y) = year_text.as_deref() {
        query.push(("year", y));
    }

    let resp: TmdbSearchResult = match tmdb.get_json("/search/movie", &query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };
    let search_item = resp.results.into_iter().next()?;
    let tmdb_id = search_item.id;

    // Step 2: Fetch full details from the details endpoint
    let details_endpoint = format!("/movie/{}", tmdb_id);
    let details_query = [("append_to_response", "credits,external_ids")];
    let details: TmdbMovieDetails = match tmdb.get_json(&details_endpoint, &details_query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    // Step 3: Fetch external IDs (IMDb, TVDb)
    let external_ids: Option<TmdbExternalIds> = {
        let endpoint = format!("/movie/{}/external_ids", tmdb_id);
        match tmdb.get_json(&endpoint, &[]).await {
            Ok(json) => serde_json::from_value(json).ok(),
            Err(_) => None,
        }
    };

    // Step 4: Fetch images
    let images = fetch_images(tmdb, "movie", tmdb_id).await;

    // Parse production year from release_date
    let production_year = details
        .release_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    // Genres from details (names, not IDs)
    let genres = details
        .genres
        .map(|gs| gs.into_iter().map(|g| g.name).collect());

    // Studios from production companies
    let studios = details
        .production_companies
        .as_ref()
        .map(|pcs| pcs.iter().map(|pc| pc.name.clone()).collect());

    // Production countries
    let production_countries = details
        .production_countries
        .as_ref()
        .map(|countries| countries.iter().map(|c| c.iso_3166_1.clone()).collect());

    // Runtime in ticks (TMDb returns minutes, 1 tick = 100ns)
    let runtime_ticks = details
        .runtime
        .map(|mins| (mins as i64) * 60_i64 * 10_000_000);

    // Directors from credits (fetched via append_to_response)
    let directors = None; // Will be fetched via get_movie_people

    // Writers from credits
    let writers = None; // Will be fetched via get_movie_people

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
        directors,
        writers,
        imdb_id: external_ids.and_then(|e| e.imdb_id),
    })
}

/// Search TMDb for a TV show and return full metadata from details endpoint
pub async fn search_tv(tmdb: &TmdbApi, name: &str) -> Option<ExternalMetadata> {
    // Step 1: Search to find the TMDb ID
    let query = [("query", name)];
    let resp: TmdbSearchResult = match tmdb.get_json("/search/tv", &query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };
    let search_item = resp.results.into_iter().next()?;
    let tmdb_id = search_item.id;

    // Step 2: Fetch full details from the details endpoint
    let details_endpoint = format!("/tv/{}", tmdb_id);
    let details_query = [("append_to_response", "credits,external_ids")];
    let details: TmdbTvDetails = match tmdb.get_json(&details_endpoint, &details_query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    // Step 3: Fetch external IDs (IMDb, TVDb)
    let external_ids: Option<TmdbExternalIds> = {
        let endpoint = format!("/tv/{}/external_ids", tmdb_id);
        match tmdb.get_json(&endpoint, &[]).await {
            Ok(json) => serde_json::from_value(json).ok(),
            Err(_) => None,
        }
    };

    // Step 4: Fetch images
    let images = fetch_images(tmdb, "tv", tmdb_id).await;

    // Parse production year from first_air_date
    let production_year = details
        .first_air_date
        .as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    // Genres from details (names, not IDs)
    let genres = details
        .genres
        .map(|gs| gs.into_iter().map(|g| g.name).collect());

    // Studios from production companies
    let studios = details
        .production_companies
        .as_ref()
        .map(|pcs| pcs.iter().map(|pc| pc.name.clone()).collect());

    // Production countries
    let production_countries = details
        .production_countries
        .as_ref()
        .map(|countries| countries.iter().map(|c| c.iso_3166_1.clone()).collect());

    // Runtime from episode_run_time (first value, in minutes)
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

/// Map TMDb genre IDs to genre names
/// Movie genres: https://developer.themoviedb.org/reference/movie-genres-list
/// TV genres: https://developer.themoviedb.org/reference/tv-genres-list
#[allow(dead_code)]
fn tmdb_genre_name(id: i32) -> Option<&'static str> {
    match id {
        28 => Some("Action"),
        12 => Some("Adventure"),
        16 => Some("Animation"),
        35 => Some("Comedy"),
        80 => Some("Crime"),
        99 => Some("Documentary"),
        18 => Some("Drama"),
        10751 => Some("Family"),
        14 => Some("Fantasy"),
        36 => Some("History"),
        27 => Some("Horror"),
        10402 => Some("Music"),
        9648 => Some("Mystery"),
        10749 => Some("Romance"),
        878 => Some("Science Fiction"),
        10770 => Some("TV Movie"),
        53 => Some("Thriller"),
        10752 => Some("War"),
        37 => Some("Western"),
        10762 => Some("Kids"),
        10763 => Some("News"),
        10764 => Some("Reality"),
        10765 => Some("Sci-Fi & Fantasy"),
        10766 => Some("Soap"),
        10767 => Some("Talk"),
        10768 => Some("War & Politics"),
        _ => None,
    }
}

async fn fetch_images(tmdb: &TmdbApi, media_type: &str, id: i64) -> Vec<MetadataImage> {
    let endpoint = format!("/{}/{}/images", media_type, id);
    let mut result = Vec::new();

    if let Ok(json) = tmdb.get_json(&endpoint, &[]).await
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
                    url: tmdb.image_url(&p.file_path),
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
                    url: tmdb.image_url(&b.file_path),
                });
            }
        }
        if let Some(logos) = imgs.logos
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: tmdb.image_url(&l.file_path),
            });
        }
    }

    result
}

/// Fetch TheTVDB ID for a TV show from TMDb external_ids endpoint
pub async fn get_tvdb_id(tmdb: &TmdbApi, tmdb_tv_id: i64) -> Option<i64> {
    let endpoint = format!("/tv/{}/external_ids", tmdb_tv_id);
    let ids: TmdbExternalIds = match tmdb.get_json(&endpoint, &[]).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };
    ids.tvdb_id
}

/// TMDb video result for trailers
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
/// Returns a list of (name, url) pairs for YouTube trailers.
pub async fn get_trailers(
    tmdb: &TmdbApi,
    media_type: &str, // "movie" or "tv"
    tmdb_id: i64,
) -> Vec<(String, String)> {
    let endpoint = format!("/{}/{}/videos", media_type, tmdb_id);
    let mut result = Vec::new();

    if let Ok(json) = tmdb.get_json(&endpoint, &[]).await
        && let Ok(videos) = serde_json::from_value::<TmdbVideos>(json)
    {
        // Filter for YouTube trailers, prefer "Teaser" or "Trailer" type
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
        // Limit to 3 trailers
        result.truncate(3);
    }
    result
}

/// TMDb similar item result
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
/// Returns a list of (tmdb_id, name) pairs for similar items.
pub async fn get_similar(
    tmdb: &TmdbApi,
    media_type: &str, // "movie" or "tv"
    tmdb_id: i64,
) -> Vec<(i64, String)> {
    let endpoint = format!("/{}/{}/similar", media_type, tmdb_id);
    let mut result = Vec::new();

    if let Ok(json) = tmdb.get_json(&endpoint, &[]).await
        && let Ok(similar) = serde_json::from_value::<TmdbSimilarResult>(json)
    {
        for item in similar.results {
            let name = item.title.or(item.name).unwrap_or_default();
            result.push((item.id, name));
        }
        // Limit to 20 similar items
        result.truncate(20);
    }
    result
}
