//! The TMDb providers go through the shared client: its key, base URLs, User-Agent and cache.
//!
//! A stub TMDb served by axum on 127.0.0.1:0 stands in for the real one. The key is fake.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use plugin_tmdb::tmdb_client::{TmdbApi, TmdbClient, TmdbSettings};
use plugin_tmdb::{
    TmdbPlugin, get_movie_people, get_person_photo, get_similar, get_trailers, get_tv_people,
    get_tvdb_id, register_tmdb_provider, search_movie, search_tv,
};
use rustyfin_core::plugin_manager::{BuiltinPluginInfo, PluginManager};
use rustyfin_core::tmdb_client::USER_AGENT;
use rustyfin_model::plugin::{Plugin, PluginStatus, PluginType};
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

    /// A shared client on this stub, with short waits so retries stay fast.
    fn client(&self) -> TmdbClient {
        let settings = TmdbSettings {
            api_base_url: format!("{}/3", self.origin),
            image_base_url: self.image_base(),
            min_interval: std::time::Duration::from_millis(1),
            initial_backoff: std::time::Duration::from_millis(1),
            ..TmdbSettings::default()
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        TmdbClient::new(http, settings)
    }

    /// A new client's handle with `KEY` as the API key.
    fn api(&self) -> TmdbApi {
        self.client().api(Some(KEY)).expect("a key")
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
    // A season answers slowly, so that concurrent requests for it overlap
    if path.contains("/season/") {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
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
            "imdb_id": "tt9001",
            "production_companies": [{"id": 5, "name": "G12 Pictures"}],
            "images": images(),
            "releases": {"countries": [
                {"iso_3166_1": "DE", "certification": "12"},
                {"iso_3166_1": "US", "certification": ""},
                {"iso_3166_1": "US", "certification": "R"}
            ]}
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
            "first_air_date": "2003-01-01",
            "credits": people(),
            "images": images(),
            "external_ids": {"tvdb_id": 79101, "imdb_id": "tt9101"},
            "content_ratings": {"results": [
                {"iso_3166_1": "GB", "rating": "15"},
                {"iso_3166_1": "US", "rating": "TV-MA"}
            ]},
            "networks": [{"id": 4, "name": "G12 Network"}],
            "production_companies": [{"id": 5, "name": "G12 Pictures"}]
        }),
        "/tv/9101/season/1" => json!({
            "id": 5101,
            "name": "Season 1",
            "overview": "R1 season overview",
            "air_date": "2003-01-01",
            "poster_path": "/r1-season-poster.jpg",
            "credits": {"cast": [{"id": 77, "name": "Dana Actor", "character": "Self",
                                  "profile_path": "/g12-dana.jpg"}]},
            "episodes": [
                {
                    "id": 6101, "episode_number": 1, "name": "Pilot: One",
                    "overview": "R1 episode one", "air_date": "2003-01-01", "vote_average": 7.5,
                    "still_path": "/r1-still-1.jpg",
                    "crew": [{"id": 78, "name": "Eli Director", "job": "Director",
                              "department": "Directing", "profile_path": null}],
                    "guest_stars": [{"id": 79, "name": "Gus Guest", "character": "Mailman",
                                     "profile_path": "/r1-gus.jpg"}]
                },
                {"id": 6102, "episode_number": 2, "name": "Two", "air_date": "2003-01-08"}
            ]
        }),
        "/tv/9101/external_ids" => json!({"id": 9101, "tvdb_id": 79101, "imdb_id": "tt9101"}),
        "/person/77" => json!({"id": 77, "name": "Dana Actor", "profile_path": "/g12-dana.jpg"}),
        _ => return None,
    };
    Some(body)
}

/// Posters, backdrops (one with English text, a Thumb) and a logo, as `images` appends them.
fn images() -> Value {
    json!({
        "posters": [
            {"file_path": "/g12-poster-fr.jpg", "iso_639_1": "fr", "vote_average": 9.0},
            {"file_path": "/g12-poster-1.jpg", "iso_639_1": "en", "vote_average": 5.0},
            {"file_path": "/g12-poster-2.jpg", "iso_639_1": null, "vote_average": 8.0}
        ],
        "backdrops": [
            {"file_path": "/g12-thumb.jpg", "iso_639_1": "en", "vote_average": 9.0},
            {"file_path": "/g12-backdrop.jpg", "iso_639_1": null, "vote_average": 1.0}
        ],
        "logos": [{"file_path": "/g12-logo.png", "iso_639_1": "en"}]
    })
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
        Some("credits,images,keywords,releases,videos")
    );
    // R1: the details call carries the IMDb id and images; nothing else is asked
    assert_eq!(r1scan_paths(&stub), ["/search/movie", "/movie/9001"]);
    stub.assert_all_signed();
}

#[tokio::test]

async fn search_tv_goes_through_the_client() {
    let stub = Stub::start(Mode::Canned).await;

    let meta = search_tv(&stub.api(), "Gamma Show", None)
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
        Some(
            "credits,images,external_ids,content_ratings,keywords,videos,episode_groups,aggregate_credits"
        )
    );
    assert_eq!(r1scan_paths(&stub), ["/search/tv", "/tv/9101"]);
    stub.assert_all_signed();
}

/// The paths the stub was asked for, in order.
fn r1scan_paths(stub: &Stub) -> Vec<String> {
    stub.hits().into_iter().map(|hit| hit.path).collect()
}

/// R1 scan: a movie's one details call gives the IMDb id, the US certification, studios, people
/// (cast as Actor, the director as Director, with photos), the premiere date and the images,
/// English posters first and a backdrop with text as Thumb.
#[tokio::test]
async fn r1scan_movie_details_fill_everything_from_one_call() {
    let stub = Stub::start(Mode::Canned).await;
    let meta = plugin_tmdb::movie_by_id(&stub.api(), 9001)
        .await
        .expect("movie metadata");

    assert_eq!(meta.imdb_id.as_deref(), Some("tt9001"));
    assert_eq!(meta.official_rating.as_deref(), Some("R"));
    assert_eq!(meta.premiere_date.as_deref(), Some("2001-05-04"));
    assert_eq!(meta.studios, Some(vec!["G12 Pictures".to_string()]));
    let people = meta.people.expect("people");
    let kinds: Vec<(&str, &str)> = people
        .iter()
        .map(|p| (p.name.as_str(), p.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [("Dana Actor", "Actor"), ("Eli Director", "Director")]
    );
    assert_eq!(people[0].role.as_deref(), Some("Self"));
    assert_eq!(
        people[0].image_url,
        Some(format!("{}/g12-dana.jpg", stub.image_base()))
    );
    let url = |kind: &str| {
        meta.image_urls
            .iter()
            .find(|i| i.image_type == kind)
            .map(|i| i.url.clone())
    };
    let base = stub.image_base();
    assert_eq!(url("Primary"), Some(format!("{base}/g12-poster-1.jpg")));
    assert_eq!(url("Thumb"), Some(format!("{base}/g12-thumb.jpg")));
    assert_eq!(url("Backdrop"), Some(format!("{base}/g12-backdrop.jpg")));
    assert_eq!(url("Logo"), Some(format!("{base}/g12-logo.png")));
    assert_eq!(r1scan_paths(&stub), ["/movie/9001"]);
}

/// R1 scan: a series' details call gives the TVDb and IMDb ids, the US content rating and the
/// networks as studios; a series search sends the first-air year.
#[tokio::test]
async fn r1scan_series_details_and_year_search() {
    let stub = Stub::start(Mode::Canned).await;
    let meta = search_tv(&stub.api(), "Gamma Show", Some(2003))
        .await
        .expect("show metadata");

    assert_eq!(
        stub.requested("/search/tv").param("first_air_date_year"),
        Some("2003")
    );
    assert_eq!(meta.tvdb_id, Some(79101));
    assert_eq!(meta.imdb_id.as_deref(), Some("tt9101"));
    assert_eq!(meta.official_rating.as_deref(), Some("TV-MA"));
    assert_eq!(meta.studios, Some(vec!["G12 Network".to_string()]));
    assert_eq!(meta.premiere_date.as_deref(), Some("2003-01-01"));
    assert!(meta.people.is_some_and(|p| !p.is_empty()));
}

/// R1 scan (B147): one season call gives the season and every episode it lists.
#[tokio::test]
async fn r1scan_season_lists_its_episodes() {
    let stub = Stub::start(Mode::Canned).await;
    let season = plugin_tmdb::season(&stub.api(), 9101, 1)
        .await
        .expect("season");

    assert_eq!(
        stub.requested("/tv/9101/season/1")
            .param("append_to_response"),
        Some("credits,images,external_ids,videos")
    );
    assert_eq!(r1scan_paths(&stub), ["/tv/9101/season/1"]);
    let base = stub.image_base();
    assert_eq!(season.meta.overview.as_deref(), Some("R1 season overview"));
    assert_eq!(season.meta.tmdb_id, Some(5101));
    assert_eq!(
        season.meta.image_urls[0].url,
        format!("{base}/r1-season-poster.jpg")
    );
    assert_eq!(season.episodes.len(), 2);
    let (number, one) = &season.episodes[0];
    assert_eq!(*number, 1);
    assert_eq!(one.name.as_deref(), Some("Pilot: One"));
    assert_eq!(one.tmdb_id, Some(6101));
    assert_eq!(one.community_rating, Some(7.5));
    assert_eq!(one.premiere_date.as_deref(), Some("2003-01-01"));
    assert_eq!(one.image_urls[0].image_type, "Primary");
    assert_eq!(one.image_urls[0].url, format!("{base}/r1-still-1.jpg"));
    let people: Vec<(&str, &str)> = one
        .people
        .iter()
        .flatten()
        .map(|p| (p.name.as_str(), p.kind.as_str()))
        .collect();
    assert_eq!(
        people,
        [
            ("Dana Actor", "Actor"),
            ("Gus Guest", "GuestStar"),
            ("Eli Director", "Director")
        ]
    );
    // An episode without a rating or still has none
    let (_, two) = &season.episodes[1];
    assert_eq!(two.community_rating, None);
    assert!(two.image_urls.is_empty());
    // The regular cast is credited on every episode
    assert_eq!(two.people.as_ref().map(Vec::len), Some(1));
}

#[tokio::test]

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

async fn each_client_has_its_own_cache() {
    let first = Stub::start(Mode::Canned).await;
    let second = Stub::start(Mode::Canned).await;

    for stub in [&first, &second] {
        let trailers = get_trailers(&stub.api(), "movie", 9001).await;
        assert_eq!(trailers.len(), 1);
        stub.requested("/movie/9001/videos");
    }
}

/// B134a: the registered provider uses the client it is given, so every search shares one cache,
/// and it reads the plugin's current key on each call.
#[tokio::test]
async fn b134a_registered_provider_shares_the_client_cache() {
    let stub = Stub::start(Mode::Canned).await;
    let dir = std::env::temp_dir().join(format!("b134a_{}", std::process::id()));
    let pm = PluginManager::new(&dir);
    pm.register_builtin_plugin(BuiltinPluginInfo {
        name: "TMDb".to_string(),
        version: "0".to_string(),
        id: "tmdb".to_string(),
        description: String::new(),
        plugin_type: PluginType::MetadataProvider,
        status: PluginStatus::Active,
    })
    .await;
    let plugin = TmdbPlugin::default();
    plugin.set_configuration(json!({ "api_key": KEY })).unwrap();
    register_tmdb_provider(&plugin, stub.client(), &pm)
        .await
        .unwrap();

    let providers = pm.metadata_providers().await;
    let tmdb = providers
        .iter()
        .find(|p| p.provider_name == "tmdb")
        .expect("an active tmdb provider");
    let http = reqwest::Client::new();
    for _ in 0..2 {
        let meta = (tmdb.search_movie)(&http, "Alpha Film", Some(2001)).await;
        assert_eq!(meta.and_then(|m| m.tmdb_id), Some(9001));
    }
    let searches = stub
        .hits()
        .iter()
        .filter(|hit| hit.path == "/search/movie")
        .count();
    assert_eq!(searches, 1, "the second search missed the cache");
    stub.assert_all_signed();

    // A cleared key turns the provider off without a request.
    plugin
        .set_configuration(json!({ "api_key": null }))
        .unwrap();
    let before = stub.hits().len();
    assert!(
        (tmdb.search_movie)(&http, "Beta Film", None)
            .await
            .is_none(),
        "searched without a key"
    );
    assert_eq!(
        stub.hits().len(),
        before,
        "a request went out without a key"
    );
}

/// R1 scan (review L3): concurrent season lookups for one series and season make one request; a
/// season TMDb lacks (404) is asked for once; a failure (500) is asked again.
#[tokio::test]
async fn r1scan_season_lookups_are_single_flight_and_remember_a_404() {
    async fn lookups_on(stub: &Stub) -> Arc<dyn rustyfin_model::metadata::MetadataLookups> {
        let dir = std::env::temp_dir().join(format!("r1scan_l3_{}", std::process::id()));
        let pm = PluginManager::new(&dir);
        pm.register_builtin_plugin(BuiltinPluginInfo {
            name: "TMDb".to_string(),
            version: "0".to_string(),
            id: "tmdb".to_string(),
            description: String::new(),
            plugin_type: PluginType::MetadataProvider,
            status: PluginStatus::Active,
        })
        .await;
        let plugin = TmdbPlugin::default();
        plugin.set_configuration(json!({ "api_key": KEY })).unwrap();
        register_tmdb_provider(&plugin, stub.client(), &pm)
            .await
            .unwrap();
        pm.metadata_providers()
            .await
            .into_iter()
            .find(|p| p.provider_name == "tmdb")
            .and_then(|p| p.lookups)
            .expect("tmdb lookups")
    }
    let count = |stub: &Stub, path: &str| stub.hits().iter().filter(|h| h.path == path).count();

    let stub = Stub::start(Mode::Canned).await;
    let lookups = lookups_on(&stub).await;
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let lookups = lookups.clone();
        tasks.push(tokio::spawn(async move { lookups.season(9101, 1).await }));
    }
    for task in tasks {
        assert!(task.await.unwrap().is_some());
    }
    assert_eq!(count(&stub, "/tv/9101/season/1"), 1);

    // A 404 is remembered
    for _ in 0..3 {
        assert!(lookups.season(9101, 9).await.is_none());
    }
    assert_eq!(count(&stub, "/tv/9101/season/9"), 1);

    // A failure is not
    let failing = Stub::start(Mode::Status(500)).await;
    let lookups = lookups_on(&failing).await;
    assert!(lookups.season(9101, 1).await.is_none());
    let first = count(&failing, "/tv/9101/season/1");
    assert!(lookups.season(9101, 1).await.is_none());
    assert!(count(&failing, "/tv/9101/season/1") > first);
}

// ── Failures ──

/// A TMDb failure is not an error for the caller: the providers return `None` or nothing.
#[tokio::test]

async fn failures_give_none_or_nothing() {
    for status in [404, 500] {
        let stub = Stub::start(Mode::Status(status)).await;
        let api = stub.api();

        let movie = search_movie(&api, "Alpha Film", None).await;
        assert!(movie.is_none(), "{status}");
        assert!(
            search_tv(&api, "Gamma Show", None).await.is_none(),
            "{status}"
        );
        assert!(
            plugin_tmdb::season(&api, 9101, 1).await.is_none(),
            "{status}"
        );
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
