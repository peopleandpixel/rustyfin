use axum::Router;
use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::header;
use axum::response::Response;
use axum::routing::get;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    // Axum can't capture part of a segment, so `{index}.jpg` and `tiles.m3u8` share one route.
    Router::new().route(
        "/Videos/{itemId}/Trickplay/{width}/{file}",
        get(get_trickplay_file),
    )
}

/// The value of query parameter `name` (any case) as sent, still URL-encoded.
fn raw_param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(n, v)| n.eq_ignore_ascii_case(name) && !v.is_empty())
        .map(|(_, v)| v)
}

/// The query each tile URL of the playlist carries: the media source, and the caller's key when
/// it sent one in the URL, as Jellyfin does, so a player that fetches the tiles by URL alone is
/// let in. A key sent in a header is never written into the playlist.
fn tile_query(item_id: Uuid, query: Option<&str>) -> String {
    let source = raw_param(query, "MediaSourceId")
        .map(str::to_string)
        .unwrap_or_else(|| item_id.simple().to_string());
    let mut out = format!("?MediaSourceId={source}");
    if let Some(key) = raw_param(query, "ApiKey").or_else(|| raw_param(query, "api_key")) {
        out.push_str("&ApiKey=");
        out.push_str(key);
    }
    out
}

/// `GET /Videos/{itemId}/Trickplay/{width}/tiles.m3u8`: the HLS image playlist of the item's tile
/// set at `width`. `GET /Videos/{itemId}/Trickplay/{width}/{index}.jpg`: one tile image. Both 404
/// when the item has no tile set at that width, or no such tile.
async fn get_trickplay_file(
    State(state): State<AppState>,
    Path((item_id, width, file)): Path<(Uuid, u32, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let not_found = || ApiError::NotFound("trickplay not found".into());
    let info = rustyfin_db::repo_trickplay::get_trickplay_width(&state.db_ro, item_id, width)
        .await?
        .ok_or_else(not_found)?;

    if file.eq_ignore_ascii_case("tiles.m3u8") {
        let playlist = info.hls_playlist(&tile_query(item_id, query.as_deref()));
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "application/x-mpegURL")
            .body(Body::from(playlist))
            .unwrap());
    }

    let index: u32 = file
        .strip_suffix(".jpg")
        .and_then(|n| n.parse().ok())
        .ok_or_else(not_found)?;
    if index >= info.tile_count() {
        return Err(not_found());
    }
    let path = rustyfin_core::trickplay::tile_path(&state.data_dir, item_id, &info, index);
    let bytes = tokio::fs::read(&path).await.map_err(|_| not_found())?;
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(bytes))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2chapters_tile_query_echoes_only_url_keys() {
        let id = Uuid::nil();
        assert_eq!(
            tile_query(id, None),
            "?MediaSourceId=00000000000000000000000000000000"
        );
        assert_eq!(
            tile_query(id, Some("mediaSourceId=abc&api_key=k%2B1")),
            "?MediaSourceId=abc&ApiKey=k%2B1"
        );
        assert_eq!(
            tile_query(id, Some("ApiKey=xyz")),
            "?MediaSourceId=00000000000000000000000000000000&ApiKey=xyz"
        );
    }
}
