use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use rustyfin_model::auth::Claims;
use rustyfin_model::quick_connect::QuickConnectResult;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::resolve_client;
use crate::state::AppState;

/// Expiry duration for quick connect codes (5 minutes).
const EXPIRY_SECS: i64 = 300;

/// Most live requests stored at once. Initiate answers 429 beyond it.
const MAX_PENDING: usize = 100;

/// Most live requests one client (limiter key) has at once. Initiate answers 429 beyond it.
const MAX_PENDING_PER_CLIENT: usize = 5;

/// Whether a request is live at `now`: younger than `EXPIRY_SECS`.
fn is_live(request: &QuickConnectResult, now: DateTime<Utc>) -> bool {
    (now - request.date_added).num_seconds() < EXPIRY_SECS
}

/// Public routes (no auth required).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/QuickConnect/Enabled", get(quick_connect_enabled))
        .route("/QuickConnect/Connect", get(connect))
}

/// Initiate, which is rate limited with the sign-in endpoints (no auth required).
pub fn login_routes() -> Router<AppState> {
    Router::new().route("/QuickConnect/Initiate", post(initiate))
}

/// Protected routes (auth required).
pub fn routes() -> Router<AppState> {
    Router::new().route("/QuickConnect/Authorize", post(authorize))
}

/// `GET /QuickConnect/Enabled` – whether Quick Connect is enabled.
async fn quick_connect_enabled() -> Json<bool> {
    Json(true)
}

/// `POST /QuickConnect/Initiate` – create a new quick connect request. Answers 429 when
/// `MAX_PENDING` live requests are stored, when `MAX_PENDING_PER_CLIENT` are stored for the
/// caller's limiter key (AC47), or when no code is free.
async fn initiate(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<QuickConnectResult>, ApiError> {
    let secret = Uuid::new_v4().to_string().replace("-", "");
    let client_key = resolve_client(peer.ip(), &headers, &state.known_proxies).limiter_key();

    // Purge expired entries while holding the lock. A refused request stores nothing.
    let mut store = state.quick_connect.write().await;
    let now = Utc::now();
    store.retain(|_, v| is_live(v, now));
    if store.len() >= MAX_PENDING {
        return Err(ApiError::RateLimited);
    }
    let pending_for_client = store
        .values()
        .filter(|v| v.client_key == client_key)
        .count();
    if pending_for_client >= MAX_PENDING_PER_CLIENT {
        return Err(ApiError::RateLimited);
    }
    // A short 6-digit code for the user to enter, unique among live requests.
    let code = free_code(&store, rand_code(), now).ok_or(ApiError::RateLimited)?;

    let result = QuickConnectResult {
        authenticated: false,
        secret: secret.clone(),
        code,
        device_id: String::new(),
        device_name: String::new(),
        app_name: String::new(),
        app_version: String::new(),
        date_added: now,
        user_id: None,
        client_key,
    };
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
    if !is_live(result, Utc::now()) {
        drop(store);
        state.quick_connect.write().await.remove(&q.secret);
        return Err(ApiError::NotFound(
            "Quick connect request expired".to_string(),
        ));
    }

    // If authenticated, generate and return a token
    let result = result.clone();
    if result.authenticated {
        // Token will be returned via WebSocket notification
        // The Connect endpoint returns the authenticated status
    }

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
struct AuthorizeQuery {
    code: String,
}

/// `POST /QuickConnect/Authorize?code=` – authorize a pending quick connect request (authenticated).
/// Wrong codes count against the sign-in rate limit, and a client over that limit gets 429
/// before its code is looked at.
async fn authorize(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(claims): Extension<Claims>,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Json<bool>, ApiError> {
    let client = resolve_client(peer.ip(), &headers, &state.known_proxies);
    let counted = !client.is_exempt();
    let key = client.limiter_key();

    // AC49: the limit check, the code lookup and any miss record share this one store lock.
    let mut store = state.quick_connect.write().await;
    if counted && state.rate_limiter.is_limited(&key).await {
        tracing::warn!(ip = %client.ip, "rate limit exceeded");
        return Err(ApiError::RateLimited);
    }
    let now = Utc::now();

    // Approve the live request with the code. An expired one counts as unknown.
    let Some(entry) = store
        .values_mut()
        .find(|v| v.code == q.code && is_live(v, now))
    else {
        if counted {
            state.rate_limiter.record(&key).await;
        }
        return Err(ApiError::NotFound("Unknown code".to_string()));
    };

    entry.authenticated = true;
    entry.user_id = Some(claims.sub);

    // No socket message, as in Jellyfin: every socket would see the secret.

    Ok(Json(true))
}

/// Takes an approved Quick Connect request for a sign-in (AC26) and returns the id of the user who
/// approved it. The request is removed as it is taken, so a secret signs in once. An unknown,
/// unapproved or expired request is refused; an expired one is also removed.
pub(super) async fn redeem(state: &AppState, secret: &str) -> Result<Uuid, ApiError> {
    let mut store = state.quick_connect.write().await;
    let entry = store.get(secret).ok_or(ApiError::Unauthorized)?;
    if !is_live(entry, Utc::now()) {
        store.remove(secret);
        return Err(ApiError::Unauthorized);
    }
    if !entry.authenticated {
        return Err(ApiError::Unauthorized);
    }
    let user_id = entry.user_id.ok_or(ApiError::Unauthorized)?;
    store.remove(secret);
    Ok(user_id)
}

/// The first code from `start % 1_000_000` on, wrapping after 999999, that no live request
/// carries, as six digits. `None` when all 1,000,000 codes are taken.
fn free_code(
    store: &HashMap<String, QuickConnectResult>,
    start: u32,
    now: DateTime<Utc>,
) -> Option<String> {
    let taken: HashSet<&str> = store
        .values()
        .filter(|request| is_live(request, now))
        .map(|request| request.code.as_str())
        .collect();
    let start = start % 1_000_000;
    (0..1_000_000u32)
        .map(|step| format!("{:06}", (start + step) % 1_000_000))
        .find(|code| !taken.contains(code.as_str()))
}

/// Generate a random 6-digit code using basic means (no extra crate needed).
fn rand_code() -> u32 {
    // Use uuid randomness to derive a number.
    let id = Uuid::new_v4();
    let bytes = id.as_bytes();
    let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    n % 1_000_000
}

#[cfg(test)]
mod tests;
