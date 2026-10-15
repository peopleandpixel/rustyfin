use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use rustyfin_model::auth::Claims;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::error::ApiError;
use crate::state::AppState;

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
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
        let claims = Claims {
            sub: uuid::Uuid::nil(),
            name: "ApiKey".to_string(),
            is_admin: true,
            iat: 0,
            exp: i64::MAX,
        };
        req.extensions_mut().insert(claims);
        return Ok(next.run(req).await);
    }

    // Otherwise validate as JWT
    let claims = rustyfin_auth::validate_token(&token, &state.jwt_secret)
        .map_err(|_| ApiError::Unauthorized)?;

    req.extensions_mut().insert(claims);
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
#[derive(Clone)]
pub struct RateLimiter {
    /// Map of IP → list of request timestamps
    state: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    /// Maximum requests allowed within the window
    max_requests: usize,
    /// Time window duration
    window: std::time::Duration,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_secs: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
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
                let mut map = state.lock().await;
                map.retain(|_, timestamps| {
                    timestamps.retain(|t| now.duration_since(*t) < window);
                    !timestamps.is_empty()
                });
            }
        });
    }

    async fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.state.lock().await;
        let timestamps = map.entry(key.to_string()).or_default();
        timestamps.retain(|t| now.duration_since(*t) < self.window);
        if timestamps.len() >= self.max_requests {
            return false;
        }
        timestamps.push(now);
        true
    }
}

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

    if !state.rate_limiter.check(&ip).await {
        tracing::warn!(ip = %ip, "rate limit exceeded");
        return Err(ApiError::RateLimited);
    }

    Ok(next.run(req).await)
}
