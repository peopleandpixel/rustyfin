//! The TMDb providers go through the shared client: its key, base URLs, User-Agent and cache.
//!
//! A stub TMDb served by axum on 127.0.0.1:0 stands in for the real one. The key is fake.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use plugin_tmdb::tmdb_client::TmdbApi;
use plugin_tmdb::{
    get_movie_people, get_person_photo, get_similar, get_trailers, get_tv_people, get_tvdb_id,
    search_movie, search_tv,
};
use rustyfin_core::tmdb_client::USER_AGENT;
use serde_json::{Value, json};

const KEY: &str = "g12-provider-fake-key-4444";

// ── Stub ──

#[derive(Clone, Debug)]
struct Hit {
    /// Without the base URL's `/3`.
    path: String,
    query: HashMap<String, String>,
    user_agent: String,
}

impl Hit {
    fn param(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str)
    }
}

#[derive(Clone, Copy)]
enum Mode {
    /// The canned responses below. Any other path is a 404.
    Canned,
    /// Every request gets this status and an empty body.
    Status(u16),
}

#[derive(Clone)]
struct StubState {
    mode: Mode,
    hits: Arc<Mutex<Vec<Hit>>>,
}

struct Stub {
    origin: String,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Stub {
    async fn start(mode: Mode) -> Stub {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let state = StubState {
            mode,
            hits: hits.clone(),
        };
        let app = axum::Router::new().fallback(handle).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Stub { origin, hits }
    }

    fn image_base(&self) -> String {
        format!("{}/t/p/original", self.origin)
    }

    /// A simple TMDb API client with `KEY` as the API key.
    fn api(&self) -> TmdbApi {
        TmdbApi::new(KEY)
    }

    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// The first request for `path`. Panics if there was none.
    fn requested(&self, path: &str) -> Hit {
        let hits = self.hits();
        match hits.iter().find(|hit| hit.path == path) {
            Some(hit) => hit.clone(),
            None => {
                let paths: Vec<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
                panic!("{path} was not requested; got {paths:?}")
            }
        }
    }

    /// Every request came under the base URL with the key as `api_key` and Rustyfin's
    /// User-Agent, and there was at least one.
    fn assert_all_signed(&self) {
        let hits = self.hits();
        assert!(!hits.is_empty(), "no request reached the stub");
        for hit in &hits {
            assert_eq!(
                hit.param("api_key"),
                Some(KEY),
                "{} without the key",
                hit.path
            );
            assert_eq!(
                hit.user_agent, USER_AGENT,
                "{} with another User-Agent",
                hit.path
            );
        }
    }
}

async fn handle(
    State(state): State<StubState>,
    uri: Uri,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let path = match uri.path().strip_prefix("/3") {
        Some(path) => path.to_string(),
        None => format!("outside the base URL: {}", uri.path()),
    };
    let user_agent = headers
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state.hits.lock().unwrap().push(Hit {
        path: path.clone(),
        query,
        user_agent,
    });
    match state.mode {
        Mode::Status(code) => StatusCode::from_u16(code).unwrap().into_response(),
        Mode::Canned => match canned(&path) {
            Some(body) => axum::Json(body).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

/// Enough of TMDb for the providers, in TMDb's shapes.
fn canned(path: &str) -> Option<Value> {
    let body = match path {
        "/search/movie" => json!({"results": [{"id": 9001, "title": "Alpha Film"}]}),
        "/movie/9001" => json!({
            "id": 9001,
            "title": "Alpha Film",
            "overview": "G12 stub overview",
            "original_language": "en",
            "release_date": "2001-05-04",
            "genres": [{"id": 18, "name": "Drama"}],
            "credits": people(),
            "external_ids": {"imdb_id": "tt9001"}
        }),
        "/movie/9001/external_ids" => json!({"id": 9001, "imdb_id": "tt9001"}),
        "/movie/9001/images" | "/tv/9101/images" => json!({
            "posters": [{"file_path": "/g12-poster-1.jpg"}, {"file_path": "/g12-poster-2.jpg"}],
            "backdrops": [{"file_path": "/g12-backdrop.jpg"}],
            "logos": [{"file_path": "/g12-logo.png"}]
        }),
        "/movie/9001/credits" | "/tv/9101/season/1/credits" => credits(),
        "/movie/9001/videos" => json!({"results": [
            {"key": "g12clip", "name": "G12 Clip", "type": "Clip", "site": "YouTube"},
            {"key": "g12trailer", "name": "G12 Trailer", "type": "Trailer", "site": "YouTube"},
            {"key": "g12vimeo", "name": "G12 Vimeo", "type": "Trailer", "site": "Vimeo"}
        ]}),
        "/movie/9001/similar" => json!({"results": [{"id": 9002, "title": "Beta Film"}]}),
        "/search/tv" => json!({"results": [{"id": 9101, "name": "Gamma Show"}]}),
        "/tv/9101" => json!({
            "id": 9101,
            "name": "Gamma Show",
            "overview": "G12 stub show overview",
            "original_language": "en",
            "first_air_date": "2003-01-01"
        }),
        "/tv/9101/external_ids" => json!({"id": 9101, "tvdb_id": 79101, "imdb_id": "tt9101"}),
        "/person/77" => json!({"id": 77, "name": "Dana Actor", "profile_path": "/g12-dana.jpg"}),
        _ => return None,
    };
    Some(body)
}

/// A cast and crew list, each person with a photo.
fn people() -> Value {
    json!({
        "cast": [{"id": 77, "name": "Dana Actor", "character": "Self", "profile_path": "/g12-dana.jpg"}],
        "crew": [{
            "id": 78,
            "name": "Eli Director",
            "job": "Director",
            "department": "Directing",
            "profile_path": "/g12-eli.jpg"
        }]
    })
}

/// A credits endpoint's answer. TMDb puts `cast` and `crew` at the top level. The same lists
/// are also nested under `credits`, the shape `provider_tmdb` parses today, so these tests
/// hold whichever of the two it reads.
fn credits() -> Value {
    let mut body = people();
    body["credits"] = people();
    body
}

/// Every URL is on the stub's image base, and there is at least one.
fn assert_on_image_base<'a>(stub: &Stub, urls: impl IntoIterator<Item = &'a str>) {
    let base = format!("{}/", stub.image_base());
    let urls: Vec<&str> = urls.into_iter().collect();
    assert!(!urls.is_empty(), "no image URLs");
    for url in urls {
        assert!(
            url.starts_with(&base),
            "{url} is not on the image base {base}"
        );
    }
}

// ── Movies and shows ──

#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn search_movie_goes_through_the_client() {
    let stub = Stub::start(Mode::Canned).await;

    let meta = search_movie(&stub.api(), "Alpha Film", Some(2001))
        .await
        .expect("movie metadata");

    assert_eq!(meta.tmdb_id, Some(9001));
    assert_eq!(meta.name.as_deref(), Some("Alpha Film"));
    assert_eq!(meta.overview.as_deref(), Some("G12 stub overview"));
    assert_eq!(meta.imdb_id.as_deref(), Some("tt9001"));
    assert_on_image_base(
        &stub,
        meta.image_urls.iter().map(|image| image.url.as_str()),
    );
    let poster = format!("{}/g12-poster-1.jpg", stub.image_base());
    assert!(
        meta.image_urls.iter().any(|image| image.url == poster),
        "no {poster} in {:?}",
        meta.image_urls
    );

    let search = stub.requested("/search/movie");
    assert_eq!(search.param("query"), Some("Alpha Film"));
    assert_eq!(search.param("year"), Some("2001"));
    let details = stub.requested("/movie/9001");
    assert_eq!(
        details.param("append_to_response"),
        Some("credits,external_ids")
    );
    stub.requested("/movie/9001/external_ids");
    stub.requested("/movie/9001/images");
    stub.assert_all_signed();
}

#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn search_tv_goes_through_the_client() {
    let stub = Stub::start(Mode::Canned).await;

    let meta = search_tv(&stub.api(), "Gamma Show")
        .await
        .expect("show metadata");

    assert_eq!(meta.tmdb_id, Some(9101));
    assert_eq!(meta.tvdb_id, Some(79101));
    assert_eq!(meta.overview.as_deref(), Some("G12 stub show overview"));
    assert_on_image_base(
        &stub,
        meta.image_urls.iter().map(|image| image.url.as_str()),
    );

    assert_eq!(
        stub.requested("/search/tv").param("query"),
        Some("Gamma Show")
    );
    assert_eq!(
        stub.requested("/tv/9101").param("append_to_response"),
        Some("credits,external_ids")
    );
    stub.requested("/tv/9101/external_ids");
    stub.requested("/tv/9101/images");
    stub.assert_all_signed();
}

#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn people_photos_use_the_image_base() {
    let stub = Stub::start(Mode::Canned).await;
    let api = stub.api();

    let photo = get_person_photo(&api, 77).await.expect("person photo");
    assert_eq!(photo.person_id, "77");
    assert_eq!(
        photo.photo_url,
        format!("{}/g12-dana.jpg", stub.image_base())
    );

    let movie_people = get_movie_people(&api, 9001).await;
    assert_on_image_base(&stub, movie_people.iter().map(|p| p.photo_url.as_str()));
    let tv_people = get_tv_people(&api, 9101, 1).await;
    assert_on_image_base(&stub, tv_people.iter().map(|p| p.photo_url.as_str()));

    stub.requested("/person/77");
    stub.requested("/movie/9001/credits");
    stub.requested("/tv/9101/season/1/credits");
    stub.assert_all_signed();
}

#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn trailers_similar_and_tvdb_id_go_through_the_client() {
    let stub = Stub::start(Mode::Canned).await;
    let api = stub.api();

    assert_eq!(
        get_trailers(&api, "movie", 9001).await,
        vec![(
            "G12 Trailer".to_string(),
            "https://www.youtube.com/watch?v=g12trailer".to_string()
        )]
    );
    assert_eq!(
        get_similar(&api, "movie", 9001).await,
        vec![(9002, "Beta Film".to_string())]
    );
    assert_eq!(get_tvdb_id(&api, 9101).await, Some(79101));

    stub.requested("/movie/9001/videos");
    stub.requested("/movie/9001/similar");
    stub.requested("/tv/9101/external_ids");
    stub.assert_all_signed();
}

// ── Cache ──

/// The cache belongs to a `TmdbClient`, not to the process: another client asks its own
/// server.
#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn each_client_has_its_own_cache() {
    let first = Stub::start(Mode::Canned).await;
    let second = Stub::start(Mode::Canned).await;

    for stub in [&first, &second] {
        let trailers = get_trailers(&stub.api(), "movie", 9001).await;
        assert_eq!(trailers.len(), 1);
        stub.requested("/movie/9001/videos");
    }
}

// ── Failures ──

/// A TMDb failure is not an error for the caller: the providers return `None` or nothing.
#[tokio::test]
#[ignore = "B128: plugin_tmdb::TmdbApi::new always targets api.themoviedb.org, so the test reaches the real TMDb instead of the stub"]
async fn failures_give_none_or_nothing() {
    for status in [404, 500] {
        let stub = Stub::start(Mode::Status(status)).await;
        let api = stub.api();

        let movie = search_movie(&api, "Alpha Film", None).await;
        assert!(movie.is_none(), "{status}");
        assert!(search_tv(&api, "Gamma Show").await.is_none(), "{status}");
        let photo = get_person_photo(&api, 77).await;
        assert!(photo.is_none(), "{status}");
        let movie_people = get_movie_people(&api, 9001).await;
        assert!(movie_people.is_empty(), "{status}");
        let tv_people = get_tv_people(&api, 9101, 1).await;
        assert!(tv_people.is_empty(), "{status}");
        let trailers = get_trailers(&api, "movie", 9001).await;
        assert!(trailers.is_empty(), "{status}");
        let similar = get_similar(&api, "movie", 9001).await;
        assert!(similar.is_empty(), "{status}");
        assert!(get_tvdb_id(&api, 9101).await.is_none(), "{status}");
        stub.assert_all_signed();
    }
}
