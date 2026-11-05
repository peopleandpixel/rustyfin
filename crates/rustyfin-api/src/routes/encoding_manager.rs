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
}

async fn get_encoder_path() -> Json<MediaEncoderPath> {
    Json(MediaEncoderPath {
        value: "ffmpeg".to_string(),
    })
}

async fn test_encoder() -> Result<StatusCode, ApiError> {
    // Test FFmpeg by running a simple command. A hung child is killed after 10 seconds.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("ffmpeg")
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
