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
/// Environment variable overriding the API base URL
pub const ENV_BASE_URL: &str = "RUSTYFIN_TMDB_BASE_URL";
/// Environment variable overriding the image base URL
pub const ENV_IMAGE_BASE_URL: &str = "RUSTYFIN_TMDB_IMAGE_BASE_URL";

/// Trims `raw` and any trailing '/'; returns it if it is an absolute http(s) URL with a host, else `default`.
fn base_url_from(raw: Option<&str>, default: &str) -> String {
    let trimmed = raw.unwrap_or("").trim().trim_end_matches('/');
    match reqwest::Url::parse(trimmed) {
        Ok(url)
            if matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some_and(|h| !h.is_empty()) =>
        {
            trimmed.to_string()
        }
        _ => default.to_string(),
    }
}

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
    /// Create a new TMDb API client with the given API key. The API and image
    /// base URLs come from `ENV_BASE_URL` and `ENV_IMAGE_BASE_URL` when set and valid.
    pub fn new(api_key: &str) -> Self {
        Self {
            http: HttpClient::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| HttpClient::new()),
            api_base_url: Arc::new(base_url_from(
                std::env::var(ENV_BASE_URL).ok().as_deref(),
                DEFAULT_API_BASE_URL,
            )),
            image_base_url: Arc::new(base_url_from(
                std::env::var(ENV_IMAGE_BASE_URL).ok().as_deref(),
                DEFAULT_IMAGE_BASE_URL,
            )),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_from_unset_or_blank_is_default() {
        assert_eq!(
            base_url_from(None, DEFAULT_API_BASE_URL),
            DEFAULT_API_BASE_URL
        );
        assert_eq!(
            base_url_from(Some("   "), DEFAULT_IMAGE_BASE_URL),
            DEFAULT_IMAGE_BASE_URL
        );
    }

    #[test]
    fn base_url_from_trims_whitespace_and_trailing_slashes() {
        assert_eq!(
            base_url_from(Some(" http://127.0.0.1:8193/3/ \n"), DEFAULT_API_BASE_URL),
            "http://127.0.0.1:8193/3"
        );
        assert_eq!(
            base_url_from(
                Some("http://127.0.0.1:8193/t/p/original//"),
                DEFAULT_IMAGE_BASE_URL
            ),
            "http://127.0.0.1:8193/t/p/original"
        );
    }

    #[test]
    fn base_url_from_rejects_non_http_urls() {
        for raw in ["not a url", "ftp://example.com/3", "/3", "http://"] {
            assert_eq!(
                base_url_from(Some(raw), DEFAULT_API_BASE_URL),
                DEFAULT_API_BASE_URL,
                "{raw}"
            );
        }
    }

    #[test]
    fn base_url_from_env_var_names_match_core() {
        assert_eq!(ENV_BASE_URL, "RUSTYFIN_TMDB_BASE_URL");
        assert_eq!(ENV_IMAGE_BASE_URL, "RUSTYFIN_TMDB_IMAGE_BASE_URL");
    }
}
