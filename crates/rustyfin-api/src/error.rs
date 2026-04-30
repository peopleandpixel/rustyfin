use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Jellyfin-compatible error response format
#[derive(Debug, Serialize)]
pub struct JellyfinError {
    #[serde(rename = "Type")]
    pub error_type: &'static str,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "Detail")]
    pub detail: String,
    #[serde(rename = "Status")]
    pub status: u16,
}

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    Internal(String),
    RateLimited,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, title, detail) = match self {
            Self::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "Not Found".to_string(),
                msg,
            ),
            Self::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "Bad Request".to_string(),
                msg,
            ),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "Unauthorized".to_string(),
                "Authentication is required to access this resource.".to_string(),
            ),
            Self::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                "Forbidden".to_string(),
                msg,
            ),
            Self::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error".to_string(),
                msg,
            ),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests".to_string(),
                "Rate limit exceeded. Please try again later.".to_string(),
            ),
        };

        let body = JellyfinError {
            error_type: "https://tools.ietf.org/html/rfc9110",
            title,
            detail,
            status: status.as_u16(),
        };

        (status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        Self::Internal("An internal database error occurred.".to_string())
    }
}

impl From<rustyfin_auth::AuthError> for ApiError {
    fn from(e: rustyfin_auth::AuthError) -> Self {
        match e {
            rustyfin_auth::AuthError::InvalidCredentials | rustyfin_auth::AuthError::UserNotFound => {
                Self::Unauthorized
            }
            other => {
                tracing::error!(error = %other, "auth error");
                Self::Internal("An internal authentication error occurred.".to_string())
            }
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "Not Found: {msg}"),
            Self::BadRequest(msg) => write!(f, "Bad Request: {msg}"),
            Self::Unauthorized => write!(f, "Unauthorized"),
            Self::Forbidden(msg) => write!(f, "Forbidden: {msg}"),
            Self::Internal(msg) => write!(f, "Internal: {msg}"),
            Self::RateLimited => write!(f, "Rate Limited"),
        }
    }
}
