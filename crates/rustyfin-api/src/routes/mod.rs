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
mod jellyfin_import;
pub(crate) mod library;
mod library_monitor;
mod live_tv;
mod localization;
mod media_segments;
mod oidc;
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

pub use startup::{StartupOwner, settle_owner_at_startup};

use axum::Router;
use axum::middleware as axum_mw;
use std::sync::Arc;

use crate::middleware::{
    rate_limit, rate_limit_resource, require_admin, require_auth, require_setup_or_admin,
};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    // The library monitor's rescans clear this router's caches and send its events.
    state.task_scheduler.changes().set_runner(Arc::new(
        library_monitor::LibraryMonitorRunner::for_state(&state),
    ));

    // Open while the server has no users, then administrators only (Jellyfin's FirstTimeSetupOrElevated).
    let setup = Router::new()
        .merge(startup::routes())
        .merge(system::setup_routes())
        .merge(library::setup_routes())
        .merge(jellyfin_import::setup_routes())
        .route_layer(axum_mw::from_fn_with_state(
            state.clone(),
            require_setup_or_admin,
        ));

    let public = Router::new()
        .merge(system::public_routes())
        .merge(users::public_routes())
        .merge(oidc::public_routes())
        .merge(quick_connect::public_routes())
        .merge(dlna::routes().route_layer(axum_mw::from_fn_with_state(
            state.clone(),
            dlna::require_dlna_enabled,
        )))
        .merge(setup)
        .merge(websocket::routes());

    // Administrator routes. `require_auth` answers 401 without a valid token. `require_admin`
    // then answers 403 to a logged-in user who is not an administrator.
    let admin = Router::new()
        .merge(system::admin_routes())
        .merge(configuration::admin_routes())
        .merge(activity::admin_routes())
        .merge(session::admin_routes())
        .merge(users::admin_routes())
        .merge(oidc::admin_routes())
        .merge(library::admin_routes())
        .merge(library_monitor::admin_routes())
        .merge(images::admin_routes())
        .merge(items::admin_routes())
        .merge(subtitle::admin_routes())
        .merge(collections::admin_routes())
        .merge(live_tv::admin_routes())
        .merge(dlna::admin_routes())
        .merge(localization::admin_routes())
        .merge(encoding_manager::routes())
        .merge(scheduled_tasks::routes())
        .merge(plugins::routes())
        .merge(packages::routes())
        .merge(backup::routes())
        .merge(devices::routes())
        .merge(dlna_streaming::routes())
        .merge(jellyfin_import::routes())
        .route_layer(axum_mw::from_fn(require_admin));

    let protected = Router::new()
        .merge(system::protected_routes())
        .merge(users::routes())
        .merge(library::routes())
        .merge(items::routes())
        .merge(images::routes())
        .merge(quick_connect::routes())
        .merge(streaming::routes().route_layer(axum_mw::from_fn_with_state(
            state.clone(),
            rate_limit_resource,
        )))
        .merge(transcode::routes().route_layer(axum_mw::from_fn_with_state(
            state.clone(),
            rate_limit_resource,
        )))
        .merge(playback::routes())
        .merge(subtitle::routes())
        .merge(session::routes())
        .merge(search::routes())
        .merge(collections::routes())
        .merge(persons::routes())
        .merge(activity::routes())
        .merge(display_preferences::routes())
        .merge(configuration::routes())
        .merge(shows::routes())
        .merge(live_tv::routes())
        .merge(additional::routes())
        .merge(media_segments::routes())
        .merge(dynamic_hls::routes())
        .merge(localization::routes())
        .merge(sync_play::routes())
        .merge(oidc::protected_routes())
        .merge(admin)
        .layer(axum_mw::from_fn_with_state(state.clone(), require_auth));

    // Rate limit only the endpoints that check a password or PIN, and Quick Connect's Initiate,
    // which starts a sign-in. route_layer applies the limiter to matched routes only, so every
    // other route is unlimited.
    let login = Router::new()
        .merge(auth::routes())
        .merge(users::login_routes())
        .merge(oidc::login_routes())
        .merge(quick_connect::login_routes())
        .route_layer(axum_mw::from_fn_with_state(state.clone(), rate_limit));

    #[allow(unused_mut)]
    let mut router = Router::new().merge(public).merge(protected).merge(login);

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
    use axum::extract::Request;
    use axum::http::{HeaderValue, header};
    use axum::middleware::{Next, from_fn};
    use axum::response::{Redirect, Response};
    use tower_http::services::ServeDir;

    use crate::state::AppState;

    /// True for `/assets/<stem>-<8 chars of [A-Za-z0-9_-]>.<ext>`, with or without the `/web` prefix.
    fn is_hashed_asset(path: &str) -> bool {
        let path = path.strip_prefix("/web").unwrap_or(path);
        let Some(rest) = path.strip_prefix("/assets/") else {
            return false;
        };
        let Some((stem, ext)) = rest.rsplit_once('.') else {
            return false;
        };
        if ext.is_empty()
            || !ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return false;
        }
        let b = stem.as_bytes();
        b.len() > 9
            && b[b.len() - 9] == b'-'
            && b[b.len() - 8..]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
    }

    /// Hashed assets never change, so cache them forever; everything else revalidates,
    /// errors included, so a 404 during an upgrade is not kept.
    async fn web_cache_headers(req: Request, next: Next) -> Response {
        let hashed = is_hashed_asset(req.uri().path());
        let mut resp = next.run(req).await;
        let value = if hashed && resp.status().is_success() {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        resp.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
        resp
    }

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

        let web = Router::new()
            .fallback_service(serve_dir)
            .layer(from_fn(web_cache_headers));

        router
            .nest_service("/web", web)
            .route(
                "/",
                axum::routing::get(|| async { Redirect::permanent("/web/index.html") }),
            )
            .route(
                "/index.html",
                axum::routing::get(|| async { Redirect::permanent("/web/index.html") }),
            )
    }

    #[cfg(test)]
    mod tests {
        use super::is_hashed_asset;

        #[test]
        fn is_hashed_asset_matches_only_hashed_names() {
            assert!(is_hashed_asset("/assets/index-AbC_-123.js"));
            assert!(is_hashed_asset("/web/assets/app-1a2b3c4d.js"));
            assert!(is_hashed_asset("/assets/hls.min-1a2b3c4d.js"));
            assert!(!is_hashed_asset("/assets/logo.png"));
            assert!(!is_hashed_asset("/assets/index.js"));
            assert!(!is_hashed_asset("/index.html"));
            assert!(!is_hashed_asset("/assets/a-1234567.js"));
        }
    }
}
