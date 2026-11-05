//! The shared TMDb client: key handling, a rate limiter, retries and a cache.
//!
//! The API key is private to this module. Errors, logs and Debug output carry the endpoint
//! path only, never the URL, the query string or the key.

use std::error::Error as _;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::Url;
use tokio::sync::Semaphore;
use tokio::time::{Instant, sleep, sleep_until};

pub const DEFAULT_API_BASE_URL: &str = "https://api.themoviedb.org/3";
pub const DEFAULT_IMAGE_BASE_URL: &str = "https://image.tmdb.org/t/p/original";
pub const ENV_API_KEY: &str = "RUSTYFIN_TMDB_API_KEY";
pub const ENV_BASE_URL: &str = "RUSTYFIN_TMDB_BASE_URL";
pub const ENV_IMAGE_BASE_URL: &str = "RUSTYFIN_TMDB_IMAGE_BASE_URL";
pub const USER_AGENT: &str = concat!("Rustyfin/", env!("CARGO_PKG_VERSION"));

const CACHE_CAPACITY: u64 = 10_000;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REDACTED: &str = "<redacted>";

/// A TMDb v3 API key or v4 read access token. The value is private to this module.
#[derive(Clone)]
pub struct TmdbKey(String);

impl TmdbKey {
    /// Trims `raw`. `None` when nothing is left.
    pub fn new(raw: &str) -> Option<TmdbKey> {
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| TmdbKey(trimmed.to_string()))
    }

    /// A v4 read access token: a JWT, so it starts with `eyJ` and has exactly two dots.
    pub fn is_bearer(&self) -> bool {
        self.0.starts_with("eyJ") && self.0.matches('.').count() == 2
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TmdbKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TmdbKey({REDACTED})")
    }
}

/// Settings for a [`TmdbClient`]. `Default` holds the values from the spec; `from_env` reads
/// the `RUSTYFIN_TMDB_*` variables.
#[derive(Clone, Debug)]
pub struct TmdbSettings {
    /// No trailing '/'.
    pub api_base_url: String,
    /// No trailing '/'.
    pub image_base_url: String,
    pub env_key: Option<TmdbKey>,
    /// Spacing between request starts.
    pub min_interval: Duration,
    /// Requests in flight.
    pub max_concurrent: usize,
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Doubles per retry.
    pub initial_backoff: Duration,
    /// Cap on a 429's `Retry-After`.
    pub max_retry_after: Duration,
    /// Per attempt.
    pub request_timeout: Duration,
}

impl Default for TmdbSettings {
    fn default() -> Self {
        TmdbSettings {
            api_base_url: DEFAULT_API_BASE_URL.to_string(),
            image_base_url: DEFAULT_IMAGE_BASE_URL.to_string(),
            env_key: None,
            min_interval: Duration::from_millis(110),
            max_concurrent: 4,
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            max_retry_after: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
        }
    }
}

impl TmdbSettings {
    /// Reads the `RUSTYFIN_TMDB_*` variables. Empty or blank values count as unset.
    pub fn from_env() -> Result<TmdbSettings, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// `from_env` with the variables supplied by `lookup`. Errors name a variable, never its value.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<TmdbSettings, String> {
        let get = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let base_url = |var: &str, default: &str| match get(var) {
            Some(raw) => absolute_base_url(var, &raw),
            None => Ok(default.to_string()),
        };
        Ok(TmdbSettings {
            api_base_url: base_url(ENV_BASE_URL, DEFAULT_API_BASE_URL)?,
            image_base_url: base_url(ENV_IMAGE_BASE_URL, DEFAULT_IMAGE_BASE_URL)?,
            env_key: get(ENV_API_KEY).as_deref().and_then(TmdbKey::new),
            ..TmdbSettings::default()
        })
    }
}

/// Trims trailing slashes, then checks the URL is absolute http(s) with a host and no
/// credentials, query or fragment. The error names `var` and never echoes the value.
fn absolute_base_url(var: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim_end_matches('/');
    let valid = Url::parse(trimmed).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    });
    if valid {
        Ok(trimmed.to_string())
    } else {
        Err(format!(
            "{var} must be an absolute http(s) URL without credentials, query or fragment"
        ))
    }
}

/// Spacing and concurrency shared by every clone and handle of one client. No attempt starts
/// during a pause.
struct Limiter {
    permits: Semaphore,
    min_interval: Duration,
    /// Locked only inside `reserve`, `pause` and `is_paused`, never across an await.
    state: Mutex<State>,
}

/// What `Limiter::state` guards.
struct State {
    /// Earliest start for the next attempt.
    next_slot: Instant,
    /// Attempts may not start before this instant. Moved only by `pause`, and only forward.
    paused_until: Instant,
}

impl Limiter {
    fn new(max_concurrent: usize, min_interval: Duration) -> Limiter {
        let now = Instant::now();
        Limiter {
            permits: Semaphore::new(max_concurrent.max(1)),
            min_interval,
            state: Mutex::new(State {
                next_slot: now,
                paused_until: now,
            }),
        }
    }

    /// Takes the next start slot, `max(now, next_slot)`. The slot after it is `min_interval` later.
    /// The slot may fall inside a pause. `attempt` then takes a new one.
    fn reserve(&self) -> Instant {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let start = state.next_slot.max(Instant::now());
        state.next_slot = start + self.min_interval;
        start
    }

    /// A 429 pauses every request until `wait` from now. Slots taken from now on come after the
    /// pause. Slots already taken are checked again when they arrive, in `attempt`.
    fn pause(&self, wait: Duration) {
        let until = Instant::now() + wait;
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.next_slot = state.next_slot.max(until);
        state.paused_until = state.paused_until.max(until);
    }

    /// True while a pause runs at this moment.
    fn is_paused(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Instant::now() < state.paused_until
    }
}

struct Shared {
    http: reqwest::Client,
    settings: TmdbSettings,
    limiter: Limiter,
    cache: moka::future::Cache<String, serde_json::Value>,
}

/// One per process. Clones share the limiter and cache.
#[derive(Clone)]
pub struct TmdbClient(Arc<Shared>);

impl TmdbClient {
    pub fn new(http: reqwest::Client, settings: TmdbSettings) -> TmdbClient {
        let limiter = Limiter::new(settings.max_concurrent, settings.min_interval);
        let cache = moka::future::Cache::builder()
            .max_capacity(CACHE_CAPACITY)
            .time_to_live(CACHE_TTL)
            .build();
        TmdbClient(Arc::new(Shared {
            http,
            settings,
            limiter,
            cache,
        }))
    }

    /// The env key if set, else the trimmed non-empty config key, else `None` (TMDb off).
    pub fn api(&self, config_key: Option<&str>) -> Option<TmdbApi> {
        let key = self
            .0
            .settings
            .env_key
            .clone()
            .or_else(|| config_key.and_then(TmdbKey::new))?;
        Some(TmdbApi {
            client: self.clone(),
            key: Arc::new(key),
        })
    }

    pub fn api_base_url(&self) -> &str {
        &self.0.settings.api_base_url
    }

    pub fn image_base_url(&self) -> &str {
        &self.0.settings.image_base_url
    }

    pub fn has_env_key(&self) -> bool {
        self.0.settings.env_key.is_some()
    }
}

impl fmt::Debug for TmdbClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = &self.0.settings;
        f.debug_struct("TmdbClient")
            .field("api_base_url", &s.api_base_url)
            .field("image_base_url", &s.image_base_url)
            .field("has_env_key", &s.env_key.is_some())
            .field("min_interval", &s.min_interval)
            .field("max_concurrent", &s.max_concurrent)
            .field("max_retries", &s.max_retries)
            .field("initial_backoff", &s.initial_backoff)
            .field("max_retry_after", &s.max_retry_after)
            .field("request_timeout", &s.request_timeout)
            .finish()
    }
}

/// A client plus the key to use. Cheap to clone.
#[derive(Clone)]
pub struct TmdbApi {
    client: TmdbClient,
    key: Arc<TmdbKey>,
}

impl TmdbApi {
    /// GET {api_base_url}{path}?{query}. `path` starts with '/', e.g. "/movie/603/videos".
    /// Query values are raw; this function encodes them. Adds the key, the User-Agent, the
    /// limiter, retries and the cache.
    pub async fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value, TmdbError> {
        let shared: &Shared = &self.client.0;
        let mut url = match Url::parse(&format!("{}{path}", shared.settings.api_base_url)) {
            Ok(url) => url,
            Err(_) => {
                return Err(give_up(TmdbError {
                    endpoint: path.to_string(),
                    kind: TmdbErrorKind::Network("invalid request URL".to_string()),
                    attempts: 1,
                }));
            }
        };
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for &(name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        // The cache key is the path and the encoded query. The key is added below, after this.
        let cache_key = match url.query() {
            Some(encoded) => format!("{path}?{encoded}"),
            None => path.to_string(),
        };
        if let Some(value) = shared.cache.get(&cache_key).await {
            return Ok(value);
        }
        if !self.key.is_bearer() {
            url.query_pairs_mut()
                .append_pair("api_key", self.key.expose());
        }
        let value = self.send_with_retries(path, &url).await?;
        shared.cache.insert(cache_key, value.clone()).await;
        Ok(value)
    }

    /// `image_base_url` + `file_path` (TMDb file paths start with '/').
    pub fn image_url(&self, file_path: &str) -> String {
        format!("{}{file_path}", self.client.0.settings.image_base_url)
    }

    /// Runs attempts until one succeeds, the error is not retryable, or the retries run out.
    async fn send_with_retries(
        &self,
        path: &str,
        url: &Url,
    ) -> Result<serde_json::Value, TmdbError> {
        let shared: &Shared = &self.client.0;
        let settings = &shared.settings;
        let mut attempt: u32 = 1;
        loop {
            let failure = match self.attempt(url).await {
                Ok(value) => return Ok(value),
                Err(failure) => failure,
            };
            // The permit is already released here, so backoff never holds it.
            let wait = failure
                .retry_after
                .unwrap_or_else(|| backoff(settings.initial_backoff, attempt));
            if failure.is_rate_limited() {
                shared.limiter.pause(wait);
            }
            if !failure.retryable || attempt > settings.max_retries {
                return Err(give_up(TmdbError {
                    endpoint: path.to_string(),
                    kind: failure.kind,
                    attempts: attempt,
                }));
            }
            tracing::debug!(
                endpoint = path,
                attempt,
                wait = ?wait,
                reason = %describe(&failure.kind),
                "retrying TMDb request"
            );
            sleep(wait).await;
            attempt += 1;
        }
    }

    /// One HTTP attempt: permit, then start slot, then send and read the body. The permit is
    /// released when this returns. If a pause runs when the slot arrives, the attempt takes a
    /// new slot and waits again.
    async fn attempt(&self, url: &Url) -> Result<serde_json::Value, Failure> {
        let shared: &Shared = &self.client.0;
        let _permit = shared
            .limiter
            .permits
            .acquire()
            .await
            .expect("the limiter semaphore is never closed");
        loop {
            sleep_until(shared.limiter.reserve()).await;
            if !shared.limiter.is_paused() {
                break;
            }
        }

        let mut request = shared
            .http
            .get(url.clone())
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(shared.settings.request_timeout);
        if self.key.is_bearer() {
            request = request.bearer_auth(self.key.expose());
        }
        let response = request
            .send()
            .await
            .map_err(|err| transport_failure(err, self.key.expose()))?;

        let status = response.status();
        if status.is_success() {
            let body = response
                .bytes()
                .await
                .map_err(|err| transport_failure(err, self.key.expose()))?;
            return serde_json::from_slice(&body).map_err(|_| Failure {
                kind: TmdbErrorKind::Decode,
                retryable: false,
                retry_after: None,
            });
        }
        let rate_limited = status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        let retry_after = if rate_limited {
            parse_retry_after(response.headers())
                .map(|wait| wait.min(shared.settings.max_retry_after))
        } else {
            None
        };
        Err(Failure {
            kind: TmdbErrorKind::Status(status.as_u16()),
            retryable: rate_limited || status.is_server_error(),
            retry_after,
        })
    }
}

impl fmt::Debug for TmdbApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TmdbApi")
            .field("client", &self.client)
            .field("key", &self.key)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmdbErrorKind {
    /// The final non-2xx status.
    Status(u16),
    /// The attempt exceeded `request_timeout`.
    Timeout,
    /// Connect or body error text, without the URL, with the key replaced.
    Network(String),
    /// A 2xx body that is not JSON.
    Decode,
}

/// A failed request. `endpoint` is the path only.
#[derive(Debug, Clone)]
pub struct TmdbError {
    pub endpoint: String,
    pub kind: TmdbErrorKind,
    pub attempts: u32,
}

impl TmdbError {
    pub fn status(&self) -> Option<u16> {
        match self.kind {
            TmdbErrorKind::Status(code) => Some(code),
            _ => None,
        }
    }

    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }
}

impl fmt::Display for TmdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = if self.attempts == 1 {
            "attempt"
        } else {
            "attempts"
        };
        write!(
            f,
            "TMDb GET {} failed after {} {noun}: {}",
            self.endpoint,
            self.attempts,
            describe(&self.kind)
        )
    }
}

impl std::error::Error for TmdbError {}

fn describe(kind: &TmdbErrorKind) -> String {
    match kind {
        TmdbErrorKind::Status(code) => format!("HTTP {code}"),
        TmdbErrorKind::Timeout => "timed out".to_string(),
        TmdbErrorKind::Network(message) => format!("network error: {message}"),
        TmdbErrorKind::Decode => "invalid JSON".to_string(),
    }
}

/// Logs a final failure once: a 404 at debug, anything else at warn.
fn give_up(err: TmdbError) -> TmdbError {
    if err.is_not_found() {
        tracing::debug!("{err}");
    } else {
        tracing::warn!("{err}");
    }
    err
}

/// One failed attempt, before the retry decision.
struct Failure {
    kind: TmdbErrorKind,
    retryable: bool,
    /// A 429's `Retry-After`, integer seconds only, capped at `max_retry_after`.
    retry_after: Option<Duration>,
}

impl Failure {
    fn is_rate_limited(&self) -> bool {
        self.kind == TmdbErrorKind::Status(429)
    }
}

/// Wait before retry `retry` (1-based): `initial_backoff × 2^(retry-1)`.
fn backoff(initial: Duration, retry: u32) -> Duration {
    let factor = 2u32
        .checked_pow(retry.saturating_sub(1))
        .unwrap_or(u32::MAX);
    initial.saturating_mul(factor)
}

/// `Retry-After` as integer seconds. HTTP dates and anything else give `None`.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Classifies a reqwest error. The message is built without the URL, with `secret` replaced.
fn transport_failure(err: reqwest::Error, secret: &str) -> Failure {
    if err.is_timeout() {
        return Failure {
            kind: TmdbErrorKind::Timeout,
            retryable: true,
            retry_after: None,
        };
    }
    let retryable = err.is_connect() || err.is_body();
    let err = err.without_url();
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    let message = parts.join(": ").replace(secret, REDACTED);
    Failure {
        kind: TmdbErrorKind::Network(message),
        retryable,
        retry_after: None,
    }
}
