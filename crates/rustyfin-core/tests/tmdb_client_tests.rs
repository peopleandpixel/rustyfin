//! The shared TMDb client: key handling, redaction, rate limiting, retries and caching.
//!
//! Every test talks to a stub TMDb served by axum on 127.0.0.1:0, never to the real one.
//! The keys below are fake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rustyfin_core::tmdb_client::{
    DEFAULT_API_BASE_URL, DEFAULT_IMAGE_BASE_URL, ENV_API_KEY, ENV_BASE_URL, ENV_IMAGE_BASE_URL,
    TmdbApi, TmdbClient, TmdbErrorKind, TmdbKey, TmdbSettings, USER_AGENT,
};
use serde_json::json;

const KEY: &str = "g12-unit-fake-key-5555";
/// Shaped like a TMDb v4 read access token (a JWT). Fake.
const BEARER: &str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiJnMTItdW5pdC1mYWtlIn0.g12-fake-signature";
const CONFIG_KEY: &str = "g12-config-fake-key-6666";
/// Slack for timer and scheduling jitter when comparing arrival times.
const JITTER: Duration = Duration::from_millis(30);

// ── Stub TMDb ──

/// One request as the stub saw it.
#[derive(Clone, Debug)]
struct Hit {
    at: Instant,
    /// Path below `/3`, e.g. `/movie/1`.
    path: String,
    query: String,
    authorization: Option<String>,
    user_agent: Option<String>,
}

enum Reply {
    Json(serde_json::Value),
    Status(u16),
    /// 429 with this `Retry-After` value.
    RateLimited(&'static str),
    /// 200 after this delay.
    Slow(Duration),
    /// 200 with a body that is not JSON.
    NotJson,
}

/// Picks the reply from the path and how many earlier requests had that path.
type Script = Arc<dyn Fn(&str, usize) -> Reply + Send + Sync>;

#[derive(Clone)]
struct StubState {
    hits: Arc<Mutex<Vec<Hit>>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    /// Added before every reply.
    delay: Duration,
    script: Script,
}

struct Stub {
    origin: String,
    state: StubState,
}

impl Stub {
    fn hits(&self) -> Vec<Hit> {
        self.state.hits.lock().unwrap().clone()
    }

    fn hits_for(&self, path: &str) -> Vec<Hit> {
        self.hits().into_iter().filter(|h| h.path == path).collect()
    }

    fn max_in_flight(&self) -> usize {
        self.state.max_in_flight.load(Ordering::SeqCst)
    }

    /// Default limits, pointed at this stub, with the env key set.
    fn settings(&self) -> TmdbSettings {
        TmdbSettings {
            api_base_url: format!("{}/3", self.origin),
            image_base_url: format!("{}/t/p/original", self.origin),
            env_key: TmdbKey::new(KEY),
            ..TmdbSettings::default()
        }
    }

    /// Like `settings`, with short waits for tests that don't measure them.
    fn fast_settings(&self) -> TmdbSettings {
        TmdbSettings {
            min_interval: Duration::from_millis(1),
            initial_backoff: Duration::from_millis(10),
            ..self.settings()
        }
    }
}

struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn handle(State(stub): State<StubState>, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri
        .path()
        .strip_prefix("/3")
        .unwrap_or(uri.path())
        .to_string();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let seen = {
        let mut hits = stub.hits.lock().unwrap();
        let seen = hits.iter().filter(|h| h.path == path).count();
        hits.push(Hit {
            at: Instant::now(),
            path: path.clone(),
            query: uri.query().unwrap_or("").to_string(),
            authorization: header("authorization"),
            user_agent: header("user-agent"),
        });
        seen
    };
    let now = stub.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    stub.max_in_flight.fetch_max(now, Ordering::SeqCst);
    let _in_flight = InFlight(stub.in_flight.clone());
    let reply = (stub.script)(&path, seen);
    tokio::time::sleep(stub.delay).await;
    match reply {
        Reply::Json(body) => axum::Json(body).into_response(),
        Reply::Status(code) => {
            let status = StatusCode::from_u16(code).unwrap();
            let body = json!({ "success": false, "status_code": code });
            (status, axum::Json(body)).into_response()
        }
        Reply::RateLimited(retry_after) => {
            let body = json!({ "success": false, "status_code": 25 });
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
            resp.headers_mut()
                .insert("retry-after", HeaderValue::from_static(retry_after));
            resp
        }
        Reply::Slow(wait) => {
            tokio::time::sleep(wait).await;
            axum::Json(json!({ "path": path })).into_response()
        }
        Reply::NotJson => (StatusCode::OK, "<html>not json</html>").into_response(),
    }
}

async fn start_stub(
    delay: Duration,
    script: impl Fn(&str, usize) -> Reply + Send + Sync + 'static,
) -> Stub {
    let state = StubState {
        hits: Arc::default(),
        in_flight: Arc::default(),
        max_in_flight: Arc::default(),
        delay,
        script: Arc::new(script),
    };
    let app = axum::Router::new()
        .fallback(handle)
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Stub { origin, state }
}

fn ok(path: &str) -> Reply {
    Reply::Json(json!({ "path": path }))
}

/// A stub that answers every request with 200 `{"path": ...}`.
async fn ok_stub() -> Stub {
    start_stub(Duration::ZERO, |path, _| ok(path)).await
}

// ── Helpers ──

/// A plain HTTP client: no User-Agent, and no proxy even when `HTTP_PROXY` is set.
fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |name| vars.get(name).cloned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Sends every `(handle, path)` request at the same time; fails the test on any error.
async fn get_all(requests: Vec<(TmdbApi, String)>) {
    let mut tasks = tokio::task::JoinSet::new();
    for (api, path) in requests {
        tasks.spawn(async move { api.get_json(&path, &[]).await.map(|_| ()) });
    }
    while let Some(done) = tasks.join_next().await {
        done.unwrap().unwrap();
    }
}

fn paths(api: &TmdbApi, ids: std::ops::Range<u32>) -> Vec<(TmdbApi, String)> {
    ids.map(|id| (api.clone(), format!("/movie/{id}")))
        .collect()
}

fn by_arrival(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by_key(|h| h.at);
    hits
}

fn gaps(hits: &[Hit]) -> Vec<Duration> {
    hits.windows(2)
        .map(|w| w[1].at.duration_since(w[0].at))
        .collect()
}

/// The most hits that arrived within any `window`.
fn max_in_window(hits: &[Hit], window: Duration) -> usize {
    let hits = by_arrival(hits.to_vec());
    let mut best = 0;
    let mut start = 0;
    for end in 0..hits.len() {
        while hits[end].at.duration_since(hits[start].at) >= window {
            start += 1;
        }
        best = best.max(end - start + 1);
    }
    best
}

fn assert_spaced(hits: &[Hit], interval: Duration) {
    for gap in gaps(&by_arrival(hits.to_vec())) {
        assert!(
            gap + JITTER >= interval,
            "requests arrived {gap:?} apart, want at least {interval:?}"
        );
    }
}

/// A local port with nothing listening on it.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn wait_until(cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for the stub");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Everything any test in this binary logs from now on, at every level, `log` records included.
fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = buf.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || LogWriter(sink.clone()))
            .try_init()
            .expect("no other global subscriber in this test binary");
        buf
    })
    .clone()
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ── Settings and keys ──

#[test]
fn defaults_match_the_spec() {
    assert_eq!(DEFAULT_API_BASE_URL, "https://api.themoviedb.org/3");
    assert_eq!(
        DEFAULT_IMAGE_BASE_URL,
        "https://image.tmdb.org/t/p/original"
    );
    assert_eq!(ENV_API_KEY, "RUSTYFIN_TMDB_API_KEY");
    assert_eq!(ENV_BASE_URL, "RUSTYFIN_TMDB_BASE_URL");
    assert_eq!(ENV_IMAGE_BASE_URL, "RUSTYFIN_TMDB_IMAGE_BASE_URL");
    let s = TmdbSettings::default();
    assert_eq!(s.api_base_url, DEFAULT_API_BASE_URL);
    assert_eq!(s.image_base_url, DEFAULT_IMAGE_BASE_URL);
    assert!(s.env_key.is_none());
    assert_eq!(s.min_interval, Duration::from_millis(110));
    assert_eq!(s.max_concurrent, 4);
    assert_eq!(s.max_retries, 3);
    assert_eq!(s.initial_backoff, Duration::from_millis(500));
    assert_eq!(s.max_retry_after, Duration::from_secs(10));
    assert_eq!(s.request_timeout, Duration::from_secs(10));
}

#[test]
fn settings_come_from_the_env_names() {
    let s = TmdbSettings::from_lookup(lookup(&[])).unwrap();
    assert_eq!(s.api_base_url, DEFAULT_API_BASE_URL);
    assert_eq!(s.image_base_url, DEFAULT_IMAGE_BASE_URL);
    assert!(s.env_key.is_none());

    let padded = format!("  {KEY}\n");
    let s = TmdbSettings::from_lookup(lookup(&[
        (ENV_API_KEY, padded.as_str()),
        (ENV_BASE_URL, "http://127.0.0.1:8110/3/"),
        (
            ENV_IMAGE_BASE_URL,
            " https://images.example.test/t/p/original// ",
        ),
    ]))
    .unwrap();
    assert!(s.env_key.is_some());
    assert_eq!(s.api_base_url, "http://127.0.0.1:8110/3");
    assert_eq!(s.image_base_url, "https://images.example.test/t/p/original");
    // Only the key and the base URLs come from the environment.
    assert_eq!(s.min_interval, TmdbSettings::default().min_interval);

    // Empty or blank values count as unset.
    let s = TmdbSettings::from_lookup(lookup(&[
        (ENV_API_KEY, "  "),
        (ENV_BASE_URL, ""),
        (ENV_IMAGE_BASE_URL, " "),
    ]))
    .unwrap();
    assert!(s.env_key.is_none());
    assert_eq!(s.api_base_url, DEFAULT_API_BASE_URL);
    assert_eq!(s.image_base_url, DEFAULT_IMAGE_BASE_URL);
}

#[test]
fn bad_base_urls_are_rejected_without_echoing_them() {
    for (var, value) in [
        (ENV_BASE_URL, "ftp://g12-host.example.test/3"),
        (ENV_BASE_URL, "not a url g12-bad-value"),
        (ENV_BASE_URL, "/g12-relative/3"),
        (ENV_BASE_URL, "https://user:g12-secret@tmdb.example.test/3"),
        (
            ENV_BASE_URL,
            "https://tmdb.example.test/3?api_key=g12-secret",
        ),
        (ENV_IMAGE_BASE_URL, "file:///g12-etc/passwd"),
        (
            ENV_IMAGE_BASE_URL,
            "https://img.example.test/t/p/original#g12-fragment",
        ),
    ] {
        let err = TmdbSettings::from_lookup(lookup(&[(var, value)])).expect_err(value);
        assert!(err.contains(var), "the error should name {var}: {err}");
        assert!(
            !err.contains("g12-"),
            "the error must not echo the value: {err}"
        );
    }
}

#[test]
fn keys_are_trimmed_and_blank_means_none() {
    assert!(TmdbKey::new("").is_none());
    assert!(TmdbKey::new(" \t\n").is_none());
    assert!(TmdbKey::new(" abc ").is_some());
    assert!(!TmdbKey::new(KEY).unwrap().is_bearer());
    assert!(
        !TmdbKey::new("0123456789abcdef0123456789abcdef")
            .unwrap()
            .is_bearer()
    );
    assert!(TmdbKey::new(BEARER).unwrap().is_bearer());
    assert!(!TmdbKey::new("eyJonly.onedot").unwrap().is_bearer());
}

#[tokio::test]
async fn debug_output_never_contains_the_key() {
    for key in [KEY, BEARER] {
        let settings = TmdbSettings {
            env_key: TmdbKey::new(key),
            ..TmdbSettings::default()
        };
        let client = TmdbClient::new(http(), settings.clone());
        let api = client.api(None).unwrap();
        for text in [
            format!("{:?}", TmdbKey::new(key).unwrap()),
            format!("{settings:?}"),
            format!("{settings:#?}"),
            format!("{client:?}"),
            format!("{api:?}"),
            format!("{api:#?}"),
        ] {
            assert!(!text.contains(key), "Debug shows the key: {text}");
        }
    }
    let client = TmdbClient::new(http(), TmdbSettings::default());
    let api = client.api(Some(CONFIG_KEY)).unwrap();
    assert!(!format!("{api:?}").contains(CONFIG_KEY));
}

#[tokio::test]
async fn env_key_wins_over_config_key() {
    let stub = ok_stub().await;
    let padded = format!(" {KEY} ");
    let settings = TmdbSettings {
        env_key: TmdbKey::new(&padded),
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    assert!(client.has_env_key());
    assert!(client.api(None).is_some(), "an env key alone enables TMDb");
    let api = client.api(Some(CONFIG_KEY)).unwrap();
    api.get_json("/movie/1", &[]).await.unwrap();
    let hit = &stub.hits_for("/movie/1")[0];
    assert_eq!(query_param(&hit.query, "api_key").as_deref(), Some(KEY));
    assert!(!hit.query.contains(CONFIG_KEY));
}

#[tokio::test]
async fn config_key_is_used_when_no_env_key_is_set() {
    let stub = ok_stub().await;
    let settings = TmdbSettings {
        env_key: None,
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    assert!(!client.has_env_key());
    assert!(client.api(None).is_none());
    assert!(client.api(Some("")).is_none());
    assert!(client.api(Some("   ")).is_none());
    let padded = format!(" {CONFIG_KEY} ");
    let api = client
        .api(Some(&padded))
        .expect("a config key enables TMDb");
    api.get_json("/movie/2", &[]).await.unwrap();
    let hit = &stub.hits_for("/movie/2")[0];
    assert_eq!(
        query_param(&hit.query, "api_key").as_deref(),
        Some(CONFIG_KEY)
    );
}

// ── Requests ──

#[tokio::test]
async fn v3_key_is_sent_as_the_api_key_param() {
    let stub = ok_stub().await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let body = client
        .api(None)
        .unwrap()
        .get_json(
            "/movie/3",
            &[
                ("append_to_response", "credits,external_ids"),
                ("query", "Alpha Film"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(body["path"], "/movie/3");
    let hit = &stub.hits_for("/movie/3")[0];
    assert_eq!(query_param(&hit.query, "api_key").as_deref(), Some(KEY));
    assert!(query_param(&hit.query, "append_to_response").is_some());
    assert!(hit.query.contains("query=Alpha"), "query: {}", hit.query);
    assert!(hit.authorization.is_none());
}

#[tokio::test]
async fn v4_token_is_sent_as_bearer_and_never_in_the_url() {
    let stub = ok_stub().await;
    let settings = TmdbSettings {
        env_key: TmdbKey::new(BEARER),
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    client
        .api(None)
        .unwrap()
        .get_json("/movie/4", &[("language", "en-US")])
        .await
        .unwrap();
    let hit = &stub.hits_for("/movie/4")[0];
    let expected = format!("Bearer {BEARER}");
    assert_eq!(hit.authorization.as_deref(), Some(expected.as_str()));
    assert!(query_param(&hit.query, "api_key").is_none());
    assert!(!hit.query.contains(BEARER));
    assert!(hit.query.contains("language=en-US"), "query: {}", hit.query);
}

#[tokio::test]
async fn user_agent_is_rustyfin_and_the_version() {
    assert_eq!(
        USER_AGENT,
        format!("Rustyfin/{}", env!("CARGO_PKG_VERSION"))
    );
    let stub = ok_stub().await;
    // A plain client sends no User-Agent, and this one sends another; TMDb requests carry ours.
    let plain = TmdbClient::new(http(), stub.fast_settings());
    plain
        .api(None)
        .unwrap()
        .get_json("/movie/5", &[])
        .await
        .unwrap();
    let other = reqwest::Client::builder()
        .user_agent("Other/9")
        .no_proxy()
        .build()
        .unwrap();
    let client = TmdbClient::new(other, stub.fast_settings());
    client
        .api(None)
        .unwrap()
        .get_json("/movie/6", &[])
        .await
        .unwrap();
    let hits = stub.hits();
    assert_eq!(hits.len(), 2);
    for hit in hits {
        assert_eq!(hit.user_agent.as_deref(), Some(USER_AGENT), "{}", hit.path);
    }
}

#[tokio::test]
async fn image_urls_use_the_configured_image_base() {
    let settings = TmdbSettings::from_lookup(lookup(&[
        (ENV_API_KEY, KEY),
        (ENV_IMAGE_BASE_URL, "http://127.0.0.1:9/t/p/original/"),
    ]))
    .unwrap();
    let client = TmdbClient::new(http(), settings);
    assert_eq!(client.api_base_url(), DEFAULT_API_BASE_URL);
    assert_eq!(client.image_base_url(), "http://127.0.0.1:9/t/p/original");
    assert_eq!(
        client.api(None).unwrap().image_url("/abc.jpg"),
        "http://127.0.0.1:9/t/p/original/abc.jpg"
    );
    let default = TmdbClient::new(http(), TmdbSettings::default());
    assert_eq!(
        default.api(Some(CONFIG_KEY)).unwrap().image_url("/abc.jpg"),
        "https://image.tmdb.org/t/p/original/abc.jpg"
    );
}

// ── Limiter ──

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_are_spaced_by_min_interval() {
    let stub = ok_stub().await;
    let settings = stub.settings();
    let interval = settings.min_interval;
    let client = TmdbClient::new(http(), settings);
    get_all(paths(&client.api(None).unwrap(), 100..112)).await;
    let hits = stub.hits();
    assert_eq!(hits.len(), 12);
    assert_spaced(&hits, interval);
    let burst = max_in_window(&hits, Duration::from_secs(1));
    assert!(burst <= 10, "{burst} requests within one second");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn at_most_max_concurrent_requests_are_in_flight() {
    let stub = start_stub(Duration::from_millis(250), |path, _| ok(path)).await;
    let settings = stub.fast_settings();
    assert_eq!(settings.max_concurrent, 4);
    let client = TmdbClient::new(http(), settings);
    get_all(paths(&client.api(None).unwrap(), 200..212)).await;
    assert_eq!(stub.hits().len(), 12);
    let peak = stub.max_in_flight();
    assert!(peak <= 4, "{peak} requests in flight");
    assert!(
        peak >= 2,
        "requests should overlap up to the limit, peak was {peak}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clones_and_handles_share_one_limiter() {
    let stub = ok_stub().await;
    let interval = Duration::from_millis(150);
    let settings = TmdbSettings {
        min_interval: interval,
        ..stub.settings()
    };
    let client = TmdbClient::new(http(), settings);
    let clone = client.clone();
    let mut requests = paths(&client.api(None).unwrap(), 300..303);
    requests.extend(paths(&clone.api(Some(CONFIG_KEY)).unwrap(), 303..306));
    requests.extend(paths(&client.api(None).unwrap(), 306..309));
    get_all(requests).await;
    let hits = stub.hits();
    assert_eq!(hits.len(), 9);
    assert_spaced(&hits, interval);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_retry_takes_its_own_slot() {
    // Each path fails once with 503, then succeeds.
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if seen == 0 {
            Reply::Status(503)
        } else {
            ok(path)
        }
    })
    .await;
    let settings = TmdbSettings {
        initial_backoff: Duration::from_millis(1),
        ..stub.settings()
    };
    let interval = settings.min_interval;
    let client = TmdbClient::new(http(), settings);
    get_all(paths(&client.api(None).unwrap(), 400..404)).await;
    let hits = stub.hits();
    assert_eq!(hits.len(), 8);
    assert_spaced(&hits, interval);
}

// ── Retries ──

#[tokio::test]
async fn rate_limited_requests_wait_for_retry_after() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if seen == 0 {
            Reply::RateLimited("1")
        } else {
            ok(path)
        }
    })
    .await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let body = client
        .api(None)
        .unwrap()
        .get_json("/movie/500", &[])
        .await
        .unwrap();
    assert_eq!(body["path"], "/movie/500");
    let hits = stub.hits_for("/movie/500");
    assert_eq!(hits.len(), 2);
    let gap = hits[1].at.duration_since(hits[0].at);
    assert!(
        gap >= Duration::from_millis(990),
        "retried after {gap:?}; Retry-After was 1 s"
    );
    assert!(gap < Duration::from_secs(3), "retried after {gap:?}");
}

#[tokio::test]
async fn retry_after_is_capped() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if seen == 0 {
            Reply::RateLimited("30")
        } else {
            ok(path)
        }
    })
    .await;
    let settings = TmdbSettings {
        max_retry_after: Duration::from_millis(300),
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    let started = Instant::now();
    client
        .api(None)
        .unwrap()
        .get_json("/movie/501", &[])
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    let hits = stub.hits_for("/movie/501");
    assert_eq!(hits.len(), 2);
    let gap = hits[1].at.duration_since(hits[0].at);
    assert!(
        gap + JITTER >= Duration::from_millis(300),
        "retried after {gap:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limit_pauses_every_request() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if path == "/movie/510" && seen == 0 {
            Reply::RateLimited("1")
        } else {
            ok(path)
        }
    })
    .await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let api = client.api(None).unwrap();
    let limited = tokio::spawn({
        let api = api.clone();
        async move { api.get_json("/movie/510", &[]).await.map(|_| ()) }
    });
    wait_until(|| !stub.hits_for("/movie/510").is_empty()).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    api.get_json("/movie/511", &[]).await.unwrap();
    limited.await.unwrap().unwrap();
    let first = stub.hits_for("/movie/510")[0].at;
    let other = stub.hits_for("/movie/511")[0].at;
    let gap = other.duration_since(first);
    assert!(
        gap >= Duration::from_millis(990),
        "another request went out {gap:?} after a 429 with Retry-After: 1"
    );
}

/// Requests that took their start slot before the 429 came back must wait for the pause too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limit_holds_requests_already_waiting_for_a_slot() {
    let arrivals = Arc::new(AtomicUsize::new(0));
    // The 50 ms reply delay lets every request take its slot before the 429 arrives.
    let stub = start_stub(Duration::from_millis(50), move |path, _| {
        if arrivals.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::RateLimited("1")
        } else {
            ok(path)
        }
    })
    .await;
    let interval = Duration::from_millis(200);
    let settings = TmdbSettings {
        min_interval: interval,
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    get_all(paths(&client.api(None).unwrap(), 520..523)).await;
    let hits = by_arrival(stub.hits());
    let order: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(hits.len(), 4, "three requests and one retry, got {order:?}");
    let first = hits[0].at;
    for hit in &hits[1..] {
        let gap = hit.at.duration_since(first);
        assert!(
            gap >= Duration::from_millis(990),
            "{} went out {gap:?} after a 429 with Retry-After: 1",
            hit.path
        );
    }
    assert_spaced(&hits[1..], interval);
}

#[tokio::test]
async fn server_errors_are_retried_with_backoff() {
    let stub = start_stub(Duration::ZERO, |path, seen| match seen {
        0 => Reply::Status(500),
        1 => Reply::Status(502),
        _ => ok(path),
    })
    .await;
    let settings = stub.settings();
    let backoff = settings.initial_backoff;
    let client = TmdbClient::new(http(), settings);
    let body = client
        .api(None)
        .unwrap()
        .get_json("/movie/520", &[])
        .await
        .unwrap();
    assert_eq!(body["path"], "/movie/520");
    let hits = stub.hits_for("/movie/520");
    assert_eq!(hits.len(), 3);
    let gaps = gaps(&hits);
    let slack = Duration::from_millis(10);
    assert!(
        gaps[0] + slack >= backoff,
        "first retry after {:?}",
        gaps[0]
    );
    assert!(
        gaps[1] + slack >= backoff * 2,
        "second retry after {:?}",
        gaps[1]
    );
    assert!(
        gaps[1] < Duration::from_secs(4),
        "second retry after {:?}",
        gaps[1]
    );
}

#[tokio::test]
async fn timeouts_are_retried() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if seen == 0 {
            Reply::Slow(Duration::from_secs(2))
        } else {
            ok(path)
        }
    })
    .await;
    let settings = TmdbSettings {
        request_timeout: Duration::from_millis(200),
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    let started = Instant::now();
    let body = client
        .api(None)
        .unwrap()
        .get_json("/movie/530", &[])
        .await
        .unwrap();
    assert_eq!(body["path"], "/movie/530");
    assert_eq!(stub.hits_for("/movie/530").len(), 2);
    assert!(started.elapsed() < Duration::from_millis(1500));
}

#[tokio::test]
async fn a_request_that_keeps_timing_out_gives_up() {
    let stub = start_stub(Duration::ZERO, |_, _| Reply::Slow(Duration::from_secs(2))).await;
    let settings = TmdbSettings {
        request_timeout: Duration::from_millis(100),
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    let err = client
        .api(None)
        .unwrap()
        .get_json("/movie/531", &[])
        .await
        .unwrap_err();
    assert_eq!(err.kind, TmdbErrorKind::Timeout);
    assert_eq!(err.attempts, 4);
    assert_eq!(stub.hits_for("/movie/531").len(), 4);
    let text = format!("{err} | {err:?}");
    assert!(text.contains("/movie/531"), "{text}");
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn gives_up_after_three_retries_naming_the_endpoint() {
    let stub = start_stub(Duration::ZERO, |_, _| Reply::Status(500)).await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let err = client
        .api(None)
        .unwrap()
        .get_json("/movie/540/videos", &[("language", "en-US")])
        .await
        .unwrap_err();
    assert_eq!(stub.hits_for("/movie/540/videos").len(), 4);
    assert_eq!(err.attempts, 4);
    assert_eq!(err.kind, TmdbErrorKind::Status(500));
    assert_eq!(err.status(), Some(500));
    assert!(!err.is_not_found());
    assert_eq!(err.endpoint, "/movie/540/videos");
    assert_eq!(
        err.to_string(),
        "TMDb GET /movie/540/videos failed after 4 attempts: HTTP 500"
    );
    for text in [err.to_string(), format!("{err:?}")] {
        assert!(!text.contains(KEY), "{text}");
        assert!(!text.contains("api_key"), "{text}");
        assert!(
            !text.contains("language"),
            "no query string in errors: {text}"
        );
        assert!(!text.contains("127.0.0.1"), "no URL in errors: {text}");
    }
}

#[tokio::test]
async fn network_errors_are_retried_and_redacted() {
    let settings = TmdbSettings {
        api_base_url: format!("http://127.0.0.1:{}/3", closed_port()),
        env_key: TmdbKey::new(KEY),
        min_interval: Duration::from_millis(1),
        initial_backoff: Duration::from_millis(10),
        ..TmdbSettings::default()
    };
    let client = TmdbClient::new(http(), settings);
    let err = client
        .api(None)
        .unwrap()
        .get_json("/movie/550", &[("query", "g12 probe")])
        .await
        .unwrap_err();
    assert!(matches!(err.kind, TmdbErrorKind::Network(_)), "{err:?}");
    assert_eq!(err.attempts, 4);
    let text = format!("{err} | {err:?}");
    assert!(text.contains("/movie/550"), "{text}");
    assert!(!text.contains(KEY), "{text}");
    assert!(!text.contains("api_key"), "{text}");
    assert!(!text.contains("probe"), "no query string in errors: {text}");
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let stub = start_stub(Duration::ZERO, |path, _| match path {
        "/movie/401" => Reply::Status(401),
        "/movie/404" => Reply::Status(404),
        "/movie/422" => Reply::Status(422),
        _ => ok(path),
    })
    .await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let api = client.api(None).unwrap();
    for (code, path) in [
        (401, "/movie/401"),
        (404, "/movie/404"),
        (422, "/movie/422"),
    ] {
        let err = api.get_json(path, &[]).await.unwrap_err();
        assert_eq!(err.status(), Some(code));
        assert_eq!(err.attempts, 1);
        assert_eq!(stub.hits_for(path).len(), 1, "{path} must not be retried");
        assert_eq!(err.is_not_found(), code == 404);
        assert_eq!(
            err.to_string(),
            format!("TMDb GET {path} failed after 1 attempt: HTTP {code}")
        );
    }
}

// ── Cache ──

#[tokio::test]
async fn only_successes_are_cached() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if path == "/movie/600" && seen == 0 {
            Reply::Status(404)
        } else {
            ok(path)
        }
    })
    .await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let api = client.api(None).unwrap();
    // A failure is not cached: the next call asks again, and its success is kept.
    assert!(api.get_json("/movie/600", &[]).await.is_err());
    assert!(api.get_json("/movie/600", &[]).await.is_ok());
    assert!(api.get_json("/movie/600", &[]).await.is_ok());
    assert_eq!(stub.hits_for("/movie/600").len(), 2);
    // The cache key includes the query.
    let alpha_2001 = [("query", "Alpha"), ("year", "2001")];
    api.get_json("/search/movie", &alpha_2001).await.unwrap();
    api.get_json("/search/movie", &alpha_2001).await.unwrap();
    assert_eq!(stub.hits_for("/search/movie").len(), 1);
    api.get_json("/search/movie", &[("query", "Alpha"), ("year", "2002")])
        .await
        .unwrap();
    assert_eq!(stub.hits_for("/search/movie").len(), 2);
}

#[tokio::test]
async fn the_cache_key_never_includes_the_key() {
    let stub = ok_stub().await;
    let settings = TmdbSettings {
        env_key: None,
        ..stub.fast_settings()
    };
    let client = TmdbClient::new(http(), settings);
    let first = client.api(Some(CONFIG_KEY)).unwrap();
    let second = client.api(Some("g12-other-fake-key-9999")).unwrap();
    first.get_json("/movie/610", &[]).await.unwrap();
    second.get_json("/movie/610", &[]).await.unwrap();
    assert_eq!(stub.hits_for("/movie/610").len(), 1);
}

#[tokio::test]
async fn invalid_json_is_final_and_not_cached() {
    let stub = start_stub(Duration::ZERO, |path, seen| {
        if seen == 0 { Reply::NotJson } else { ok(path) }
    })
    .await;
    let client = TmdbClient::new(http(), stub.fast_settings());
    let api = client.api(None).unwrap();
    let err = api.get_json("/movie/620", &[]).await.unwrap_err();
    assert_eq!(err.kind, TmdbErrorKind::Decode);
    assert_eq!(err.attempts, 1);
    assert_eq!(stub.hits_for("/movie/620").len(), 1);
    assert!(api.get_json("/movie/620", &[]).await.is_ok());
    assert_eq!(stub.hits_for("/movie/620").len(), 2);
}

// ── Logs ──

#[tokio::test]
async fn logs_never_contain_the_key() {
    let logs = captured_logs();
    let stub = start_stub(Duration::ZERO, |path, _| match path {
        "/movie/700/videos" => Reply::Status(500),
        "/movie/701" => Reply::Status(401),
        "/movie/702" => Reply::Status(404),
        "/movie/703" => Reply::RateLimited("0"),
        "/movie/704" => Reply::NotJson,
        "/movie/705" => Reply::Slow(Duration::from_secs(2)),
        _ => ok(path),
    })
    .await;
    for key in [KEY, BEARER] {
        let settings = TmdbSettings {
            env_key: TmdbKey::new(key),
            request_timeout: Duration::from_millis(100),
            ..stub.fast_settings()
        };
        let client = TmdbClient::new(http(), settings);
        let api = client.api(None).unwrap();
        for path in [
            "/movie/700/videos",
            "/movie/701",
            "/movie/702",
            "/movie/703",
            "/movie/704",
            "/movie/705",
            "/movie/706",
        ] {
            let _ = api.get_json(path, &[("query", "g12 log probe")]).await;
        }
        let unreachable = TmdbSettings {
            api_base_url: format!("http://127.0.0.1:{}/3", closed_port()),
            env_key: TmdbKey::new(key),
            min_interval: Duration::from_millis(1),
            initial_backoff: Duration::from_millis(10),
            ..TmdbSettings::default()
        };
        let client = TmdbClient::new(http(), unreachable);
        let _ = client.api(None).unwrap().get_json("/movie/707", &[]).await;
    }
    let text = String::from_utf8_lossy(&logs.lock().unwrap()).into_owned();
    for key in [KEY, BEARER] {
        if let Some(at) = text.find(key) {
            let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
            panic!("a log line contains the key: {}", &text[line..at]);
        }
    }
    let final_failures = text
        .lines()
        .filter(|l| {
            l.contains("WARN") && l.contains("TMDb GET /movie/700/videos failed after 4 attempts")
        })
        .count();
    assert_eq!(
        final_failures, 2,
        "one warn line per final failure (one per key)"
    );
}
