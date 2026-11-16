//! `GET /Items/Suggestions`: the home row's random pick of the caller's items.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use rustyfin_model::media::QueryResult;
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::query::{self, Query};
use crate::routes::items::{ImageOptions, enrich_items_with};
use crate::routes::playback::query_user;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Items/Suggestions", get(suggestions))
}

/// Jellyfin's `SuggestionsController.GetSuggestions`: the items of the given types (`type`) and
/// media types (`mediaType`) that the user may see, in random order, recursive over every
/// library. The item query leaves out extras and takes the parental limits as `/Items` does.
async fn suggestions(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Response, ApiError> {
    let user = query::uuid_param(&params, "userId")?;
    let target = query_user(&state, &claims, user).await?;
    // The limits are the target user's: `policy` reads only the subject of its claims.
    let subject = Claims {
        sub: target.unwrap_or(Uuid::nil()),
        ..claims.clone()
    };
    let hidden = HiddenItems::for_caller(&state, &subject).await?;
    let hidden_json = hidden.json();

    let kinds = named_list(&params, &["type", "type[]"], query::item_kinds);
    let media = named_list(&params, &["mediaType", "mediaType[]"], query::media_types);
    let start_index = query::count_param(&params, "startIndex")?.map_or(0, |n| n as i64);
    let limit = query::count_param(&params, "limit")?.map_or(10_000, |n| n as i64);
    let enable_total = query::bool_param(&params, "enableTotalRecordCount")?.unwrap_or(false);

    let (mut items, total) = rustyfin_db::repo_media::get_items_filtered(
        &state.db_ro,
        None,
        None,
        kinds.as_deref(),
        None,
        None,
        None,
        None,
        None,
        None,
        Some("Random"),
        None,
        start_index,
        limit,
        true,
        None,
        None,
        None,
        None,
        None,
        media.as_deref(),
        None,
        None,
        None,
        None,
        None,
        target,
        None,
        hidden_json.as_deref(),
    )
    .await?;

    let tmdb = state.tmdb.api(None);
    enrich_items_with(
        &state.db_ro,
        &state.server_id,
        target,
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
        &ImageOptions::default(),
    )
    .await;

    // Without enableTotalRecordCount Jellyfin counts the items it returns.
    let total_record_count = if enable_total {
        total
    } else {
        items.len() as i64
    };
    Ok(Json(QueryResult {
        items,
        total_record_count,
        start_index,
    })
    .into_response())
}

/// Reads the names a query parameter lists.
type ParamReader = fn(&[(String, String)], &str) -> Vec<&'static str>;

/// The names the entries under any of `keys` name (`read` gives them, ignoring case), each
/// once, or `None` when none is named.
fn named_list(
    params: &[(String, String)],
    keys: &[&str],
    read: ParamReader,
) -> Option<Vec<String>> {
    let mut names: Vec<String> = keys
        .iter()
        .flat_map(|key| read(params, key))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    (!names.is_empty()).then_some(names)
}
