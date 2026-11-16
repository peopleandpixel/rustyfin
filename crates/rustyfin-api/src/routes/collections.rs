use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_playlist::PlaylistCaller;
use rustyfin_model::auth::Claims;
use rustyfin_model::collection::CollectionDto;
use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use rustyfin_model::playlist::{PlaylistDto, PlaylistUserPermissions};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
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
            get(get_playlist_user)
                .post(set_playlist_user)
                .delete(remove_playlist_user),
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

/// GET /Collections: every collection the caller may see, by name. Collections are shared by all
/// users, as in Jellyfin.
async fn get_collections(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<CollectionDto>>, ApiError> {
    let all = rustyfin_db::repo_collection::get_all_collections(&state.db_ro).await?;
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let mut collections = Vec::with_capacity(all.len());
    for mut collection in all {
        if visible_collection_count(&state, &hidden, &mut collection).await? {
            collections.push(collection);
        }
    }
    Ok(Json(collections))
}

async fn get_collection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<CollectionDto>, ApiError> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let collection = visible_collection(&state, &hidden, id)
        .await?
        .ok_or_else(collection_not_found)?;
    Ok(Json(collection))
}

fn collection_not_found() -> ApiError {
    ApiError::NotFound("Collection not found".to_string())
}

/// A collection the caller may see, with its count of the items they see; None when it doesn't
/// exist or is hidden from them.
async fn visible_collection(
    state: &AppState,
    hidden: &HiddenItems,
    id: Uuid,
) -> Result<Option<CollectionDto>, ApiError> {
    let Some(mut collection) =
        rustyfin_db::repo_collection::get_collection(&state.db_ro, id).await?
    else {
        return Ok(None);
    };
    let visible = visible_collection_count(state, hidden, &mut collection).await?;
    Ok(visible.then_some(collection))
}

/// For a user with parental limits, a collection's `ChildCount` counts only the items they may
/// see, as its item list shows them. Answers whether they may see the collection at all: as
/// Jellyfin's `BoxSet.IsVisible`, an empty one shows, and one whose every item is hidden from
/// them doesn't.
async fn visible_collection_count(
    state: &AppState,
    hidden: &HiddenItems,
    collection: &mut CollectionDto,
) -> Result<bool, ApiError> {
    if hidden.is_restricted() && collection.child_count > 0 {
        let mut ids =
            rustyfin_db::repo_collection::get_collection_item_ids(&state.db_ro, collection.id)
                .await?;
        hidden.retain(&mut ids, |i| *i);
        collection.child_count = ids.len() as i64;
        return Ok(!ids.is_empty());
    }
    Ok(true)
}

/// The same for a playlist the caller can read.
async fn visible_playlist_count(
    state: &AppState,
    claims: &Claims,
    hidden: &HiddenItems,
    playlist: &mut PlaylistDto,
) -> Result<(), ApiError> {
    if hidden.is_restricted() {
        let caller = playlist_caller(claims);
        let mut items =
            rustyfin_db::repo_playlist::get_playlist_items(&state.db_ro, playlist.id, caller)
                .await?;
        hidden.retain(&mut items, |i| i.id);
        playlist.child_count = items.len() as i64;
    }
    Ok(())
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

/// The most ids one request may name.
const MAX_IDS: usize = 1_000;

/// The ids given for `key` in the query (a comma list, or the key repeated), or else in the body
/// (an array or a comma list). A value that isn't a UUID, or more than `MAX_IDS` of them, is a 400.
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
    if values.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!(
            "{key} may name at most {MAX_IDS} items"
        )));
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
        .ok_or_else(collection_not_found)
}

async fn get_collection_items(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    visible_collection(&state, &hidden, id)
        .await?
        .ok_or_else(collection_not_found)?;
    let mut items = rustyfin_db::repo_collection::get_collection_items(&state.db_ro, id).await?;
    hidden.retain(&mut items, |i| i.id);
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
    let mut playlists = rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await?;
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    for playlist in &mut playlists {
        visible_playlist_count(&state, &claims, &hidden, playlist).await?;
    }
    Ok(Json(playlists))
}

/// Administrators reach any playlist; everyone else their own, public ones, and ones shared with
/// them (the repository's access rules tell reading from changing).
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
    let mut playlist = rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, caller)
        .await?
        .ok_or_else(playlist_not_found)?;
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    visible_playlist_count(&state, &claims, &hidden, &mut playlist).await?;
    let mut ids =
        rustyfin_db::repo_playlist::get_playlist_item_ids(&state.db_ro, id, caller).await?;
    hidden.retain(&mut ids, |i| *i);
    playlist.item_ids = Some(ids);
    Ok(Json(playlist))
}

async fn delete_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    let caller = playlist_caller(&claims);
    if !rustyfin_db::repo_playlist::delete_playlist(&state.db, id, caller).await? {
        // 403 for a playlist the caller reads but doesn't own, 404 for one they can't see
        ensure_playlist_visible(&state, id, &claims).await?;
        return Err(playlist_delete_forbidden());
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
    let shares = playlist_users(body)?;
    let public = flag_param(&params, body, "IsPublic")?;
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
    if let Some(shares) = shares {
        rustyfin_db::repo_playlist::replace_shares(&state.db, id, &shares).await?;
    }
    if let Some(public) = public {
        rustyfin_db::repo_playlist::set_open_access(&state.db, id, public).await?;
    }

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
    playlist_entries(&state, id, &claims, start, limit, false)
        .await
        .map(Json)
}

/// The entries of a playlist the caller sees, in playlist order, or shuffled when `shuffle` is
/// set (`/Items?ParentId=<playlist>&SortBy=Random`, as Jellyfin answers it). Each has its
/// PlaylistItemId and is enriched like any item list. Items hidden from the caller by parental
/// limits are left out, and out of the total.
pub(crate) async fn playlist_entries(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
    start: usize,
    limit: Option<usize>,
    shuffle: bool,
) -> Result<QueryResult, ApiError> {
    let caller = playlist_caller(claims);
    let mut all = rustyfin_db::repo_playlist::get_playlist_items(&state.db_ro, id, caller).await?;
    HiddenItems::for_caller(state, claims)
        .await?
        .retain(&mut all, |i| i.id);
    if shuffle {
        rustyfin_db::repo_media::shuffle(&mut all);
    }
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

/// The `Users` of a create or update body (Jellyfin's `PlaylistUserPermissions` list), or None
/// when the body has none.
fn playlist_users(body: Option<&Value>) -> Result<Option<Vec<PlaylistUserPermissions>>, ApiError> {
    let Some(users) = body_field(body, "Users").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let users: Vec<PlaylistUserPermissions> = serde_json::from_value(users.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid Users: {e}")))?;
    if users.len() > MAX_IDS {
        return Err(ApiError::BadRequest(format!(
            "Users may name at most {MAX_IDS} users"
        )));
    }
    Ok(Some(users))
}

/// A true or false given for `key` in the query, or else in the body.
fn flag_param(
    params: &[(String, String)],
    body: Option<&Value>,
    key: &str,
) -> Result<Option<bool>, ApiError> {
    if let Some(value) = query::first_value(params, key) {
        return match value.to_ascii_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(ApiError::BadRequest(format!("{key} must be true or false"))),
        };
    }
    match body_field(body, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(ApiError::BadRequest(format!("{key} must be true or false"))),
    }
}

/// POST /Playlists/{id} (Jellyfin's UpdatePlaylistDto): its Name, MediaType, Ids (replacing the
/// items), Users (replacing the shares) and IsPublic. Its owner, an editor or an administrator.
async fn update_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let body = body.as_ref();
    let name = body_field(body, "Name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let media_type = body_field(body, "MediaType").and_then(Value::as_str);
    let item_ids = match body_field(body, "Ids") {
        Some(Value::Null) | None => None,
        Some(_) => Some(id_list(&[], body, "Ids")?),
    };
    let shares = playlist_users(body)?;
    let public = flag_param(&[], body, "IsPublic")?;

    ensure_playlist_editable(&state, id, &claims).await?;
    // An editor with parental limits sends the entries they see; the hidden ones stay
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let update = rustyfin_db::repo_playlist::PlaylistUpdate {
        name,
        media_type,
        item_ids: item_ids.as_deref(),
        shares: shares.as_deref(),
        public,
    };
    let caller = playlist_caller(&claims);
    if !rustyfin_db::repo_playlist::update_playlist_with(
        &state.db,
        id,
        &update,
        hidden.ids(),
        caller,
    )
    .await?
    {
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
    ensure_playlist_editable(&state, playlist_id, &claims).await?;
    let caller = playlist_caller(&claims);
    rustyfin_db::repo_playlist::add_items_to_playlist(&state.db, playlist_id, &item_ids, caller)
        .await?;
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
    ensure_playlist_editable(&state, playlist_id, &claims).await?;
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
    ensure_playlist_editable(&state, playlist_id, &claims).await?;
    let item_id = rustyfin_db::repo_playlist::entry_item(playlist_id, entry);
    // The index counts the entries the caller sees: an editor with parental limits sees fewer
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    if hidden.hides(item_id) {
        return Err(ApiError::NotFound("Playlist item not found".to_string()));
    }
    let moved = rustyfin_db::repo_playlist::move_playlist_item(
        &state.db,
        playlist_id,
        item_id,
        new_index,
        hidden.ids(),
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
) -> Result<PlaylistDto, ApiError> {
    rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims))
        .await?
        .ok_or_else(playlist_not_found)
}

/// The check for the routes that change a playlist: 404 unless the caller can see it, and 403
/// unless they may change it (Jellyfin forbids a user it is shared with to read only).
async fn ensure_playlist_editable(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<PlaylistDto, ApiError> {
    let playlist = ensure_playlist_visible(state, id, claims).await?;
    if !playlist.can_edit {
        return Err(ApiError::Forbidden(
            "You may not change this playlist.".to_string(),
        ));
    }
    Ok(playlist)
}

/// GET /Playlists/{id}/Users: the users the playlist is shared with. Its editors only.
async fn get_playlist_users(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<PlaylistUserPermissions>>, ApiError> {
    let playlist = ensure_playlist_editable(&state, id, &claims).await?;
    Ok(Json(playlist.shares))
}

/// GET /Playlists/{id}/Users/{userId}: a user's share of the playlist, as Jellyfin answers it.
/// Its owner, an editor or the user asking about themselves may ask (else 403); a user without a
/// share, the owner too, is a 404.
async fn get_playlist_user(
    State(state): State<AppState>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<PlaylistUserPermissions>, ApiError> {
    let playlist = ensure_playlist_visible(&state, id, &claims).await?;
    if !playlist.can_edit && user_id != claims.sub {
        return Err(ApiError::Forbidden(
            "You may not see this playlist's users.".to_string(),
        ));
    }
    playlist
        .shares
        .into_iter()
        .find(|share| share.user_id == user_id)
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("Playlist user not found".to_string()))
}

/// POST /Playlists/{id}/Users/{userId}?CanEdit= (or `CanEdit` in a JSON body): shares the
/// playlist with a user. Its editors only.
async fn set_playlist_user(
    State(state): State<AppState>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
    Query(params): Params,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let body = optional_body(&body)?;
    let can_edit = flag_param(&params, body.as_ref(), "CanEdit")?.unwrap_or(false);
    ensure_playlist_editable(&state, id, &claims).await?;
    rustyfin_db::repo_playlist::set_share(&state.db, id, user_id, can_edit).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Playlists/{id}/Users/{userId}: stops sharing the playlist with a user. Its editors
/// only.
async fn remove_playlist_user(
    State(state): State<AppState>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<StatusCode, ApiError> {
    ensure_playlist_editable(&state, id, &claims).await?;
    rustyfin_db::repo_playlist::remove_share(&state.db, id, user_id).await?;
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

/// A collection hidden from the caller by parental limits is a 404, as a hidden item is.
pub(crate) async fn container_of(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<Option<Container>, ApiError> {
    if rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .is_some()
    {
        let hidden = HiddenItems::for_caller(state, claims).await?;
        visible_collection(state, &hidden, id)
            .await?
            .ok_or_else(collection_not_found)?;
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

/// GET /Items/{id} for a collection the caller may see, or a playlist they reach. A collection
/// hidden from them is a 404.
pub(crate) async fn container_details(
    state: &AppState,
    id: Uuid,
    claims: &Claims,
) -> Result<Option<MediaItem>, ApiError> {
    if rustyfin_db::repo_collection::get_collection(&state.db_ro, id)
        .await?
        .is_some()
    {
        let hidden = HiddenItems::for_caller(state, claims).await?;
        let c = visible_collection(state, &hidden, id)
            .await?
            .ok_or_else(collection_not_found)?;
        return Ok(Some(collection_item(state, c)));
    }
    let playlist =
        rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims)).await?;
    Ok(playlist.map(|p| {
        // Only its owner (or an administrator) may delete it; an editor may not.
        let can_delete = claims.is_admin || p.user_id == claims.sub;
        let mut item = playlist_item(state, p);
        item.can_delete = can_delete;
        item
    }))
}

/// /Items?IncludeItemTypes=BoxSet,Playlist: the collections the caller may see and the playlists
/// they reach, sorted by name and paged with StartIndex and Limit. Under a library (`ParentId`),
/// only those holding an item of it, so a movie library's Collections tab and a music library's
/// Playlists tab list their own; the other filters don't apply.
pub(crate) async fn container_listing(
    state: &AppState,
    claims: &Claims,
    collections: bool,
    playlists: bool,
    library: Option<Uuid>,
    start: usize,
    limit: usize,
) -> Result<QueryResult, ApiError> {
    let mut all = Vec::new();
    if collections {
        let hidden = HiddenItems::for_caller(state, claims).await?;
        let in_library = match library {
            Some(lib) => {
                Some(rustyfin_db::repo_collection::collections_in_library(&state.db_ro, lib).await?)
            }
            None => None,
        };
        for mut c in rustyfin_db::repo_collection::get_all_collections(&state.db_ro).await? {
            if in_library.as_ref().is_some_and(|ids| !ids.contains(&c.id)) {
                continue;
            }
            if visible_collection_count(state, &hidden, &mut c).await? {
                all.push(collection_item(state, c));
            }
        }
    }
    if playlists {
        let in_library = match library {
            Some(lib) => {
                Some(rustyfin_db::repo_playlist::playlists_in_library(&state.db_ro, lib).await?)
            }
            None => None,
        };
        let hidden = HiddenItems::for_caller(state, claims).await?;
        for mut p in rustyfin_db::repo_playlist::get_playlists(&state.db_ro, claims.sub).await? {
            if in_library.as_ref().is_some_and(|ids| !ids.contains(&p.id)) {
                continue;
            }
            visible_playlist_count(state, claims, &hidden, &mut p).await?;
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
    // A playlist the caller reads (public or shared) but doesn't own
    if rustyfin_db::repo_playlist::get_playlist(&state.db_ro, id, playlist_caller(claims))
        .await?
        .is_some()
    {
        return Err(playlist_delete_forbidden());
    }
    Ok(None)
}

fn playlist_delete_forbidden() -> ApiError {
    ApiError::Forbidden("Only the playlist's owner may delete it.".to_string())
}
