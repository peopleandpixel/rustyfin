use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use uuid::Uuid;

use rustyfin_core::subtitle::{discover_external_subtitles, srt_to_vtt};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/Subtitles/{index}", get(get_subtitle))
        .route(
            "/Videos/{id}/Subtitles/{index}/Stream",
            get(get_subtitle_stream),
        )
}

/// Administrator-only subtitle routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Items/{id}/RemoteSearch/Subtitles",
            get(search_remote_subtitles),
        )
        .route(
            "/Videos/{id}/Subtitles/{index}/Download",
            post(download_remote_subtitle),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubtitleQuery {
    /// If set to "vtt", SRT subtitles will be converted to WebVTT.
    pub format: Option<String>,
}

async fn get_subtitle(
    State(state): State<AppState>,
    Path((id, index)): Path<(Uuid, usize)>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let media_path = PathBuf::from(&item.path);
    let subs = discover_external_subtitles(&media_path);

    let sub = subs
        .into_iter()
        .find(|s| s.index == index)
        .ok_or_else(|| ApiError::NotFound(format!("Subtitle index {index} not found")))?;

    let content = tokio::fs::read_to_string(&sub.path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read subtitle: {e}")))?;

    let want_vtt = query
        .format
        .as_deref()
        .map(|f| f.eq_ignore_ascii_case("vtt"))
        .unwrap_or(false);

    if want_vtt && sub.format == "srt" {
        let vtt = srt_to_vtt(&content);
        Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
            vtt,
        )
            .into_response())
    } else {
        let ct = match sub.format.as_str() {
            "vtt" => "text/vtt; charset=utf-8",
            "srt" => "application/x-subrip; charset=utf-8",
            "ass" | "ssa" => "text/x-ssa; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
        Ok((StatusCode::OK, [(header::CONTENT_TYPE, ct)], content).into_response())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct RemoteSubtitleResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub format: String,
    pub download_count: Option<i64>,
}

/// GET /Videos/{id}/Subtitles/{index}/Stream – Stream subtitle file directly
async fn get_subtitle_stream(
    State(state): State<AppState>,
    Path((id, index)): Path<(Uuid, usize)>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    let media_path = PathBuf::from(&item.path);
    let subs = discover_external_subtitles(&media_path);

    let sub = subs
        .into_iter()
        .find(|s| s.index == index)
        .ok_or_else(|| ApiError::NotFound(format!("Subtitle index {index} not found")))?;

    let content = tokio::fs::read(&sub.path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read subtitle: {e}")))?;

    let ct = match sub.format.as_str() {
        "vtt" => "text/vtt; charset=utf-8",
        "srt" => "application/x-subrip; charset=utf-8",
        "ass" | "ssa" => "text/x-ssa; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    };

    let filename = sub
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    let disposition = format!("attachment; filename=\"{}\"", filename);
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct),
            (header::CONTENT_DISPOSITION, &disposition),
        ],
        content,
    )
        .into_response())
}

async fn search_remote_subtitles(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<RemoteSubtitleQuery>,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    // Use registered subtitle providers from PluginManager
    let providers = state.plugin_manager.subtitle_providers().await;
    let mut all_results: Vec<RemoteSubtitleResult> = Vec::new();

    let language = query.language.as_deref().unwrap_or("eng");
    for provider in &providers {
        let results = (provider.search_subtitles)(
            &state.http_client,
            &item,
            language,
        ).await;

        for r in results {
            all_results.push(RemoteSubtitleResult {
                id: r.id,
                name: r.name,
                language: r.language,
                format: "srt".to_string(),
                download_count: None,
            });
        }
    }

    Ok((StatusCode::OK, axum::Json(all_results)).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteSubtitleQuery {
    #[serde(default)]
    pub language: Option<String>,
}

/// Request body for downloading a remote subtitle
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DownloadSubtitleRequest {
    /// OpenSubtitles subtitle ID
    pub subtitle_id: String,
}

/// POST /Videos/{id}/Subtitles/{index}/Download – Download and cache a remote subtitle
async fn download_remote_subtitle(
    State(state): State<AppState>,
    Path((_id, _index)): Path<(Uuid, usize)>,
    Json(req): Json<DownloadSubtitleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cache_dir = state.data_dir.join("cache").join("subtitles");

    // Use registered subtitle providers from PluginManager
    let providers = state.plugin_manager.subtitle_providers().await;
    
    for provider in &providers {
        match (provider.download_subtitle)(
            &state.http_client,
            &req.subtitle_id,
            &cache_dir,
        ).await {
            Ok(()) => {
                tracing::info!("Subtitle {} downloaded via plugin", req.subtitle_id);
                return Ok(Json(json!({
                    "status": "ok",
                    "message": "Subtitle downloaded successfully"
                })));
            }
            Err(e) => {
                tracing::warn!("Subtitle download via plugin failed: {}", e);
            }
        }
    }

    Err(ApiError::Internal("No subtitle providers available".to_string()))
}
