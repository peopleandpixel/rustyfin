use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DeviceDto {
    id: String,
    name: String,
    #[serde(rename = "AppName")]
    app_name: String,
    #[serde(rename = "AppVersion")]
    app_version: String,
    #[serde(rename = "DeviceName")]
    device_name: String,
    #[serde(rename = "LastActivityDate")]
    last_activity_date: String,
    #[serde(rename = "UserId", skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    #[serde(rename = "Image", skip_serializing_if = "Option::is_none")]
    image: Option<String>,
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

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Devices", get(get_devices).post(create_device))
        .route("/Devices/{deviceId}", get(get_device).delete(delete_device))
        .route("/Devices/{deviceId}/Image", post(device_image_placeholder))
}

async fn get_devices(State(state): State<AppState>) -> Result<Json<Vec<DeviceDto>>, ApiError> {
    let devices = rustyfin_db::repo_device::get_devices(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<DeviceDto> = devices
        .into_iter()
        .map(|d| {
            let has_image = !d.image_path.is_empty();
            DeviceDto {
                id: d.id.clone(),
                name: d.name,
                app_name: d.app_name,
                app_version: d.app_version,
                device_name: d.device_name,
                last_activity_date: d.last_activity_date,
                user_id: d.user_id,
                image: if has_image {
                    Some(format!("/Devices/{}/Image", d.id))
                } else {
                    None
                },
            }
        })
        .collect();

    Ok(Json(items))
}

async fn get_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<DeviceDto>, ApiError> {
    let device = rustyfin_db::repo_device::get_device(&state.db_ro, &device_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Device not found".to_string()))?;

    let has_image = !device.image_path.is_empty();
    Ok(Json(DeviceDto {
        id: device.id.clone(),
        name: device.name,
        app_name: device.app_name,
        app_version: device.app_version,
        device_name: device.device_name,
        last_activity_date: device.last_activity_date,
        user_id: device.user_id,
        image: if has_image {
            Some(format!("/Devices/{}/Image", device.id))
        } else {
            None
        },
    }))
}

async fn create_device(
    State(state): State<AppState>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<StatusCode, ApiError> {
    let id = if req.id.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        req.id.clone()
    };

    rustyfin_db::repo_device::create_device(
        &state.db,
        &id,
        &req.name,
        None,
        &req.app_name,
        &req.app_version,
        &req.device_name,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn delete_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_device::delete_device(&state.db, &device_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

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
