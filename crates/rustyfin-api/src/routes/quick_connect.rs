use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use rustyfin_model::auth::Claims;
use rustyfin_model::quick_connect::QuickConnectResult;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Expiry duration for quick connect codes (5 minutes).
const EXPIRY_SECS: i64 = 300;

/// Public routes (no auth required).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/QuickConnect/Enabled", get(quick_connect_enabled))
        .route("/QuickConnect/Initiate", post(initiate))
        .route("/QuickConnect/Connect", get(connect))
}

/// Protected routes (auth required).
pub fn routes() -> Router<AppState> {
    Router::new().route("/QuickConnect/Authorize", post(authorize))
}

/// `GET /QuickConnect/Enabled` – whether Quick Connect is enabled.
async fn quick_connect_enabled() -> Json<bool> {
    Json(true)
}

/// `POST /QuickConnect/Initiate` – create a new quick connect request.
async fn initiate(State(state): State<AppState>) -> Result<Json<QuickConnectResult>, ApiError> {
    let secret = Uuid::new_v4().to_string().replace("-", "");
    // Generate a short 6-digit numeric code for the user to enter.
    let code: String = format!("{:06}", rand_code());

    let result = QuickConnectResult {
        authenticated: false,
        secret: secret.clone(),
        code,
        device_id: String::new(),
        device_name: String::new(),
        app_name: String::new(),
        app_version: String::new(),
        date_added: Utc::now(),
        user_id: None,
    };

    // Purge expired entries while holding the lock.
    let mut store = state.quick_connect.write().await;
    let now = Utc::now();
    store.retain(|_, v| (now - v.date_added).num_seconds() < EXPIRY_SECS);
    store.insert(secret, result.clone());

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
struct ConnectQuery {
    secret: String,
}

/// `GET /QuickConnect/Connect?secret=` – poll the status of a quick connect request.
async fn connect(
    State(state): State<AppState>,
    Query(q): Query<ConnectQuery>,
) -> Result<Json<QuickConnectResult>, ApiError> {
    let store = state.quick_connect.read().await;
    let result = store
        .get(&q.secret)
        .ok_or_else(|| ApiError::NotFound("Unknown secret".to_string()))?;

    // Check expiry.
    if (Utc::now() - result.date_added).num_seconds() >= EXPIRY_SECS {
        drop(store);
        state.quick_connect.write().await.remove(&q.secret);
        return Err(ApiError::NotFound("Quick connect request expired".to_string()));
    }

    Ok(Json(result.clone()))
}

#[derive(serde::Deserialize)]
struct AuthorizeQuery {
    code: String,
}

/// `POST /QuickConnect/Authorize?code=` – authorize a pending quick connect request (authenticated).
async fn authorize(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Json<bool>, ApiError> {
    let mut store = state.quick_connect.write().await;

    // Find the entry by code.
    let entry = store
        .values_mut()
        .find(|v| v.code == q.code)
        .ok_or_else(|| ApiError::NotFound("Unknown code".to_string()))?;

    if (Utc::now() - entry.date_added).num_seconds() >= EXPIRY_SECS {
        return Err(ApiError::NotFound("Quick connect request expired".to_string()));
    }

    entry.authenticated = true;
    entry.user_id = Some(claims.sub);

    Ok(Json(true))
}

/// Generate a random 6-digit code using basic means (no extra crate needed).
fn rand_code() -> u32 {
    // Use uuid randomness to derive a number.
    let id = Uuid::new_v4();
    let bytes = id.as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    n % 1_000_000
}
