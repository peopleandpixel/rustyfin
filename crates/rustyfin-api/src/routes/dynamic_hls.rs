use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use rustyfin_model::auth::Claims;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// An alternative URL for the segments of an HLS transcode: `/DynamicHls/{playbackId}/{streamId}/
/// {segment}`. Jellyfin's own clients use `/Videos/{id}/hls/...`; this route serves the same files.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/DynamicHls/{playbackId}/{streamId}/{segment}",
        get(get_hls_segment),
    )
}

/// What a segment name asks for: numbered segment `n` (`{n}.ts`, `{n}.mp4`, `{n}.m4s`) or the
/// fragmented MP4 init segment (`init.mp4`). Anything else is no segment.
fn parse_segment(name: &str) -> Option<Option<u32>> {
    if name == "init.mp4" {
        return Some(None);
    }
    let (number, extension) = name.rsplit_once('.')?;
    if !matches!(extension, "ts" | "mp4" | "m4s")
        || number.is_empty()
        || number.len() > 9
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    number.parse().ok().map(Some)
}

/// GET /DynamicHls/{playbackId}/{streamId}/{segment}: a segment of the HLS transcode
/// `playbackId`, found by the transcode's id and the segment's number only; no part of the URL
/// becomes part of a file path. The file lies in the transcode's directory under the configured
/// cache, and is streamed. 400 for a name that isn't a segment (`{n}.ts`, `{n}.mp4`, `{n}.m4s`,
/// `init.mp4`) or a playback id that isn't a UUID; 404 for an unknown transcode, a segment not
/// written yet, or an item the caller's limits hide. `streamId` is not used.
async fn get_hls_segment(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path((playback_id, _stream_id, segment)): Path<(Uuid, String, String)>,
) -> Result<Response, ApiError> {
    let index = parse_segment(&segment)
        .ok_or_else(|| ApiError::BadRequest(format!("{segment:?} is not an HLS segment")))?;
    let not_found = || ApiError::NotFound("Segment not found".to_string());
    let (item_id, path) = state
        .transcode_manager
        .session_segment(playback_id, index)
        .ok_or_else(not_found)?;
    // The route has no item id for the item gate: check the transcode's item here
    crate::parental::ensure_visible(&state, &claims, item_id).await?;

    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| not_found())?;
    let length = file
        .metadata()
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?
        .len();
    // The file's own kind: a TS or an fMP4 transcode, whatever extension the request named
    let content_type = if path.extension().is_some_and(|e| e == "ts") {
        "video/mp2t"
    } else {
        "video/mp4"
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_LENGTH, length.to_string()),
            (header::CACHE_CONTROL, "max-age=3600".to_string()),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::parse_segment;

    #[test]
    fn r3security_segment_names_are_numbers_or_the_init_segment() {
        assert_eq!(parse_segment("0.ts"), Some(Some(0)));
        assert_eq!(parse_segment("0042.mp4"), Some(Some(42)));
        assert_eq!(parse_segment("7.m4s"), Some(Some(7)));
        assert_eq!(parse_segment("init.mp4"), Some(None));
        for name in [
            "",
            ".ts",
            "../0.ts",
            "..",
            "a.ts",
            "-1.ts",
            "+1.ts",
            "1.m3u8",
            "segment_0000.ts",
            "1.ts/..",
            "1.ts\0",
            "1234567890.ts",
            "main.m3u8",
        ] {
            assert_eq!(parse_segment(name), None, "{name:?}");
        }
    }
}
