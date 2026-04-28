mod system;
mod auth;
mod users;
mod library;
mod items;
mod quick_connect;
mod images;
mod streaming;

use axum::Router;
use axum::middleware as axum_mw;

use crate::middleware::require_auth;
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .merge(system::public_routes())
        .merge(auth::routes())
        .merge(users::public_routes())
        .merge(quick_connect::public_routes());

    let protected = Router::new()
        .merge(system::protected_routes())
        .merge(users::routes())
        .merge(library::routes())
        .merge(items::routes())
        .merge(images::routes())
        .merge(quick_connect::routes())
        .merge(streaming::routes())
        .layer(axum_mw::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
}
