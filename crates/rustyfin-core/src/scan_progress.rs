//! Library scans in progress, so that clients can show which libraries are scanning and how far
//! along each one is.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{broadcast, watch};
use uuid::Uuid;

/// Whether a library's scan waits for its turn or runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Queued,
    Active,
}

/// A library's scan in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanState {
    pub status: ScanStatus,
    /// The whole percent done, 0 to 99, once the amount of work is known.
    pub progress: Option<u8>,
}

/// A change in a library's progress: 0 when its scan is queued, each higher percent as it is
/// reached, and 100 when the scan ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanUpdate {
    pub library_id: Uuid,
    pub progress: u8,
}

/// The scans in progress, one entry per library, and a channel of their updates.
#[derive(Clone)]
pub struct ScanRegistry {
    inner: Arc<Inner>,
}

/// Shared by the registry and its tickets. Every change and every send happens under the lock.
struct Inner {
    entries: Mutex<BTreeMap<Uuid, Entry>>,
    tx: broadcast::Sender<ScanUpdate>,
}

/// One library's scan: what the registry lists, and the counts behind its percent.
struct Entry {
    state: ScanState,
    /// The live tickets of the scan, clones included.
    tickets: usize,
    total: usize,
    done: usize,
    /// Dropped when the scan ends, which ends the `wait_end` calls waiting on it.
    ended: watch::Sender<()>,
}

impl Entry {
    /// The percent of the work done, 0 to 99, or `None` while the total is 0.
    fn percent(&self) -> Option<u8> {
        if self.total == 0 {
            return None;
        }
        let done = self.done.min(self.total);
        Some((done * 100 / self.total).min(99) as u8)
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<Uuid, Entry>> {
        // A panic under the lock leaves the map usable, so a poisoned lock is still used.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, library_id: Uuid, progress: u8) {
        // A send fails only when nothing is subscribed, which is fine.
        let _ = self.tx.send(ScanUpdate {
            library_id,
            progress,
        });
    }

    /// Changes the library's entry, if it has one, and sends its percent when that rose.
    fn change(&self, library_id: Uuid, change: impl FnOnce(&mut Entry)) {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(&library_id) else {
            return;
        };
        change(entry);
        if let Some(percent) = entry.percent() {
            // The state keeps the highest percent so far. A queued scan has sent 0 already.
            let shown = entry.state.progress.unwrap_or(0);
            if percent > shown {
                self.send(library_id, percent);
            }
            entry.state.progress = Some(shown.max(percent));
        }
    }
}

impl Default for ScanRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanRegistry {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            inner: Arc::new(Inner {
                entries: Mutex::new(BTreeMap::new()),
                tx,
            }),
        }
    }

    /// The updates of every library's scan from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<ScanUpdate> {
        self.inner.tx.subscribe()
    }

    /// The snapshot and a receiver of exactly the updates after it, both under the lock.
    pub fn subscribe_with_snapshot(
        &self,
    ) -> (Vec<(Uuid, ScanState)>, broadcast::Receiver<ScanUpdate>) {
        let entries = self.inner.lock();
        let updates = self.inner.tx.subscribe();
        (snapshot_of(&entries), updates)
    }

    /// The library's scan in progress, or `None` when it isn't scanning.
    pub fn state(&self, library_id: Uuid) -> Option<ScanState> {
        let entries = self.inner.lock();
        entries.get(&library_id).map(|entry| entry.state)
    }

    /// Every scan in progress.
    pub fn snapshot(&self) -> Vec<(Uuid, ScanState)> {
        snapshot_of(&self.inner.lock())
    }

    /// Queues a scan of the library. Its entry lasts until the last clone of the last ticket of
    /// the library drops.
    pub fn queue(&self, library_id: Uuid) -> ScanTicket {
        {
            let mut entries = self.inner.lock();
            if let Some(entry) = entries.get_mut(&library_id) {
                entry.tickets += 1;
            } else {
                let (ended, _) = watch::channel(());
                let state = ScanState {
                    status: ScanStatus::Queued,
                    progress: None,
                };
                entries.insert(
                    library_id,
                    Entry {
                        state,
                        tickets: 1,
                        total: 0,
                        done: 0,
                        ended,
                    },
                );
                self.inner.send(library_id, 0);
            }
        }
        ScanTicket {
            inner: Some(self.inner.clone()),
            library_id,
        }
    }
}

/// The scans in progress, in the order of their library ids.
fn snapshot_of(entries: &BTreeMap<Uuid, Entry>) -> Vec<(Uuid, ScanState)> {
    entries
        .iter()
        .map(|(id, entry)| (*id, entry.state))
        .collect()
}

/// One caller's share of a library's scan. Every clone is a share of its own, and the scan ends
/// when the last of them drops.
pub struct ScanTicket {
    inner: Option<Arc<Inner>>,
    library_id: Uuid,
}

impl Clone for ScanTicket {
    fn clone(&self) -> Self {
        self.change(|entry| entry.tickets += 1);
        Self {
            inner: self.inner.clone(),
            library_id: self.library_id,
        }
    }
}

impl Drop for ScanTicket {
    fn drop(&mut self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut entries = inner.lock();
        let Some(entry) = entries.get_mut(&self.library_id) else {
            return;
        };
        entry.tickets = entry.tickets.saturating_sub(1);
        if entry.tickets == 0 {
            // Removing the entry drops its sender, which ends `wait_end`. The 100 goes out after.
            entries.remove(&self.library_id);
            inner.send(self.library_id, 100);
        }
    }
}

impl ScanTicket {
    /// A ticket that tracks nothing.
    pub fn untracked() -> Self {
        Self {
            inner: None,
            library_id: Uuid::nil(),
        }
    }

    /// The scan has taken the scan lock and runs.
    pub fn start(&self) {
        self.change(|entry| entry.state.status = ScanStatus::Active);
    }

    /// `n` more units of work.
    pub fn add_total(&self, n: usize) {
        self.change(|entry| entry.total += n);
    }

    /// `n` units of work done.
    pub fn advance(&self, n: usize) {
        self.change(|entry| entry.done += n);
    }

    /// Drops this ticket and waits until the library's scan has ended.
    pub async fn wait_end(self) {
        let ended = match &self.inner {
            Some(inner) => inner
                .lock()
                .get(&self.library_id)
                .map(|entry| entry.ended.subscribe()),
            None => None,
        };
        // Dropping this ticket may end the scan, which drops the sender and so ends the wait.
        drop(self);
        if let Some(mut ended) = ended {
            let _ = ended.changed().await;
        }
    }

    fn change(&self, change: impl FnOnce(&mut Entry)) {
        if let Some(inner) = &self.inner {
            inner.change(self.library_id, change);
        }
    }
}
