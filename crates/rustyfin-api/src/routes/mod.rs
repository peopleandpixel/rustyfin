mod activity;
mod additional;
mod auth;
mod backup;
mod collections;
mod configuration;
mod devices;
mod display_preferences;
mod dlna;
mod dlna_streaming;
mod dynamic_hls;
mod encoding_manager;
mod images;
mod items;
mod library;
mod live_tv;
mod localization;
mod media_segments;
mod packages;
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
mod sync_play;
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
        .merge(media_segments::routes())
        .merge(devices::routes())
        .merge(dynamic_hls::routes())
        .merge(localization::routes())
        .merge(packages::routes())
        .merge(dlna_streaming::routes())
        .merge(backup::routes())
        .merge(sync_play::routes())
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
        let web_dir = std::env::var("RUSTYFIN_WEBCLIENT_DIR")
            .ok()
            .filter(|p| std::path::Path::new(p).exists())
            .or_else(|| {
                // Check relative to current executable location
                if let Some(exe_dir) = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                {
                    let candidates = [
                        exe_dir.join("web"),
                        exe_dir.join("../web"),
                        exe_dir.join("../../crates/rustyfin-web/dist"),
                        exe_dir.join("../../../crates/rustyfin-web/dist"),
                    ];
                    for c in candidates {
                        if c.exists() {
                            return Some(c.to_string_lossy().to_string());
                        }
                    }
                }
                None
            })
            .or_else(|| {
                // Check relative to current working directory
                let candidates = [
                    std::path::PathBuf::from("crates/rustyfin-web/dist"),
                    std::path::PathBuf::from("../crates/rustyfin-web/dist"),
                    std::path::PathBuf::from("../../crates/rustyfin-web/dist"),
                    std::path::PathBuf::from("rustyfin-web/dist"),
                    std::path::PathBuf::from("dist"),
                    std::path::PathBuf::from("web"),
                ];
                for c in candidates {
                    if c.exists() {
                        return Some(c.to_string_lossy().to_string());
                    }
                }
                None
            })
            .or_else(|| {
                // Compile-time manifest dir fallback for local workspace development
                let compile_time_path =
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rustyfin-web/dist");
                if compile_time_path.exists() {
                    Some(compile_time_path.to_string_lossy().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "crates/rustyfin-web/dist".to_string());

        tracing::info!("serving web client from: {}", web_dir);

        let serve_dir = ServeDir::new(&web_dir).append_index_html_on_directories(true);

        router
            .nest_service("/web", serve_dir)
            .route(
                "/",
                axum::routing::get(|| async { Redirect::permanent("/web/index.html") }),
            )
            .route(
                "/index.html",
                axum::routing::get(|| async { Redirect::permanent("/web/index.html") }),
            )
    }
}
