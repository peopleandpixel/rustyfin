pub mod tmdb_client;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::tmdb_client::{TmdbApi, TmdbClient};
use rustyfin_model::media::MediaType;
use rustyfin_model::metadata::{
    ExternalMetadata, MetadataImage, MetadataLookups, MetadataPerson, SeasonMetadata,
};
use rustyfin_model::plugin::{Plugin, PluginStatus, PluginType};
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

    /// Attach id, year-aware and season lookups to the provider registered as `provider_name`.
    async fn register_metadata_lookups(
        &self,
        _provider_name: String,
        _lookups: Arc<dyn MetadataLookups>,
    ) {
    }
}

/// The TMDb lookups behind `MetadataLookups`, sharing the provider's key and client.
struct TmdbLookups {
    key: Arc<Mutex<Option<String>>>,
    client: TmdbClient,
}

impl TmdbLookups {
    fn api(&self) -> Option<TmdbApi> {
        let current = self.key.lock().unwrap().clone();
        self.client.api(current.as_deref())
    }
}

impl MetadataLookups for TmdbLookups {
    fn movie_by_id(
        &self,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send + '_>> {
        Box::pin(async move { movie_by_id(&self.api()?, id).await })
    }

    fn series_by_id(
        &self,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send + '_>> {
        Box::pin(async move { series_by_id(&self.api()?, id).await })
    }

    fn search_series(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Pin<Box<dyn Future<Output = Option<ExternalMetadata>> + Send + '_>> {
        let name = name.to_string();
        Box::pin(async move { search_tv(&self.api()?, &name, year).await })
    }

    fn season(
        &self,
        series_id: i64,
        season_number: i32,
    ) -> Pin<Box<dyn Future<Output = Option<SeasonMetadata>> + Send + '_>> {
        Box::pin(async move { season(&self.api()?, series_id, season_number).await })
    }
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

/// Works out the TMDb plugin's status from its saved config and whether `RUSTYFIN_TMDB_API_KEY`
/// is set.
///
/// A saved `"status"` string wins: "Active", "Disabled" or "Malfunctioned" map to that status and
/// any other string is Disabled. Without one, the plugin is Active when `env_key` is set or the
/// config's `api_key` is a string that is non-blank after trimming, and Disabled otherwise.
pub fn status_for(config: Option<&serde_json::Value>, env_key: bool) -> PluginStatus {
    rustyfin_model::plugin::key_status(config, env_key)
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
    let client3 = client.clone();
    let lookups = Arc::new(TmdbLookups {
        key: plugin.api_key.clone(),
        client,
    });

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
                    search_tv(&tmdb, &name, None).await
                })
            },
            move |_client: &reqwest::Client,
                  tmdb_id: Option<i64>,
                  _tvdb_id: Option<i64>,
                  media_type: MediaType|
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
                    fetch_images_for_tmdb(&tmdb, id, media_type).await
                })
            },
        )
        .await;
    plugin_manager
        .register_metadata_lookups("tmdb".to_string(), lookups)
        .await;

    Ok(())
}

/// The images of a movie's or series' TMDb id. They come with the details call, which the client
/// has cached from the identification, so this adds no request. A TMDb id is a movie's or a
/// series' by the item's type; other types have none here.
async fn fetch_images_for_tmdb(
    tmdb: &TmdbApi,
    tmdb_id: i64,
    media_type: MediaType,
) -> Vec<MetadataImage> {
    let meta = match media_type {
        MediaType::Movie => movie_by_id(tmdb, tmdb_id).await,
        MediaType::Series => series_by_id(tmdb, tmdb_id).await,
        _ => None,
    };
    meta.map(|m| m.image_urls).unwrap_or_default()
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
    character: Option<String>,
    profile_path: Option<String>,
}

/// TMDb crew member
#[derive(Debug, Deserialize)]
struct TmdbCrewMember {
    id: i64,
    name: String,
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
    original_title: Option<String>,
    original_name: Option<String>,
    overview: Option<String>,
    release_date: Option<String>,
    first_air_date: Option<String>,
    vote_average: Option<f64>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    genre_ids: Option<Vec<i32>>,
}

#[derive(Debug, Default, Deserialize)]
struct TmdbImages {
    posters: Option<Vec<TmdbImage>>,
    backdrops: Option<Vec<TmdbImage>>,
    logos: Option<Vec<TmdbImage>>,
}

/// `releases` appended to a movie: the release (and certification) per country
#[derive(Debug, Deserialize)]
struct TmdbReleases {
    countries: Option<Vec<TmdbReleaseCountry>>,
}

#[derive(Debug, Deserialize)]
struct TmdbReleaseCountry {
    iso_3166_1: String,
    certification: Option<String>,
}

/// `content_ratings` appended to a series
#[derive(Debug, Deserialize)]
struct TmdbContentRatings {
    results: Option<Vec<TmdbContentRating>>,
}

#[derive(Debug, Deserialize)]
struct TmdbContentRating {
    iso_3166_1: String,
    rating: Option<String>,
}

/// A season with its episodes (`/tv/{id}/season/{n}`)
#[derive(Debug, Deserialize)]
struct TmdbSeason {
    id: Option<i64>,
    overview: Option<String>,
    air_date: Option<String>,
    poster_path: Option<String>,
    images: Option<TmdbImages>,
    credits: Option<TmdbCredits>,
    episodes: Option<Vec<TmdbEpisode>>,
}

#[derive(Debug, Deserialize)]
struct TmdbEpisode {
    id: i64,
    episode_number: i32,
    name: Option<String>,
    overview: Option<String>,
    air_date: Option<String>,
    vote_average: Option<f64>,
    runtime: Option<i32>,
    still_path: Option<String>,
    crew: Option<Vec<TmdbCrewMember>>,
    guest_stars: Option<Vec<TmdbCastMember>>,
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
    imdb_id: Option<String>,
    credits: Option<TmdbCredits>,
    images: Option<TmdbImages>,
    releases: Option<TmdbReleases>,
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
    networks: Option<Vec<TmdbProductionCompany>>,
    credits: Option<TmdbCredits>,
    images: Option<TmdbImages>,
    external_ids: Option<TmdbExternalIds>,
    content_ratings: Option<TmdbContentRatings>,
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
    iso_639_1: Option<String>,
    vote_average: Option<f64>,
}

/// What a movie details call appends, as Jellyfin 12.4's TMDb plugin asks for it, so that one call
/// gives the details, credits, images, IMDb id and certifications (and the TMDb cache shares the
/// entry with Jellyfin's).
const MOVIE_APPEND: &str = "credits,images,keywords,releases,videos";
/// What a series details call appends (Jellyfin 12.4's list).
const SERIES_APPEND: &str =
    "credits,images,external_ids,content_ratings,keywords,videos,episode_groups,aggregate_credits";
/// What a season call appends (Jellyfin 12.4's list). The season lists all its episodes.
const SEASON_APPEND: &str = "credits,images,external_ids,videos";
/// Jellyfin's default for the TMDb plugin's MaxCastMembers.
const MAX_CAST: usize = 15;
/// The country whose certification becomes the official rating (Jellyfin's default metadata
/// country).
const RATING_COUNTRY: &str = "US";

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
    let search_item = best_match(resp.results, name)?;

    // Step 2: everything else comes with the details
    movie_by_id(tmdb, search_item.id).await
}

/// A movie's metadata by TMDb id, from one details call.
pub async fn movie_by_id(tmdb: &TmdbApi, tmdb_id: i64) -> Option<ExternalMetadata> {
    let details_endpoint = format!("/movie/{}", tmdb_id);
    let details_query = [("append_to_response", MOVIE_APPEND)];
    let details: TmdbMovieDetails = match tmdb.get_json(&details_endpoint, &details_query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    let images = convert_images(tmdb, details.images.unwrap_or_default());

    // Parse production year from release_date
    let production_year = year_of(details.release_date.as_deref());

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

    // The certification of the rating country's release
    let official_rating = details
        .releases
        .and_then(|r| r.countries)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.iso_3166_1 == RATING_COUNTRY)
        .find_map(|c| non_empty(c.certification));

    let people = details.credits.map(|c| convert_people(tmdb, c, "Actor"));

    Some(ExternalMetadata {
        name: details.title,
        overview: details.overview,
        production_year,
        community_rating: details.vote_average,
        official_rating,
        genres,
        premiere_date: non_empty(details.release_date),
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
        imdb_id: non_empty(details.imdb_id),
        people,
    })
}

/// Search TMDb for a TV show and return full metadata from details endpoint. `year` is the first
/// air year, which Jellyfin sends from the folder name ("One Night (2023)").
pub async fn search_tv(tmdb: &TmdbApi, name: &str, year: Option<i32>) -> Option<ExternalMetadata> {
    // Step 1: Search to find the TMDb ID
    let year_text = year.map(|y| y.to_string());
    let mut query = vec![("query", name)];
    if let Some(y) = year_text.as_deref() {
        query.push(("first_air_date_year", y));
    }
    let resp: TmdbSearchResult = match tmdb.get_json("/search/tv", &query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };
    let search_item = best_match(resp.results, name)?;

    // Step 2: everything else comes with the details
    series_by_id(tmdb, search_item.id).await
}

/// A series' metadata by TMDb id, from one details call.
pub async fn series_by_id(tmdb: &TmdbApi, tmdb_id: i64) -> Option<ExternalMetadata> {
    let details_endpoint = format!("/tv/{}", tmdb_id);
    let details_query = [("append_to_response", SERIES_APPEND)];
    let details: TmdbTvDetails = match tmdb.get_json(&details_endpoint, &details_query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    let images = convert_images(tmdb, details.images.unwrap_or_default());

    // Parse production year from first_air_date
    let production_year = year_of(details.first_air_date.as_deref());

    // Genres from details (names, not IDs)
    let genres = details
        .genres
        .map(|gs| gs.into_iter().map(|g| g.name).collect());

    // Studios: the networks, as Jellyfin's series provider does; else the production companies
    let studios = details
        .networks
        .as_ref()
        .filter(|ns| !ns.is_empty())
        .or(details.production_companies.as_ref())
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

    // The rating country's content rating, else the first one listed (Jellyfin's order)
    let ratings: Vec<TmdbContentRating> = details
        .content_ratings
        .and_then(|r| r.results)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.rating.as_deref().is_some_and(|v| !v.trim().is_empty()))
        .collect();
    let official_rating = ratings
        .iter()
        .find(|r| r.iso_3166_1 == RATING_COUNTRY)
        .or(ratings.first())
        .and_then(|r| r.rating.clone());

    let people = details.credits.map(|c| convert_people(tmdb, c, "Actor"));
    let external_ids = details.external_ids;

    Some(ExternalMetadata {
        name: details.name,
        overview: details.overview,
        production_year,
        community_rating: details.vote_average,
        official_rating,
        genres,
        premiere_date: non_empty(details.first_air_date),
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
        imdb_id: external_ids.and_then(|e| non_empty(e.imdb_id)),
        people,
    })
}

/// A season and all its episodes from one call. Episodes carry their name, overview, air date,
/// rating, still and guest stars and crew; their IMDb and TVDb ids would need one call each, so
/// they are left out.
pub async fn season(tmdb: &TmdbApi, series_id: i64, season_number: i32) -> Option<SeasonMetadata> {
    let endpoint = format!("/tv/{}/season/{}", series_id, season_number);
    let query = [("append_to_response", SEASON_APPEND)];
    let season: TmdbSeason = match tmdb.get_json(&endpoint, &query).await {
        Ok(json) => serde_json::from_value(json).ok()?,
        Err(_) => return None,
    };

    let mut images = convert_images(tmdb, season.images.unwrap_or_default());
    if !images.iter().any(|i| i.image_type == "Primary")
        && let Some(path) = season.poster_path
    {
        images.insert(
            0,
            MetadataImage {
                image_type: "Primary".to_string(),
                url: tmdb.image_url(&path),
            },
        );
    }
    let meta = ExternalMetadata {
        overview: non_empty(season.overview),
        premiere_date: non_empty(season.air_date.clone()),
        production_year: year_of(season.air_date.as_deref()),
        image_urls: images,
        tmdb_id: season.id,
        ..Default::default()
    };

    // The season's regular cast, which every episode credits before its guest stars (Jellyfin's
    // episode credits list them too)
    let regulars: Vec<MetadataPerson> = season
        .credits
        .and_then(|c| c.cast)
        .unwrap_or_default()
        .into_iter()
        .take(MAX_CAST)
        .map(|member| cast_person(tmdb, member, "Actor"))
        .collect();
    let episodes = season
        .episodes
        .unwrap_or_default()
        .into_iter()
        .map(|ep| {
            let mut people = regulars.clone();
            for guest in ep
                .guest_stars
                .unwrap_or_default()
                .into_iter()
                .take(MAX_CAST)
            {
                people.push(cast_person(tmdb, guest, "GuestStar"));
            }
            people.extend(crew_people(tmdb, ep.crew.unwrap_or_default()));
            let image_urls = ep
                .still_path
                .map(|path| MetadataImage {
                    image_type: "Primary".to_string(),
                    url: tmdb.image_url(&path),
                })
                .into_iter()
                .collect();
            let meta = ExternalMetadata {
                name: non_empty(ep.name),
                overview: non_empty(ep.overview),
                premiere_date: non_empty(ep.air_date.clone()),
                production_year: year_of(ep.air_date.as_deref()),
                community_rating: ep.vote_average.filter(|v| *v > 0.0),
                runtime_ticks: ep.runtime.map(|mins| (mins as i64) * 60_i64 * 10_000_000),
                image_urls,
                tmdb_id: Some(ep.id),
                people: Some(people),
                ..Default::default()
            };
            (ep.episode_number, meta)
        })
        .collect();

    Some(SeasonMetadata { meta, episodes })
}

/// The search result whose title (or original title) is the searched name, ignoring case, else
/// the first one, as Jellyfin picks: "One Night" (2023) is the show "One Night", though TMDb
/// lists "One Night Stand" first.
fn best_match(results: Vec<TmdbSearchItem>, name: &str) -> Option<TmdbSearchItem> {
    let wanted = name.trim().to_lowercase();
    let exact = results.iter().position(|item| {
        [
            &item.title,
            &item.name,
            &item.original_title,
            &item.original_name,
        ]
        .into_iter()
        .flatten()
        .any(|t| t.trim().to_lowercase() == wanted)
    });
    results.into_iter().nth(exact.unwrap_or(0))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

fn year_of(date: Option<&str>) -> Option<i32> {
    date.and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok())
}

/// The cast (the first `MAX_CAST`, as `cast_kind`) and the directors, writers and producers.
fn convert_people(tmdb: &TmdbApi, credits: TmdbCredits, cast_kind: &str) -> Vec<MetadataPerson> {
    let mut people: Vec<MetadataPerson> = credits
        .cast
        .unwrap_or_default()
        .into_iter()
        .take(MAX_CAST)
        .map(|member| cast_person(tmdb, member, cast_kind))
        .collect();
    people.extend(crew_people(tmdb, credits.crew.unwrap_or_default()));
    people
}

fn cast_person(tmdb: &TmdbApi, member: TmdbCastMember, kind: &str) -> MetadataPerson {
    MetadataPerson {
        name: member.name,
        kind: kind.to_string(),
        role: non_empty(member.character),
        image_url: member.profile_path.map(|p| tmdb.image_url(&p)),
    }
}

/// Crew as Jellyfin keeps it (TmdbUtils.MapCrewToPersonType): directors, writers and producers.
fn crew_people(tmdb: &TmdbApi, crew: Vec<TmdbCrewMember>) -> Vec<MetadataPerson> {
    crew.into_iter()
        .filter_map(|member| {
            let job = member.job.as_deref().unwrap_or_default();
            let department = member.department.as_deref().unwrap_or_default();
            let kind = if job.eq_ignore_ascii_case("Director") {
                "Director"
            } else if department.eq_ignore_ascii_case("Writing") {
                "Writer"
            } else if department.eq_ignore_ascii_case("Production")
                && job.to_ascii_lowercase().contains("producer")
            {
                "Producer"
            } else {
                return None;
            };
            Some(MetadataPerson {
                name: member.name,
                kind: kind.to_string(),
                role: non_empty(member.job),
                image_url: member.profile_path.map(|p| tmdb.image_url(&p)),
            })
        })
        .collect()
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

/// Image language preference, as Jellyfin ranks TMDb images: the metadata language first (for
/// posters and logos), then images without text, then the rest; within each, the best voted first.
fn rank_images(images: Option<Vec<TmdbImage>>, text_first: bool) -> Vec<TmdbImage> {
    let mut images = images.unwrap_or_default();
    let rank = |image: &TmdbImage| match image.iso_639_1.as_deref() {
        Some("en") if text_first => 0,
        None | Some("") => 1,
        Some("en") => 2,
        _ => 3,
    };
    images.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then(
            b.vote_average
                .unwrap_or(0.0)
                .total_cmp(&a.vote_average.unwrap_or(0.0)),
        )
    });
    images
}

/// The Primary (posters), Backdrop (backdrops without text first), Thumb (a backdrop with the
/// title on it) and Logo images of an appended `images` block.
fn convert_images(tmdb: &TmdbApi, images: TmdbImages) -> Vec<MetadataImage> {
    let mut result = Vec::new();
    let numbered = |kind: &str, i: usize| {
        if i == 0 {
            kind.to_string()
        } else {
            format!("{kind}_{i}")
        }
    };
    for (i, p) in rank_images(images.posters, true)
        .into_iter()
        .take(3)
        .enumerate()
    {
        result.push(MetadataImage {
            image_type: numbered("Primary", i),
            url: tmdb.image_url(&p.file_path),
        });
    }
    let backdrops = rank_images(images.backdrops, false);
    if let Some(thumb) = backdrops
        .iter()
        .filter(|b| b.iso_639_1.as_deref().is_some_and(|l| !l.is_empty()))
        .min_by_key(|b| b.iso_639_1.as_deref() != Some("en"))
    {
        result.push(MetadataImage {
            image_type: "Thumb".to_string(),
            url: tmdb.image_url(&thumb.file_path),
        });
    }
    for (i, b) in backdrops.into_iter().take(3).enumerate() {
        result.push(MetadataImage {
            image_type: numbered("Backdrop", i),
            url: tmdb.image_url(&b.file_path),
        });
    }
    if let Some(l) = rank_images(images.logos, true).into_iter().next() {
        result.push(MetadataImage {
            image_type: "Logo".to_string(),
            url: tmdb.image_url(&l.file_path),
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyfin_model::plugin::PluginStatus;
    use serde_json::json;

    // R1 scan (E0-scan-3): a search takes the result titled as searched, else the first.
    #[test]
    fn r1scan_best_match_prefers_the_exact_title() {
        let results = |json: serde_json::Value| -> Vec<TmdbSearchItem> {
            serde_json::from_value(json).unwrap()
        };
        let listed = json!([
            {"id": 231889, "name": "One Night Stand"},
            {"id": 228527, "name": "One Night"}
        ]);
        assert_eq!(
            best_match(results(listed.clone()), "one night").unwrap().id,
            228527
        );
        assert_eq!(best_match(results(listed), "Night").unwrap().id, 231889);
        let original = json!([{"id": 1, "title": "Other"}, {"id": 2, "title": "Didi", "original_title": "弟弟"}]);
        assert_eq!(best_match(results(original), "弟弟").unwrap().id, 2);
        assert!(best_match(Vec::new(), "x").is_none());
    }

    // B136: a key turns TMDb on unless a saved status says otherwise.
    #[test]
    fn b136_status_for_a_key_without_a_saved_status_is_active() {
        assert_eq!(status_for(None, true), PluginStatus::Active);
        assert_eq!(
            status_for(Some(&json!({ "api_key": "k" })), false),
            PluginStatus::Active
        );
        assert_eq!(
            status_for(Some(&json!({ "api_key": "k" })), true),
            PluginStatus::Active
        );
    }

    #[test]
    fn b136_status_for_no_key_is_disabled() {
        assert_eq!(status_for(None, false), PluginStatus::Disabled);
        for config in [
            json!({}),
            json!({ "api_key": null }),
            json!({ "api_key": "   " }),
        ] {
            assert_eq!(
                status_for(Some(&config), false),
                PluginStatus::Disabled,
                "{config}"
            );
        }
    }

    #[test]
    fn b136_status_for_a_saved_status_wins() {
        let disabled = json!({ "api_key": "k", "status": "Disabled" });
        assert_eq!(status_for(Some(&disabled), true), PluginStatus::Disabled);
        let active = json!({ "status": "Active" });
        assert_eq!(status_for(Some(&active), false), PluginStatus::Active);
        let broken = json!({ "status": "Malfunctioned" });
        assert_eq!(status_for(Some(&broken), true), PluginStatus::Malfunctioned);
    }
}
