use axum::extract::{Path, RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/MediaSegments/{itemId}", get(get_media_segments))
}

/// One segment, as Jellyfin's `MediaSegmentDto`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct MediaSegmentDto {
    id: String,
    item_id: String,
    #[serde(rename = "Type")]
    segment_type: &'static str,
    start_ticks: i64,
    end_ticks: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct MediaSegmentsResult {
    items: Vec<MediaSegmentDto>,
    total_record_count: i64,
    start_index: i64,
}

/// A stored segment type as one of Jellyfin's `MediaSegmentType` names. Rustyfin's detector
/// stores end credits as "Credits", which Jellyfin calls "Outro".
fn jellyfin_type(stored: &str) -> &'static str {
    match stored {
        s if s.eq_ignore_ascii_case("intro") => "Intro",
        s if s.eq_ignore_ascii_case("outro") || s.eq_ignore_ascii_case("credits") => "Outro",
        s if s.eq_ignore_ascii_case("recap") => "Recap",
        s if s.eq_ignore_ascii_case("preview") => "Preview",
        s if s.eq_ignore_ascii_case("commercial") => "Commercial",
        _ => "Unknown",
    }
}

/// The `includeSegmentTypes` values of a query (repeated, or comma-separated), any case.
fn included_types(query: Option<&str>) -> Vec<String> {
    let Some(query) = query else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(name, _)| name.eq_ignore_ascii_case("includeSegmentTypes"))
        .flat_map(|(_, value)| {
            value
                .replace("%2C", ",")
                .replace("%2c", ",")
                .split(',')
                .map(|v| v.trim().to_ascii_lowercase())
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `GET /MediaSegments/{itemId}`: the item's intro, outro and other segments, in start order,
/// only of the types `includeSegmentTypes` names when given. 404 when there is no such item.
async fn get_media_segments(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    RawQuery(query): RawQuery,
) -> Result<Json<MediaSegmentsResult>, ApiError> {
    rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;
    let segments = rustyfin_db::repo_media_segment::get_media_segments(&state.db_ro, item_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let wanted = included_types(query.as_deref());
    let items: Vec<MediaSegmentDto> = segments
        .into_iter()
        .map(|seg| MediaSegmentDto {
            id: seg.id,
            item_id: seg.item_id,
            segment_type: jellyfin_type(&seg.segment_type),
            start_ticks: seg.segment_start_ticks,
            end_ticks: seg.segment_end_ticks,
        })
        .filter(|seg| wanted.is_empty() || wanted.contains(&seg.segment_type.to_ascii_lowercase()))
        .collect();

    Ok(Json(MediaSegmentsResult {
        total_record_count: items.len() as i64,
        items,
        start_index: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2chapters_segment_types_and_filter() {
        assert_eq!(jellyfin_type("Credits"), "Outro");
        assert_eq!(jellyfin_type("Intro"), "Intro");
        assert_eq!(jellyfin_type("whatever"), "Unknown");
        assert!(included_types(None).is_empty());
        assert_eq!(
            included_types(Some(
                "includeSegmentTypes=Intro&IncludeSegmentTypes=Outro&x=1"
            )),
            ["intro", "outro"]
        );
        assert_eq!(
            included_types(Some("includeSegmentTypes=Intro%2COutro")),
            ["intro", "outro"]
        );
    }
}
