//! TMDb client for Rustyfin. The implementation lives in `plugin-tmdb`; this module re-exports it
//! and keeps the trailer and similar-title helpers.

pub use plugin_tmdb::tmdb_client::{
    DEFAULT_API_BASE_URL, DEFAULT_IMAGE_BASE_URL, ENV_API_KEY, ENV_BASE_URL, ENV_IMAGE_BASE_URL,
    TmdbApi, TmdbClient, TmdbError, TmdbErrorKind, TmdbKey, TmdbSettings, USER_AGENT,
};

// ── Trailer and Similar helpers ──

use serde::Deserialize;

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

#[derive(Debug, Deserialize)]
struct TmdbSimilarResult {
    results: Vec<TmdbSimilarItem>,
}

#[derive(Debug, Deserialize)]
struct TmdbSimilarItem {
    id: i64,
    title: Option<String>,
    name: Option<String>,
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
        result.truncate(20);
    }
    result
}
