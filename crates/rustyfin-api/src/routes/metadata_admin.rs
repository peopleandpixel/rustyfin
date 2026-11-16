//! An administrator's per-item metadata tools, as Jellyfin has them: refresh with a mode, Identify
//! (`/Items/RemoteSearch`), the provider's images (`/Items/{id}/RemoteImages`) and the metadata
//! editor's save (`POST /Items/{id}`). The provider is the TMDb plugin, through its cached
//! client, so these add no request that a refresh wouldn't send.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rustyfin_core::metadata::RefreshOptions;

use rustyfin_model::image::ImageType;
use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::metadata::{ExternalMetadata, MetadataLookups, RemoteImage, RemoteSearchHit};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::error::ApiError;
use crate::query::{Query, bool_param, count_param, first_value};
use crate::state::AppState;

/// The provider's name in Jellyfin's answers (its TMDb plugin's).
const TMDB_NAME: &str = "TheMovieDb";
/// The image types TMDb has.
const TMDB_IMAGE_TYPES: [&str; 3] = ["Primary", "Backdrop", "Logo"];

/// Administrator-only routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Items/{id}", post(update_item))
        .route("/Items/{id}/Refresh", post(refresh_item))
        .route("/Items/RemoteSearch/Apply/{id}", post(apply_search_result))
        .route("/Items/RemoteSearch/{kind}", post(remote_search))
        .route("/Items/{id}/RemoteImages", get(remote_images))
        .route(
            "/Items/{id}/RemoteImages/Providers",
            get(remote_image_providers),
        )
        .route(
            "/Items/{id}/RemoteImages/Download",
            post(download_remote_image),
        )
}

async fn item_or_404(state: &AppState, id: Uuid) -> Result<MediaItem, ApiError> {
    rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))
}

/// Drops the answers and pictures that show the item's old metadata.
fn item_changed(state: &AppState, id: Uuid) {
    state.cache.invalidate_item(&id.to_string());
    state.image_cache.invalidate_item(&id.to_string());
    rustyfin_core::image_warmer::kick();
}

// ── Refresh ──

/// Jellyfin's MetadataRefreshMode and ImageRefreshMode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshMode {
    None,
    ValidationOnly,
    Default,
    FullRefresh,
}

fn refresh_mode(params: &[(String, String)], key: &str) -> Result<Option<RefreshMode>, ApiError> {
    let Some(value) = first_value(params, key) else {
        return Ok(None);
    };
    let mode = match value.to_ascii_lowercase().as_str() {
        "none" => RefreshMode::None,
        "validationonly" => RefreshMode::ValidationOnly,
        "default" => RefreshMode::Default,
        "fullrefresh" => RefreshMode::FullRefresh,
        _ => return Err(ApiError::BadRequest(format!("invalid {key}: {value}"))),
    };
    Ok(Some(mode))
}

/// What a refresh asks for.
#[derive(Debug, Clone, Copy)]
struct RefreshRequest {
    metadata: RefreshMode,
    images: RefreshMode,
    replace_all_metadata: bool,
    replace_all_images: bool,
}

/// `POST /Items/{id}/Refresh`, with Jellyfin's query: MetadataRefreshMode, ImageRefreshMode,
/// ReplaceAllMetadata and ReplaceAllImages. Without a mode, a full refresh, as before Rustyfin
/// took the modes. Unlike Jellyfin (204 at once, the refresh queued) it answers when the item's
/// refresh is done, so a client can show the result; a folder's scan for new files runs on.
async fn refresh_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let item = item_or_404(&state, id).await?;
    let metadata =
        refresh_mode(&params, "MetadataRefreshMode")?.unwrap_or(RefreshMode::FullRefresh);
    let request = RefreshRequest {
        metadata,
        images: refresh_mode(&params, "ImageRefreshMode")?.unwrap_or(metadata),
        replace_all_metadata: bool_param(&params, "ReplaceAllMetadata")?.unwrap_or(false),
        replace_all_images: bool_param(&params, "ReplaceAllImages")?.unwrap_or(false),
    };
    // An admin's "replace all metadata" asks the providers again, past their answer caches
    if request.replace_all_metadata && request.metadata == RefreshMode::FullRefresh {
        rustyfin_core::metadata::forget_cached_answers(&state.db, &item, &state.plugin_manager)
            .await
            .map_err(ApiError::Internal)?;
    }
    run_refresh(&state, &item, request).await?;
    Ok(Json(json!({ "status": "ok" })))
}

/// Runs a refresh as Jellyfin's modes have it:
/// - None: nothing.
/// - ValidationOnly ("scan for new and updated files" on a folder): the folder's library is
///   scanned.
/// - Default: that scan for a folder, and the sources fill what is missing, without asking a
///   provider again about a season or episode it answered.
/// - FullRefresh ("search for missing metadata"): the sources are asked again and fill what is
///   missing; with ReplaceAllMetadata ("replace all metadata") they replace every field that is
///   not locked.
///
/// ReplaceAllImages replaces the providers' and the admin's images when ImageRefreshMode is
/// FullRefresh, and also tries failed chapter images and trickplay again.
async fn run_refresh(
    state: &AppState,
    item: &MediaItem,
    request: RefreshRequest,
) -> Result<(), ApiError> {
    if request.replace_all_images {
        rustyfin_db::repo_trickplay::clear_failures(&state.db, item.id).await?;
    }
    if matches!(
        request.metadata,
        RefreshMode::ValidationOnly | RefreshMode::Default
    ) && is_folder(item.media_type)
    {
        let state = state.clone();
        let library = item.library_id;
        tokio::spawn(async move {
            if let Err(e) = super::library::scan_library(State(state), Path(library)).await {
                tracing::warn!("scan of library {library} for a refresh failed: {e:?}");
            }
        });
    }
    let base = match request.metadata {
        RefreshMode::None | RefreshMode::ValidationOnly => return Ok(()),
        RefreshMode::Default => RefreshOptions::SCAN,
        RefreshMode::FullRefresh => RefreshOptions::EXPLICIT,
    };
    let options = RefreshOptions {
        replace_metadata: request.replace_all_metadata,
        replace_images: request.replace_all_images && request.images == RefreshMode::FullRefresh,
        ..base
    };
    rustyfin_core::metadata::refresh_item_metadata_with(
        &state.db,
        &state.http_client,
        item,
        &state.plugin_manager,
        options,
    )
    .await
    .map_err(ApiError::Internal)?;
    item_changed(state, item.id);
    Ok(())
}

/// The types whose refresh in Jellyfin also looks for new and changed files under them.
fn is_folder(media_type: MediaType) -> bool {
    matches!(
        media_type,
        MediaType::Series | MediaType::Season | MediaType::MusicAlbum | MediaType::MusicArtist
    )
}

// ── Identify ──

/// The active provider with lookups (TMDb), if there is one.
async fn lookups(state: &AppState) -> Option<Arc<dyn MetadataLookups>> {
    state
        .plugin_manager
        .metadata_providers()
        .await
        .into_iter()
        .find_map(|provider| provider.lookups)
}

/// A JSON object's value for `key`, the key matched without case as ASP.NET binds it.
fn field<'a>(object: &'a Value, key: &str) -> Option<&'a Value> {
    object
        .as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value)
}

fn field_str<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    field(object, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The TMDb id in a ProviderIds object.
fn tmdb_id_of(object: &Value) -> Option<i64> {
    let ids = field(object, "ProviderIds")?;
    let value = field(ids, "Tmdb")?;
    match value {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// A date as Jellyfin writes it.
fn jellyfin_date(date: &str) -> Option<String> {
    let day = chrono::NaiveDate::parse_from_str(date.get(..10)?, "%Y-%m-%d").ok()?;
    Some(format!("{}T00:00:00.0000000Z", day.format("%Y-%m-%d")))
}

/// Jellyfin's RemoteSearchResult for a hit.
fn search_result(hit: &RemoteSearchHit) -> Value {
    let mut result = Map::new();
    result.insert("Name".into(), json!(hit.name));
    result.insert("ProviderIds".into(), json!({ "Tmdb": hit.id.to_string() }));
    if let Some(year) = hit.production_year {
        result.insert("ProductionYear".into(), json!(year));
    }
    if let Some(date) = hit.premiere_date.as_deref().and_then(jellyfin_date) {
        result.insert("PremiereDate".into(), json!(date));
    }
    if let Some(url) = &hit.image_url {
        result.insert("ImageUrl".into(), json!(url));
    }
    result.insert("SearchProviderName".into(), json!(TMDB_NAME));
    if let Some(overview) = &hit.overview {
        result.insert("Overview".into(), json!(overview));
    }
    Value::Object(result)
}

/// A by-id lookup's answer as a search hit.
fn hit_of(id: i64, meta: &ExternalMetadata) -> RemoteSearchHit {
    RemoteSearchHit {
        id,
        name: meta.name.clone().unwrap_or_default(),
        production_year: meta.production_year,
        premiere_date: meta.premiere_date.clone(),
        overview: meta.overview.clone(),
        image_url: meta
            .image_urls
            .iter()
            .find(|image| image.image_type == "Primary")
            .map(|image| image.url.clone()),
    }
}

/// `POST /Items/RemoteSearch/{Movie|Series|...}`: the provider's matches for
/// `{SearchInfo: {Name, Year, ProviderIds}, ItemId}`. A TMDb id in ProviderIds is looked up
/// directly, as Jellyfin's TMDb plugin does. Types other than Movie and Series have no provider
/// here and get none.
async fn remote_search(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let series = if kind.eq_ignore_ascii_case("Series") {
        true
    } else if kind.eq_ignore_ascii_case("Movie") {
        false
    } else {
        return Ok(Json(Vec::new()));
    };
    if field_str(&body, "SearchProviderName").is_some_and(|name| name != TMDB_NAME) {
        return Ok(Json(Vec::new()));
    }
    let Some(lookups) = lookups(&state).await else {
        return Ok(Json(Vec::new()));
    };
    let info = field(&body, "SearchInfo").cloned().unwrap_or(Value::Null);
    let hits = if let Some(id) = tmdb_id_of(&info) {
        let found = if series {
            lookups.series_by_id(id).await
        } else {
            lookups.movie_by_id(id).await
        };
        found_or_log(found, "lookup")
            .map(|meta| vec![hit_of(id, &meta)])
            .unwrap_or_default()
    } else {
        let Some(name) = field_str(&info, "Name") else {
            return Err(ApiError::BadRequest("SearchInfo.Name is required".into()));
        };
        let year = field(&info, "Year")
            .and_then(Value::as_i64)
            .and_then(|y| i32::try_from(y).ok());
        found_or_log(lookups.search_all(series, name, year).await, "search").unwrap_or_default()
    };
    Ok(Json(hits.iter().map(search_result).collect()))
}

/// A lookup's answer; a failure is logged and counts as none.
fn found_or_log<T>(result: rustyfin_model::metadata::LookupResult<T>, what: &str) -> Option<T> {
    result.unwrap_or_else(|e| {
        tracing::warn!("{TMDB_NAME} {what} for Identify failed: {e:?}");
        None
    })
}

/// `POST /Items/RemoteSearch/Apply/{id}?ReplaceAllImages=true`: the item becomes the chosen
/// result (its TMDb id), and a full refresh replaces its metadata, except the locked fields, and
/// (by default, as in Jellyfin) its images. 204.
async fn apply_search_result(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    let item = item_or_404(&state, id).await?;
    if !matches!(item.media_type, MediaType::Movie | MediaType::Series) {
        return Err(ApiError::BadRequest(
            "only a movie or a series can be identified".into(),
        ));
    }
    let tmdb_id = tmdb_id_of(&body)
        .ok_or_else(|| ApiError::BadRequest("the result has no Tmdb provider id".into()))?;
    // The provider must answer for the new id before anything changes: on a failure the item
    // keeps its id and metadata. The answer is cached for the refresh below.
    let lookups = lookups(&state)
        .await
        .ok_or_else(|| ApiError::ServiceUnavailable("no metadata provider is enabled".into()))?;
    let answer = if item.media_type == MediaType::Series {
        lookups.series_by_id(tmdb_id).await
    } else {
        lookups.movie_by_id(tmdb_id).await
    };
    match answer {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(ApiError::BadRequest(format!(
                "{TMDB_NAME} has no item with id {tmdb_id}"
            )));
        }
        Err(e) => {
            return Err(ApiError::BadGateway(format!(
                "{TMDB_NAME} did not answer: {e:?}"
            )));
        }
    }
    rustyfin_db::repo_media::set_tmdb_id(&state.db, id, tmdb_id).await?;
    let item = item_or_404(&state, id).await?;
    let request = RefreshRequest {
        metadata: RefreshMode::FullRefresh,
        images: RefreshMode::FullRefresh,
        replace_all_metadata: true,
        replace_all_images: bool_param(&params, "ReplaceAllImages")?.unwrap_or(true),
    };
    run_refresh(&state, &item, request).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Remote images ──

/// The provider's images for an item it knows: a movie or series with a TMDb id.
async fn provider_images(
    state: &AppState,
    item: &MediaItem,
) -> Option<(Arc<dyn MetadataLookups>, i64, bool)> {
    let series = match item.media_type {
        MediaType::Movie => false,
        MediaType::Series => true,
        _ => return None,
    };
    let id = item.tmdb_id?;
    Some((lookups(state).await?, id, series))
}

fn remote_image_json(image: &RemoteImage) -> Value {
    let mut out = Map::new();
    out.insert("ProviderName".into(), json!(TMDB_NAME));
    out.insert("Url".into(), json!(image.url));
    if let Some(thumb) = &image.thumbnail_url {
        out.insert("ThumbnailUrl".into(), json!(thumb));
    }
    if let Some(h) = image.height {
        out.insert("Height".into(), json!(h));
    }
    if let Some(w) = image.width {
        out.insert("Width".into(), json!(w));
    }
    if let Some(r) = image.community_rating {
        out.insert("CommunityRating".into(), json!(r));
    }
    if let Some(v) = image.vote_count {
        out.insert("VoteCount".into(), json!(v));
    }
    if let Some(l) = &image.language {
        out.insert("Language".into(), json!(l));
    }
    out.insert("Type".into(), json!(image.image_type));
    out.insert("RatingType".into(), json!("Score"));
    Value::Object(out)
}

/// `GET /Items/{id}/RemoteImages?type=&startIndex=&limit=&providerName=&includeAllLanguages=`:
/// Jellyfin's RemoteImageResult. Without includeAllLanguages, only images in English or without
/// text, as Jellyfin keeps the metadata language's.
async fn remote_images(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let item = item_or_404(&state, id).await?;
    let wanted_type = first_value(&params, "type");
    let all_languages = bool_param(&params, "includeAllLanguages")?.unwrap_or(false);
    let start = count_param(&params, "startIndex")?.unwrap_or(0);
    let limit = count_param(&params, "limit")?;
    let provider_ok = first_value(&params, "providerName")
        .is_none_or(|name| name.eq_ignore_ascii_case(TMDB_NAME));

    let mut providers = Vec::new();
    let mut images = Vec::new();
    if let Some((lookups, tmdb_id, series)) = provider_images(&state, &item).await {
        providers.push(TMDB_NAME);
        if provider_ok {
            images =
                found_or_log(lookups.images(series, tmdb_id).await, "images").unwrap_or_default();
        }
    }
    images.retain(|image| {
        wanted_type.is_none_or(|t| image.image_type.eq_ignore_ascii_case(t))
            && (all_languages || image.language.as_deref().is_none_or(|l| l == "en"))
    });
    let total = images.len();
    let page: Vec<Value> = images
        .iter()
        .skip(start)
        .take(limit.unwrap_or(usize::MAX))
        .map(remote_image_json)
        .collect();
    Ok(Json(json!({
        "Images": page,
        "TotalRecordCount": total,
        "Providers": providers,
    })))
}

/// `GET /Items/{id}/RemoteImages/Providers`: Jellyfin's ImageProviderInfo list.
async fn remote_image_providers(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let item = item_or_404(&state, id).await?;
    let known = matches!(item.media_type, MediaType::Movie | MediaType::Series)
        && lookups(&state).await.is_some();
    Ok(Json(if known {
        vec![json!({ "Name": TMDB_NAME, "SupportedImages": TMDB_IMAGE_TYPES })]
    } else {
        Vec::new()
    }))
}

/// The index a new image of `image_type` takes: a backdrop or screenshot is added after the
/// item's others, as in Jellyfin; any other type replaces the item's one.
pub(crate) async fn new_image_index(
    state: &AppState,
    id: Uuid,
    image_type: ImageType,
) -> Result<i32, ApiError> {
    if !matches!(image_type, ImageType::Backdrop | ImageType::Screenshot) {
        return Ok(0);
    }
    let images = rustyfin_db::repo_image::get_images_for_item(&state.db, id).await?;
    Ok(images
        .iter()
        .filter(|image| image.image_type == image_type)
        .map(|image| image.image_index + 1)
        .max()
        .unwrap_or(0))
}

/// The most a downloaded image may weigh.
const DOWNLOAD_MAX_BYTES: usize = 30 * 1024 * 1024;

/// The origins an admin may download an image from: the image provider's (TMDb's image host and
/// the configured image base URL). Nothing else, so the server fetches no address an admin
/// names, such as a cloud metadata service or a LAN host.
fn image_provider_origins(state: &AppState) -> Vec<reqwest::Url> {
    ["https://image.tmdb.org/", state.tmdb.image_base_url()]
        .iter()
        .filter_map(|url| reqwest::Url::parse(url).ok())
        .collect()
}

fn same_origin(allowed: &[reqwest::Url], url: &reqwest::Url) -> bool {
    allowed.iter().any(|a| a.origin() == url.origin())
}

/// The image at `url`, fetched once with redirects followed only within the allowed origins.
async fn fetch_provider_image(state: &AppState, url: &str) -> Result<Vec<u8>, ApiError> {
    let allowed = image_provider_origins(state);
    let parsed = reqwest::Url::parse(url)
        .ok()
        .filter(|u| same_origin(&allowed, u))
        .ok_or_else(|| ApiError::BadRequest("imageUrl is not an image provider's".into()))?;
    let redirects = allowed.clone();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if same_origin(&redirects, attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(rustyfin_core::tmdb_client::USER_AGENT)
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| ApiError::BadGateway(format!("the image could not be fetched: {e}")))?;
    if !resp.status().is_success() {
        return Err(ApiError::BadGateway(format!(
            "the image host answered {}",
            resp.status()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| ApiError::BadGateway(format!("the image could not be fetched: {e}")))?
    {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > DOWNLOAD_MAX_BYTES {
            return Err(ApiError::PayloadTooLarge("the image is too large".into()));
        }
    }
    if !super::images::looks_like_image(&bytes) {
        return Err(ApiError::BadGateway("the image host sent no image".into()));
    }
    Ok(bytes)
}

/// `POST /Items/{id}/RemoteImages/Download?type=&imageUrl=`: as in Jellyfin, the server downloads
/// the image once, now, and stores the file as the item's (a backdrop is added). Only the image
/// provider's hosts are allowed. A refresh keeps it until one replaces all images. 204.
async fn download_remote_image(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<StatusCode, ApiError> {
    item_or_404(&state, id).await?;
    let type_name = first_value(&params, "type")
        .ok_or_else(|| ApiError::BadRequest("type is required".into()))?;
    let image_type = ImageType::from_str(type_name)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown image type: {type_name}")))?;
    let url = first_value(&params, "imageUrl")
        .ok_or_else(|| ApiError::BadRequest("imageUrl is required".into()))?;
    let bytes = fetch_provider_image(&state, url).await?;
    super::images::save_item_image(&state, id, image_type, &bytes, "user").await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Metadata editor ──

/// A date-time or date value as UTC midnight of its day, or none.
fn date_value(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let text = value.as_str()?.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    let day = chrono::NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()?;
    Some(day.and_time(chrono::NaiveTime::MIN).and_utc())
}

/// A string list value; `None` for null or anything else.
fn string_list(value: &Value) -> Option<Vec<String>> {
    let list = value.as_array()?;
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let Some(text) = entry.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !out.iter().any(|seen| seen.eq_ignore_ascii_case(text)) {
            out.push(text.to_string());
        }
    }
    Some(out)
}

fn text_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `POST /Items/{id}`: saves the metadata editor's BaseItemDto, as Jellyfin's ItemUpdateController
/// does for the fields Rustyfin stores: Name, ForcedSortName, OriginalTitle, Overview, Taglines,
/// ProductionYear, PremiereDate, EndDate, CommunityRating, CriticRating, OfficialRating, Genres,
/// Tags, ProductionLocations, Studios, People, LockData and LockedFields. An empty ForcedSortName
/// sets the sort name back to the name's. Unlike Jellyfin, a field missing from the
/// body is kept (null clears it), so a client sending only some fields changes only those; a
/// client sending the whole item, as Jellyfin Web does, gets Jellyfin's result. 204.
async fn update_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    if !body.is_object() {
        return Err(ApiError::BadRequest("the body must be an item".into()));
    }
    let item = item_or_404(&state, id).await?;
    let mut updated = item.clone();

    if let Some(name) = field(&body, "Name").and_then(text_value) {
        updated.name = name;
    }
    // A ForcedSortName sets the forced sort name and an empty one clears it. The sort name is
    // made from the forced one, or else from the name, as Jellyfin's SortName is; a season keeps
    // its own.
    let forced_now = rustyfin_db::repo_media::forced_sort_name(&state.db, id).await?;
    let forced = match field(&body, "ForcedSortName") {
        Some(value) => text_value(value),
        None => forced_now.clone(),
    };

    if forced.is_some() || item.media_type != MediaType::Season {
        updated.sort_name = Some(match forced.as_deref() {
            Some(forced) => rustyfin_model::sort_name::forced_sort_name(forced),
            None => rustyfin_model::sort_name::item_sort_name(
                updated.media_type,
                &updated.name,
                updated.parent_index_number,
                updated.index_number,
                &rustyfin_model::sort_name::current_rules(),
            ),
        });
    }
    if let Some(value) = field(&body, "Overview") {
        updated.overview = text_value(value);
    }
    if let Some(value) = field(&body, "Taglines") {
        updated.taglines = string_list(value).unwrap_or_default();
    }
    if let Some(value) = field(&body, "ProductionYear") {
        updated.production_year = value.as_i64().and_then(|y| i32::try_from(y).ok());
    }
    if let Some(value) = field(&body, "PremiereDate") {
        updated.premiere_date = date_value(value);
    }
    if let Some(value) = field(&body, "EndDate") {
        updated.end_date = date_value(value);
    }
    if let Some(value) = field(&body, "CommunityRating") {
        updated.community_rating = value.as_f64();
    }
    if let Some(value) = field(&body, "OfficialRating") {
        updated.official_rating = text_value(value);
    }
    // Lists, as in Jellyfin, change only when given
    if let Some(list) = field(&body, "Genres").and_then(string_list) {
        updated.genres = list;
    }
    if let Some(list) = field(&body, "Tags").and_then(string_list) {
        updated.tags = list;
    }
    if let Some(list) = field(&body, "ProductionLocations").and_then(string_list) {
        updated.production_locations = list;
    }
    // The forced sort name and the sort name made from it are saved together: when another
    // write changed the name or sort name since the read, neither is.
    let mut tx = rustyfin_db::begin_write(&state.db).await?;
    let written =
        rustyfin_db::repo_media::save_refresh_changes_on(&mut tx, &item, &updated).await?;
    let sort_saved = updated.sort_name == item.sort_name || written.contains(&"sort_name");
    if forced != forced_now && sort_saved {
        rustyfin_db::repo_media::set_forced_sort_name_on(&mut tx, id, forced.as_deref()).await?;
    }
    tx.commit().await?;

    // Studios ({Name, Id} pairs) and People ({Name, Role, Type}), when given, replace the item's
    if let Some(list) = field(&body, "Studios").and_then(Value::as_array) {
        let names: Vec<String> = list
            .iter()
            .filter_map(|studio| field_str(studio, "Name").or_else(|| studio.as_str()))
            .map(str::to_string)
            .collect();
        rustyfin_db::repo_media::replace_item_studios(&state.db, id, &names).await?;
    }
    if let Some(list) = field(&body, "People").and_then(Value::as_array) {
        let people: Vec<rustyfin_model::metadata::MetadataPerson> = list
            .iter()
            .filter_map(|person| {
                Some(rustyfin_model::metadata::MetadataPerson {
                    name: field_str(person, "Name")?.to_string(),
                    kind: field_str(person, "Type").unwrap_or("Actor").to_string(),
                    role: field_str(person, "Role").map(str::to_string),
                    image_url: None,
                })
            })
            .collect();
        rustyfin_db::repo_person::replace_item_people(&state.db, id, &people).await?;
    }

    let extras = rustyfin_db::repo_jellyfin_import::item_extras(&state.db, id)
        .await?
        .unwrap_or_default();
    let mut locks = rustyfin_db::repo_media::item_locks(&state.db, id).await?;
    if let Some(value) = field(&body, "LockData") {
        locks.lock_data = value.as_bool().unwrap_or(false);
    }
    if let Some(list) = field(&body, "LockedFields").and_then(string_list) {
        locks.fields = list;
    }
    let original_title = match field(&body, "OriginalTitle") {
        Some(value) => text_value(value),
        None => extras.original_title,
    };
    let critic_rating = match field(&body, "CriticRating") {
        Some(value) => value.as_f64(),
        None => extras.critic_rating,
    };
    rustyfin_db::repo_media::save_item_extras_edit(
        &state.db,
        id,
        original_title.as_deref(),
        critic_rating,
        &locks,
    )
    .await?;
    item_changed(&state, id);
    Ok(StatusCode::NO_CONTENT)
}
