//! Library scans in progress, so that clients can show which libraries are scanning and how far
//! along each one is.
//!
//! A library's scans are runs, oldest first. A scan queued while the newest run still waits joins
//! that run, and one of the run's tickets claims it, so that only the claimer scans. A scan queued
//! after the newest run has started makes a run of its own. A library's frames follow its oldest run.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{broadcast, watch};
use uuid::Uuid;

/// Where a library's scan is. `Queued` waits for its turn and `Active` runs. `Idle` means that no
/// scan is listed, which is the state a scan ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Queued,
    Active,
    Idle,
}

/// A library's scan in progress. Its status is never `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanState {
    pub status: ScanStatus,
    /// The whole percent done, 0 to 99, once the amount of work is known.
    pub progress: Option<u8>,
}

/// A change in a library's frame: its status and its percent. A scan's frames go `Queued` at 0,
/// then `Active` at 0 when it starts, each higher percent as it is reached, and `Idle` at 100
/// when it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanUpdate {
    pub library_id: Uuid,
    pub progress: u8,
    pub status: ScanStatus,
}

/// The scans in progress, each library's runs, and a channel of their updates.
#[derive(Clone)]
pub struct ScanRegistry {
    inner: Arc<Inner>,
}

/// Shared by the registry and its tickets. Every change and every send happens under the lock.
struct Inner {
    libraries: Mutex<Libraries>,
    tx: broadcast::Sender<ScanUpdate>,
}

/// What the lock guards: the libraries with runs, and how many runs have been made.
#[derive(Default)]
struct Libraries {
    runs_made: u64,
    entries: BTreeMap<Uuid, Entry>,
}

/// A library with at least one run: its runs, oldest first, and the frame last sent for it.
struct Entry {
    runs: Vec<Run>,
    sent: (ScanStatus, u8),
}

/// One scan of a library, from its queueing until it ends, and the counts behind its percent.
struct Run {
    id: u64,
    /// `Queued` until a holder starts the run, then `Active`.
    status: ScanStatus,
    /// The highest percent so far, 0 to 99. `None` while the amount of work is not known.
    progress: Option<u8>,
    total: usize,
    done: usize,
    /// Whether a ticket has claimed the run. Only a claimed run's holders scan it.
    claimed: bool,
    /// Whether a caller sends the run's Started and Completed. See `ScanTicket::announce`.
    announced: bool,
    /// The tickets that claimed the run, clones included.
    holders: usize,
    /// The tickets waiting for the run, clones included.
    waiters: usize,
    /// Dropped when the run ends, which ends the `wait_end` calls waiting on it.
    ended: watch::Sender<()>,
    /// Run in order when the run ends, before `ended` is dropped. See `ScanTicket::wait_end_then`.
    at_end: Vec<Box<dyn FnOnce() + Send>>,
}

impl Run {
    /// The percent of the work done, 0 to 99, or `None` while the total is 0.
    fn percent(&self) -> Option<u8> {
        if self.total == 0 {
            return None;
        }
        let done = self.done.min(self.total);
        Some((done * 100 / self.total).min(99) as u8)
    }

    fn state(&self) -> ScanState {
        ScanState {
            status: self.status,
            progress: self.progress,
        }
    }
}

impl Libraries {
    /// The run `run_id` of the library, unless it has ended.
    fn run_mut(&mut self, library_id: Uuid, run_id: u64) -> Option<&mut Run> {
        self.entries
            .get_mut(&library_id)?
            .runs
            .iter_mut()
            .find(|run| run.id == run_id)
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Libraries> {
        // A panic under the lock leaves the map usable, so a poisoned lock is still used.
        self.libraries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends the library's frame when it differs from the last one sent. The frame is the oldest
    /// run's state, or `Idle` at 100 when no run is left.
    fn publish(&self, library_id: Uuid, entry: &mut Entry) {
        let shown = match entry.runs.first() {
            Some(run) => (run.status, run.progress.unwrap_or(0)),
            None => (ScanStatus::Idle, 100),
        };
        if shown != entry.sent {
            entry.sent = shown;
            // A send fails only when nothing is subscribed, which is fine.
            let _ = self.tx.send(ScanUpdate {
                library_id,
                progress: shown.1,
                status: shown.0,
            });
        }
    }

    /// Changes a run of the library, then publishes the library's frame.
    fn change(&self, library_id: Uuid, run_id: u64, change: impl FnOnce(&mut Run)) {
        let mut libraries = self.lock();
        let Some(entry) = libraries.entries.get_mut(&library_id) else {
            return;
        };
        let Some(run) = entry.runs.iter_mut().find(|run| run.id == run_id) else {
            return;
        };
        change(run);
        if let Some(percent) = run.percent() {
            // The percent never goes down, and a run whose work is not known yet has none.
            run.progress = Some(run.progress.unwrap_or(0).max(percent));
        }
        self.publish(library_id, entry);
    }

    /// Ends a run: it leaves the list, the frame goes out, and its `at_end` callbacks run. Only then
    /// does its `ended` sender drop, so that its waiters wake once the frame and the callbacks'
    /// sends are in their channels.
    fn end(&self, libraries: &mut Libraries, library_id: Uuid, run_id: u64) {
        let Some(entry) = libraries.entries.get_mut(&library_id) else {
            return;
        };
        let Some(index) = entry.runs.iter().position(|run| run.id == run_id) else {
            return;
        };
        let mut run = entry.runs.remove(index);
        self.publish(library_id, entry);
        if entry.runs.is_empty() {
            libraries.entries.remove(&library_id);
        }
        for at_end in run.at_end.drain(..) {
            at_end();
        }
        drop(run);
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
                libraries: Mutex::new(Libraries::default()),
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
        let libraries = self.inner.lock();
        let updates = self.inner.tx.subscribe();
        (snapshot_of(&libraries), updates)
    }

    /// The state of the library's oldest run, or `None` when no run of it is listed.
    pub fn state(&self, library_id: Uuid) -> Option<ScanState> {
        let libraries = self.inner.lock();
        libraries
            .entries
            .get(&library_id)
            .and_then(|entry| entry.runs.first())
            .map(Run::state)
    }

    /// The oldest run of every library that has one.
    pub fn snapshot(&self) -> Vec<(Uuid, ScanState)> {
        snapshot_of(&self.inner.lock())
    }

    /// How many `wait_end` calls are waiting on the library's runs.
    pub fn waiting(&self, library_id: Uuid) -> usize {
        let libraries = self.inner.lock();
        libraries.entries.get(&library_id).map_or(0, |entry| {
            entry
                .runs
                .iter()
                .map(|run| run.ended.receiver_count())
                .sum()
        })
    }

    /// Queues a scan of the library. The scan joins the newest run while that run still waits,
    /// and otherwise makes a run of its own.
    pub fn queue(&self, library_id: Uuid) -> ScanTicket {
        let mut guard = self.inner.lock();
        let libraries = &mut *guard;
        let entry = libraries
            .entries
            .entry(library_id)
            .or_insert_with(|| Entry {
                runs: Vec::new(),
                sent: (ScanStatus::Idle, 100),
            });
        let run_id = match entry.runs.last_mut() {
            Some(newest) if newest.status == ScanStatus::Queued => {
                newest.waiters += 1;
                newest.id
            }
            _ => {
                libraries.runs_made += 1;
                let (ended, _) = watch::channel(());
                entry.runs.push(Run {
                    id: libraries.runs_made,
                    status: ScanStatus::Queued,
                    progress: None,
                    total: 0,
                    done: 0,
                    claimed: false,
                    announced: false,
                    holders: 0,
                    waiters: 1,
                    ended,
                    at_end: Vec::new(),
                });
                libraries.runs_made
            }
        };
        self.inner.publish(library_id, entry);
        ScanTicket {
            inner: Some(self.inner.clone()),
            library_id,
            run: run_id,
            holder: AtomicBool::new(false),
        }
    }
}

/// The oldest run of each library, in the order of the library ids.
fn snapshot_of(libraries: &Libraries) -> Vec<(Uuid, ScanState)> {
    libraries
        .entries
        .iter()
        .filter_map(|(id, entry)| entry.runs.first().map(|run| (*id, run.state())))
        .collect()
}

/// One caller's share of a library's run. A share that claims the run holds it, and a claimed run
/// ends with its last holder. A share of an unclaimed run waits for it, and an unclaimed run ends
/// with its last share. Every clone is a share of its own.
pub struct ScanTicket {
    inner: Option<Arc<Inner>>,
    library_id: Uuid,
    run: u64,
    /// Whether this share holds the run, as opposed to waiting for it.
    holder: AtomicBool,
}

impl Clone for ScanTicket {
    fn clone(&self) -> Self {
        let holder = self.holder.load(Ordering::Relaxed);
        if let Some(inner) = &self.inner {
            let mut libraries = inner.lock();
            // A run that has ended has nothing to count the clone in, so the clone is inert.
            if let Some(run) = libraries.run_mut(self.library_id, self.run) {
                if holder {
                    run.holders += 1;
                } else {
                    run.waiters += 1;
                }
            }
        }
        Self {
            inner: self.inner.clone(),
            library_id: self.library_id,
            run: self.run,
            holder: AtomicBool::new(holder),
        }
    }
}

impl Drop for ScanTicket {
    fn drop(&mut self) {
        let Some(inner) = &self.inner else {
            return;
        };
        let mut libraries = inner.lock();
        // A run that has ended has nothing to count this share in.
        let Some(run) = libraries.run_mut(self.library_id, self.run) else {
            return;
        };
        if self.holder.load(Ordering::Relaxed) {
            run.holders = run.holders.saturating_sub(1);
        } else {
            run.waiters = run.waiters.saturating_sub(1);
        }
        let ends = run.holders == 0 && (run.claimed || run.waiters == 0);
        if ends {
            inner.end(&mut libraries, self.library_id, self.run);
        }
    }
}

impl ScanTicket {
    /// A ticket that tracks nothing. It always claims, and its scan ends at once.
    pub fn untracked() -> Self {
        Self {
            inner: None,
            library_id: Uuid::nil(),
            run: 0,
            holder: AtomicBool::new(false),
        }
    }

    /// Claims the run, so that this share's caller scans the library. True when this share holds
    /// the run already and when it is the first share to claim it. False when another share has
    /// claimed the run, or when the run has ended.
    pub fn claim(&self) -> bool {
        self.claim_run(false)
    }

    /// `claim`, for a caller that sends the run's Started and Completed. When the claim holds, the
    /// run is marked as announced, so that `announce` returns false for it.
    pub fn claim_announced(&self) -> bool {
        self.claim_run(true)
    }

    /// Announces a claimed run that no share has announced, for the share that asks first. True for
    /// that share, which then sends the run's Started now and its Completed at its end. False for a
    /// run nobody has claimed, a run already announced, a run that has ended, and an untracked
    /// ticket.
    pub fn announce(&self) -> bool {
        let Some(inner) = &self.inner else {
            return false;
        };
        let mut libraries = inner.lock();
        let Some(run) = libraries.run_mut(self.library_id, self.run) else {
            return false;
        };
        if !run.claimed || run.announced {
            return false;
        }
        run.announced = true;
        true
    }

    /// The scan has taken the scan lock and runs. It starts only when this share claims the run,
    /// so a share whose run another share has claimed does nothing.
    pub fn start(&self) {
        if self.claim() {
            self.change(|run| run.status = ScanStatus::Active);
        }
    }

    /// `n` more units of work.
    pub fn add_total(&self, n: usize) {
        self.change(|run| run.total += n);
    }

    /// `n` units of work done.
    pub fn advance(&self, n: usize) {
        self.change(|run| run.done += n);
    }

    /// Drops this ticket and waits until the library's run has ended.
    pub async fn wait_end(self) {
        self.wait_end_then(|| {}).await;
    }

    /// Drops this ticket and waits until the library's run has ended. `at_end` runs when the run
    /// ends, under the registry's lock and before the waiters wake, so what it sends goes out
    /// before anything a waiter sends after waking. `at_end` runs while the registry is locked, so
    /// it must not block or use the registry, since that would deadlock; sending on a broadcast
    /// channel is fine.
    pub async fn wait_end_then(self, at_end: impl FnOnce() + Send + 'static) {
        let ended = match &self.inner {
            Some(inner) => {
                let mut libraries = inner.lock();
                match libraries.run_mut(self.library_id, self.run) {
                    Some(run) => {
                        run.at_end.push(Box::new(at_end));
                        Some(run.ended.subscribe())
                    }
                    // The run has ended already, so `at_end` runs now.
                    None => {
                        at_end();
                        None
                    }
                }
            }
            None => {
                at_end();
                None
            }
        };
        // Dropping this ticket may end the run, which runs `at_end` and then drops the sender.
        drop(self);
        if let Some(mut ended) = ended {
            let _ = ended.changed().await;
        }
    }

    fn change(&self, change: impl FnOnce(&mut Run)) {
        if let Some(inner) = &self.inner {
            inner.change(self.library_id, self.run, change);
        }
    }

    /// `claim`, and when the claim holds, also marks the run as announced if `announces` is set.
    /// Both happen under one lock acquisition, so no share can announce the run in between.
    fn claim_run(&self, announces: bool) -> bool {
        let Some(inner) = &self.inner else {
            return true;
        };
        let mut libraries = inner.lock();
        let Some(run) = libraries.run_mut(self.library_id, self.run) else {
            return false;
        };
        if !self.holder.load(Ordering::Relaxed) {
            if run.claimed {
                return false;
            }
            run.claimed = true;
            run.waiters = run.waiters.saturating_sub(1);
            run.holders += 1;
            self.holder.store(true, Ordering::Relaxed);
        }
        run.announced |= announces;
        true
    }
}
