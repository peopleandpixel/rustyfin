//! A running task's percent done, as Jellyfin's `IProgress<double>` reports it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The bits stored while no percent is known.
const UNKNOWN: u64 = u64::MAX;

/// The percent done, 0 to 100, of a running task, or none while the task cannot count its work.
/// Clones share one value, so the task reports on its clone and the scheduler reads its own.
#[derive(Clone, Debug)]
pub struct TaskProgress(Arc<AtomicU64>);

impl Default for TaskProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskProgress {
    /// A progress with no percent known.
    pub fn new() -> Self {
        Self(Arc::new(AtomicU64::new(UNKNOWN)))
    }

    /// Sets the percent done, clamped to 0..=100. A NaN is ignored.
    pub fn report(&self, percent: f64) {
        if percent.is_nan() {
            return;
        }
        self.0
            .store(percent.clamp(0.0, 100.0).to_bits(), Ordering::Relaxed);
    }

    /// Sets the percent from `done` of `total` steps. Does nothing while `total` is 0.
    pub fn report_count(&self, done: usize, total: usize) {
        if total > 0 {
            self.report(done as f64 * 100.0 / total as f64);
        }
    }

    /// Forgets the percent, as when the task ends.
    pub fn clear(&self) {
        self.0.store(UNKNOWN, Ordering::Relaxed);
    }

    /// The percent done, if one was reported since the last `clear`.
    pub fn get(&self) -> Option<f64> {
        match self.0.load(Ordering::Relaxed) {
            UNKNOWN => None,
            bits => Some(f64::from_bits(bits)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3tasks_progress_reports_clamps_and_clears() {
        let progress = TaskProgress::new();
        assert_eq!(progress.get(), None);
        let shared = progress.clone();
        shared.report_count(1, 4);
        assert_eq!(progress.get(), Some(25.0));
        shared.report_count(3, 0);
        assert_eq!(progress.get(), Some(25.0), "a total of 0 reports nothing");
        shared.report(140.0);
        assert_eq!(progress.get(), Some(100.0));
        shared.report(f64::NAN);
        assert_eq!(progress.get(), Some(100.0));
        shared.clear();
        assert_eq!(progress.get(), None);
    }
}
