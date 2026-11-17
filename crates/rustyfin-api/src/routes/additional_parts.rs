//! `GET /Videos/{itemId}/AdditionalParts`: the later parts of a movie split into files
//! (`movie-cd1.mkv`, `movie-cd2.mkv`), as Jellyfin's VideosController.GetAdditionalPart answers
//! them. The scan stores them as the movie's extras of type `Part`.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;
use crate::query::Query;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/Videos/{itemId}/AdditionalParts",
        get(get_additional_parts),
    )
}

#[derive(Debug, Default, serde::Deserialize)]
struct PartsQuery {
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
}

/// The item's parts after its first, as a QueryResult of items in part order; none for an item
/// that isn't split. 404 for an unknown item.
async fn get_additional_parts(
    State(state): State<AppState>,
    Path(item_id): Path<Uuid>,
    Query(q): Query<PartsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Value>, ApiError> {
    let user_id = super::playback::query_user(&state, &claims, q.user_id)
        .await?
        .unwrap_or(claims.sub);
    if rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound("Item not found".into()));
    }
    let mut parts = super::items::extra_items(&state, item_id, user_id, &["Part"]).await?;
    // A part is no extra to Jellyfin: it has no ExtraType.
    for part in &mut parts {
        part.extra_type = None;
    }
    let count = parts.len();
    Ok(Json(serde_json::json!({
        "Items": parts,
        "TotalRecordCount": count,
        "StartIndex": 0,
    })))
}
