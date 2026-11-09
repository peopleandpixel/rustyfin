//! The startup task for the date added backfill: it runs the pending backfill and drops the cached
//! answers that hold `DateCreated` after each batch it writes, so the new dates show at once.

use crate::state::AppState;

/// Runs the pending backfill, if any. It drops state.cache.item_details and
/// state.cache.search_results after each batch it writes; an error is logged as a warning.
pub async fn run(state: &AppState) {
    let result = rustyfin_core::date_backfill::run_pending(&state.db, |_| {
        state.cache.item_details.invalidate_all();
        state.cache.search_results.invalidate_all();
    })
    .await;
    if let Err(e) = result {
        tracing::warn!(
            "date added backfill stopped: {e}; the next start goes on from the last batch it saved"
        );
    }
}
