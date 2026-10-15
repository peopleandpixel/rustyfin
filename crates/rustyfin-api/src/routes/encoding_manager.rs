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
        .route(
            "/System/MediaEncoder/Path",
            get(get_encoder_path).put(set_encoder_path),
        )
        .route(
            "/System/MediaEncoder/Path/Validate",
            post(validate_encoder_path),
        )
        .route("/System/MediaEncoder/Test", post(test_encoder))
}

async fn get_encoder_path() -> Json<MediaEncoderPath> {
    Json(MediaEncoderPath {
        value: "ffmpeg".to_string(),
    })
}

async fn set_encoder_path(
    Json(req): Json<MediaEncoderPath>,
) -> Result<Json<MediaEncoderPath>, ApiError> {
    // Validate that the path exists
    if !req.value.is_empty() {
        let output = tokio::process::Command::new(&req.value)
            .arg("-version")
            .output()
            .await;
        if let Err(e) = output {
            return Err(ApiError::BadRequest(format!(
                "FFmpeg path is not valid: {}",
                e
            )));
        }
    }
    Ok(Json(MediaEncoderPath {
        value: req.value.clone(),
    }))
}

async fn validate_encoder_path(
    Json(req): Json<MediaEncoderPath>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.value.is_empty() {
        return Ok(Json(serde_json::json!({
            "IsValid": false,
            "ErrorMessage": "FFmpeg path is required"
        })));
    }

    let output = tokio::process::Command::new(&req.value)
        .arg("-version")
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => Ok(Json(serde_json::json!({
            "IsValid": true,
            "ErrorMessage": ""
        }))),
        Ok(_) => Ok(Json(serde_json::json!({
            "IsValid": false,
            "ErrorMessage": "FFmpeg returned non-zero exit code"
        }))),
        Err(e) => Ok(Json(serde_json::json!({
            "IsValid": false,
            "ErrorMessage": format!("FFmpeg not found: {}", e)
        }))),
    }
}

async fn test_encoder() -> Result<StatusCode, ApiError> {
    // Test FFmpeg by running a simple command
    let output = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => Ok(StatusCode::OK),
        _ => Err(ApiError::Internal("FFmpeg test failed".to_string())),
    }
}
