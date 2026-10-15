use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use serde::Deserialize;

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

/// Search OMDb for a title and return metadata
pub async fn search(api_key: &str, name: &str, year: Option<i32>) -> Option<ExternalMetadata> {
    let client = reqwest::Client::new();
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
        .and_then(|y| y.split('–').next()) // handle ranges like "2019–2023"
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
        overview: resp.plot,
        production_year,
        community_rating,
        official_rating: resp.rated,
        genres,
        premiere_date: None,
        runtime_ticks,
        image_urls: images,
        tmdb_id: None,
    })
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('?', "%3F")
        .replace('#', "%23")
}
