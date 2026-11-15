use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_playlist::PlaylistCaller;
use rustyfin_model::auth::Claims;
use rustyfin_model::collection::CollectionDto;
use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use rustyfin_model::playlist::PlaylistDto;
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;
use crate::query;
use crate::state::AppState;

type Params = Query<Vec<(String, String)>>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Collections", get(get_collections))
        .route("/Collections/{id}", get(get_collection))
        .route("/Collections/{id}/Items", get(get_collection_items))
        .route("/Playlists", get(get_playlists))
        .route("/Playlists", post(create_playlist))
        .route("/Playlists/{id}", get(get_playlist).post(update_playlist))
        .route("/Playlists/{id}", delete(delete_playlist))
        .route(
            "/Playlists/{id}/Items",
            get(get_playlist_items)
                .post(add_items_to_playlist)
                .delete(remove_items_from_playlist),
        )
        .route(
            "/Playlists/{id}/Items/{entry_id}/Move/{new_index}",
            post(move_playlist_item),
        )
        .route("/Playlists/{id}/Users", get(get_playlist_users))
        .route(
            "/Playlists/{id}/Users/{userId}",
            get(get_playlist_user).delete(remove_playlist_user),
        )
}

/// Administrator-only collection routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Collections", post(create_collection))
        .route("/Collections/{id}", delete(delete_collection))
        .route(
            "/Collections/{id}/Items",
            post(add_items_to_collection).delete(remove_items_from_collection),
        )
        .route(
            "/Collections/{id}/Items/{item_id}",
            post(add_item_to_collection).delete(remove_item_from_collection),
        )
}

async fn get_collections(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<CollectionDto>>, ApiError> {
    let collections =
        rustyfin_db::repo_collection::get_collections(&state.db_ro, claims.sub).await?;
    Ok(Json(collections))
}

async fn get_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CollectionDto>, ApiError> {
    let collection = rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("Collection not found".to_string()))?;
    Ok(Json(collection))
}

async fn delete_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::delete_collection(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A JSON body, if the request has one. Jellyfin clients send most of these calls with query
/// parameters and no body at all.
fn optional_body(body: &Bytes) -> Result<Option<Value>, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(body)
        .map(Some)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}

fn body_field<'a>(body: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    body?
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

/// The text given for `key` in the query, or else in the body.
fn text_param(params: &[(String, String)], body: Option<&Value>, key: &str) -> Option<String> {
    query::first_value(params, key)
        .or_else(|| body_field(body, key)?.as_str().map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The ids given for `key` in the query (a comma list, or the key repeated), or else in the body
/// (an array or a comma list). A value that isn't a UUID is a 400.
fn id_list(
    params: &[(String, String)],
    body: Option<&Value>,
    key: &str,
) -> Result<Vec<Uuid>, ApiError> {
    let mut values = query::list_values(params, key, ',');
    if values.is_empty() {
        values = match body_field(body, key) {
            Some(Value::Array(entries)) => entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(list)) => list
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
    }
    values
        .iter()
        .map(|value| query::parse_uuid(key, value))
        .collect()
}

/// POST /Collections?Name=&Ids= (or the same keys in a JSON body).
async fn create_collection(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
    body: Bytes,
) -> Result<Json<CollectionDto>, ApiError> {
    let body = optional_body(&body)?;
    let name = text_param(&params, body.as_ref(), "Name")
        .ok_or_else(|| ApiError::BadRequest("name is required".into()))?;
    let item_ids = id_list(&params, body.as_ref(), "Ids")?;
    let id = Uuid::new_v4();
    rustyfin_db::repo_collection::create_collection(&state.db, id, &name, claims.sub, &item_ids)
        .await?;

    let col = rustyfin_db::repo_collection::get_collection(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::Internal("collection not found after creation".into()))?;
    Ok(Json(col))
}

async fn ensure_collection(state: &AppState, id: Uuid) -> Result<(), ApiError> {
    rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .map(|_| ())
        .ok_or_else(|| ApiError::NotFound("Collection not found".to_string()))
}

async fn get_collection_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let items = rustyfin_db::repo_collection::get_collection_items(&state.db_ro, id).await?;
    let count = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: count,
        start_index: 0,
    }))
}

async fn add_item_to_collection(
    State(state): State<AppState>,
    Path((collection_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::add_item_to_collection(&state.db, collection_id, item_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_item_from_collection(
    State(state): State<AppState>,
    Path((collection_id, item_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_collection::remove_item_from_collection(&state.db, collection_id, item_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_playlists(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<PlaylistDto>>, ApiError> {
    let playlists = rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await?;
    Ok(Json(playlists))
}

/// Administrators reach any playlist; everyone else only their own.
fn playlist_caller(claims: &Claims) -> PlaylistCaller {
    if claims.is_admin {
        PlaylistCaller::Admin
    } else {
        PlaylistCaller::Owner(claims.sub)
    }
}

fn playlist_not_found() -> ApiError {
    ApiError::NotFound("Playlist not found".to_string())
}

async fn get_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PlaylistDto>, ApiError> {
    let caller = playlist_caller(&claims);
    let playlist = rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, caller)
        .await?
        .ok_or_else(playlist_not_found)?;
    Ok(Json(playlist))
}

async fn delete_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    let caller = playlist_caller(&claims);
    if !rustyfin_db::repo_playlist::delete_playlist(&state.db, id, caller).await? {
        return Err(playlist_not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Playlists with Name, Ids, UserId and MediaType in the query or a JSON body; the query
/// wins, as in Jellyfin.
async fn create_playlist(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
    body: Bytes,
) -> Result<Json<PlaylistDto>, ApiError> {
    let body = optional_body(&body)?;
    let body = body.as_ref();
    let name = text_param(&params, body, "Name")
        .ok_or_else(|| ApiError::BadRequest("name is required".into()))?;
    let mut item_ids = id_list(&params, body, "Ids")?;
    if item_ids.is_empty() {
        // The Rustyfin web client's name for the list.
        item_ids = id_list(&params, body, "ItemIds")?;
    }
    if let Some(user) = text_param(&params, body, "UserId") {
        let user = query::parse_uuid("UserId", &user)?;
        if user != claims.sub && !claims.is_admin {
            return Err(ApiError::Forbidden(
                "A playlist can only be created for yourself.".into(),
            ));
        }
    }
    let media_type = text_param(&params, body, "MediaType");
    let id = Uuid::new_v4();
    rustyfin_db::repo_playlist::create_playlist(
        &state.db,
        id,
        &name,
        claims.sub,
        media_type.as_deref(),
        &item_ids,
    )
    .await?;

    let pl = rustyfin_db::repo_playlist::get_playlist(&state.db, id, PlaylistCaller::Admin)
        .await?
        .ok_or_else(|| ApiError::Internal("playlist not found after creation".into()))?;
    Ok(Json(pl))
}

/// GET /Playlists/{id}/Items?StartIndex=&Limit=
async fn get_playlist_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
) -> Result<Json<QueryResult>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    let start = query::count_param(&params, "StartIndex")?.unwrap_or(0);
    let limit = query::count_param(&params, "Limit")?;
    playlist_entries(&state, id, &claims, start, limit)
        .await
        .map(Json)
}

/// The entries of a playlist the caller sees, in playlist order, each with its PlaylistItemId,
/// enriched like any item list.
pub(crate) async fn playlist_entries(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
    start: usize,
    limit: Option<usize>,
) -> Result<QueryResult, ApiError> {
    let caller = playlist_caller(claims);
    let all = rustyfin_db::repo_playlist::get_playlist_items(&state.db_ro, id, caller).await?;
    let total = all.len() as i64;
    let mut items: Vec<MediaItem> = all
        .into_iter()
        .skip(start)
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    enrich(state, claims, &mut items).await;
    Ok(QueryResult {
        items,
        total_record_count: total,
        start_index: start as i64,
    })
}

async fn enrich(state: &AppState, claims: &Claims, items: &mut [MediaItem]) {
    let tmdb = state.tmdb.api(None);
    super::items::enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(claims.sub),
        items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
    )
    .await;
}

/// POST /Playlists/{id} - Update playlist
async fn update_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    let name = body
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let media_type = body
        .get("MediaType")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let found = rustyfin_db::repo_playlist::update_playlist(
        &state.db,
        id,
        name.as_deref(),
        media_type.as_deref(),
        playlist_caller(&claims),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !found {
        return Err(playlist_not_found());
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Playlists/{id}/Items?Ids= (or `Ids` in a JSON body): appends the items in order.
async fn add_items_to_playlist(
    State(state): State<AppState>,
    Path(playlist_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let item_ids = id_list(&params, body.as_ref(), "Ids")?;
    ensure_playlist_visible(&state, playlist_id, &claims).await?;
    let caller = playlist_caller(&claims);
    for item_id in item_ids {
        rustyfin_db::repo_playlist::add_item_to_playlist(&state.db, playlist_id, item_id, caller)
            .await
            .ok();
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Playlists/{id}/Items?EntryIds= (PlaylistItemIds, as Jellyfin), or item `Ids` in a
/// JSON body.
async fn remove_items_from_playlist(
    State(state): State<AppState>,
    Path(playlist_id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let mut item_ids: Vec<Uuid> = id_list(&params, body.as_ref(), "EntryIds")?
        .into_iter()
        .map(|entry| rustyfin_db::repo_playlist::entry_item(playlist_id, entry))
        .collect();
    item_ids.extend(id_list(&params, body.as_ref(), "Ids")?);
    ensure_playlist_visible(&state, playlist_id, &claims).await?;
    let caller = playlist_caller(&claims);
    for item_id in item_ids {
        rustyfin_db::repo_playlist::remove_item_from_playlist(
            &state.db,
            playlist_id,
            item_id,
            caller,
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Playlists/{id}/Items/{entryId}/Move/{newIndex}
async fn move_playlist_item(
    State(state): State<AppState>,
    Path((playlist_id, entry, new_index)): Path<(Uuid, Uuid, usize)>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    let item_id = rustyfin_db::repo_playlist::entry_item(playlist_id, entry);
    let moved = rustyfin_db::repo_playlist::move_playlist_item(
        &state.db,
        playlist_id,
        item_id,
        new_index,
        playlist_caller(&claims),
    )
    .await?;
    if !moved {
        return Err(ApiError::NotFound("Playlist item not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Shared visibility check for the playlist routes: 404 unless the caller can see the playlist.
async fn ensure_playlist_visible(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<(), ApiError> {
    rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims))
        .await?
        .map(|_| ())
        .ok_or_else(playlist_not_found)
}

/// GET /Playlists/{id}/Users - Get playlist users
async fn get_playlist_users(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    Ok(Json(serde_json::json!([])))
}

/// GET /Playlists/{id}/Users/{userId}
async fn get_playlist_user(
    State(state): State<AppState>,
    Path((id, _user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    Ok(Json(serde_json::json!({})))
}

/// DELETE /Playlists/{id}/Users/{userId}
async fn remove_playlist_user(
    State(state): State<AppState>,
    Path((id, _user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    ensure_playlist_visible(&state, id, &claims).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Collections/{id}/Items?Ids= (or `Ids` in a JSON body) - Add items to a collection
async fn add_items_to_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<Uuid>,
    Query(params): Params,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let item_ids = id_list(&params, body.as_ref(), "Ids")?;
    ensure_collection(&state, collection_id).await?;
    for item_id in item_ids {
        rustyfin_db::repo_collection::add_item_to_collection(&state.db, collection_id, item_id)
            .await
            .ok();
    }
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Collections/{id}/Items?Ids= (or `Ids` in a JSON body) - Remove items from a collection
async fn remove_items_from_collection(
    State(state): State<AppState>,
    Path(collection_id): Path<Uuid>,
    Query(params): Params,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let item_ids = id_list(&params, body.as_ref(), "Ids")?;
    ensure_collection(&state, collection_id).await?;
    for item_id in item_ids {
        rustyfin_db::repo_collection::remove_item_from_collection(
            &state.db,
            collection_id,
            item_id,
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// What an id names when it isn't a media item: a collection (with its item ids) or a playlist
/// the caller sees.
pub(crate) enum Container {
    Collection(Vec<Uuid>),
    Playlist,
}

pub(crate) async fn container_of(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<Option<Container>, ApiError> {
    if rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .is_some()
    {
        let ids = rustyfin_db::repo_collection::get_collection_item_ids(&state.db_ro, id).await?;
        return Ok(Some(Container::Collection(ids)));
    }
    let playlist =
        rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims)).await?;
    Ok(playlist.map(|_| Container::Playlist))
}

fn container_item(
    state: &AppState,
    id: Uuid,
    name: String,
    media_type: MediaType,
    child_count: i64,
    date_created: chrono::DateTime<chrono::Utc>,
) -> MediaItem {
    MediaItem {
        id,
        sort_name: Some(name.to_lowercase()),
        name,
        media_type,
        child_count: Some(child_count as i32),
        date_created,
        server_id: Some(state.server_id.to_string()),
        ..MediaItem::default_dto_fields()
    }
}

fn collection_item(state: &AppState, c: CollectionDto) -> MediaItem {
    container_item(
        state,
        c.id,
        c.name,
        MediaType::BoxSet,
        c.child_count,
        c.date_created,
    )
}

fn playlist_item(state: &AppState, p: PlaylistDto) -> MediaItem {
    container_item(
        state,
        p.id,
        p.name,
        MediaType::Playlist,
        p.child_count,
        p.date_created,
    )
}

/// GET /Items/{id} for a collection, or a playlist the caller sees.
pub(crate) async fn container_details(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<Option<MediaItem>, ApiError> {
    if let Some(c) = rustyfin_db::repo_collection::get_collection(&state.db_ro, id).await? {
        return Ok(Some(collection_item(state, c)));
    }
    let playlist =
        rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims)).await?;
    Ok(playlist.map(|p| playlist_item(state, p)))
}

/// /Items?IncludeItemTypes=BoxSet,Playlist: every collection and the caller's playlists, sorted
/// by name and paged with StartIndex and Limit; the other filters don't apply.
pub(crate) async fn container_listing(
    state: &AppState,
    claims: &Claims,
    collections: bool,
    playlists: bool,
    start: usize,
    limit: usize,
) -> Result<QueryResult, ApiError> {
    let mut all = Vec::new();
    if collections {
        for c in rustyfin_db::repo_collection::get_all_collections(&state.db_ro).await? {
            all.push(collection_item(state, c));
        }
    }
    if playlists {
        for p in rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await? {
            all.push(playlist_item(state, p));
        }
    }
    all.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));
    let total = all.len() as i64;
    let items = all.into_iter().skip(start).take(limit).collect();
    Ok(QueryResult {
        items,
        total_record_count: total,
        start_index: start as i64,
    })
}

/// DELETE /Items/{id} for a collection (administrators) or a playlist (its owner or an
/// administrator). None when the id names neither, so the caller goes on to media items.
pub(crate) async fn delete_container(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<Option<StatusCode>, ApiError> {
    if rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .is_some()
    {
        if !claims.is_admin {
            return Err(ApiError::Forbidden(
                "Administrator access is required.".to_string(),
            ));
        }
        rustyfin_db::repo_collection::delete_collection(&state.db, id).await?;
        return Ok(Some(StatusCode::NO_CONTENT));
    }
    if rustyfin_db::repo_playlist::delete_playlist(&state.db, id, playlist_caller(claims)).await? {
        return Ok(Some(StatusCode::NO_CONTENT));
    }
    Ok(None)
}
