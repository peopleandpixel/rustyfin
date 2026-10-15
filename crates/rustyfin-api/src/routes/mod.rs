mod activity;
mod additional;
mod auth;
mod collections;
mod configuration;
mod display_preferences;
mod dlna;
mod encoding_manager;
mod images;
mod items;
mod library;
mod live_tv;
mod persons;
mod playback;
mod plugins;
mod quick_connect;
mod scheduled_tasks;
mod search;
mod session;
mod shows;
mod startup;
mod streaming;
mod subtitle;
mod system;
mod transcode;
mod users;
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
        .merge(encoding_manager::routes())
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
        .merge(display_preferences::routes())
        .merge(configuration::routes())
        .merge(shows::routes())
        .merge(live_tv::routes())
        .merge(additional::routes())
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
            if let Some(exe_dir) = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            {
                let candidate = exe_dir.join("web");
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
            // Fallback: workspace-relative path (development)
            "crates/rustyfin-web/dist".to_string()
        });

        tracing::info!("serving web client from: {}", web_dir);

        router.nest_service("/web", ServeDir::new(&web_dir)).route(
            "/",
            axum::routing::get(|| async { Redirect::permanent("/web/index.html") }),
        )
    }
}
