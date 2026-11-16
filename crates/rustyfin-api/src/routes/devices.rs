use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_db::repo_device::DeviceRow;
use rustyfin_model::auth::Claims;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;
use crate::query;
use crate::state::AppState;

/// Jellyfin's `DeviceInfoDto`. Its `AccessToken` is left out: the device list never hands out
/// another device's token.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceDto {
    id: String,
    name: String,
    custom_name: Option<String>,
    app_name: String,
    app_version: String,
    last_user_id: Option<String>,
    last_user_name: Option<String>,
    date_last_activated: String,
    capabilities: serde_json::Value,
    icon_url: Option<String>,
}

impl From<DeviceRow> for DeviceDto {
    fn from(d: DeviceRow) -> Self {
        Self {
            id: d.id,
            name: d.name,
            custom_name: d.custom_name,
            app_name: d.app_name,
            app_version: d.app_version,
            last_user_id: d.user_id,
            last_user_name: d.user_name,
            date_last_activated: d.last_seen,
            capabilities: json!({
                "PlayableMediaTypes": [],
                "SupportedCommands": [],
                "SupportsMediaControl": false,
                "SupportsPersistentIdentifier": true,
            }),
            icon_url: None,
        }
    }
}

/// Jellyfin's `DeviceOptionsDto`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceOptionsDto {
    id: i64,
    device_id: String,
    custom_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceOptionsUpdate {
    #[serde(default)]
    custom_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateDeviceRequest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    app_name: String,
    #[serde(default)]
    app_version: String,
    #[serde(default)]
    device_name: String,
}

/// Administrator-only device routes. `routes::build_router` wraps them in `require_admin`;
/// `DELETE /Devices?id=` is in `session::admin_routes`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Devices", get(get_devices).post(create_device))
        .route("/Devices/Info", get(get_device_info))
        .route(
            "/Devices/Options",
            get(get_device_options).post(update_device_options),
        )
        .route("/Devices/{deviceId}", get(get_device).delete(delete_device))
        .route("/Devices/{deviceId}/Image", post(device_image_placeholder))
}

/// GET /Devices - Jellyfin's `QueryResult<DeviceInfoDto>`; `userId` keeps that user's devices.
async fn get_devices(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = query::uuid_param(&params, "userId")?.map(|u| u.to_string());
    let devices = rustyfin_db::repo_device::get_devices(&state.db_ro, user_id.as_deref()).await?;
    let items: Vec<DeviceDto> = devices.into_iter().map(DeviceDto::from).collect();
    Ok(Json(json!({
        "TotalRecordCount": items.len(),
        "StartIndex": 0,
        "Items": items,
    })))
}

/// The `id` query parameter of the device routes, required.
fn device_id_param(params: &[(String, String)]) -> Result<String, ApiError> {
    query::first_value(params, "id")
        .map(str::to_string)
        .ok_or_else(|| ApiError::BadRequest("id is required".to_string()))
}

async fn find_device(state: &AppState, device_id: &str) -> Result<DeviceRow, ApiError> {
    rustyfin_db::repo_device::get_device(&state.db_ro, device_id)
        .await?
        .ok_or(ApiError::NotFound("Device not found".to_string()))
}

/// GET /Devices/Info?id= - one device, as Jellyfin
async fn get_device_info(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<DeviceDto>, ApiError> {
    let id = device_id_param(&params)?;
    Ok(Json(find_device(&state, &id).await?.into()))
}

/// GET /Devices/Options?id= - the device's custom name
async fn get_device_options(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<DeviceOptionsDto>, ApiError> {
    let id = device_id_param(&params)?;
    let device = find_device(&state, &id).await?;
    Ok(Json(DeviceOptionsDto {
        id: 0,
        device_id: device.id,
        custom_name: device.custom_name,
    }))
}

/// The longest custom device name, in bytes.
const MAX_CUSTOM_NAME_BYTES: usize = 256;

/// POST /Devices/Options?id= - sets the device's custom name (204); 400 over 256 bytes
async fn update_device_options(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Json(body): Json<DeviceOptionsUpdate>,
) -> Result<StatusCode, ApiError> {
    let id = device_id_param(&params)?;
    if body
        .custom_name
        .as_ref()
        .is_some_and(|n| n.trim().len() > MAX_CUSTOM_NAME_BYTES)
    {
        return Err(ApiError::BadRequest(format!(
            "CustomName: longer than {MAX_CUSTOM_NAME_BYTES} bytes"
        )));
    }
    if !rustyfin_db::repo_device::set_custom_name(&state.db, &id, body.custom_name.as_deref())
        .await?
    {
        return Err(ApiError::NotFound("Device not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceDto>, ApiError> {
    Ok(Json(find_device(&state, &device_id).await?.into()))
}

/// POST /Devices - adds a device for the caller
async fn create_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<StatusCode, ApiError> {
    let id = if req.id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        req.id.clone()
    };
    let name = if req.name.is_empty() {
        &req.device_name
    } else {
        &req.name
    };

    rustyfin_db::repo_device::create_device(
        &state.db,
        &id,
        name,
        Some(&claims.sub.to_string()),
        &req.app_name,
        &req.app_version,
    )
    .await?;

    Ok(StatusCode::OK)
}

async fn delete_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Sign the device out like DELETE /Devices?id=: its sessions, its tokens, then the row
    let ended = rustyfin_db::repo_session::session_ids(&state.db, None, Some(&device_id)).await?;
    rustyfin_db::repo_session::delete_device(&state.db, &device_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::sessions::forget_all(&ended);
    crate::sessions::changed(&state);

    Ok(StatusCode::OK)
}

async fn device_image_placeholder(
    State(_state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Image upload placeholder - multipart not available without feature flag
    tracing::info!("Device image upload requested for {}", device_id);
    Ok(StatusCode::OK)
}
