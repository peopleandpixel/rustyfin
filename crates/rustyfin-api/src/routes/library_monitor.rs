//! The library monitor's runner, and the endpoints that report changed paths to it: Jellyfin's
//! `Media/Updated` and the *arr Webhook.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rustyfin_core::library_monitor::{MAX_REPORTED_PATHS, RescanRunner};
use rustyfin_core::scan_progress::ScanTicket;
use rustyfin_core::scanner::{self, ScanChanges, ScanScope};
use rustyfin_core::tmdb_client::TmdbClient;
use rustyfin_model::library::Library;
use rustyfin_model::server_info::ServerConfiguration;
use rustyfin_model::ws_event::WsEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::sync::{RwLock, broadcast};

use crate::arr_webhook::{PathMap, changed_paths};
use crate::cache::{ApiCache, ImageCache};
use crate::error::ApiError;
use crate::state::AppState;

/// The library monitor's runner in the API. A rescan scans as a library scan does, without progress
/// events. When it changed something, the caches are cleared and `LibraryChanged` is sent.
#[derive(Clone)]
pub struct LibraryMonitorRunner {
    db: SqlitePool,
    cache: ApiCache,
    image_cache: ImageCache,
    ws_broadcast: broadcast::Sender<WsEvent>,
    config: Arc<RwLock<ServerConfiguration>>,
    http_client: reqwest::Client,
    tmdb: TmdbClient,
}

impl LibraryMonitorRunner {
    /// The runner for the state `state`, which holds the same handles as the runner's fields.
    pub fn for_state(state: &AppState) -> Self {
        Self {
            db: state.db.clone(),
            cache: state.cache.clone(),
            image_cache: state.image_cache.clone(),
            ws_broadcast: state.ws_broadcast.clone(),
            config: state.config.clone(),
            http_client: state.http_client.clone(),
            tmdb: state.tmdb.clone(),
        }
    }
}

impl RescanRunner for LibraryMonitorRunner {
    fn rescan(
        &self,
        library: Library,
        scope: ScanScope,
    ) -> Pin<Box<dyn Future<Output = Result<ScanChanges, String>> + Send>> {
        let runner = self.clone();
        Box::pin(async move {
            // The metadata providers are the ones a library scan uses.
            let config = runner.config.read().await;
            let tmdb = runner.tmdb.api(config.tmdb_api_key.as_deref());
            let omdb_key = config.omdb_api_key.clone();
            let fanart_key = config.fanart_api_key.clone();
            drop(config);

            let changes = scanner::scan_with_metadata(
                &runner.db,
                &library,
                &scope,
                &runner.http_client,
                tmdb.as_ref(),
                omdb_key.as_deref(),
                fanart_key.as_deref(),
                &ScanTicket::untracked(),
            )
            .await
            .map_err(|e| e.to_string())?;
            if changes.changed() {
                runner.cache.invalidate_on_scan();
                runner.image_cache.invalidate_all();
                let _ = runner.ws_broadcast.send(WsEvent::LibraryChanged {
                    data: json!({
                        "LibraryId": library.id.to_string(),
                        "LibraryName": library.name,
                    }),
                });
            }
            Ok(changes)
        })
    }
}

/// Administrator-only routes that report changed paths to the monitor. `routes::build_router`
/// wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Library/Media/Updated", post(media_updated))
        .route("/Library/Media/Webhook", post(arr_webhook).put(arr_webhook))
}

/// The body of `POST /Library/Media/Updated`, as Jellyfin's clients and the *arr apps send it.
#[derive(Deserialize)]
struct MediaUpdates {
    #[serde(rename = "Updates", alias = "updates")]
    updates: Vec<MediaUpdate>,
}

/// One changed path of a `MediaUpdates` body. Its `UpdateType` is not read.
#[derive(Deserialize)]
struct MediaUpdate {
    #[serde(rename = "Path", alias = "path")]
    path: String,
}

/// `POST /Library/Media/Updated`: the files at the paths of the body changed. The whole body is
/// checked before any path is reported, so one bad update queues nothing.
async fn media_updated(
    State(state): State<AppState>,
    body: Result<Json<MediaUpdates>, JsonRejection>,
) -> Response {
    let updates = match body {
        Ok(Json(updates)) => updates,
        // A body that is not sent as JSON gets axum's own 415.
        Err(rejection @ JsonRejection::MissingJsonContentType(_)) => {
            return rejection.into_response();
        }
        Err(rejection) => return ApiError::BadRequest(rejection.body_text()).into_response(),
    };
    if updates.updates.iter().any(|update| update.path.is_empty()) {
        return ApiError::BadRequest("an update has an empty Path".to_string()).into_response();
    }
    if updates.updates.len() > MAX_REPORTED_PATHS {
        return ApiError::PayloadTooLarge(format!(
            "at most {MAX_REPORTED_PATHS} paths per request, got {}",
            updates.updates.len()
        ))
        .into_response();
    }
    let paths: Vec<PathBuf> = updates
        .updates
        .iter()
        .map(|update| PathBuf::from(&update.path))
        .collect();
    let changes = state.task_scheduler.changes();
    changes.report_all(&paths).await;
    StatusCode::NO_CONTENT.into_response()
}

/// `POST|PUT /Library/Media/Webhook`: the Webhook of Sonarr, Radarr and Lidarr. The `from` and `to`
/// pairs of the URL map the apps' paths to this server's. The body is read whatever its type.
async fn arr_webhook(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let map = match PathMap::from_query(query.as_deref().unwrap_or("")) {
        Ok(map) => map,
        Err(msg) => return ApiError::BadRequest(msg).into_response(),
    };
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(e) => {
            return ApiError::BadRequest(format!("the body is not JSON: {e}")).into_response();
        }
    };
    let mapped: Vec<String> = changed_paths(&payload)
        .iter()
        .map(|path| map.apply(path))
        .collect();
    if mapped.len() > MAX_REPORTED_PATHS {
        return ApiError::PayloadTooLarge(format!(
            "at most {MAX_REPORTED_PATHS} paths per request, got {}",
            mapped.len()
        ))
        .into_response();
    }
    let paths: Vec<PathBuf> = mapped.iter().map(PathBuf::from).collect();
    let changes = state.task_scheduler.changes();
    let answers = changes.report_all(&paths).await;
    for (path, queued) in mapped.iter().zip(answers) {
        if !queued {
            tracing::info!("webhook: {path} is in no library");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}
