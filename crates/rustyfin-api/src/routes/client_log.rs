//! Jellyfin's client log upload, `POST /ClientLog/Document`. Web and mobile clients send their
//! console or crash log as plain text, and it is kept in the server's log directory, where
//! `GET /System/Logs` lists it. Jellyfin's `AllowClientLogUpload` is always on here, because the
//! server configuration has no field for it.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::routing::post;
use axum::{Extension, Json, Router};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::state::AppState;

/// Jellyfin's `MaxDocumentSize`: a larger upload gets 413.
const MAX_DOCUMENT_BYTES: usize = 1_000_000;

/// The longest client or version name kept in a file name.
const MAX_NAME_CHARS: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/ClientLog/Document",
        post(log_document).layer(DefaultBodyLimit::max(MAX_DOCUMENT_BYTES)),
    )
}

/// POST /ClientLog/Document
async fn log_document(
    State(state): State<AppState>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err(ApiError::PayloadTooLarge(format!(
            "a client log may be at most {MAX_DOCUMENT_BYTES} bytes"
        )));
    }

    // The client and version the caller's token was issued to, as Jellyfin names the file.
    let (client, version) = match token {
        Some(Extension(rt)) => match rustyfin_auth::token_device(&state.db, &rt.0).await? {
            Some((_, device)) => (device.app_name, device.app_version),
            None => (String::new(), String::new()),
        },
        None => (String::new(), String::new()),
    };
    let client = sanitise_name(&client);
    let version = sanitise_name(&version);

    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let unique = uuid::Uuid::new_v4().simple();
    let file_name = format!("upload_{client}_{version}_{now}_{unique}.log");

    let log_dir = state.data_dir.join("logs");
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("the log directory could not be created: {e}")))?;
    tokio::fs::write(log_dir.join(&file_name), &body)
        .await
        .map_err(|e| ApiError::Internal(format!("the client log could not be written: {e}")))?;

    Ok(Json(json!({ "FileName": file_name })))
}

/// Keeps only ASCII alphanumerics, `.` and `-` and turns every other character into `_`. A `..`
/// pair becomes `_` so that no traversal can appear, and the result is at most 64 characters.
/// An empty result becomes `unknown`.
fn sanitise_name(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .replace("..", "_");
    let kept: String = mapped.chars().take(MAX_NAME_CHARS).collect();
    if kept.is_empty() {
        "unknown".to_string()
    } else {
        kept
    }
}
