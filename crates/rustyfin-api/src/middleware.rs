use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use dashmap::DashMap;
use rustyfin_model::auth::Claims;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use crate::error::ApiError;
use crate::state::AppState;

/// Reads the credentials of a request and returns the claims they prove.
///
/// The token comes from the Bearer header, `X-Emby-Token`, `X-MediaBrowser-Token` or the `api_key`
/// query parameter. The configured API key gives administrator claims. Any other token must be a JWT.
fn request_claims(state: &AppState, req: &Request) -> Result<Claims, ApiError> {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .or_else(|| {
            // Jellyfin clients also send token via X-Emby-Token
            req.headers()
                .get("X-Emby-Token")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            // Also check X-MediaBrowser-Token
            req.headers()
                .get("X-MediaBrowser-Token")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            // Check query parameter api_key
            req.uri()
                .query()
                .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("api_key=")))
                .map(|s| s.to_string())
        })
        .ok_or(ApiError::Unauthorized)?;

    // Check if the token matches a configured API key
    if let Some(ref api_key) = state.api_key
        && token == **api_key
    {
        // API key auth: create admin claims
        return Ok(Claims {
            sub: uuid::Uuid::nil(),
            name: "ApiKey".to_string(),
            is_admin: true,
            iat: 0,
            exp: i64::MAX,
        });
    }

    // Otherwise validate as JWT
    rustyfin_auth::validate_token(&token, &state.jwt_secret).map_err(|_| ApiError::Unauthorized)
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let claims = request_claims(&state, &req)?;
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

/// Guards the setup wizard routes. Until an owner exists, anyone may use them. After that, only an
/// administrator may.
pub async fn require_setup_or_admin(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !rustyfin_db::repo_user::owner_exists(&state.db).await? {
        return Ok(next.run(req).await);
    }
    let claims = request_claims(&state, &req)?;
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

/// Guards the administrator routes. It runs after `require_auth`, which inserts the claims, so a
/// request without claims gets 401. A logged-in user who is not an administrator gets 403. The
/// configured API key carries administrator claims and passes.
pub async fn require_admin(req: Request, next: Next) -> Result<Response, ApiError> {
    let claims = req
        .extensions()
        .get::<Claims>()
        .ok_or(ApiError::Unauthorized)?;
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }
    Ok(next.run(req).await)
}

#[allow(dead_code)]
pub trait ClaimsExt {
    fn claims(&self) -> Option<&Claims>;
}

impl<T> ClaimsExt for axum::extract::Request<T> {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }
}

/// Simple in-memory rate limiter using a sliding window per IP.
/// Uses DashMap for lock-striping to reduce contention under high concurrency.
#[derive(Clone)]
pub struct RateLimiter {
    /// Map of IP → list of request timestamps
    state: Arc<DashMap<String, Vec<Instant>>>,
    /// Maximum requests allowed within the window
    max_requests: usize,
    /// Time window duration
    window: std::time::Duration,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_secs: u64) -> Self {
        Self {
            state: Arc::new(DashMap::new()),
            max_requests,
            window: std::time::Duration::from_secs(window_secs),
        }
    }

    /// Periodically clean up expired entries to prevent memory leaks.
    pub fn start_cleanup(&self) {
        let state = self.state.clone();
        let window = self.window;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let now = Instant::now();
                state.retain(|_, timestamps| {
                    timestamps.retain(|t| now.duration_since(*t) < window);
                    !timestamps.is_empty()
                });
            }
        });
    }

    pub async fn check(&self, key: &str) -> bool {
        // A limit of 0 disables rate limiting
        if self.max_requests == 0 {
            return true;
        }

        let now = Instant::now();
        let mut entry = self.state.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        if entry.len() >= self.max_requests {
            return false;
        }
        entry.push(now);
        true
    }
}

/// Per-client-IP budget for the login endpoints only (AuthenticateByName, ForgotPassword,
/// ResetPassword), mounted on those routes in `routes::build_router`. Loopback clients are
/// exempt, and a budget of 0 disables the limit.
pub async fn rate_limit(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let ip = req
        .headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| addr.ip().to_string());

    // Exempt loopback/localhost traffic (127.0.0.1, ::1) from aggressive client throttling
    let is_loopback = ip == "127.0.0.1" || ip == "::1" || ip == "localhost";
    if !is_loopback && !state.rate_limiter.check(&ip).await {
        tracing::warn!(ip = %ip, "rate limit exceeded");
        return Err(ApiError::RateLimited);
    }

    Ok(next.run(req).await)
}
