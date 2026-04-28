use axum::extract::{Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::Response;
use rustyfin_model::auth::Claims;

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
                .and_then(|q| {
                    q.split('&')
                        .find_map(|pair| pair.strip_prefix("api_key="))
                })
                .map(|s| s.to_string())
        })
        .ok_or(ApiError::Unauthorized)?;

    // Check if the token matches a configured API key
    if let Some(ref api_key) = state.api_key {
        if token == **api_key {
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
    }

    // Otherwise validate as JWT
    let claims =
        rustyfin_auth::validate_token(&token, &state.jwt_secret).map_err(|_| ApiError::Unauthorized)?;

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

pub trait ClaimsExt {
    fn claims(&self) -> Option<&Claims>;
}

impl<T> ClaimsExt for axum::extract::Request<T> {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }
}
