// B21 fix round (spec §10, AC43): the request span names the path, never the query string.
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use tower_http::trace::TraceLayer;

use super::request_span;

/// What the test's subscriber writes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn request_spans_leave_query_strings_out() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let app = axum::Router::new()
        .route("/Auth/Oidc/Callback", axum::routing::get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http().make_span_with(|req: &Request<Body>| request_span(req)));

    let uri = "/Auth/Oidc/Callback?code=b21-code-value&state=b21-state-value&api_key=b21-key-value";
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("started processing request"),
        "the request is logged: {text}"
    );
    assert!(
        text.contains("request{method=GET uri=/Auth/Oidc/Callback version=HTTP/1.1}"),
        "the span names the method, the path and the version: {text}"
    );
    for value in ["b21-code-value", "b21-state-value", "b21-key-value"] {
        assert!(!text.contains(value), "{value} is in the log: {text}");
    }
}
