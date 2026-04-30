use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use rustyfin_core::subtitle::{discover_external_subtitles, srt_to_vtt};

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Videos/{id}/Subtitles/{index}", get(get_subtitle))
        .route(
            "/Items/{id}/RemoteSearch/Subtitles",
            get(search_remote_subtitles),
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
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
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
struct RemoteSubtitleResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub format: String,
    pub download_count: Option<i64>,
}

async fn search_remote_subtitles(
    Path(_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    // OpenSubtitles integration placeholder – returns empty list.
    // Full implementation requires an OpenSubtitles API key and REST client.
    let results: Vec<RemoteSubtitleResult> = vec![];
    Ok((StatusCode::OK, axum::Json(results)).into_response())
}
