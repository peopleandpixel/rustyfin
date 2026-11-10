//! Minimal TMDb API client for the plugin-tmdb crate.
//!
//! Provides `TmdbApi` which wraps a `reqwest::Client` with API key handling
//! and image URL construction.

use reqwest::Client as HttpClient;
use std::sync::Arc;

/// TMDb API base URL
pub const DEFAULT_API_BASE_URL: &str = "https://api.themoviedb.org/3";
/// TMDb image base URL
pub const DEFAULT_IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p/original";
/// Environment variable for the API key
pub const ENV_API_KEY: &str = "RUSTYFIN_TMDB_API_KEY";

/// Error type for TMDb API requests
#[derive(Debug)]
pub struct TmdbError {
    pub endpoint: String,
    pub kind: TmdbErrorKind,
}

impl std::fmt::Display for TmdbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TMDb error for {}: {:?}", self.endpoint, self.kind)
    }
}

impl std::error::Error for TmdbError {}

#[derive(Debug)]
pub enum TmdbErrorKind {
    Network(String),
    Http(u16, String),
    Parse(String),
}

/// A simple TMDb API client.
#[derive(Clone)]
pub struct TmdbApi {
    http: HttpClient,
    api_base_url: Arc<String>,
    image_base_url: Arc<String>,
    api_key: Arc<String>,
}

impl TmdbApi {
    /// Create a new TMDb API client with the given API key.
    pub fn new(api_key: &str) -> Self {
        Self {
            http: HttpClient::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| HttpClient::new()),
            api_base_url: Arc::new(DEFAULT_API_BASE_URL.to_string()),
            image_base_url: Arc::new(DEFAULT_IMAGE_BASE_URL.to_string()),
            api_key: Arc::new(api_key.trim().to_string()),
        }
    }

    /// Create a new TMDb API client, reading the key from the environment.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var(ENV_API_KEY).ok()?;
        (!key.trim().is_empty()).then(|| Self::new(&key))
    }

    /// GET {api_base_url}{path}?{query}. `path` starts with '/'.
    /// Query values are raw; this function encodes them. Adds the API key.
    pub async fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value, TmdbError> {
        let mut url =
            reqwest::Url::parse(&format!("{}{}", self.api_base_url, path)).map_err(|e| {
                TmdbError {
                    endpoint: path.to_string(),
                    kind: TmdbErrorKind::Network(e.to_string()),
                }
            })?;

        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
            pairs.append_pair("api_key", &self.api_key);
        }

        let resp = self
            .http
            .get(url.clone())
            .header("User-Agent", "Rustyfin")
            .send()
            .await
            .map_err(|e| TmdbError {
                endpoint: path.to_string(),
                kind: TmdbErrorKind::Network(e.to_string()),
            })?;

        let status = resp.status();
        let body = resp.text().await.map_err(|e| TmdbError {
            endpoint: path.to_string(),
            kind: TmdbErrorKind::Network(e.to_string()),
        })?;

        if !status.is_success() {
            return Err(TmdbError {
                endpoint: path.to_string(),
                kind: TmdbErrorKind::Http(status.as_u16(), body),
            });
        }

        serde_json::from_str(&body).map_err(|e| TmdbError {
            endpoint: path.to_string(),
            kind: TmdbErrorKind::Parse(e.to_string()),
        })
    }

    /// `image_base_url` + `file_path` (TMDb file paths start with '/').
    pub fn image_url(&self, file_path: &str) -> String {
        format!("{}{}", self.image_base_url, file_path)
    }
}
