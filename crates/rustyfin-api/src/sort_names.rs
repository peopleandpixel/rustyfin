//! The sort rules task: it puts the configuration's sort rules in use and makes the stored sort
//! names again when they were made with other rules, at startup and when an administrator saves
//! other rules.

use crate::state::AppState;

/// One pass at a time. Each reads the rules once it holds this, so the last pass to run uses the
/// latest rules and stores their fingerprint, never an older pass's.
static PASS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Puts the configuration's sort rules in use, then makes again every sort name made with other
/// rules (`repo_media::recompute_sort_names`) and drops the cached answers that hold them. An
/// error is logged as a warning; the next start tries again.
pub async fn apply(state: &AppState) {
    let _pass = PASS.lock().await;
    let rules = state.config.read().await.sort_rules();
    rustyfin_model::sort_name::set_rules(rules.clone());
    match rustyfin_db::repo_media::recompute_sort_names(&state.db, &rules).await {
        Ok(0) => {}
        Ok(changed) => {
            tracing::info!("sort names: {changed} made again with the current sort rules");
            state.cache.item_details.invalidate_all();
            state.cache.search_results.invalidate_all();
        }
        Err(e) => tracing::warn!("sort names: could not make them again: {e}"),
    }
}
