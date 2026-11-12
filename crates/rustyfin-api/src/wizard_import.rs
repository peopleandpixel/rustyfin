//! The wizard's Jellyfin import job (B119). A queued import waits in one file in the data dir
//! (`jellyfin-import-queue.json`) until its run ends: its libraries are scanned first, then the
//! import runs. A restart resumes or reports the file (`resume`).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rustyfin_core::jellyfin_import::{ImportRequest, ImportStage, JobHandle, prepare, run};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::routes::library::{run_claimed_scan, send_scan_completed};
use crate::state::AppState;

/// The queue file's name in the data dir.
const QUEUE_FILE: &str = "jellyfin-import-queue.json";
/// The temp file a queue file is written to before the rename.
const QUEUE_TEMP: &str = "jellyfin-import-queue.json.tmp";

/// Where a queued import is (B119).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueState {
    /// The libraries are being scanned, before the import reads anything.
    WaitingForScans,
    /// The import is running (or was when the server stopped).
    Importing,
}

/// The queue file (B119): the import the administrator queued, who queued it, and the libraries it
/// created. Codes never go to disk.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueueFile {
    pub state: QueueState,
    pub caller: Uuid,
    pub library_ids: Vec<Uuid>,
    pub request: ImportRequest,
}

/// The queue file's path in the data dir.
pub(crate) fn queue_path(data_dir: &Path) -> PathBuf {
    data_dir.join(QUEUE_FILE)
}

/// Writes the queue file atomically: a temp file, then a rename over the queue file.
pub(crate) async fn write_queue(data_dir: &Path, file: &QueueFile) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(file).map_err(std::io::Error::other)?;
    let temp = data_dir.join(QUEUE_TEMP);
    tokio::fs::write(&temp, bytes).await?;
    tokio::fs::rename(&temp, queue_path(data_dir)).await
}

/// Removes the queue file. A missing file is fine; other failures are logged without the path.
pub(crate) async fn remove_queue(data_dir: &Path) {
    match tokio::fs::remove_file(queue_path(data_dir)).await {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(_) => tracing::warn!("jellyfin import: could not remove the queue file"),
    }
}

/// The cache clears when an import writes metadata, since the item and image caches serve what it
/// changed. The admin run and the wizard's queued job both use them (B119).
pub(crate) fn written_cache_clear(state: &AppState) -> impl FnOnce() + Send + 'static {
    let cache_state = state.clone();
    move || {
        cache_state.cache.invalidate_on_scan();
        cache_state.cache.item_details.invalidate_all();
        cache_state.cache.search_results.invalidate_all();
        cache_state.image_cache.invalidate_all();
    }
}

/// Runs a queued import (B119): scans its libraries, then imports. `handle` is the job's; the
/// queue file is removed when the run ends, whatever its outcome.
pub(crate) async fn run_queued(state: AppState, mut file: QueueFile, handle: JobHandle) {
    let data_dir = (*state.data_dir).clone();
    scan_libraries(&state, &file.library_ids, &handle).await;
    tracing::info!("jellyfin import: scans done, importing");

    file.state = QueueState::Importing;
    if write_queue(&data_dir, &file).await.is_err() {
        handle
            .fail("The queued Jellyfin import could not be saved, so it did not run.".to_string());
        remove_queue(&data_dir).await;
        return;
    }
    let prepared = match prepare(&file.request, file.caller, &data_dir).await {
        Ok(prepared) => prepared,
        Err(error) => {
            handle.fail(error.to_string());
            remove_queue(&data_dir).await;
            return;
        }
    };
    run(
        state.db.clone(),
        file.request,
        prepared,
        handle,
        data_dir.clone(),
        written_cache_clear(&state),
    )
    .await;
    remove_queue(&data_dir).await;
}

/// Scans each library in order and waits for each run to end (probes included), so the import reads
/// the scanned rows. A library no longer in the database is skipped; a failed scan logs a WARN and
/// counts as done, since the import still runs and reports the rows it can't match.
async fn scan_libraries(state: &AppState, library_ids: &[Uuid], handle: &JobHandle) {
    handle.stage(ImportStage::Scanning, library_ids.len());
    for (index, id) in library_ids.iter().enumerate() {
        match rustyfin_db::repo_library::get_library_by_id(&state.db_ro, *id).await {
            Ok(Some(lib)) => {
                let ticket = state.task_scheduler.scans().queue(lib.id);
                // A run another caller claimed is scanned by that caller; this waits for its end.
                if ticket.claim_announced() {
                    let count = match run_claimed_scan(state, &lib, &ticket).await {
                        Ok(count) => count,
                        Err(_) => {
                            tracing::warn!(
                                "jellyfin import: a library scan failed; importing anyway"
                            );
                            0
                        }
                    };
                    ticket.wait_end().await;
                    send_scan_completed(state, &lib, count);
                } else {
                    ticket.wait_end().await;
                }
            }
            Ok(None) => {}
            Err(_) => tracing::warn!("jellyfin import: could not read a library to scan"),
        }
        handle.progress(index + 1);
    }
}

/// Resumes a queued wizard import after a restart (B119), called once the scheduler starts. No
/// file: nothing. A file still waiting for scans runs its job. One that stopped while importing is
/// reported failed, not run again. An unreadable file is reported failed. Either way the file is
/// removed, and the job's status says what happened.
pub async fn resume(state: &AppState) {
    let data_dir = (*state.data_dir).clone();
    let bytes = match tokio::fs::read(queue_path(&data_dir)).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(_) => return fail_resumed(state, UNREADABLE).await,
    };
    let file: QueueFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(_) => return fail_resumed(state, UNREADABLE).await,
    };
    match file.state {
        QueueState::WaitingForScans => {
            tracing::info!("resuming the queued Jellyfin import");
            match state.jellyfin_import.try_start() {
                Some(handle) => {
                    tokio::spawn(run_queued(state.clone(), file, handle));
                }
                None => tracing::warn!("jellyfin import: a run is already going; not resumed"),
            }
        }
        QueueState::Importing => fail_resumed(state, INTERRUPTED).await,
    }
}

/// The error of a queued import the server stopped while importing.
const INTERRUPTED: &str = "The server stopped while the Jellyfin import was running, so it may be \
    incomplete. Check the import page and run it again.";
/// The error of a queue file that can't be read or parsed.
const UNREADABLE: &str = "The queued Jellyfin import could not be read, so it did not run.";

/// Reports a resumed import as failed with `message`, and removes its queue file.
async fn fail_resumed(state: &AppState, message: &str) {
    tracing::warn!("jellyfin import: {message}");
    if let Some(handle) = state.jellyfin_import.try_start() {
        handle.fail(message.to_string());
    }
    remove_queue(&(*state.data_dir).clone()).await;
}
