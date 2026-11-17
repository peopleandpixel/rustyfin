//! The server's one import job and its status (spec 2.18, 2.19, 2.21, 2.25).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use super::{ImportReport, UserRow};

/// The state of the import job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum JobState {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

/// The stage a running import is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ImportStage {
    Scanning,
    Reading,
    Matching,
    Users,
    WatchData,
    Metadata,
}

/// The answer of `GET /JellyfinImport/Status` (spec 2.19).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportStatus {
    pub state: JobState,
    pub stage: Option<ImportStage>,
    pub done: usize,
    pub total: usize,
    pub result: Option<ImportReport>,
    pub error: Option<String>,
    /// The users a failed run created before it failed, each with its one-time code (H19). Empty
    /// while running and after a completed run, whose `result` lists them; left out of the JSON
    /// when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created_users: Vec<UserRow>,
}

/// The server's one import job. Clones share it. `preview` gates the setup wizard's preview, which
/// is separate from a run: one preview at a time (B119).
#[derive(Clone, Default)]
pub struct ImportJob {
    shared: Arc<Mutex<ImportStatus>>,
    preview: Arc<AtomicBool>,
}

/// The setup preview's gate: held while a guard lives, released when it drops (B119).
pub struct PreviewGuard {
    preview: Arc<AtomicBool>,
}

impl Drop for PreviewGuard {
    fn drop(&mut self) {
        self.preview.store(false, Ordering::Release);
    }
}

impl ImportJob {
    /// Takes the setup preview's gate: `None` while another preview holds it (B119).
    pub fn try_preview(&self) -> Option<PreviewGuard> {
        self.preview
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| PreviewGuard {
                preview: Arc::clone(&self.preview),
            })
    }

    /// Starts a run unless one is running; the handle reports the run's progress.
    pub fn try_start(&self) -> Option<JobHandle> {
        let mut status = lock(&self.shared);
        if status.state == JobState::Running {
            return None;
        }
        *status = ImportStatus {
            state: JobState::Running,
            stage: Some(ImportStage::Reading),
            ..ImportStatus::default()
        };
        Some(JobHandle {
            shared: Arc::clone(&self.shared),
            finished: false,
        })
    }

    /// The job's status. The first answer that shows a finished run carries its codes: a completed
    /// run's in `result`, a failed run's in `created_users`. The job forgets them then.
    pub fn status(&self) -> ImportStatus {
        let mut status = lock(&self.shared);
        let mut answer = status.clone();
        match status.state {
            JobState::Running => answer.created_users.clear(),
            JobState::Completed | JobState::Failed => {
                let status = &mut *status;
                for user in status
                    .result
                    .iter_mut()
                    .flat_map(|report| &mut report.users)
                    .chain(status.created_users.iter_mut())
                {
                    user.code = None;
                }
            }
            JobState::Idle => {}
        }
        answer
    }
}

/// A running import's link to its job. Dropped without `complete` or `fail`, the run counts as
/// failed.
pub struct JobHandle {
    shared: Arc<Mutex<ImportStatus>>,
    finished: bool,
}

impl JobHandle {
    /// Enters `stage`, with `total` steps and none done.
    pub fn stage(&self, stage: ImportStage, total: usize) {
        let mut status = lock(&self.shared);
        status.stage = Some(stage);
        status.done = 0;
        status.total = total;
    }

    /// Records `done` steps of the current stage.
    pub fn progress(&self, done: usize) {
        lock(&self.shared).done = done;
    }

    /// Records a user the run just created, with its code, so a failed run still shows it (H19).
    pub fn created(&self, row: &UserRow) {
        lock(&self.shared).created_users.push(row.clone());
    }

    /// Ends the run with its report.
    pub fn complete(mut self, report: ImportReport) {
        self.end(JobState::Completed, Some(report), None);
    }

    /// Ends the run with an error (no secrets, no codes). The users created before it keep their
    /// codes in `created_users`.
    pub fn fail(mut self, error: String) {
        self.end(JobState::Failed, None, Some(error));
    }

    /// Records how the run ended. Sets `finished`, so the drop does not record a second ending. A
    /// completed run's report lists its created users, so `created_users` is kept only on failure.
    fn end(&mut self, state: JobState, result: Option<ImportReport>, error: Option<String>) {
        self.finished = true;
        let mut status = lock(&self.shared);
        if result.is_some() {
            status.created_users.clear();
        }
        status.state = state;
        status.stage = None;
        status.result = result;
        status.error = error;
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if !self.finished {
            self.end(
                JobState::Failed,
                None,
                Some("the import stopped before it finished".to_string()),
            );
        }
    }
}

/// The status behind `shared`, read through a poisoned lock too: every update is a few plain
/// assignments, so a panic can't leave it half-written.
fn lock(shared: &Mutex<ImportStatus>) -> MutexGuard<'_, ImportStatus> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
