mod system;
mod auth;
mod users;
mod library;
mod items;
mod quick_connect;
mod images;
mod streaming;
mod transcode;
mod playback;
mod subtitle;
mod session;
mod search;
mod collections;
mod persons;
mod activity;
mod dlna;
mod startup;
mod scheduled_tasks;
mod plugins;
mod websocket;

use axum::Router;
use axum::middleware as axum_mw;

use crate::middleware::{rate_limit, require_auth};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .merge(system::public_routes())
        .merge(auth::routes())
        .merge(users::public_routes())
        .merge(quick_connect::public_routes())
        .merge(dlna::routes())
        .merge(startup::routes())
        .merge(websocket::routes());

    let protected = Router::new()
        .merge(system::protected_routes())
        .merge(users::routes())
        .merge(library::routes())
        .merge(items::routes())
        .merge(images::routes())
        .merge(quick_connect::routes())
        .merge(streaming::routes())
        .merge(transcode::routes())
        .merge(playback::routes())
        .merge(subtitle::routes())
        .merge(session::routes())
        .merge(search::routes())
        .merge(collections::routes())
        .merge(persons::routes())
        .merge(activity::routes())
        .merge(scheduled_tasks::routes())
        .merge(plugins::routes())
        .layer(axum_mw::from_fn_with_state(state.clone(), require_auth));

    #[allow(unused_mut)]
    let mut router = Router::new()
        .merge(public)
        .merge(protected)
        .layer(axum_mw::from_fn_with_state(state.clone(), rate_limit));

    // Optionally serve the static web client (enabled via "webclient" feature)
    #[cfg(feature = "webclient")]
    {
        router = webclient::mount_webclient(router);
    }

    router.with_state(state)
}

/// Static web client serving (only compiled when "webclient" feature is enabled)
#[cfg(feature = "webclient")]
mod webclient {
    use axum::Router;
    use axum::response::Redirect;
    use tower_http::services::ServeDir;

    use crate::state::AppState;

    pub fn mount_webclient(router: Router<AppState>) -> Router<AppState> {
        let web_dir = std::env::var("RUSTYFIN_WEBCLIENT_DIR").unwrap_or_else(|_| {
            // Default: look for the bundled web client next to the binary
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_default();
            let candidate = exe_dir.join("web");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
            // Fallback: workspace-relative path (development)
            "crates/rustyfin-web/dist".to_string()
        });

        tracing::info!("serving web client from: {}", web_dir);

        router
            .nest_service("/web", ServeDir::new(&web_dir))
            .route("/", axum::routing::get(|| async { Redirect::permanent("/web/index.html") }))
    }
}
