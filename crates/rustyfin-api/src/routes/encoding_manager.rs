use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MediaEncoderPath {
    value: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/System/MediaEncoder/Path", get(get_encoder_path))
        .route("/System/MediaEncoder/Test", post(test_encoder))
        .route(
            "/System/MediaEncoder/HardwareAcceleration",
            get(get_hardware_acceleration),
        )
        .route(
            "/System/MediaEncoder/HardwareAcceleration/Detect",
            post(detect_hardware_acceleration),
        )
}

/// The hardware detection, for the dashboard's transcoding page (Rustyfin's own; Jellyfin has
/// no detection).
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct HardwareAcceleration {
    /// True while the administrator hasn't chosen a `HardwareAccelerationType`: the selected
    /// device is the one in use
    automatic: bool,
    /// A detection is running
    detecting: bool,
    /// The last detection's devices, selection and time; null before the first one ends
    detection: Option<rustyfin_core::hwaccel::HardwareDetection>,
}

async fn hardware_acceleration(state: &AppState) -> HardwareAcceleration {
    let saved = rustyfin_core::encoding::load_saved(&state.data_dir);
    HardwareAcceleration {
        automatic: rustyfin_core::hwaccel::type_is_automatic(saved.as_ref()),
        detecting: state.transcode_manager.detecting_hardware(),
        detection: state.transcode_manager.hardware_detection(),
    }
}

/// GET /System/MediaEncoder/HardwareAcceleration: what the last detection found.
async fn get_hardware_acceleration(State(state): State<AppState>) -> Json<HardwareAcceleration> {
    Json(hardware_acceleration(&state).await)
}

/// POST /System/MediaEncoder/HardwareAcceleration/Detect: detect the devices again (a few
/// seconds of ffmpeg probes) and answer with the result; 409 while a detection is running.
async fn detect_hardware_acceleration(
    State(state): State<AppState>,
) -> Result<Json<HardwareAcceleration>, ApiError> {
    state
        .transcode_manager
        .try_detect_hardware(&state.data_dir)
        .await
        .ok_or_else(|| ApiError::Conflict("a hardware detection is already running".into()))?;
    Ok(Json(hardware_acceleration(&state).await))
}

async fn get_encoder_path() -> Json<MediaEncoderPath> {
    Json(MediaEncoderPath {
        value: rustyfin_core::media_tools::ffmpeg()
            .to_string_lossy()
            .into_owned(),
    })
}

async fn test_encoder() -> Result<StatusCode, ApiError> {
    // Test FFmpeg by running a simple command. A hung child is killed after 10 seconds.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new(rustyfin_core::media_tools::ffmpeg())
            .arg("-version")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| ApiError::Internal("FFmpeg test timed out.".to_string()))?;

    match output {
        Ok(output) if output.status.success() => Ok(StatusCode::OK),
        _ => Err(ApiError::Internal("FFmpeg test failed".to_string())),
    }
}
