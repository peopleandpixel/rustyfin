use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use rustyfin_model::auth::{AuthResponse, LoginRequest};
use rustyfin_model::user::UserDto;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Users/AuthenticateByName", post(login))
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    let (user, token) =
        rustyfin_auth::authenticate(&state.db, &req.username, &req.password, &state.jwt_secret)
            .await?;

    Ok(Json(AuthResponse {
        user: UserDto::from(user),
        access_token: token,
        server_id: state.server_id.to_string(),
    }))
}
