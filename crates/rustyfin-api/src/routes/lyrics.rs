//! Jellyfin's lyrics controller (`Jellyfin.Api/Controllers/LyricsController.cs`): the lyrics of an
//! audio item, an uploaded lyric file, and deleting its lyric files.
//!
//! The lyrics are read from, in order: the file uploaded through this API (kept in the server's
//! data folder), the lyric files beside the track (`.lrc`, `.elrc`, `.txt`), and the track's
//! lyrics tag. The remote lyric search of Jellyfin has no provider here and is not served.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_core::lyrics;
use rustyfin_model::auth::Claims;
use rustyfin_model::lyrics::LyricDto;
use rustyfin_model::media::{MediaItem, MediaType};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::state::AppState;

/// The largest lyric file an upload may send.
const MAX_LYRIC_BYTES: usize = 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/Audio/{id}/Lyrics",
        get(get_lyrics).post(upload_lyrics).delete(delete_lyrics),
    )
}

/// The audio item `id`, when the caller may see it.
async fn audio_item(state: &AppState, claims: &Claims, id: Uuid) -> Result<MediaItem, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .filter(|item| item.media_type == MediaType::Audio)
        .ok_or_else(|| ApiError::NotFound("Audio item not found".into()))?;
    if HiddenItems::for_caller(state, claims)
        .await?
        .hides(item.id, item.library_id)
    {
        return Err(ApiError::NotFound("Audio item not found".into()));
    }
    Ok(item)
}

/// The folder uploaded lyric files are kept in, under the data folder.
fn upload_dir(state: &AppState) -> std::path::PathBuf {
    state.data_dir.join("metadata").join("lyrics")
}

async fn read_file(path: &std::path::Path) -> Option<LyricDto> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let content = String::from_utf8_lossy(&bytes);
    lyrics::parse_file(&path.to_string_lossy(), &content)
}

/// The lyrics of `item`, from the first source that has some.
pub(crate) async fn item_lyrics(state: &AppState, item: &MediaItem) -> Option<LyricDto> {
    let stored = rustyfin_db::repo_lyrics::get(&state.db_ro, item.id)
        .await
        .ok()
        .flatten();
    if let Some(uploaded) = stored.as_ref().and_then(|s| s.uploaded_path.as_deref())
        && let Some(dto) = read_file(std::path::Path::new(uploaded)).await
    {
        return Some(dto);
    }
    let audio = std::path::PathBuf::from(&item.path);
    let sidecars = tokio::task::spawn_blocking(move || lyrics::find_sidecars(&audio))
        .await
        .unwrap_or_default();
    for sidecar in sidecars {
        if let Some(dto) = read_file(&sidecar).await {
            return Some(dto);
        }
    }
    // A probe that recorded no lyrics tag saves running ffprobe; an item no probe of this version
    // has read yet has no row, so its tag is read.
    if stored.as_ref().is_none_or(|s| s.embedded) {
        let tag = lyrics::read_lyrics_tag(&item.path).await?;
        return Some(lyrics::parse_tag(&tag));
    }
    None
}

/// `GET /Audio/{id}/Lyrics`: the item's lyrics as a `LyricDto`, or 404 when it has none.
async fn get_lyrics(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<LyricDto>, ApiError> {
    let item = audio_item(&state, &claims, id).await?;
    item_lyrics(&state, &item)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("No lyrics found".into()))
}

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(alias = "FileName", alias = "filename")]
    #[serde(rename = "fileName")]
    file_name: Option<String>,
}

/// `POST /Audio/{id}/Lyrics?fileName=…`: saves the body as the item's lyric file, in the format
/// of the file name's extension, and answers with the parsed lyrics. Needs lyric management,
/// which Jellyfin gives administrators.
async fn upload_lyrics(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<UploadQuery>,
    Extension(claims): Extension<Claims>,
    body: Bytes,
) -> Result<Json<LyricDto>, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Lyric management is not allowed".into(),
        ));
    }
    let item = audio_item(&state, &claims, id).await?;
    let file_name = query
        .file_name
        .filter(|f| !f.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("fileName is required".into()))?;
    if body.is_empty() {
        return Err(ApiError::BadRequest("No lyrics uploaded".into()));
    }
    if body.len() > MAX_LYRIC_BYTES {
        return Err(ApiError::BadRequest("The lyric file is too large".into()));
    }
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ApiError::BadRequest("Extension is required on filename".into()))?;
    if !lyrics::LYRIC_EXTENSIONS.contains(&ext.as_str()) {
        return Err(ApiError::BadRequest("Unsupported lyric format".into()));
    }
    let content = String::from_utf8_lossy(&body).into_owned();
    let dto = lyrics::parse_file(&file_name, &content)
        .ok_or_else(|| ApiError::BadRequest("The lyrics could not be read".into()))?;

    let dir = upload_dir(&state);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // One uploaded file per item: a new upload replaces the old one, whatever its format.
    remove_uploaded(&state, item.id).await;
    let path = dir.join(format!("{}.{ext}", item.id.simple()));
    tokio::fs::write(&path, content.as_bytes())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    rustyfin_db::repo_lyrics::set_uploaded(&state.db, item.id, Some(&path.to_string_lossy()))
        .await?;
    Ok(Json(dto))
}

async fn remove_uploaded(state: &AppState, item: Uuid) {
    let stored = rustyfin_db::repo_lyrics::get(&state.db, item)
        .await
        .ok()
        .flatten();
    if let Some(path) = stored.and_then(|s| s.uploaded_path) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

/// `DELETE /Audio/{id}/Lyrics`: deletes the item's lyric files, the uploaded one and the ones
/// beside the track, as Jellyfin deletes its lyric streams' files. A lyrics tag stays. Needs lyric
/// management.
async fn delete_lyrics(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Lyric management is not allowed".into(),
        ));
    }
    let item = audio_item(&state, &claims, id).await?;
    remove_uploaded(&state, item.id).await;
    rustyfin_db::repo_lyrics::set_uploaded(&state.db, item.id, None).await?;
    let audio = std::path::PathBuf::from(&item.path);
    let sidecars = tokio::task::spawn_blocking(move || lyrics::find_sidecars(&audio))
        .await
        .unwrap_or_default();
    for sidecar in sidecars {
        if let Err(e) = tokio::fs::remove_file(&sidecar).await {
            tracing::warn!(
                "failed to delete the lyric file '{}': {}",
                sidecar.display(),
                e
            );
        }
    }
    // A file that couldn't be deleted (a read-only share) still counts.
    let audio = std::path::PathBuf::from(&item.path);
    let left = tokio::task::spawn_blocking(move || lyrics::find_sidecars(&audio))
        .await
        .unwrap_or_default()
        .into_iter()
        .next()
        .map(|p| p.to_string_lossy().into_owned());
    rustyfin_db::repo_lyrics::set_sidecar(&state.db, item.id, left.as_deref()).await?;
    Ok(StatusCode::NO_CONTENT)
}
