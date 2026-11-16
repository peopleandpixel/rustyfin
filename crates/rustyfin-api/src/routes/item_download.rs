//! `GET /Items/{id}/Download`: an item's own file, as Jellyfin's `LibraryController.GetDownload`
//! serves it. `item_gate` has already answered 404 for an item the caller's limits hide.

use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use axum::routing::get;
use rustyfin_model::auth::Claims;
use rustyfin_model::media::{MediaItem, MediaType};
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::streaming::{serve_file, validate_path_against_library};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Items/{id}/Download", get(download_item))
}

/// Jellyfin's `BaseItem.CanDownload()`: a video, song or photo with a file of its own. Folders
/// (series, seasons, albums, collections) have none.
pub(crate) fn downloadable(item: &MediaItem) -> bool {
    matches!(
        item.media_type,
        MediaType::Movie
            | MediaType::Episode
            | MediaType::Video
            | MediaType::Audio
            | MediaType::Photo
    ) && !item.path.is_empty()
        && !item.path.contains("://")
}

/// Jellyfin's `EnableContentDownloading` for the caller; the API key may.
pub(crate) async fn may_download(state: &AppState, claims: &Claims) -> Result<bool, ApiError> {
    Ok(crate::parental::caller_access(state, claims)
        .await?
        .enable_content_downloading)
}

/// `CanDownload` on an item `user` reads: the item has a file and the user may download.
pub(crate) async fn can_download(state: &AppState, user: Uuid, item: &MediaItem) -> bool {
    if !downloadable(item) {
        return false;
    }
    if user.is_nil() {
        return true;
    }
    crate::parental::user_policy(state, user)
        .await
        .is_ok_and(|policy| policy.access.enable_content_downloading)
}

/// Whether the request resumes a download already logged: a byte range that doesn't start at 0.
/// A client that downloads with `Range: bytes=0-` starts one.
fn resumes(headers: &HeaderMap) -> bool {
    headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|range| {
            let start = range
                .trim()
                .strip_prefix("bytes=")
                .and_then(|spec| spec.split(',').next())
                .and_then(|first| first.trim().split('-').next())
                .map(str::trim);
            start != Some("0")
        })
}

/// `attachment` with the file's name, quotes dropped as Jellyfin drops them, and its UTF-8 form.
fn content_disposition(path: &str) -> Option<HeaderValue> {
    let name = std::path::Path::new(path)
        .file_name()?
        .to_string_lossy()
        .replace('"', "");
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .ok()
}

/// `GET /Items/{id}/Download`: 403 without `EnableContentDownloading` (Jellyfin's `Download`
/// policy), 400 for an item without a file of its own, else the file with range support and an
/// `attachment` disposition. The download is written to the activity log, as Jellyfin does.
async fn download_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !may_download(&state, &claims).await? {
        return Err(ApiError::Forbidden(
            "This user may not download media.".into(),
        ));
    }
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Item not found".into()))?;
    if !downloadable(&item) {
        return Err(ApiError::BadRequest(
            "Item does not support downloading".into(),
        ));
    }
    let path = validate_path_against_library(&state, &item.path).await?;
    if !std::path::Path::new(&path).is_file() {
        return Err(ApiError::BadRequest(
            "Item does not support downloading".into(),
        ));
    }
    if !claims.sub.is_nil() && !resumes(&headers) {
        let name = format!("{} is downloading {}", claims.name, item.name);
        super::activity::record(
            &state,
            &name,
            "UserDownloadingContent",
            Some(claims.sub),
            "Information",
        )
        .await;
    }
    let mut response = serve_file(path, &headers).await?;
    if let Some(value) = content_disposition(&item.path) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3item2_content_disposition_names_the_file() {
        let value = content_disposition("/m/Films/Zébra \"Quay\\x\".mkv").unwrap();
        assert_eq!(
            value.to_str().unwrap(),
            "attachment; filename=\"Z_bra Quay_x.mkv\"; filename*=UTF-8''Z%C3%A9bra%20Quay%5Cx.mkv"
        );
    }

    #[test]
    fn r3item2_a_range_from_0_starts_a_download() {
        let with = |range: Option<&str>| {
            let mut headers = HeaderMap::new();
            if let Some(range) = range {
                headers.insert(header::RANGE, HeaderValue::from_str(range).unwrap());
            }
            resumes(&headers)
        };
        assert!(!with(None));
        assert!(!with(Some("bytes=0-")));
        assert!(!with(Some("bytes=0-1023")));
        assert!(with(Some("bytes=1024-")));
        assert!(with(Some("bytes=-500")));
    }

    #[test]
    fn r3item2_only_files_are_downloadable() {
        let mut item = MediaItem::default_dto_fields();
        item.path = "/m/Film.mkv".into();
        item.media_type = MediaType::Movie;
        assert!(downloadable(&item));
        item.media_type = MediaType::Series;
        assert!(!downloadable(&item));
        item.media_type = MediaType::Audio;
        item.path = String::new();
        assert!(!downloadable(&item));
    }
}
