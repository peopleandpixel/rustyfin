use moka::sync::Cache;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use serde::Deserialize;
use serde_json;
use std::sync::LazyLock;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

/// L1 in-memory cache for TMDb API responses.
/// Key format: "<endpoint>:<id>" or "<endpoint>:<query>"
/// TTL: 24 hours — metadata changes rarely, so caching aggressively saves API calls.
static TMDB_CACHE: LazyLock<Cache<String, String>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(10_000)
        .time_to_live(std::time::Duration::from_secs(24 * 3600))
        .build()
});

/// Fetch JSON from TMDb with caching.
/// Checks the cache first; on miss, performs the HTTP request and caches the raw JSON.
async fn fetch_tmdb_json(
    client: &reqwest::Client,
    cache_key: &str,
    url: &str,
) -> Option<serde_json::Value> {
    // Check cache first
    if let Some(cached) = TMDB_CACHE.get(cache_key) {
        return serde_json::from_str(&cached).ok();
    }

    // Fetch from API
    let resp = client.get(url).send().await.ok()?;
    let text = resp.text().await.ok()?;

    // Parse and cache
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    TMDB_CACHE.insert(cache_key.to_string(), text);
    Some(json)
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
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
    year: Option<i32>,
) -> Option<ExternalMetadata> {
    // Step 1: Search to find the TMDb ID (with caching)
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

    // Step 2: Fetch full details from the details endpoint (with caching)
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

    // Step 3: Fetch external IDs (IMDb, TVDb) (with caching)
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

    // Step 4: Fetch images (with caching)
    let images = fetch_images(client, api_key, "movie", tmdb_id).await;

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
pub async fn search_tv(
    client: &reqwest::Client,
    api_key: &str,
    name: &str,
) -> Option<ExternalMetadata> {
    // Step 1: Search to find the TMDb ID (with caching)
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

    // Step 2: Fetch full details from the details endpoint (with caching)
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

    // Step 3: Fetch external IDs (IMDb, TVDb) (with caching)
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

    // Step 4: Fetch images (with caching)
    let images = fetch_images(client, api_key, "tv", tmdb_id).await;

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

/// Fetch TheTVDB ID for a TV show from TMDb external_ids endpoint
pub async fn get_tvdb_id(client: &reqwest::Client, api_key: &str, tmdb_tv_id: i64) -> Option<i64> {
    let url = format!(
        "{}/tv/{}/external_ids?api_key={}",
        TMDB_BASE, tmdb_tv_id, api_key
    );
    let ids: TmdbExternalIds = client.get(&url).send().await.ok()?.json().await.ok()?;
    ids.tvdb_id
}

fn urlencoded(s: &str) -> String {
    // Properly URL-encode a search query: percent-encode non-ASCII and special chars
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
                // Percent-encode everything else (including UTF-8 multibyte)
                for byte in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
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
    client: &reqwest::Client,
    api_key: &str,
    media_type: &str, // "movie" or "tv"
    tmdb_id: i64,
) -> Vec<(String, String)> {
    let url = format!(
        "{}/{}/{}/videos?api_key={}",
        TMDB_BASE, media_type, tmdb_id, api_key
    );
    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(videos) = resp.json::<TmdbVideos>().await
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
    client: &reqwest::Client,
    api_key: &str,
    media_type: &str, // "movie" or "tv"
    tmdb_id: i64,
) -> Vec<(i64, String)> {
    let url = format!(
        "{}/{}/{}/similar?api_key={}",
        TMDB_BASE, media_type, tmdb_id, api_key
    );
    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(similar) = resp.json::<TmdbSimilarResult>().await
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
