use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use serde::Deserialize;

const TMDB_BASE: &str = "https://api.themoviedb.org/3";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

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
}

#[derive(Debug, Deserialize)]
struct TmdbImages {
    posters: Option<Vec<TmdbImage>>,
    backdrops: Option<Vec<TmdbImage>>,
    logos: Option<Vec<TmdbImage>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TmdbImage {
    file_path: String,
    width: Option<i32>,
    height: Option<i32>,
}

/// Search TMDb for a movie and return metadata
pub async fn search_movie(api_key: &str, name: &str, year: Option<i32>) -> Option<ExternalMetadata> {
    let client = reqwest::Client::new();
    let mut url = format!("{}/search/movie?api_key={}&query={}", TMDB_BASE, api_key, urlencoded(name));
    if let Some(y) = year {
        url.push_str(&format!("&year={}", y));
    }

    let resp: TmdbSearchResult = client.get(&url).send().await.ok()?.json().await.ok()?;
    let item = resp.results.into_iter().next()?;

    let images = fetch_images(&client, api_key, "movie", item.id).await;

    let production_year = item.release_date.as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    Some(ExternalMetadata {
        name: item.title.or(item.name),
        overview: item.overview,
        production_year,
        community_rating: item.vote_average,
        official_rating: None,
        genres: None,
        premiere_date: item.release_date,
        runtime_ticks: None,
        image_urls: images,
    })
}

/// Search TMDb for a TV show and return metadata
pub async fn search_tv(api_key: &str, name: &str) -> Option<ExternalMetadata> {
    let client = reqwest::Client::new();
    let url = format!("{}/search/tv?api_key={}&query={}", TMDB_BASE, api_key, urlencoded(name));

    let resp: TmdbSearchResult = client.get(&url).send().await.ok()?.json().await.ok()?;
    let item = resp.results.into_iter().next()?;

    let images = fetch_images(&client, api_key, "tv", item.id).await;

    let production_year = item.first_air_date.as_deref()
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse().ok());

    Some(ExternalMetadata {
        name: item.name.or(item.title),
        overview: item.overview,
        production_year,
        community_rating: item.vote_average,
        official_rating: None,
        genres: None,
        premiere_date: item.first_air_date,
        runtime_ticks: None,
        image_urls: images,
    })
}

async fn fetch_images(client: &reqwest::Client, api_key: &str, media_type: &str, id: i64) -> Vec<MetadataImage> {
    let url = format!("{}/{}/{}/images?api_key={}", TMDB_BASE, media_type, id, api_key);
    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await {
        if let Ok(imgs) = resp.json::<TmdbImages>().await {
            if let Some(posters) = imgs.posters {
                for (i, p) in posters.into_iter().take(3).enumerate() {
                    result.push(MetadataImage {
                        image_type: if i == 0 { "Primary".to_string() } else { format!("Primary_{}", i) },
                        url: format!("{}{}", TMDB_IMAGE_BASE, p.file_path),
                    });
                }
            }
            if let Some(backdrops) = imgs.backdrops {
                for (i, b) in backdrops.into_iter().take(3).enumerate() {
                    result.push(MetadataImage {
                        image_type: if i == 0 { "Backdrop".to_string() } else { format!("Backdrop_{}", i) },
                        url: format!("{}{}", TMDB_IMAGE_BASE, b.file_path),
                    });
                }
            }
            if let Some(logos) = imgs.logos {
                if let Some(l) = logos.into_iter().next() {
                    result.push(MetadataImage {
                        image_type: "Logo".to_string(),
                        url: format!("{}{}", TMDB_IMAGE_BASE, l.file_path),
                    });
                }
            }
        }
    }

    result
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('?', "%3F")
        .replace('#', "%23")
}
