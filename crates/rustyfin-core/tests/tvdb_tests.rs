//! The TVDB plugin's TVMaze calls, against a stub TVMaze.

use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;

/// A stub TVMaze: one show, TVMaze id 500 and TheTVDB id 81189, whose ids differ, plus another
/// show under TVMaze id 81189, so a TheTVDB id sent to a show endpoint gets the wrong show.
async fn tvmaze_stub() -> (String, Arc<Mutex<Vec<String>>>) {
    let hits: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = hits.clone();
    let handle = move |uri: axum::http::Uri| {
        let seen = seen.clone();
        async move {
            let path = uri.path().to_string();
            let query = uri.query().unwrap_or_default().to_string();
            seen.lock().unwrap().push(format!("{path}?{query}"));
            let show = serde_json::json!({
                "id": 500,
                "name": "Lantern Hour",
                "externals": {"thetvdb": 81189, "imdb": "tt0000500", "tvrage": null},
                "image": {"medium": "http://img/500-m.jpg", "original": "http://img/500.jpg"}
            });
            let body = match path.as_str() {
                "/search/shows" => serde_json::json!([{"score": 0.9, "show": show}]),
                "/lookup/shows" if query == "thetvdb=81189" => show,
                "/shows/500/images" => serde_json::json!([{
                    "type": "background", "main": true,
                    "resolutions": {"original": {"url": "http://img/500-bg.jpg"}}
                }]),
                "/shows/500/cast" => serde_json::json!([{
                    "character": {"id": 70, "name": "Keeper"},
                    "person": {"id": 7, "name": "Right Actor",
                               "image": {"medium": "http://img/p7-m.jpg",
                                         "original": "http://img/p7.jpg"}}
                }]),
                "/shows/81189/images" => serde_json::json!([{
                    "type": "background", "main": true,
                    "resolutions": {"original": {"url": "http://img/other-show.jpg"}}
                }]),
                "/shows/81189/cast" => serde_json::json!([{
                    "character": {"id": 90, "name": "Stranger"},
                    "person": {"id": 9, "name": "Other Show Actor",
                               "image": {"medium": "http://img/p9-m.jpg",
                                         "original": "http://img/p9.jpg"}}
                }]),
                _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
            };
            axum::Json(body).into_response()
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(handle);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, hits)
}

/// TVMaze's show endpoints take TVMaze's ids: the plugin finds a show by TVMaze's search, and
/// asks for its images and cast under its TVMaze id, translating a TheTVDB id through
/// `/lookup/shows`, so it never gets another show's artwork or cast.
#[tokio::test]
async fn r3meta_tvdb_plugin_asks_tvmaze_with_tvmaze_ids() {
    let (base, hits) = tvmaze_stub().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let found = plugin_tvdb::search_tv_at(&client, &base, "Lantern Hour")
        .await
        .expect("TVMaze's search finds the show");
    assert_eq!(found.tvdb_id, Some(81189));
    let urls: Vec<&str> = found.image_urls.iter().map(|i| i.url.as_str()).collect();
    assert!(urls.contains(&"http://img/500-bg.jpg"), "{urls:?}");
    assert!(!urls.contains(&"http://img/other-show.jpg"), "{urls:?}");

    let images = plugin_tvdb::images_for_tvdb_id(&client, &base, 81189).await;
    let urls: Vec<&str> = images.iter().map(|i| i.url.as_str()).collect();
    assert_eq!(urls, ["http://img/500-bg.jpg"]);

    let cast = plugin_tvdb::get_tv_people_at(&client, &base, 81189).await;
    let names: Vec<&str> = cast.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Right Actor"]);

    // A TheTVDB id TVMaze doesn't know gives nothing, not a show that has it as its own id
    assert!(
        plugin_tvdb::images_for_tvdb_id(&client, &base, 500)
            .await
            .is_empty()
    );

    let asked = hits.lock().unwrap().clone();
    assert!(
        !asked.iter().any(|h| h.starts_with("/shows/81189")),
        "{asked:?}"
    );
}
