use chrono::Utc;
use rustyfin_model::scheduled_task::{
    ScheduledTaskInfo, TaskCompletionStatus, TaskResult, TaskState, TaskTriggerInfo,
};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::watch;

use crate::library_monitor::ChangeQueue;
use crate::library_watch::LibraryWatcher;
use crate::plugin_manager::PluginManager;
use crate::scan_progress::ScanRegistry;
use crate::task_progress::TaskProgress;

/// Known task IDs
pub const TASK_LIBRARY_SCAN: &str = "LibraryScan";
pub const TASK_METADATA_REFRESH: &str = "MetadataRefresh";
pub const TASK_TRANSCODE_CLEANUP: &str = "TranscodeCacheCleanup";
pub const TASK_INTRO_DETECTION: &str = "IntroDetection";
pub const TASK_DB_VACUUM: &str = "DatabaseVacuum";
/// How soon a scheduled VACUUM put off for a scan or an image run is tried again.
const VACUUM_RETRY: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Jellyfin's key for its chapter image task.
pub const TASK_CHAPTER_IMAGES: &str = "RefreshChapterImages";
/// Jellyfin's key for its trickplay task.
pub const TASK_TRICKPLAY: &str = "RefreshTrickplayImages";
pub const TASK_CLEAN_ACTIVITY_LOG: &str = "CleanActivityLog";
/// Jellyfin's key for its TMDb upcoming and missing episodes task.
pub const TASK_MISSING_EPISODES: &str = crate::missing_episodes::TASK_KEY;

/// Jellyfin's default `ActivityLogRetentionDays`.
const DEFAULT_ACTIVITY_LOG_RETENTION_DAYS: u32 = 30;

/// One hour in .NET ticks (1 tick = 100ns)
const TICKS_1H: u64 = 60 * 60 * 10_000_000;
const TICKS_6H: u64 = 6 * TICKS_1H;
const TICKS_12H: u64 = 12 * TICKS_1H;
const TICKS_24H: u64 = 24 * TICKS_1H;

/// A trigger that runs the task at `hour` o'clock each day, server time.
fn daily_at(hour: u64) -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: "DailyTrigger".to_string(),
        interval_ticks: None,
        time_of_day_ticks: Some(hour * TICKS_1H),
        day_of_week: None,
        max_runtime_ticks: None,
    }
}

/// A trigger that runs the task once after each server start.
fn at_startup() -> TaskTriggerInfo {
    TaskTriggerInfo {
        trigger_type: "StartupTrigger".to_string(),
        interval_ticks: None,
        time_of_day_ticks: None,
        day_of_week: None,
        max_runtime_ticks: None,
    }
}

/// Logs a start skipped because the task runs already (by its triggers or by hand).
fn skipped(id: &str) {
    tracing::info!(
        task = id,
        "the task is running already, so this start is skipped"
    );
}

/// The callback that runs when a scheduled task that writes items ends.
type OnItemsChanged = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct TaskEntry {
    info: ScheduledTaskInfo,
}

#[derive(Clone)]
pub struct TaskScheduler {
    tasks: Arc<RwLock<HashMap<String, TaskEntry>>>,
    db: SqlitePool,
    data_dir: Arc<PathBuf>,
    scans: ScanRegistry,
    changes: ChangeQueue,
    watcher: LibraryWatcher,
    on_items_changed: Arc<std::sync::RwLock<Option<OnItemsChanged>>>,
    plugin_manager: crate::plugin_manager::PluginManager,
    http_client: reqwest::Client,
    /// Every task run going on, each with the handle that stops it. A run stays listed until it
    /// has wound down, so a stopped one shows Cancelling until then.
    image_runs: Arc<std::sync::Mutex<HashMap<String, crate::trickplay::Cancel>>>,
    /// Woken each time a run ends (is unlisted), for the starts waiting on it.
    run_ended: Arc<tokio::sync::Notify>,
    /// The tasks with a start by hand waiting for their current run to end.
    queued: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Each task's percent done, shown while it runs.
    progress: Arc<HashMap<String, TaskProgress>>,
    /// Woken when a task's triggers change, so its loop works out its next run again.
    triggers_changed: Arc<tokio::sync::Notify>,
    /// Each task's built-in triggers. Only triggers that differ from them are saved, so a later
    /// change of the defaults reaches the tasks an admin never edited.
    default_triggers: Arc<HashMap<String, Vec<TaskTriggerInfo>>>,
    /// The tasks running now, started by their triggers or by hand: a second start is skipped.
    running: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Held while the triggers file is written, and the results file: one save at a time, each
    /// of the state as it is once the lock is taken, so the last save is never an older state.
    saving_triggers: Arc<tokio::sync::Mutex<()>>,
    saving_results: Arc<tokio::sync::Mutex<()>>,
    /// The tasks an admin stopped. A library scan's end doesn't start them again; their next
    /// trigger or a start by hand does, as a cancelled task in Jellyfin waits for its trigger.
    held: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

/// A task's claim on running, from `TaskScheduler::claim` until it is dropped. The run is listed
/// with `cancel` meanwhile, which Stop fires; the run checks it between its units of work.
struct RunClaim {
    running: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    runs: Arc<std::sync::Mutex<HashMap<String, crate::trickplay::Cancel>>>,
    run_ended: Arc<tokio::sync::Notify>,
    id: String,
    cancel: crate::trickplay::Cancel,
}

impl RunClaim {
    /// Whether Stop has asked this run to end.
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

impl Drop for RunClaim {
    fn drop(&mut self) {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.run_ended.notify_waiters();
    }
}

/// `status`, or Cancelled for a run Stop asked to end that would otherwise have completed.
fn cancelled_or(
    status: TaskCompletionStatus,
    cancel: &crate::trickplay::Cancel,
) -> TaskCompletionStatus {
    if status == TaskCompletionStatus::Completed && cancel.is_cancelled() {
        TaskCompletionStatus::Cancelled
    } else {
        status
    }
}

/// The tasks a run can be stopped in (their ffmpeg killed): `MaxRuntimeTicks` applies to them.
/// The others store it only.
const STOPPABLE_TASKS: [&str; 3] = [TASK_CHAPTER_IMAGES, TASK_TRICKPLAY, TASK_INTRO_DETECTION];

/// The file, in the data folder, that keeps the triggers set through the API, by task id.
const TRIGGERS_FILE: &str = "scheduled_task_triggers.json";

/// The file, in the data folder, that keeps each task's last result, by task id, so a restart
/// neither forgets when a task last ran nor runs it again at once.
const RESULTS_FILE: &str = "scheduled_task_results.json";

/// A map by task id saved in `data_dir`. A missing file is empty; an unreadable one is logged and
/// taken as empty, so the server still starts.
fn load_saved<T: serde::de::DeserializeOwned>(
    data_dir: &std::path::Path,
    file: &str,
) -> HashMap<String, T> {
    let path = data_dir.join(file);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "could not read the saved task state");
            return HashMap::new();
        }
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(path = %path.display(), error = %e, "saved task state unreadable; starting without it");
        HashMap::new()
    })
}

/// The version of the triggers file this build writes. A file without one is from before the
/// defaults followed Jellyfin's.
const TRIGGERS_VERSION: u32 = 2;

/// The triggers file: the edited tasks' triggers by task id, with the file's version.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SavedTriggers<T> {
    version: u32,
    triggers: HashMap<String, T>,
}

/// The triggers file's text for `triggers`.
fn triggers_json<T: serde::Serialize>(triggers: HashMap<String, T>) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(&SavedTriggers {
        version: TRIGGERS_VERSION,
        triggers,
    })
    .map_err(std::io::Error::other)
}

/// The triggers saved in `data_dir`, by task id.
///
/// A file without a version is migrated once, then written again with one: a task whose saved
/// triggers are only its built-in triggers from before the defaults followed Jellyfin's (every
/// task but the library scan ran each 24 hours, and an early build saved every task's triggers)
/// takes the new defaults, as never edited. A versioned file holds only edits and is kept as is.
fn saved_triggers(data_dir: &std::path::Path) -> HashMap<String, Vec<TaskTriggerInfo>> {
    let value: serde_json::Value = match load_saved::<serde_json::Value>(data_dir, TRIGGERS_FILE) {
        map if map.is_empty() => return HashMap::new(),
        map => serde_json::Value::Object(map.into_iter().collect()),
    };
    if value.get("Version").is_some() {
        return serde_json::from_value::<SavedTriggers<Vec<TaskTriggerInfo>>>(value)
            .map(|f| f.triggers)
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "task triggers unreadable; using the defaults");
                HashMap::new()
            });
    }
    let mut saved: HashMap<String, Vec<TaskTriggerInfo>> = match serde_json::from_value(value) {
        Ok(saved) => saved,
        Err(e) => {
            tracing::warn!(error = %e, "task triggers unreadable; using the defaults");
            return HashMap::new();
        }
    };
    let old_default = [TaskTriggerInfo::interval(TICKS_24H)];
    saved.retain(|id, triggers| {
        !([
            TASK_TRANSCODE_CLEANUP,
            TASK_INTRO_DETECTION,
            TASK_DB_VACUUM,
            TASK_CHAPTER_IMAGES,
            TASK_TRICKPLAY,
            TASK_CLEAN_ACTIVITY_LOG,
        ]
        .contains(&id.as_str())
            && triggers[..] == old_default)
    });
    let written = triggers_json(saved.clone())
        .and_then(|json| write_atomic_sync(&data_dir.join(TRIGGERS_FILE), &json));
    if let Err(e) = written {
        tracing::warn!(error = %e, "could not save the migrated task triggers");
    }
    saved
}

/// Writes `bytes` to `path` through a temporary file synced to disk, so a crash never leaves half
/// a file.
fn write_atomic_sync(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// `write_atomic_sync` off the async runtime.
async fn write_atomic(path: std::path::PathBuf, bytes: Vec<u8>) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || write_atomic_sync(&path, &bytes))
        .await
        .map_err(std::io::Error::other)?
}

impl TaskScheduler {
    pub fn new(db: SqlitePool, data_dir: Arc<PathBuf>) -> Self {
        Self::with_http_client(
            db,
            data_dir,
            reqwest::Client::builder()
                .user_agent(crate::tmdb_client::USER_AGENT)
                .pool_max_idle_per_host(10)
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        )
    }

    /// Create a scheduler with a shared HTTP client (connection pool reuse).
    /// It gets its own empty plugin manager; use `with_plugin_manager` to share the server's.
    pub fn with_http_client(
        db: SqlitePool,
        data_dir: Arc<PathBuf>,
        http_client: reqwest::Client,
    ) -> Self {
        let plugin_manager = PluginManager::new(&data_dir);
        Self::with_plugin_manager(db, data_dir, http_client, plugin_manager)
    }

    /// Create a scheduler that uses the given plugin manager, so scheduled scans reach the
    /// metadata providers registered on it (B135).
    pub fn with_plugin_manager(
        db: SqlitePool,
        data_dir: Arc<PathBuf>,
        http_client: reqwest::Client,
        plugin_manager: PluginManager,
    ) -> Self {
        let mut tasks = HashMap::new();

        // Library Scan
        tasks.insert(
            TASK_LIBRARY_SCAN.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_LIBRARY_SCAN.to_string(),
                    key: "RefreshLibrary".to_string(),
                    is_hidden: false,
                    name: "Scan Media Library".to_string(),
                    description: "Scans all libraries for new and updated media files.".to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo::interval(TICKS_12H)],
                    last_execution_result: None,
                },
            },
        );

        // Metadata Refresh
        tasks.insert(
            TASK_METADATA_REFRESH.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_METADATA_REFRESH.to_string(),
                    // Jellyfin has no task like it; its refresh runs inside RefreshLibrary.
                    key: TASK_METADATA_REFRESH.to_string(),
                    is_hidden: false,
                    name: "Refresh Metadata".to_string(),
                    description:
                        "Fills in metadata for media items that are not identified yet, from local and remote sources."
                            .to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    // Like Jellyfin, no timer by default: identified items are never re-queried.
                    triggers: vec![],
                    last_execution_result: None,
                },
            },
        );

        // Transcode Cache Cleanup
        tasks.insert(
            TASK_TRANSCODE_CLEANUP.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_TRANSCODE_CLEANUP.to_string(),
                    key: "DeleteTranscodeFiles".to_string(),
                    is_hidden: false,
                    name: "Transcode Cache Cleanup".to_string(),
                    description: "Removes old transcoded files from the cache directory."
                        .to_string(),
                    category: "Maintenance".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    // As Jellyfin: once after each start, then every 24 hours
                    triggers: vec![at_startup(), TaskTriggerInfo::interval(TICKS_24H)],
                    last_execution_result: None,
                },
            },
        );

        // Intro/Outro Detection
        tasks.insert(
            TASK_INTRO_DETECTION.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_INTRO_DETECTION.to_string(),
                    // Jellyfin's Media Segment Scan, which finds intros and outros the same way.
                    key: "TaskExtractMediaSegments".to_string(),
                    is_hidden: false,
                    name: "Detect Intros and Outros".to_string(),
                    description: "Analyzes episodes to detect intro and outro segments for skip functionality.".to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo::interval(TICKS_12H)],
                    last_execution_result: None,
                },
            },
        );

        // Database VACUUM, every 6 hours as Jellyfin's Optimize database task
        tasks.insert(
            TASK_DB_VACUUM.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_DB_VACUUM.to_string(),
                    key: "OptimizeDatabaseTask".to_string(),
                    is_hidden: false,
                    name: "Database Maintenance".to_string(),
                    description:
                        "Runs VACUUM to defragment the SQLite database and reclaim disk space."
                            .to_string(),
                    category: "Maintenance".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo::interval(TICKS_6H)],
                    last_execution_result: None,
                },
            },
        );

        // Chapter images and trickplay tiles: nightly, for the libraries that turn them on. As
        // Jellyfin: chapter images at 02:00 for at most 4 hours, trickplay at 03:00.
        let chapter_images = TaskTriggerInfo {
            max_runtime_ticks: Some(4 * TICKS_1H),
            ..daily_at(2)
        };
        for (id, name, description, trigger) in [
            (
                TASK_CHAPTER_IMAGES,
                "Extract Chapter Images",
                "Creates thumbnails for videos that have chapters, in the libraries that turn chapter images on.",
                chapter_images,
            ),
            (
                TASK_TRICKPLAY,
                "Generate Trickplay Images",
                "Creates trickplay images for videos, in the libraries that turn trickplay on.",
                daily_at(3),
            ),
        ] {
            tasks.insert(
                id.to_string(),
                TaskEntry {
                    info: ScheduledTaskInfo {
                        id: id.to_string(),
                        key: id.to_string(),
                        is_hidden: false,
                        name: name.to_string(),
                        description: description.to_string(),
                        category: "Library".to_string(),
                        state: TaskState::Idle,
                        current_progress_percentage: None,
                        triggers: vec![trigger],
                        last_execution_result: None,
                    },
                },
            );
        }

        // Activity log cleanup, as Jellyfin's Clean Activity Log task
        tasks.insert(
            TASK_CLEAN_ACTIVITY_LOG.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_CLEAN_ACTIVITY_LOG.to_string(),
                    key: TASK_CLEAN_ACTIVITY_LOG.to_string(),
                    is_hidden: false,
                    name: "Clean Activity Log".to_string(),
                    description:
                        "Deletes activity log entries older than the configured age (ActivityLogRetentionDays)."
                            .to_string(),
                    category: "Maintenance".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    // Like Jellyfin, no trigger by default: it runs when started or scheduled
                    triggers: vec![],
                    last_execution_result: None,
                },
            },
        );

        // Jellyfin 12.4's TMDb upcoming and missing episodes task
        tasks.insert(
            TASK_MISSING_EPISODES.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_MISSING_EPISODES.to_string(),
                    key: TASK_MISSING_EPISODES.to_string(),
                    is_hidden: false,
                    name: crate::missing_episodes::TASK_NAME.to_string(),
                    description: crate::missing_episodes::TASK_DESCRIPTION.to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    // MissingEpisodeRefreshIntervalDays' default
                    triggers: vec![TaskTriggerInfo::interval(
                        crate::missing_episodes::DEFAULT_INTERVAL_DAYS as u64 * TICKS_24H,
                    )],
                    last_execution_result: None,
                },
            },
        );

        let progress = tasks
            .keys()
            .map(|id| (id.clone(), TaskProgress::new()))
            .collect();
        let changes = ChangeQueue::new(db.clone());
        let watcher = LibraryWatcher::new(db.clone(), changes.clone(), (*data_dir).clone());
        let default_triggers: HashMap<String, Vec<TaskTriggerInfo>> = tasks
            .iter()
            .map(|(id, e)| (id.clone(), e.info.triggers.clone()))
            .collect();
        for (id, triggers) in saved_triggers(&data_dir) {
            if let Some(entry) = tasks.get_mut(&id) {
                entry.info.triggers = triggers;
            }
        }
        for (id, result) in load_saved::<TaskResult>(&data_dir, RESULTS_FILE) {
            if let Some(entry) = tasks.get_mut(&id) {
                entry.info.last_execution_result = Some(result);
            }
        }
        Self {
            tasks: Arc::new(RwLock::new(tasks)),
            db,
            data_dir,
            scans: ScanRegistry::new(),
            changes,
            watcher,
            on_items_changed: Arc::new(std::sync::RwLock::new(None)),
            plugin_manager,
            http_client,
            image_runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            run_ended: Arc::default(),
            queued: Arc::default(),
            progress: Arc::new(progress),
            triggers_changed: Arc::new(tokio::sync::Notify::new()),
            default_triggers: Arc::new(default_triggers),
            running: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            saving_triggers: Arc::default(),
            saving_results: Arc::default(),
            held: Arc::default(),
        }
    }

    /// Claims the task `id` for one run, listed with a new `Cancel` that Stop fires; None when a
    /// run of it is going on already.
    fn claim(&self, id: &str) -> Option<RunClaim> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(id.to_string()) {
            return None;
        }
        let cancel = crate::trickplay::Cancel::new();
        self.image_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), cancel.clone());
        Some(RunClaim {
            running: self.running.clone(),
            runs: self.image_runs.clone(),
            run_ended: self.run_ended.clone(),
            id: id.to_string(),
            cancel,
        })
    }

    /// Whether a run of task `id` is going on (a stopped one included, until it has wound down).
    fn is_listed(&self, id: &str) -> bool {
        self.image_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }

    /// Whether `id` names a task.
    pub async fn has_task(&self, id: &str) -> bool {
        self.tasks.read().await.contains_key(id)
    }

    /// The shortest `MaxRuntimeTicks` of the task's triggers, for a task that can be stopped.
    async fn max_runtime(&self, id: &str) -> Option<std::time::Duration> {
        if !STOPPABLE_TASKS.contains(&id) {
            return None;
        }
        let tasks = self.tasks.read().await;
        tasks
            .get(id)?
            .info
            .triggers
            .iter()
            .filter_map(|t| t.max_runtime_ticks)
            .filter(|t| *t > 0)
            .min()
            .map(|t| std::time::Duration::from_micros(t / 10))
    }

    /// Stops every task run going on, killing the ffmpeg of the image tasks and intro detection
    /// (at shutdown).
    pub fn cancel_runs(&self) {
        for cancel in self.image_runs.lock().unwrap().values() {
            cancel.cancel();
        }
    }

    /// Sets the callback that runs once each time a scheduled task that writes items ends,
    /// whether or not it failed: the library scan, the metadata refresh and the missing episodes.
    /// The server uses it to clear the caches those items are in.
    pub fn set_on_items_changed(&self, f: impl Fn() + Send + Sync + 'static) {
        let mut slot = self
            .on_items_changed
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(Arc::new(f));
    }

    /// Runs the `set_on_items_changed` callback, if any.
    fn items_changed(&self) {
        // Cloned out of the lock so the callback never runs while the lock is held.
        let callback = self
            .on_items_changed
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(f) = callback {
            f();
        }
    }

    /// The library scans in progress.
    pub fn scans(&self) -> &ScanRegistry {
        &self.scans
    }

    /// The queue of changed library paths, which the library monitor rescans.
    pub fn changes(&self) -> &ChangeQueue {
        &self.changes
    }

    /// The library monitor, which reports changed library paths to `changes()`.
    pub fn watcher(&self) -> &LibraryWatcher {
        &self.watcher
    }

    pub async fn get_all_tasks(&self) -> Vec<ScheduledTaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.values().map(|e| self.info_of(e)).collect()
    }

    pub async fn get_task(&self, id: &str) -> Option<ScheduledTaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.get(id).map(|e| self.info_of(e))
    }

    /// A task's info, with the percent it has reported while it is not idle.
    ///
    /// As in Jellyfin, every library scan shows as the library scan task. Its `State` and percent
    /// come only from the scan registry, the source of the web client's scan rings, the
    /// `RefreshProgress` frames and `RefreshStatus`: Running while any library has a scan listed
    /// (queued, scanning or probing), wherever it was started (the scheduler, whose run holds
    /// tickets like any other scan, a library's Scan, Scan all, the setup wizard), at the batch's
    /// percent (`library_scan_state`); Idle when none is. A stopped scheduler run reads Cancelling while
    /// its scans are listed. `LastExecutionResult` stays the scheduler's.
    fn info_of(&self, entry: &TaskEntry) -> ScheduledTaskInfo {
        let mut info = entry.info.clone();
        info.current_progress_percentage = match info.state {
            TaskState::Idle => None,
            TaskState::Running | TaskState::Cancelling => {
                self.progress.get(&info.id).and_then(TaskProgress::get)
            }
        };
        if info.id == TASK_LIBRARY_SCAN {
            let (state, percent) = library_scan_state(&self.scans, &info.state);
            info.state = state;
            info.current_progress_percentage = percent;
        }
        info
    }

    /// Where task `id` reports its percent done.
    fn progress_of(&self, id: &str) -> TaskProgress {
        self.progress.get(id).cloned().unwrap_or_default()
    }

    /// Replaces a task's triggers and saves them in the data folder, so they hold after a
    /// restart. Only the tasks whose triggers differ from the built-in ones are saved. Ok(false)
    /// for an unknown task; Err when the file could not be written (the new triggers apply until
    /// the server stops).
    pub async fn set_triggers(
        &self,
        id: &str,
        triggers: Vec<TaskTriggerInfo>,
    ) -> Result<bool, std::io::Error> {
        {
            let mut tasks = self.tasks.write().await;
            let Some(entry) = tasks.get_mut(id) else {
                return Ok(false);
            };
            entry.info.triggers = triggers;
        }
        self.triggers_changed.notify_waiters();
        let _saving = self.saving_triggers.lock().await;
        let json = {
            let tasks = self.tasks.read().await;
            triggers_json(
                tasks
                    .iter()
                    .filter(|(id, e)| self.default_triggers.get(*id) != Some(&e.info.triggers))
                    .map(|(id, e)| (id.clone(), e.info.triggers.clone()))
                    .collect(),
            )?
        };
        write_atomic(self.data_dir.join(TRIGGERS_FILE), json).await?;
        Ok(true)
    }

    /// Whether an admin stopped task `id` since its triggers or a start by hand last ran it.
    fn is_held(&self, id: &str) -> bool {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    /// Lets a scan's end start task `id` again.
    fn release(&self, id: &str) {
        self.held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    /// Starts a task by hand; false for an unknown task. While a run of it is still going (a
    /// stopped one winding down included), the start waits for that run to end, as Jellyfin
    /// queues it, rather than being dropped.
    pub async fn start_task(&self, id: &str) -> bool {
        if !self.has_task(id).await {
            return false;
        }
        self.release(id);
        if !self.is_listed(id) {
            return self.start_run(id).await;
        }
        let first = self
            .queued
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string());
        if first {
            tracing::info!(
                task = id,
                "the task is still running; it starts again once that run ends"
            );
            let sched = self.clone();
            let id = id.to_string();
            tokio::spawn(async move {
                loop {
                    let ended = sched.run_ended.notified();
                    tokio::pin!(ended);
                    ended.as_mut().enable();
                    if !sched.is_listed(&id) {
                        break;
                    }
                    ended.await;
                }
                sched
                    .queued
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                sched.start_run(&id).await;
            });
        }
        true
    }

    /// Starts a run of task `id` in the background; false for an unknown task.
    async fn start_run(&self, id: &str) -> bool {
        let tasks = self.tasks.read().await;
        if tasks.contains_key(id) {
            drop(tasks);
            // Spawn the task execution
            match id {
                TASK_LIBRARY_SCAN => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_library_scan().await });
                }
                TASK_METADATA_REFRESH => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_metadata_refresh().await });
                }
                TASK_TRANSCODE_CLEANUP => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_transcode_cleanup().await });
                }
                TASK_INTRO_DETECTION => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_intro_detection().await });
                }
                TASK_DB_VACUUM => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_db_vacuum().await });
                }
                TASK_CHAPTER_IMAGES | TASK_TRICKPLAY => {
                    let sched = self.clone();
                    let id = id.to_string();
                    tokio::spawn(async move { sched.run_video_images(&id).await });
                }
                TASK_CLEAN_ACTIVITY_LOG => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_clean_activity_log().await });
                }
                TASK_MISSING_EPISODES => {
                    let sched = self.clone();
                    tokio::spawn(async move { sched.run_missing_episodes().await });
                }
                _ => {}
            }
            true
        } else {
            false
        }
    }

    /// Stops a running task at an admin's request; false for an unknown task. A task with no run
    /// going is left as it is, as in Jellyfin. A stopped task isn't started again by a library
    /// scan's end until its triggers or a start by hand run it.
    pub async fn stop_task(&self, id: &str) -> bool {
        if !self.has_task(id).await {
            return false;
        }
        let mut stopped = self.stop_run(id).await;
        // Every scan in the registry is a run of the library scan task, as in Jellyfin: a library's
        // Scan, Scan all and the setup wizard's scans stop with it
        if id == TASK_LIBRARY_SCAN && self.scans.cancel_all() > 0 {
            stopped = true;
            if !self.is_listed(id) {
                self.record_stopped_scans();
            }
        }
        if stopped {
            self.held
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id.to_string());
        }
        true
    }

    /// Records the library scan task's result, Cancelled, once the stopped scans that weren't
    /// its own run have all ended (that run records its own).
    fn record_stopped_scans(&self) {
        let sched = self.clone();
        let start = Utc::now();
        tokio::spawn(async move {
            let mut updates = sched.scans.subscribe();
            while sched.scans.any_cancelled() {
                // Each run's end sends a frame; the timeout covers a missed one
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(1), updates.recv()).await;
            }
            if sched.is_listed(TASK_LIBRARY_SCAN) {
                return;
            }
            sched
                .set_task_result(
                    TASK_LIBRARY_SCAN,
                    TaskResult {
                        status: TaskCompletionStatus::Cancelled,
                        error_message: None,
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: Utc::now().to_rfc3339(),
                    },
                )
                .await;
        });
    }

    /// Tells the run of task `id` to stop: it shows Cancelling, as in Jellyfin, until it has wound
    /// down (between its units of work; an image task also kills the ffmpeg it runs). False when
    /// no run of it is going. The run is looked up and marked under the tasks lock, as its start
    /// and end change them, so a run that has just ended stays Idle.
    async fn stop_run(&self, id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        let run = self.image_runs.lock().unwrap().get(id).cloned();
        let Some(cancel) = run else {
            return false;
        };
        cancel.cancel();
        if let Some(entry) = tasks.get_mut(id) {
            self.apply_state(entry, TaskState::Cancelling);
        }
        true
    }

    /// Resets the state of the tasks with no run going; a running task's own state stands.
    pub async fn restart(&self) {
        tracing::info!("Restarting all scheduled tasks");
        let ids: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks.keys().cloned().collect()
        };
        for id in ids {
            let mut tasks = self.tasks.write().await;
            if !self.is_listed(&id)
                && let Some(entry) = tasks.get_mut(&id)
            {
                self.apply_state(entry, TaskState::Idle);
            }
        }
        tracing::info!("All scheduled tasks restarted");
    }

    async fn set_task_state(&self, id: &str, state: TaskState) {
        let mut tasks = self.tasks.write().await;
        if let Some(entry) = tasks.get_mut(id) {
            self.apply_state(entry, state);
        }
    }

    /// Moves a task to `state`, under the tasks lock its caller holds. Only a running task can
    /// become Cancelling, and a cancelling one stays so until it ends (Idle). A run starts with no
    /// percent known, and an ended one keeps none.
    fn apply_state(&self, entry: &mut TaskEntry, state: TaskState) {
        let allowed = match (&entry.info.state, &state) {
            (TaskState::Running, TaskState::Cancelling) => true,
            (_, TaskState::Cancelling) => false,
            (TaskState::Cancelling, TaskState::Running) => false,
            _ => true,
        };
        if !allowed {
            return;
        }
        // A run that Stop reached before it showed Running shows Cancelling instead
        let state = match state {
            TaskState::Running
                if self
                    .image_runs
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&entry.info.id)
                    .is_some_and(|c| c.is_cancelled()) =>
            {
                TaskState::Cancelling
            }
            state => state,
        };
        if state != TaskState::Cancelling {
            self.progress_of(&entry.info.id).clear();
        }
        entry.info.state = state;
    }

    /// Records the end of a run and saves every task's last result in the data folder.
    async fn set_task_result(&self, id: &str, result: TaskResult) {
        let failed = (result.status == TaskCompletionStatus::Failed)
            .then(|| result.error_message.clone().unwrap_or_default());
        let name = {
            let mut tasks = self.tasks.write().await;
            let Some(entry) = tasks.get_mut(id) else {
                return;
            };
            entry.info.last_execution_result = Some(result);
            entry.info.name.clone()
        };
        let saved = {
            let _saving = self.saving_results.lock().await;
            let json = {
                let tasks = self.tasks.read().await;
                let all: HashMap<&String, &TaskResult> = tasks
                    .iter()
                    .filter_map(|(id, e)| Some((id, e.info.last_execution_result.as_ref()?)))
                    .collect();
                serde_json::to_vec_pretty(&all)
            };
            match json {
                Ok(json) => write_atomic(self.data_dir.join(RESULTS_FILE), json).await,
                Err(e) => Err(std::io::Error::other(e)),
            }
        };
        if let Err(e) = saved {
            tracing::warn!(task = id, error = %e, "could not save the task's last result");
        }
        // A failed task goes in the activity log and to the notification providers, as in
        // Jellyfin.
        if let Some(error) = &failed {
            let message = if error.is_empty() {
                format!("{name} failed")
            } else {
                format!("{name} failed: {error}")
            };
            self.plugin_manager.notify(
                crate::plugin_manager::NotificationEvent::TaskFailed,
                &format!("{name} failed"),
                &message,
            );
        }
        if let Some(error) = failed
            && let Err(e) = rustyfin_db::repo_activity::log_activity(
                &self.db,
                &format!("{name} failed"),
                "TaskFailed",
                None,
                None,
                "Error",
                Some(&error).filter(|e| !e.is_empty()).map(String::as_str),
            )
            .await
        {
            tracing::warn!("could not write the activity log: {e}");
        }
    }

    /// The configured `ActivityLogRetentionDays` from config.json, else Jellyfin's 30.
    async fn activity_log_retention_days(&self) -> u32 {
        let Ok(text) = tokio::fs::read_to_string(self.data_dir.join("config.json")).await else {
            return DEFAULT_ACTIVITY_LOG_RETENTION_DAYS;
        };
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("ActivityLogRetentionDays")?.as_u64())
            .and_then(|d| u32::try_from(d).ok())
            .unwrap_or(DEFAULT_ACTIVITY_LOG_RETENTION_DAYS)
    }

    /// Deletes the activity log entries older than the retention age. A retention of 0 keeps
    /// every entry.
    /// Creates and prunes the virtual (unaired and missing) episodes, as the TMDb plugin's
    /// settings say (Jellyfin 12.4's TmdbRefreshUpcomingEpisodes).
    async fn run_missing_episodes(&self) {
        use crate::missing_episodes::{Settings, Source, TMDB_PLUGIN_ID};
        let Some(claim) = self.claim(TASK_MISSING_EPISODES) else {
            return skipped(TASK_MISSING_EPISODES);
        };
        self.set_task_state(TASK_MISSING_EPISODES, TaskState::Running)
            .await;
        let start = Utc::now();
        let progress = self.progress_of(TASK_MISSING_EPISODES);
        let config = self.plugin_manager.get_configuration(TMDB_PLUGIN_ID).await;
        let settings = Settings::from_config(config.as_ref().map(|c| &c.values));
        let providers = self.plugin_manager.metadata_providers().await;
        let source = providers
            .iter()
            .find(|p| p.plugin_id == TMDB_PLUGIN_ID)
            .and_then(|p| {
                Some(Source {
                    lookups: p.lookups.as_deref()?,
                    provider_name: &p.provider_name,
                })
            });
        let report = |share: f64| progress.report(share);
        let outcome = crate::missing_episodes::run_until_cancelled(
            &self.db,
            source,
            &settings,
            Utc::now().date_naive(),
            &report,
            &claim.cancel,
        )
        .await;
        let (status, error_message) = match outcome {
            Ok(outcome) => {
                tracing::info!(
                    "upcoming and missing episodes: {} series, {} created, {} updated, {} removed, {} failed",
                    outcome.series,
                    outcome.created,
                    outcome.updated,
                    outcome.pruned,
                    outcome.failed
                );
                (TaskCompletionStatus::Completed, None)
            }
            Err(e) => (TaskCompletionStatus::Failed, Some(e)),
        };
        let result = TaskResult {
            status: cancelled_or(status, &claim.cancel),
            error_message,
            start_time_utc: start.to_rfc3339(),
            end_time_utc: Utc::now().to_rfc3339(),
        };
        self.items_changed();
        self.set_task_result(TASK_MISSING_EPISODES, result).await;
        self.end_run(claim).await;
    }

    async fn run_clean_activity_log(&self) {
        let Some(claim) = self.claim(TASK_CLEAN_ACTIVITY_LOG) else {
            return skipped(TASK_CLEAN_ACTIVITY_LOG);
        };
        self.set_task_state(TASK_CLEAN_ACTIVITY_LOG, TaskState::Running)
            .await;
        let start = Utc::now();
        let days = self.activity_log_retention_days().await;
        // One delete: a stop before it skips it, a stop during it lets it finish
        let outcome = if days == 0 || claim.is_cancelled() {
            Ok(0)
        } else {
            rustyfin_db::repo_activity::delete_activity_older_than(&self.db, days).await
        };
        let (status, error_message) = match outcome {
            Ok(removed) => {
                if removed > 0 {
                    tracing::info!("activity log cleanup: removed {removed} entries");
                }
                (TaskCompletionStatus::Completed, None)
            }
            Err(e) => (TaskCompletionStatus::Failed, Some(e.to_string())),
        };
        let result = TaskResult {
            status: cancelled_or(status, &claim.cancel),
            error_message,
            start_time_utc: start.to_rfc3339(),
            end_time_utc: Utc::now().to_rfc3339(),
        };
        self.set_task_result(TASK_CLEAN_ACTIVITY_LOG, result).await;
        self.end_run(claim).await;
    }

    /// When the task's triggers next run it; `clock.last_end` is the later of its loop's last run
    /// and its last recorded result (a run started by hand counts). A recorded end in the future
    /// (the clock was ahead when it was saved) counts as now.
    async fn next_due(
        &self,
        id: &str,
        mut clock: crate::task_triggers::TriggerClock,
    ) -> Option<chrono::DateTime<Utc>> {
        let tasks = self.tasks.read().await;
        let info = &tasks.get(id)?.info;
        let recorded = info
            .last_execution_result
            .as_ref()
            .and_then(|r| chrono::DateTime::parse_from_rfc3339(&r.end_time_utc).ok())
            .map(|t| t.with_timezone(&Utc).min(Utc::now()));
        clock.last_end = clock.last_end.max(recorded);
        crate::task_triggers::next_due(&chrono::Local, &info.triggers, &clock)
    }

    /// Waits until the task's triggers say it runs. A change of triggers is taken up at once, and
    /// a far time is looked at again every hour (the clock may change meanwhile). Returns false at
    /// shutdown.
    async fn wait_for_trigger(
        &self,
        id: &str,
        clock: crate::task_triggers::TriggerClock,
        rx: &mut watch::Receiver<bool>,
    ) -> bool {
        const RECHECK: std::time::Duration = std::time::Duration::from_secs(3600);
        loop {
            let changed = self.triggers_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let wait = match self.next_due(id, clock).await {
                Some(due) => match (due - Utc::now()).to_std() {
                    Ok(left) if !left.is_zero() => left.min(RECHECK),
                    _ => return true,
                },
                None => RECHECK,
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = &mut changed => {}
                _ = rx.wait_for(|v| *v) => return false,
            }
        }
    }

    /// Runs a run the triggers started. Past the task's `MaxRuntimeTicks` it is stopped, as Stop
    /// does (a task in `STOPPABLE_TASKS` only); a run started by hand has no limit, as in Jellyfin.
    async fn run_limited(&self, id: &str, run: impl std::future::Future<Output = bool>) -> bool {
        let Some(limit) = self.max_runtime(id).await else {
            return run.await;
        };
        tokio::pin!(run);
        tokio::select! {
            done = &mut run => done,
            _ = tokio::time::sleep(limit) => {
                tracing::info!(task = id, ?limit, "the task ran past its MaxRuntimeTicks; stopping it");
                self.stop_run(id).await;
                run.await
            }
        }
    }

    /// Runs the task each time its triggers say, until shutdown. A task that has not run yet runs
    /// `delay` after the start for an interval or startup trigger, or one interval after the
    /// start when `interval_from_start`. `run` returns false for a run put off, tried again
    /// `VACUUM_RETRY` later.
    fn spawn_task_loop<F, Fut>(
        &self,
        id: &'static str,
        delay: std::time::Duration,
        interval_from_start: bool,
        shutdown_rx: &watch::Receiver<bool>,
        run: F,
    ) where
        F: Fn(TaskScheduler) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = bool> + Send,
    {
        let sched = self.clone();
        let mut rx = shutdown_rx.clone();
        let started = Utc::now();
        let mut clock = crate::task_triggers::TriggerClock {
            last_end: None,
            loop_ran: false,
            first: started + chrono::Duration::from_std(delay).unwrap_or_default(),
            interval_from: interval_from_start.then_some(started),
            started,
        };
        tokio::spawn(async move {
            loop {
                if !sched.wait_for_trigger(id, clock, &mut rx).await {
                    break;
                }
                sched.release(id);
                while !sched.run_limited(id, run(sched.clone())).await {
                    tokio::select! {
                        _ = tokio::time::sleep(VACUUM_RETRY) => {}
                        _ = rx.wait_for(|v| *v) => return,
                    }
                }
                clock.last_end = Some(Utc::now());
                clock.loop_ran = true;
            }
        });
    }

    /// Run the library scan task once
    async fn run_library_scan(&self) {
        let Some(claim) = self.claim(TASK_LIBRARY_SCAN) else {
            return skipped(TASK_LIBRARY_SCAN);
        };
        self.set_task_state(TASK_LIBRARY_SCAN, TaskState::Running)
            .await;
        let start = Utc::now();

        let result = match rustyfin_db::repo_library::get_all_libraries(&self.db).await {
            Ok(libraries) => {
                // Every library is listed as queued before the first one starts.
                let tickets: Vec<_> = libraries
                    .iter()
                    .map(|lib| self.scans.queue(lib.id))
                    .collect();
                let client = &self.http_client;
                let mut errors = Vec::new();
                for (lib, ticket) in libraries.iter().zip(tickets) {
                    // A stop ends the task between libraries; the libraries not scanned yet
                    // drop their queued runs
                    if claim.is_cancelled() {
                        break;
                    }
                    // Only the caller that claimed the library's run scans it. A library another
                    // caller claimed is left to it: the task neither scans nor waits for it (its
                    // listing alone keeps the task reading Running).
                    if !ticket.claim() {
                        continue;
                    }
                    if let Err(e) = crate::scanner::scan_library_with_metadata(
                        &self.db,
                        lib,
                        client,
                        &self.plugin_manager,
                        &ticket,
                    )
                    .await
                    {
                        errors.push(format!("{}: {}", lib.name, e));
                    }
                    // The probes the scan queued keep the library listed until they finish, and
                    // the task Running until then. The next library's scan waits for them anyway
                    // (the scan lock). A stop doesn't wait for them.
                    tokio::select! {
                        _ = ticket.wait_end() => {}
                        _ = claim.cancel.cancelled() => {}
                    }
                }
                let end = Utc::now();
                if errors.is_empty() {
                    TaskResult {
                        status: cancelled_or(TaskCompletionStatus::Completed, &claim.cancel),
                        error_message: None,
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                } else {
                    TaskResult {
                        status: TaskCompletionStatus::Failed,
                        error_message: Some(errors.join("; ")),
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                }
            }
            Err(e) => {
                let end = Utc::now();
                TaskResult {
                    status: TaskCompletionStatus::Failed,
                    error_message: Some(e.to_string()),
                    start_time_utc: start.to_rfc3339(),
                    end_time_utc: end.to_rfc3339(),
                }
            }
        };

        self.items_changed();

        self.set_task_result(TASK_LIBRARY_SCAN, result).await;
        self.end_run(claim).await;
    }

    /// Starts a run of image task `id`, unless one is still going (a stopped one included, until
    /// it has wound down). Returns the handle that stops it.
    /// The run is listed and marked Running in one step under the tasks lock, so a stop can't
    /// come in between.
    async fn begin_image_run(&self, id: &str) -> Option<crate::trickplay::Cancel> {
        let mut tasks = self.tasks.write().await;
        let cancel = {
            let mut runs = self.image_runs.lock().unwrap();
            if runs.contains_key(id) {
                return None;
            }
            let cancel = crate::trickplay::Cancel::new();
            runs.insert(id.to_string(), cancel.clone());
            cancel
        };
        if let Some(entry) = tasks.get_mut(id) {
            self.apply_state(entry, TaskState::Running);
        }
        Some(cancel)
    }

    /// Ends the run of image task `id`: unlisted and marked Idle in one step under the tasks lock.
    async fn end_image_run(&self, id: &str) {
        let mut tasks = self.tasks.write().await;
        self.image_runs.lock().unwrap().remove(id);
        if let Some(entry) = tasks.get_mut(id) {
            self.apply_state(entry, TaskState::Idle);
        }
        self.run_ended.notify_waiters();
    }

    /// Ends a claimed run: unlisted and marked Idle in one step under the tasks lock, so the card
    /// shows Idle only once the run is over.
    async fn end_run(&self, claim: RunClaim) {
        let mut tasks = self.tasks.write().await;
        let id = claim.id.clone();
        drop(claim);
        if let Some(entry) = tasks.get_mut(&id) {
            self.apply_state(entry, TaskState::Idle);
        }
    }

    /// The ids of the libraries whose options pass `wanted`.
    async fn libraries_where(
        &self,
        wanted: impl Fn(&rustyfin_model::library::LibraryOptions) -> bool,
    ) -> Result<Vec<String>, String> {
        let libraries = rustyfin_db::repo_library::get_all_libraries(&self.db)
            .await
            .map_err(|e| format!("could not list libraries: {e}"))?;
        let mut ids = Vec::new();
        for library in libraries {
            // A library whose options can't be read is skipped and named, rather than read as
            // having everything off
            let options =
                match rustyfin_db::repo_library::get_library_options(&self.db, library.id).await {
                    Ok(options) => options,
                    Err(e) => {
                        tracing::warn!(
                            library = %library.name,
                            error = %e,
                            "could not read the library's options; skipping it"
                        );
                        continue;
                    }
                };
            if wanted(&options) {
                ids.push(library.id.to_string());
            }
        }
        Ok(ids)
    }

    /// Runs the chapter image task or the trickplay task once, over the libraries that turn it
    /// on. Does nothing when that task is running already.
    async fn run_video_images(&self, id: &str) {
        let Some(cancel) = self.begin_image_run(id).await else {
            return;
        };
        let start = Utc::now();
        let progress = self.progress_of(id);
        let data_dir = self.data_dir.as_path();
        // Tiles and chapter images of deleted items and changed files go first
        if let Err(e) = crate::trickplay::sweep_unused_files(&self.db, data_dir).await {
            tracing::warn!(task = id, error = %e, "could not remove unused image files");
        }
        let result = if id == TASK_TRICKPLAY {
            match self.libraries_where(|o| o.enable_trickplay).await {
                Ok(libs) if libs.is_empty() => Ok(no_library_on(id, NO_TRICKPLAY_LIBRARY)),
                Ok(libs) => crate::trickplay::generate_for_libraries_with_progress(
                    &self.db, data_dir, &libs, &cancel, &progress,
                )
                .await
                .map(|n| format!("Made the trickplay images of {n} videos")),
                Err(e) => Err(e),
            }
        } else {
            match self.libraries_where(|o| o.enable_chapter_extraction).await {
                Ok(libs) if libs.is_empty() => Ok(no_library_on(id, NO_CHAPTER_LIBRARY)),
                Ok(libs) => crate::chapter_images::extract_for_libraries_with_progress(
                    &self.db, data_dir, &libs, &cancel, &progress,
                )
                .await
                .map(|n| format!("Made {n} chapter images")),
                Err(e) => Err(e),
            }
        };
        let (status, message) = match result {
            Ok(message) if cancel.is_cancelled() => (TaskCompletionStatus::Cancelled, message),
            Ok(message) => (TaskCompletionStatus::Completed, message),
            Err(e) => {
                tracing::warn!(task = id, error = %e, "task failed");
                (TaskCompletionStatus::Failed, e)
            }
        };
        let end = Utc::now();
        self.set_task_result(
            id,
            TaskResult {
                status,
                error_message: Some(message),
                start_time_utc: start.to_rfc3339(),
                end_time_utc: end.to_rfc3339(),
            },
        )
        .await;
        self.end_image_run(id).await;
    }

    /// After a scan of library `library_id` ends, starts the chapter image and trickplay tasks
    /// that the library asks to run during scans, unless an admin stopped them since their last
    /// run by trigger or by hand: a scan of several libraries would otherwise start a stopped
    /// task again as each library ends.
    async fn after_scan(&self, library_id: uuid::Uuid) {
        let options = rustyfin_db::repo_library::get_library_options(&self.db, library_id)
            .await
            .unwrap_or_default();
        let chapters =
            options.enable_chapter_extraction && options.extract_chapter_images_during_library_scan;
        let trickplay =
            options.enable_trickplay && options.extract_trickplay_images_during_library_scan;
        for (id, wanted) in [(TASK_CHAPTER_IMAGES, chapters), (TASK_TRICKPLAY, trickplay)] {
            if wanted && !self.is_held(id) {
                self.start_run(id).await;
            }
        }
    }

    /// Runs `work` as a run of task `id` that Stop cancels, registered like the image runs, unless
    /// a run of it is still listed (a stopped one included, until it has wound down): false then,
    /// and nothing runs. The work runs in its own task, so a panic in it ends the run as failed
    /// instead of leaving it listed; the run is unlisted and marked Idle however it ends.
    async fn run_stoppable<F, Fut>(&self, id: &str, work: F) -> bool
    where
        F: FnOnce(crate::trickplay::Cancel) -> Fut,
        Fut: std::future::Future<Output = Result<String, String>> + Send + 'static,
    {
        /// Aborts the work if the run itself is dropped (at shutdown), so it never runs unlisted.
        struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
        impl<T> Drop for AbortOnDrop<T> {
            fn drop(&mut self) {
                self.0.abort();
            }
        }

        let Some(cancel) = self.begin_image_run(id).await else {
            return false;
        };
        let start = Utc::now();
        let mut handle = AbortOnDrop(tokio::spawn(work(cancel.clone())));
        let result = match (&mut handle.0).await {
            Ok(result) => result,
            Err(e) => Err(format!("the run stopped unexpectedly: {e}")),
        };
        let (status, message) = match result {
            Ok(message) if cancel.is_cancelled() => (TaskCompletionStatus::Cancelled, message),
            Ok(message) => (TaskCompletionStatus::Completed, message),
            Err(e) => {
                tracing::warn!(task = id, error = %e, "task failed");
                (TaskCompletionStatus::Failed, e)
            }
        };
        self.set_task_result(
            id,
            TaskResult {
                status,
                error_message: Some(message),
                start_time_utc: start.to_rfc3339(),
                end_time_utc: Utc::now().to_rfc3339(),
            },
        )
        .await;
        self.end_image_run(id).await;
        true
    }

    /// Run intro/outro detection for all episodes. A second run at once (the daily one and one
    /// started by hand) would decode every episode again, so a run that finds one listed does
    /// nothing; Stop cancels the run, killing its ffmpeg.
    async fn run_intro_detection(&self) {
        let db = self.db.clone();
        let progress = self.progress_of(TASK_INTRO_DETECTION);
        let ran = self
            .run_stoppable(TASK_INTRO_DETECTION, move |cancel| async move {
                let count = crate::media_segments::detect_for_all_episodes_with_progress(
                    &db, &progress, &cancel,
                )
                .await?;
                tracing::info!("Intro detection completed: {} episodes processed", count);
                Ok(format!("Processed {} episodes", count))
            })
            .await;
        if !ran {
            tracing::info!("intro detection is running already, so this run is skipped");
        }
    }

    /// Run metadata refresh for all items across all libraries
    async fn run_metadata_refresh(&self) {
        let Some(claim) = self.claim(TASK_METADATA_REFRESH) else {
            return skipped(TASK_METADATA_REFRESH);
        };
        self.set_task_state(TASK_METADATA_REFRESH, TaskState::Running)
            .await;
        let start = Utc::now();

        let result = match rustyfin_db::repo_library::get_all_libraries(&self.db).await {
            Ok(libraries) => {
                let mut errors = Vec::new();
                let http = &self.http_client;

                // Collect all items from all libraries first. The task never searches by name
                // (RefreshOptions::TASK), so a movie or series is refreshed only when a provider
                // can look it up by its TMDb id and it has no final answer recorded
                // (provider_refreshed_at unset): an upgraded library is asked once, by id, and
                // an item whose lookup failed, or a series one of whose seasons failed (it stays
                // unanswered then), is asked again on the next run. Movies and series without a
                // TMDb id are skipped (no search, no rename). Other items get their local images
                // and NFO; their seasons' lookups come from their series' refresh. An explicit
                // refresh from the API still re-queries.
                let by_id = self
                    .plugin_manager
                    .metadata_providers()
                    .await
                    .iter()
                    .any(|p| p.lookups.is_some());
                let mut all_items = Vec::new();
                let mut skipped_identified = 0usize;
                for lib in &libraries {
                    match rustyfin_db::repo_media::get_items_by_library(&self.db, lib.id).await {
                        Ok(items) => {
                            let ids: Vec<uuid::Uuid> = items.iter().map(|i| i.id).collect();
                            let answered =
                                rustyfin_db::repo_media::provider_refreshed_ids(&self.db, &ids)
                                    .await
                                    .unwrap_or_default();
                            for item in items {
                                use rustyfin_model::media::MediaType;
                                let skip =
                                    matches!(item.media_type, MediaType::Movie | MediaType::Series)
                                        && !(by_id
                                            && item.tmdb_id.is_some()
                                            && !answered.contains(&item.id));
                                if skip {
                                    skipped_identified += 1;
                                } else {
                                    all_items.push((lib.name.clone(), item));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!("library {}: {}", lib.name, e));
                        }
                    }
                }
                tracing::info!(
                    "metadata refresh: refreshing {} items, skipped {} identified items",
                    all_items.len(),
                    skipped_identified
                );

                // Process items concurrently with a semaphore (10 concurrent)
                let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
                // A refused or missing API key fails every lookup alike: the first one stops
                // the run, and the refreshes not started yet are skipped
                let unauthorized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let mut handles = Vec::new();
                for (lib_name, item) in all_items {
                    let sem = semaphore.clone();
                    let unauthorized = unauthorized.clone();
                    let db = self.db.clone();
                    let http_client = http.clone();
                    let item_name = item.name.clone();
                    let pm = self.plugin_manager.clone();
                    let cancel = claim.cancel.clone();
                    let handle = tokio::spawn(async move {
                        let _permit = sem.acquire().await;
                        use std::sync::atomic::Ordering;
                        // A stop skips the items not started yet
                        if unauthorized.load(Ordering::Relaxed) || cancel.is_cancelled() {
                            return None;
                        }
                        match crate::metadata::refresh_item_metadata_with(
                            &db,
                            &http_client,
                            &item,
                            &pm,
                            crate::metadata::RefreshOptions::TASK,
                        )
                        .await
                        {
                            Err(e) => Some(format!("{}: {}", item_name, e)),
                            Ok(crate::metadata::RefreshOutcome::Unauthorized) => {
                                if !unauthorized.swap(true, Ordering::Relaxed) {
                                    tracing::warn!(
                                        "metadata refresh: a provider refused its API key (or \
                                         has none); stopping this run"
                                    );
                                }
                                None
                            }
                            Ok(crate::metadata::RefreshOutcome::Done) => None,
                        }
                    });
                    handles.push((lib_name, handle));
                }

                // Await all concurrent refresh tasks. There is one per item, so the percent is
                // the share of the items refreshed.
                let progress = self.progress_of(TASK_METADATA_REFRESH);
                let total = handles.len();
                progress.report_count(0, total);
                for (done, (lib_name, handle)) in handles.into_iter().enumerate() {
                    match handle.await {
                        Ok(Some(e)) => errors.push(e),
                        Ok(None) => {}
                        Err(e) => errors.push(format!("{}: task failed: {}", lib_name, e)),
                    }
                    progress.report_count(done + 1, total);
                }
                // New pictures get their grid copies in the background
                crate::image_warmer::kick();

                let end = Utc::now();
                let status = cancelled_or(TaskCompletionStatus::Completed, &claim.cancel);
                if errors.is_empty() {
                    TaskResult {
                        status,
                        error_message: None,
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                } else {
                    tracing::warn!("metadata refresh had {} errors", errors.len());
                    TaskResult {
                        status,
                        error_message: Some(format!("{} items had errors", errors.len())),
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                }
            }
            Err(e) => {
                let end = Utc::now();
                TaskResult {
                    status: TaskCompletionStatus::Failed,
                    error_message: Some(e.to_string()),
                    start_time_utc: start.to_rfc3339(),
                    end_time_utc: end.to_rfc3339(),
                }
            }
        };

        self.items_changed();
        self.set_task_result(TASK_METADATA_REFRESH, result).await;
        self.end_run(claim).await;
    }

    /// Clean up old transcode cache files
    async fn run_transcode_cleanup(&self) {
        let Some(claim) = self.claim(TASK_TRANSCODE_CLEANUP) else {
            return skipped(TASK_TRANSCODE_CLEANUP);
        };
        self.set_task_state(TASK_TRANSCODE_CLEANUP, TaskState::Running)
            .await;
        let start = Utc::now();

        let cache_dir = self.data_dir.join("cache").join("transcode");
        let result = if cache_dir.exists() {
            match cleanup_transcode_cache(
                &cache_dir,
                &self.progress_of(TASK_TRANSCODE_CLEANUP),
                &claim.cancel,
            )
            .await
            {
                Ok(removed) => {
                    if removed > 0 {
                        tracing::info!("transcode cache cleanup: removed {} files", removed);
                    }
                    let end = Utc::now();
                    TaskResult {
                        status: cancelled_or(TaskCompletionStatus::Completed, &claim.cancel),
                        error_message: None,
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                }
                Err(e) => {
                    let end = Utc::now();
                    TaskResult {
                        status: TaskCompletionStatus::Failed,
                        error_message: Some(e.to_string()),
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                }
            }
        } else {
            let end = Utc::now();
            TaskResult {
                status: TaskCompletionStatus::Completed,
                error_message: None,
                start_time_utc: start.to_rfc3339(),
                end_time_utc: end.to_rfc3339(),
            }
        };

        self.set_task_result(TASK_TRANSCODE_CLEANUP, result).await;
        self.end_run(claim).await;
    }

    /// Start all scheduled task loops. Call once at startup.
    pub fn start(self, shutdown_rx: watch::Receiver<bool>) {
        // The library monitor starts its monitors in the background and returns at once.
        self.watcher.start();

        // Each task runs when its triggers say (by default Jellyfin's). An interval or startup
        // trigger of a task that has not run yet, or whose saved last run is an interval or more
        // ago, runs the task its own delay after the start, so they do not all start at once.
        let secs = std::time::Duration::from_secs;
        self.spawn_task_loop(
            TASK_LIBRARY_SCAN,
            secs(60),
            false,
            &shutdown_rx,
            |s| async move {
                s.run_library_scan().await;
                true
            },
        );
        // No trigger by default. Its first interval run comes one interval after the start.
        self.spawn_task_loop(
            TASK_METADATA_REFRESH,
            secs(120),
            true,
            &shutdown_rx,
            |s| async move {
                s.run_metadata_refresh().await;
                true
            },
        );
        self.spawn_task_loop(
            TASK_TRANSCODE_CLEANUP,
            secs(300),
            false,
            &shutdown_rx,
            |s| async move {
                s.run_transcode_cleanup().await;
                true
            },
        );
        self.spawn_task_loop(
            TASK_INTRO_DETECTION,
            secs(600),
            false,
            &shutdown_rx,
            |s| async move {
                s.run_intro_detection().await;
                true
            },
        );
        // Chapter image and trickplay: they find work only in libraries that turn them on
        for (id, delay) in [(TASK_CHAPTER_IMAGES, 900), (TASK_TRICKPLAY, 1200)] {
            self.spawn_task_loop(id, secs(delay), false, &shutdown_rx, move |s| async move {
                s.run_video_images(id).await;
                true
            });
        }

        // After each library scan, the image tasks the library asks to run during scans
        let sched = self.clone();
        let mut rx = shutdown_rx.clone();
        let mut updates = self.scans.subscribe();
        tokio::spawn(async move {
            loop {
                let update = tokio::select! {
                    update = updates.recv() => update,
                    _ = rx.wait_for(|v| *v) => break,
                };
                match update {
                    Ok(u) if u.status == crate::scan_progress::ScanStatus::Idle => {
                        sched.after_scan(u.library_id).await;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        self.spawn_task_loop(
            TASK_CLEAN_ACTIVITY_LOG,
            secs(900),
            false,
            &shutdown_rx,
            |s| async move {
                s.run_clean_activity_log().await;
                true
            },
        );

        // Once, a little after startup: the chapters of videos probed before they were read
        // It stops at shutdown, and each read is limited in time.
        let db = self.db.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            let cancel = crate::trickplay::Cancel::new();
            let reread = async {
                tokio::time::sleep(std::time::Duration::from_secs(90)).await;
                crate::chapter_images::reread_chapters(&db, &cancel).await
            };
            tokio::select! {
                result = reread => if let Err(e) = result {
                    tracing::warn!(error = %e, "could not re-read chapters");
                },
                _ = rx.wait_for(|v| *v) => cancel.cancel(),
            }
        });

        // Database VACUUM: two hours after the start at the earliest. One put off for a
        // scan or an image run is tried again a little later.
        self.spawn_task_loop(
            TASK_DB_VACUUM,
            secs(7200),
            false,
            &shutdown_rx,
            |s| async move { s.run_db_vacuum().await },
        );
    }

    /// Whether the scheduler's own run of task `id` is going, whatever its reported state (the
    /// library scan task reports the scan registry's).
    async fn run_active(&self, id: &str) -> bool {
        self.tasks
            .read()
            .await
            .get(id)
            .is_some_and(|entry| entry.info.state != TaskState::Idle)
    }

    /// The running work a VACUUM would hold up: it holds the write lock throughout, so their
    /// writes would wait and then fail. `scan_lock_held`: whether `SCAN_LOCK` is held, as by the
    /// scans started outside the scheduler.
    async fn vacuum_blockers(&self, scan_lock_held: bool) -> Vec<String> {
        let mut blockers = Vec::new();
        for id in [
            TASK_LIBRARY_SCAN,
            TASK_METADATA_REFRESH,
            TASK_INTRO_DETECTION,
            TASK_CHAPTER_IMAGES,
            TASK_TRICKPLAY,
        ] {
            if let Some(task) = self.get_task(id).await
                && (task.state != TaskState::Idle || self.run_active(id).await)
            {
                blockers.push(task.name);
            }
        }
        let scan_listed = self
            .get_task(TASK_LIBRARY_SCAN)
            .await
            .is_some_and(|task| task.state != TaskState::Idle)
            || self.run_active(TASK_LIBRARY_SCAN).await;
        if !scan_listed && scan_lock_held {
            blockers.push("a library scan".to_string());
        }
        blockers
    }

    /// Run SQLite VACUUM to defragment the database. Put off, with the task's result saying so,
    /// while a scan or an image run is going: false then. It holds `SCAN_LOCK` throughout, so a
    /// scan that starts meanwhile waits for it.
    async fn run_db_vacuum(&self) -> bool {
        // A run going on already: this start is done, not put off
        let Some(claim) = self.claim(TASK_DB_VACUUM) else {
            skipped(TASK_DB_VACUUM);
            return true;
        };
        let no_scan = crate::scanner::SCAN_LOCK.try_lock();
        let blockers = self.vacuum_blockers(no_scan.is_err()).await;
        if !blockers.is_empty() {
            let now = Utc::now().to_rfc3339();
            let why = format!("Put off while this runs: {}", blockers.join(", "));
            tracing::info!("Database VACUUM: {why}");
            self.set_task_result(
                TASK_DB_VACUUM,
                TaskResult {
                    status: TaskCompletionStatus::Aborted,
                    error_message: Some(why),
                    start_time_utc: now.clone(),
                    end_time_utc: now,
                },
            )
            .await;
            return false;
        }
        // No blockers: the lock was free, and is this run's until it returns.
        let Ok(_no_scan) = no_scan else {
            return false;
        };
        self.set_task_state(TASK_DB_VACUUM, TaskState::Running)
            .await;
        let start = Utc::now();

        // Check integrity before vacuuming
        match sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&self.db)
            .await
        {
            Ok(status) if status == "ok" => {
                tracing::info!("Database integrity check passed, running VACUUM");
            }
            Ok(status) => {
                let end = Utc::now();
                tracing::warn!("Database integrity check returned: {}", status);
                self.set_task_result(
                    TASK_DB_VACUUM,
                    TaskResult {
                        status: TaskCompletionStatus::Failed,
                        error_message: Some(format!("Integrity check: {}", status)),
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    },
                )
                .await;
                self.end_run(claim).await;
                return true;
            }
            Err(e) => {
                let end = Utc::now();
                self.set_task_result(
                    TASK_DB_VACUUM,
                    TaskResult {
                        status: TaskCompletionStatus::Failed,
                        error_message: Some(e.to_string()),
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    },
                )
                .await;
                self.end_run(claim).await;
                return true;
            }
        }

        // VACUUM is one statement: a stop before it skips it, a stop during it lets it finish
        if claim.is_cancelled() {
            self.set_task_result(
                TASK_DB_VACUUM,
                TaskResult {
                    status: TaskCompletionStatus::Cancelled,
                    error_message: None,
                    start_time_utc: start.to_rfc3339(),
                    end_time_utc: Utc::now().to_rfc3339(),
                },
            )
            .await;
            self.end_run(claim).await;
            return true;
        }
        let result = match sqlx::query("VACUUM").execute(&self.db).await {
            Ok(_) => {
                let end = Utc::now();
                tracing::info!("Database VACUUM completed successfully");
                TaskResult {
                    status: TaskCompletionStatus::Completed,
                    error_message: None,
                    start_time_utc: start.to_rfc3339(),
                    end_time_utc: end.to_rfc3339(),
                }
            }
            Err(e) => {
                let end = Utc::now();
                tracing::error!("Database VACUUM failed: {}", e);
                TaskResult {
                    status: TaskCompletionStatus::Failed,
                    error_message: Some(e.to_string()),
                    start_time_utc: start.to_rfc3339(),
                    end_time_utc: end.to_rfc3339(),
                }
            }
        };

        self.set_task_result(TASK_DB_VACUUM, result).await;
        self.end_run(claim).await;
        true
    }
}

/// The result of the trickplay task when no library turns trickplay on.
pub const NO_TRICKPLAY_LIBRARY: &str = "No library has trickplay image extraction turned on";
/// The result of the chapter image task when no library turns chapter images on.
pub const NO_CHAPTER_LIBRARY: &str = "No library has chapter image extraction turned on";

/// Logs and returns `message`, the result of image task `id` when no library turns it on. Both
/// options are off by default, so a run that does nothing says why rather than "0 made".
fn no_library_on(id: &str, message: &str) -> String {
    tracing::info!(
        task = id,
        "{message}; turn it on in a library's options to make these images"
    );
    message.to_string()
}

/// The library scan task's state and percent from the scan registry: Idle with no percent when no
/// scan is listed, else Running (Cancelling while the scheduler's run is stopping) at the batch's
/// percent, the mean of each library's `RefreshProgress` percent, a finished one at 100 until the
/// batch ends, so the percent never drops as libraries finish.
fn library_scan_state(scans: &ScanRegistry, run: &TaskState) -> (TaskState, Option<f64>) {
    let Some(percent) = scans.batch_progress() else {
        return (TaskState::Idle, None);
    };
    // A stopped scan shows Cancelling until it has released its ticket
    let state = match run {
        TaskState::Cancelling => TaskState::Cancelling,
        _ if scans.any_cancelled() => TaskState::Cancelling,
        _ => TaskState::Running,
    };
    (state, Some(percent))
}

/// Remove transcode cache files older than 24 hours
async fn cleanup_transcode_cache(
    cache_dir: &std::path::Path,
    progress: &TaskProgress,
    cancel: &crate::trickplay::Cancel,
) -> Result<u32, String> {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let mut removed = 0u32;
    let mut rd = tokio::fs::read_dir(cache_dir)
        .await
        .map_err(|e| e.to_string())?;
    // Listed first, so the percent counts the entries looked at
    let mut entries = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        entries.push(entry);
    }

    for (i, entry) in entries.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }
        progress.report_count(i, entries.len());
        let path = entry.path();
        if let Ok(meta) = entry.metadata().await {
            let modified = meta.modified().unwrap_or(std::time::SystemTime::now());
            if modified < cutoff {
                if meta.is_dir() {
                    if tokio::fs::remove_dir_all(&path).await.is_ok() {
                        removed += 1;
                    }
                } else if tokio::fs::remove_file(&path).await.is_ok() {
                    removed += 1;
                }
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn r2chapters_stop_task_cancels_an_image_run() {
        let dir = std::env::temp_dir().join(format!("rustyfin_sched_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = rustyfin_db::init_db(dir.join("t.db").to_str().unwrap())
            .await
            .unwrap();
        let sched = TaskScheduler::new(db, Arc::new(dir.clone()));

        let run = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(
            sched.get_task(TASK_TRICKPLAY).await.unwrap().state,
            TaskState::Running
        );
        assert!(sched.stop_task(TASK_TRICKPLAY).await);
        assert!(run.is_cancelled(), "the run is told to stop");
        assert_eq!(
            sched.get_task(TASK_TRICKPLAY).await.unwrap().state,
            TaskState::Cancelling,
            "a stopped run shows Cancelling while it winds down, as in Jellyfin"
        );
        // No second run starts while the stopped one winds down.
        assert!(sched.begin_image_run(TASK_TRICKPLAY).await.is_none());
        sched.end_image_run(TASK_TRICKPLAY).await;
        assert_eq!(
            sched.get_task(TASK_TRICKPLAY).await.unwrap().state,
            TaskState::Idle
        );
        let next = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        assert!(!next.is_cancelled());
        // The other image task is separate.
        assert!(sched.begin_image_run(TASK_CHAPTER_IMAGES).await.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn r3tasks_scheduler() -> (TaskScheduler, PathBuf) {
        let dir = std::env::temp_dir().join(format!("rustyfin_sched_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = rustyfin_db::init_db(dir.join("t.db").to_str().unwrap())
            .await
            .unwrap();
        (TaskScheduler::new(db, Arc::new(dir.clone())), dir)
    }

    #[tokio::test]
    async fn r3admin_a_task_runs_once_at_a_time() {
        let (sched, dir) = r3tasks_scheduler().await;
        let claim = sched.claim(TASK_CLEAN_ACTIVITY_LOG).expect("free");
        assert!(sched.claim(TASK_CLEAN_ACTIVITY_LOG).is_none());
        // A start, by hand or by the triggers, while it runs does nothing
        sched.run_clean_activity_log().await;
        assert!(
            sched
                .get_task(TASK_CLEAN_ACTIVITY_LOG)
                .await
                .unwrap()
                .last_execution_result
                .is_none()
        );
        // A VACUUM that runs already: the second start is done, not put off, and records nothing
        let vacuum = sched.claim(TASK_DB_VACUUM).expect("free");
        assert!(sched.run_db_vacuum().await);
        assert!(
            sched
                .get_task(TASK_DB_VACUUM)
                .await
                .unwrap()
                .last_execution_result
                .is_none()
        );
        drop(vacuum);
        drop(claim);
        sched.run_clean_activity_log().await;
        assert!(
            sched
                .get_task(TASK_CLEAN_ACTIVITY_LOG)
                .await
                .unwrap()
                .last_execution_result
                .is_some()
        );
        assert!(
            sched.claim(TASK_CLEAN_ACTIVITY_LOG).is_some(),
            "free again after the run"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3admin_only_edited_triggers_are_saved() {
        let (sched, dir) = r3tasks_scheduler().await;
        let file = dir.join(TRIGGERS_FILE);
        let edited = vec![TaskTriggerInfo::interval(TICKS_12H)];
        assert!(
            sched
                .set_triggers(TASK_DB_VACUUM, edited.clone())
                .await
                .unwrap()
        );
        let saved = r3sched_saved_file(&file);
        assert_eq!(saved.keys().collect::<Vec<_>>(), [TASK_DB_VACUUM]);
        assert_eq!(saved[TASK_DB_VACUUM], edited);
        // Back to its defaults: nothing is saved for it any more
        let defaults = vec![TaskTriggerInfo::interval(TICKS_6H)];
        assert!(sched.set_triggers(TASK_DB_VACUUM, defaults).await.unwrap());
        let saved = r3sched_saved_file(&file);
        assert!(saved.is_empty(), "{saved:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    /// The triggers in a saved triggers file, which must carry this build's version.
    fn r3sched_saved_file(file: &std::path::Path) -> HashMap<String, Vec<TaskTriggerInfo>> {
        let saved: SavedTriggers<Vec<TaskTriggerInfo>> =
            serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(saved.version, TRIGGERS_VERSION);
        saved.triggers
    }

    #[tokio::test]
    async fn r3sched_a_saved_24_hour_interval_in_a_versioned_file_is_an_edit() {
        let (sched, dir) = r3tasks_scheduler().await;
        let daily_ish = vec![TaskTriggerInfo::interval(TICKS_24H)];
        assert!(
            sched
                .set_triggers(TASK_TRICKPLAY, daily_ish.clone())
                .await
                .unwrap()
        );
        for _ in 0..2 {
            let restarted = TaskScheduler::new(sched.db.clone(), Arc::new(dir.clone()));
            assert_eq!(
                restarted.get_task(TASK_TRICKPLAY).await.unwrap().triggers,
                daily_ish
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3sched_a_saved_end_in_the_future_counts_as_now() {
        let (sched, dir) = r3tasks_scheduler().await;
        let ahead = Utc::now() + chrono::Duration::days(3);
        sched
            .set_task_result(
                TASK_LIBRARY_SCAN,
                TaskResult {
                    status: TaskCompletionStatus::Completed,
                    error_message: None,
                    start_time_utc: ahead.to_rfc3339(),
                    end_time_utc: ahead.to_rfc3339(),
                },
            )
            .await;
        let started = Utc::now();
        let clock = crate::task_triggers::TriggerClock {
            last_end: None,
            loop_ran: false,
            first: started + chrono::Duration::seconds(60),
            interval_from: None,
            started,
        };
        let due = sched.next_due(TASK_LIBRARY_SCAN, clock).await.unwrap();
        let after = Utc::now() + chrono::Duration::hours(12);
        assert!(due <= after, "due {due}, not 12 hours from now");
        assert!(due >= started + chrono::Duration::hours(12), "due {due}");
        std::fs::remove_dir_all(dir).ok();
    }

    fn r3sched_triggers(sched_task: &ScheduledTaskInfo) -> serde_json::Value {
        serde_json::to_value(&sched_task.triggers).unwrap()
    }

    #[tokio::test]
    async fn r3sched_the_default_triggers_are_jellyfins() {
        let (sched, dir) = r3tasks_scheduler().await;
        let triggers = |id: &'static str| {
            let sched = sched.clone();
            async move { r3sched_triggers(&sched.get_task(id).await.unwrap()) }
        };
        let json = |t: Vec<TaskTriggerInfo>| serde_json::to_value(t).unwrap();
        let interval = |hours: u64| TaskTriggerInfo::interval(hours * TICKS_1H);
        let trigger = |ty: &str, of_day: Option<u64>, max: Option<u64>| TaskTriggerInfo {
            trigger_type: ty.into(),
            interval_ticks: None,
            time_of_day_ticks: of_day.map(|h| h * TICKS_1H),
            day_of_week: None,
            max_runtime_ticks: max.map(|h| h * TICKS_1H),
        };
        assert_eq!(triggers(TASK_LIBRARY_SCAN).await, json(vec![interval(12)]));
        assert_eq!(triggers(TASK_METADATA_REFRESH).await, json(vec![]));
        assert_eq!(
            triggers(TASK_TRANSCODE_CLEANUP).await,
            json(vec![trigger("StartupTrigger", None, None), interval(24)])
        );
        assert_eq!(
            triggers(TASK_INTRO_DETECTION).await,
            json(vec![interval(12)])
        );
        assert_eq!(triggers(TASK_DB_VACUUM).await, json(vec![interval(6)]));
        assert_eq!(
            triggers(TASK_CHAPTER_IMAGES).await,
            json(vec![trigger("DailyTrigger", Some(2), Some(4))])
        );
        assert_eq!(
            triggers(TASK_TRICKPLAY).await,
            json(vec![trigger("DailyTrigger", Some(3), None)])
        );
        assert_eq!(triggers(TASK_CLEAN_ACTIVITY_LOG).await, json(vec![]));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3sched_last_results_survive_a_restart() {
        let (sched, dir) = r3tasks_scheduler().await;
        sched.run_clean_activity_log().await;
        let ran = sched
            .get_task(TASK_CLEAN_ACTIVITY_LOG)
            .await
            .unwrap()
            .last_execution_result
            .unwrap();
        let end = Utc::now() - chrono::Duration::hours(2);
        sched
            .set_task_result(
                TASK_LIBRARY_SCAN,
                TaskResult {
                    status: TaskCompletionStatus::Failed,
                    error_message: Some("disk gone".into()),
                    start_time_utc: (end - chrono::Duration::minutes(5)).to_rfc3339(),
                    end_time_utc: end.to_rfc3339(),
                },
            )
            .await;

        let restarted = TaskScheduler::new(sched.db.clone(), Arc::new(dir.clone()));
        let last = |id: &'static str| {
            let restarted = restarted.clone();
            async move {
                serde_json::to_value(restarted.get_task(id).await.unwrap().last_execution_result)
                    .unwrap()
            }
        };
        assert_eq!(
            last(TASK_CLEAN_ACTIVITY_LOG).await,
            serde_json::to_value(&ran).unwrap()
        );
        let scan = last(TASK_LIBRARY_SCAN).await;
        assert_eq!(scan["Status"], "Failed");
        assert_eq!(scan["ErrorMessage"], "disk gone");
        assert!(last(TASK_TRICKPLAY).await.is_null(), "never ran");

        // The 12 hour interval counts from the saved end, not from the start
        let started = Utc::now();
        let clock = crate::task_triggers::TriggerClock {
            last_end: None,
            loop_ran: false,
            first: started + chrono::Duration::seconds(60),
            interval_from: None,
            started,
        };
        let due = restarted.next_due(TASK_LIBRARY_SCAN, clock).await.unwrap();
        let saved_end = chrono::DateTime::parse_from_rfc3339(&end.to_rfc3339()).unwrap();
        assert_eq!(due, saved_end + chrono::Duration::hours(12));
        // A task that never ran keeps its start-up delay
        assert_eq!(
            restarted.next_due(TASK_INTRO_DETECTION, clock).await,
            Some(clock.first)
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3sched_a_corrupt_saved_file_starts_empty() {
        let (sched, dir) = r3tasks_scheduler().await;
        std::fs::write(dir.join(RESULTS_FILE), "{ not json").unwrap();
        std::fs::write(dir.join(TRIGGERS_FILE), "[1, 2").unwrap();
        let restarted = TaskScheduler::new(sched.db.clone(), Arc::new(dir.clone()));
        for task in restarted.get_all_tasks().await {
            assert!(task.last_execution_result.is_none(), "{}", task.id);
        }
        let defaults = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        let trickplay = restarted.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(r3sched_triggers(&trickplay), r3sched_triggers(&defaults));
        // The next run's end writes a good file again
        restarted.run_clean_activity_log().await;
        let saved: HashMap<String, TaskResult> =
            serde_json::from_slice(&std::fs::read(dir.join(RESULTS_FILE)).unwrap()).unwrap();
        assert_eq!(saved.keys().collect::<Vec<_>>(), [TASK_CLEAN_ACTIVITY_LOG]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3sched_saved_triggers_win_but_old_defaults_give_way() {
        let (sched, dir) = r3tasks_scheduler().await;
        let old_default = vec![TaskTriggerInfo::interval(TICKS_24H)];
        let edited = vec![TaskTriggerInfo::interval(TICKS_6H)];
        let file: HashMap<&str, Vec<TaskTriggerInfo>> = HashMap::from([
            // As an early build saved them: every task, with the old 24 hour default
            (TASK_TRICKPLAY, old_default.clone()),
            (TASK_DB_VACUUM, old_default.clone()),
            (
                TASK_LIBRARY_SCAN,
                vec![TaskTriggerInfo::interval(TICKS_12H)],
            ),
            // Edited by an admin
            (TASK_CHAPTER_IMAGES, edited.clone()),
            (TASK_METADATA_REFRESH, old_default.clone()),
            (TASK_CLEAN_ACTIVITY_LOG, vec![]),
        ]);
        std::fs::write(dir.join(TRIGGERS_FILE), serde_json::to_vec(&file).unwrap()).unwrap();
        let restarted = TaskScheduler::new(sched.db.clone(), Arc::new(dir.clone()));
        let triggers = |id: &'static str| {
            let restarted = restarted.clone();
            async move { restarted.get_task(id).await.unwrap().triggers }
        };
        assert_eq!(triggers(TASK_TRICKPLAY).await, vec![daily_at(3)]);
        assert_eq!(
            triggers(TASK_DB_VACUUM).await,
            vec![TaskTriggerInfo::interval(TICKS_6H)]
        );
        assert_eq!(triggers(TASK_CHAPTER_IMAGES).await, edited);
        // A task whose old default was not 24 hours keeps a saved 24 hours: it was edited
        assert_eq!(triggers(TASK_METADATA_REFRESH).await, old_default);
        assert!(triggers(TASK_CLEAN_ACTIVITY_LOG).await.is_empty());
        // The file is migrated once: written again with a version, without the old defaults
        let saved = r3sched_saved_file(&dir.join(TRIGGERS_FILE));
        assert!(!saved.contains_key(TASK_TRICKPLAY), "{saved:?}");
        assert_eq!(saved[TASK_CHAPTER_IMAGES], edited);
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r3admin_max_runtime_stops_a_triggered_image_run() {
        let (sched, dir) = r3tasks_scheduler().await;
        let limited = TaskTriggerInfo {
            max_runtime_ticks: Some(1_000_000), // 100 ms
            ..TaskTriggerInfo::interval(TICKS_24H)
        };
        sched
            .set_triggers(TASK_TRICKPLAY, vec![limited.clone()])
            .await
            .unwrap();
        let cancel = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        let seen = cancel.clone();
        let started = std::time::Instant::now();
        let done = sched
            .run_limited(TASK_TRICKPLAY, async move {
                while !seen.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                true
            })
            .await;
        assert!(done && cancel.is_cancelled());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // A task that can't be stopped stores the limit only
        sched
            .set_triggers(TASK_LIBRARY_SCAN, vec![limited])
            .await
            .unwrap();
        assert!(sched.max_runtime(TASK_LIBRARY_SCAN).await.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    /// The status and message of task `id`'s last run.
    async fn r3perf_last(sched: &TaskScheduler, id: &str) -> (TaskCompletionStatus, String) {
        let result = sched
            .get_task(id)
            .await
            .unwrap()
            .last_execution_result
            .unwrap();
        (result.status, result.error_message.unwrap_or_default())
    }

    // VACUUM holds the write lock throughout, so it waits for scans and image runs to end.
    #[tokio::test]
    async fn r3dbpool_vacuum_waits_for_scans_and_image_runs() {
        let (sched, dir) = r3tasks_scheduler().await;
        let name = |id| {
            let sched = sched.clone();
            async move { sched.get_task(id).await.unwrap().name }
        };
        assert!(sched.vacuum_blockers(false).await.is_empty());
        assert_eq!(
            sched.vacuum_blockers(true).await,
            ["a library scan".to_string()]
        );

        sched
            .set_task_state(TASK_TRICKPLAY, TaskState::Running)
            .await;
        assert_eq!(
            sched.vacuum_blockers(true).await,
            [name(TASK_TRICKPLAY).await, "a library scan".to_string()]
        );
        sched
            .set_task_state(TASK_LIBRARY_SCAN, TaskState::Running)
            .await;
        assert_eq!(
            sched.vacuum_blockers(true).await,
            [name(TASK_LIBRARY_SCAN).await, name(TASK_TRICKPLAY).await],
            "the scan is named once"
        );

        assert!(!sched.run_db_vacuum().await, "put off");
        let (status, why) = r3perf_last(&sched, TASK_DB_VACUUM).await;
        assert_eq!(status, TaskCompletionStatus::Aborted);
        assert!(why.starts_with("Put off while this runs: "), "{why}");
        assert_eq!(
            sched.get_task(TASK_DB_VACUUM).await.unwrap().state,
            TaskState::Idle
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3perf_stop_then_start_does_not_overlap_a_winding_down_run() {
        let (sched, dir) = r3tasks_scheduler().await;
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let first = {
            let sched = sched.clone();
            tokio::spawn(async move {
                sched
                    .run_stoppable(TASK_INTRO_DETECTION, |cancel| async move {
                        // Stopped, it still takes a while to wind down.
                        cancel.cancelled().await;
                        let _ = released.await;
                        Ok("first".to_string())
                    })
                    .await
            })
        };
        while sched.get_task(TASK_INTRO_DETECTION).await.unwrap().state != TaskState::Running {
            tokio::task::yield_now().await;
        }

        assert!(sched.stop_task(TASK_INTRO_DETECTION).await);
        // Restart marks every task Idle, but the run is still listed.
        sched.restart().await;
        let second = sched
            .run_stoppable(TASK_INTRO_DETECTION, |_| async { Ok("second".to_string()) })
            .await;
        assert!(!second, "a second run started while the first wound down");
        sched.run_intro_detection().await;
        assert!(
            sched
                .get_task(TASK_INTRO_DETECTION)
                .await
                .unwrap()
                .last_execution_result
                .is_none(),
            "no run ended yet"
        );

        release.send(()).unwrap();
        assert!(first.await.unwrap());
        assert_eq!(
            r3perf_last(&sched, TASK_INTRO_DETECTION).await,
            (TaskCompletionStatus::Cancelled, "first".to_string())
        );
        let task = sched.get_task(TASK_INTRO_DETECTION).await.unwrap();
        assert_eq!(task.state, TaskState::Idle);
        // Once it has ended, the next run starts.
        assert!(
            sched
                .run_stoppable(TASK_INTRO_DETECTION, |_| async { Ok("next".to_string()) })
                .await
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3perf_a_panic_or_an_error_ends_the_run() {
        let (sched, dir) = r3tasks_scheduler().await;
        assert!(
            sched
                .run_stoppable(TASK_INTRO_DETECTION, |_| async {
                    if std::hint::black_box(true) {
                        panic!("detection panicked");
                    }
                    Ok(String::new())
                })
                .await
        );
        let (status, message) = r3perf_last(&sched, TASK_INTRO_DETECTION).await;
        assert_eq!(status, TaskCompletionStatus::Failed);
        assert!(message.contains("unexpectedly"), "{message}");
        assert_eq!(
            sched.get_task(TASK_INTRO_DETECTION).await.unwrap().state,
            TaskState::Idle
        );

        assert!(
            sched
                .run_stoppable(TASK_INTRO_DETECTION, |_| async {
                    Err("no episodes".to_string())
                })
                .await,
            "a run after a panic starts"
        );
        assert_eq!(
            r3perf_last(&sched, TASK_INTRO_DETECTION).await,
            (TaskCompletionStatus::Failed, "no episodes".to_string())
        );
        assert!(
            sched
                .run_stoppable(TASK_INTRO_DETECTION, |_| async { Ok("done".to_string()) })
                .await,
            "a run after an error starts"
        );
        assert_eq!(
            r3perf_last(&sched, TASK_INTRO_DETECTION).await,
            (TaskCompletionStatus::Completed, "done".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    use uuid::Uuid;

    async fn r3dash3_scan_task(sched: &TaskScheduler) -> (TaskState, Option<f64>) {
        let info = sched.get_task(TASK_LIBRARY_SCAN).await.unwrap();
        (info.state, info.current_progress_percentage)
    }

    /// Any listed library scan shows as the library scan task, at its percent, and the task reads
    /// Idle again when the scan ends.
    #[tokio::test]
    async fn r3dash3_a_listed_scan_shows_as_the_library_scan_task() {
        let (sched, dir) = r3tasks_scheduler().await;
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        // A scan started outside the scheduler (a library's Scan, Scan all, the wizard)
        let ticket = sched.scans().queue(Uuid::new_v4());
        assert_eq!(
            r3dash3_scan_task(&sched).await,
            (TaskState::Running, Some(0.0))
        );
        assert!(ticket.claim());
        ticket.start();
        ticket.add_total(4);
        ticket.advance(1);
        let other = sched.scans().queue(Uuid::new_v4());
        // The average of the listed scans: 25 and 0
        assert_eq!(
            r3dash3_scan_task(&sched).await,
            (TaskState::Running, Some(12.5))
        );
        let json = serde_json::to_value(sched.get_task(TASK_LIBRARY_SCAN).await.unwrap()).unwrap();
        assert_eq!(json["State"], "Running", "{json}");
        drop(other);
        // The probes of a scan that has returned keep it listed, and the task Running
        let probes = ticket.clone();
        drop(ticket);
        assert_eq!(r3dash3_scan_task(&sched).await.0, TaskState::Running);
        drop(probes);
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        assert!(sched.scans().snapshot().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scheduler's own run of the library scan reads only what the registry lists: not its own
    /// reported percent, Idle while nothing is listed, Running while scans are, and Cancelling
    /// once Stop has asked them to end, until they have.
    #[tokio::test]
    async fn r3dash3_the_scheduler_run_reads_the_registry_only() {
        let (sched, dir) = r3tasks_scheduler().await;
        sched
            .set_task_state(TASK_LIBRARY_SCAN, TaskState::Running)
            .await;
        sched.progress_of(TASK_LIBRARY_SCAN).report(80.0);
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        let ticket = sched.scans().queue(Uuid::new_v4());
        ticket.add_total(10);
        ticket.advance(3);
        assert_eq!(
            r3dash3_scan_task(&sched).await,
            (TaskState::Running, Some(30.0))
        );
        assert!(sched.stop_task(TASK_LIBRARY_SCAN).await);
        assert!(ticket.is_cancelled());
        assert_eq!(
            r3dash3_scan_task(&sched).await,
            (TaskState::Cancelling, Some(30.0))
        );
        drop(ticket);
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scan that is cancelled (its task aborted) or fails (panics) clears the library's listing,
    /// so neither the scan ring nor the task stays on.
    #[tokio::test]
    async fn r3dash3_a_cancelled_or_failed_scan_clears() {
        let (sched, dir) = r3tasks_scheduler().await;
        let library = Uuid::new_v4();
        let mut frames = sched.scans().subscribe();
        let scans = sched.scans().clone();
        let cancelled = tokio::spawn(async move {
            let ticket = scans.queue(library);
            ticket.claim();
            ticket.start();
            std::future::pending::<()>().await;
            drop(ticket);
        });
        while sched.scans().state(library).is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(r3dash3_scan_task(&sched).await.0, TaskState::Running);
        cancelled.abort();
        let _ = cancelled.await;
        assert!(sched.scans().state(library).is_none());
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));

        let scans = sched.scans().clone();
        let failed = tokio::spawn(async move {
            let ticket = scans.queue(library);
            ticket.claim();
            ticket.start();
            panic!("the scan failed");
        });
        assert!(failed.await.is_err());
        assert!(sched.scans().state(library).is_none());
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        // Each run ended with the Idle frame at 100, which clears the web client's ring
        let mut last = None;
        while let Ok(update) = frames.try_recv() {
            last = Some((update.status, update.progress));
        }
        assert_eq!(last, Some((crate::scan_progress::ScanStatus::Idle, 100)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scheduled library scan leaves a library another caller claimed to it: it neither scans
    /// nor waits for it, and records its result. The other scan, still listed, keeps the task
    /// reading Running until it ends.
    #[tokio::test]
    async fn r3dash3_the_scheduled_scan_skips_a_claimed_library() {
        let (sched, dir) = r3tasks_scheduler().await;
        let media = dir.join("media");
        std::fs::create_dir_all(&media).unwrap();
        let lib = rustyfin_model::library::Library {
            id: Uuid::new_v4(),
            name: "Shows A".into(),
            content_type: rustyfin_model::library::LibraryContentType::TvShows,
            paths: vec![media.to_string_lossy().to_string()],
        };
        rustyfin_db::repo_library::create_library(&sched.db, &lib)
            .await
            .unwrap();
        // Another caller (a library's Scan) has claimed the library's run, which still waits for
        // the scan lock, so the task joins that run and leaves it to its claimer
        let api_scan = sched.scans().queue(lib.id);
        assert!(api_scan.claim());
        tokio::time::timeout(std::time::Duration::from_secs(10), sched.run_library_scan())
            .await
            .expect("the task doesn't wait for the claimed library");
        let info = sched.get_task(TASK_LIBRARY_SCAN).await.unwrap();
        assert!(info.last_execution_result.is_some());
        assert_eq!(
            info.state,
            TaskState::Running,
            "the other scan is still listed"
        );
        drop(api_scan);
        assert_eq!(r3dash3_scan_task(&sched).await, (TaskState::Idle, None));
        assert!(sched.scans().snapshot().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3tasks_progress_is_shown_while_running_and_cleared_after() {
        let (sched, dir) = r3tasks_scheduler().await;
        let percent = |info: ScheduledTaskInfo| info.current_progress_percentage;

        // A report from an idle task is not shown.
        sched.progress_of(TASK_TRICKPLAY).report(10.0);
        assert_eq!(percent(sched.get_task(TASK_TRICKPLAY).await.unwrap()), None);

        // A run starts with no percent (an indeterminate Running), then shows what it reports.
        let run = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(percent(sched.get_task(TASK_TRICKPLAY).await.unwrap()), None);
        sched.progress_of(TASK_TRICKPLAY).report_count(1, 4);
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(info.state, TaskState::Running);
        assert_eq!(info.current_progress_percentage, Some(25.0));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["State"], "Running", "{json}");
        assert_eq!(json["CurrentProgressPercentage"], 25.0, "{json}");
        // Every task in the list carries the field, null while idle.
        for task in sched.get_all_tasks().await {
            let json = serde_json::to_value(&task).unwrap();
            let want = if task.id == TASK_TRICKPLAY {
                serde_json::json!(25.0)
            } else {
                serde_json::Value::Null
            };
            assert_eq!(json["CurrentProgressPercentage"], want, "{json}");
        }

        // A stopped run keeps its percent while it winds down.
        assert!(sched.stop_task(TASK_TRICKPLAY).await);
        assert!(run.is_cancelled());
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(info.state, TaskState::Cancelling);
        assert_eq!(info.current_progress_percentage, Some(25.0));

        // An ended run shows no percent, and the next run starts without the old one.
        sched.end_image_run(TASK_TRICKPLAY).await;
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(info.state, TaskState::Idle);
        assert_eq!(info.current_progress_percentage, None);
        let json = serde_json::to_value(&info).unwrap();
        assert!(json["CurrentProgressPercentage"].is_null(), "{json}");
        let _next = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(percent(sched.get_task(TASK_TRICKPLAY).await.unwrap()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Lets the spawned tasks run until each waits (the test runtime has one thread).
    async fn r3tasks_settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn r3tasks_stop_racing_the_end_of_an_image_run_leaves_it_idle() {
        let (sched, dir) = r3tasks_scheduler().await;
        let run = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();

        // A stop arrives while the tasks lock is taken: it must not look at the run yet.
        let guard = sched.tasks.write().await;
        let stopper = tokio::spawn({
            let sched = sched.clone();
            async move { sched.stop_task(TASK_TRICKPLAY).await }
        });
        r3tasks_settle().await;
        assert!(!run.is_cancelled(), "the stop waits for the tasks lock");

        // The run ends first (end_image_run's step, under the same lock), then the stop goes on.
        let mut guard = guard;
        sched.image_runs.lock().unwrap().remove(TASK_TRICKPLAY);
        let entry = guard.get_mut(TASK_TRICKPLAY).unwrap();
        sched.apply_state(entry, TaskState::Idle);
        drop(guard);
        assert!(stopper.await.unwrap(), "a known task");
        assert!(!sched.is_held(TASK_TRICKPLAY), "nothing was left to stop");
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(info.state, TaskState::Idle, "never left Cancelling");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3tasks_stop_racing_the_start_of_an_image_run_cancels_it() {
        let (sched, dir) = r3tasks_scheduler().await;

        // A start waits for the tasks lock without listing its run, so no stop can see the run
        // before it is marked Running.
        let guard = sched.tasks.write().await;
        let starter = tokio::spawn({
            let sched = sched.clone();
            async move { sched.begin_image_run(TASK_TRICKPLAY).await }
        });
        r3tasks_settle().await;
        assert!(
            !sched
                .image_runs
                .lock()
                .unwrap()
                .contains_key(TASK_TRICKPLAY),
            "the run is listed only under the tasks lock"
        );
        let stopper = tokio::spawn({
            let sched = sched.clone();
            async move { sched.stop_task(TASK_TRICKPLAY).await }
        });
        r3tasks_settle().await;
        drop(guard);

        // The start goes first (it waited first), so the stop finds it Running and cancels it.
        let run = starter.await.unwrap().expect("the run started");
        assert!(stopper.await.unwrap());
        assert!(run.is_cancelled());
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(
            info.state,
            TaskState::Cancelling,
            "Running never overwrote the stop"
        );
        sched.end_image_run(TASK_TRICKPLAY).await;
        let info = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        assert_eq!(info.state, TaskState::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3tasks_state_changes_keep_cancelling_until_the_end() {
        let (sched, dir) = r3tasks_scheduler().await;
        let state = || async { sched.get_task(TASK_TRICKPLAY).await.unwrap().state };
        // Cancelling only from Running
        sched
            .set_task_state(TASK_TRICKPLAY, TaskState::Cancelling)
            .await;
        assert_eq!(state().await, TaskState::Idle);
        sched
            .set_task_state(TASK_TRICKPLAY, TaskState::Running)
            .await;
        sched
            .set_task_state(TASK_TRICKPLAY, TaskState::Cancelling)
            .await;
        // Running doesn't overwrite Cancelling; Idle ends it
        sched
            .set_task_state(TASK_TRICKPLAY, TaskState::Running)
            .await;
        assert_eq!(state().await, TaskState::Cancelling);
        sched.set_task_state(TASK_TRICKPLAY, TaskState::Idle).await;
        assert_eq!(state().await, TaskState::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3tasks_libraries_where_skips_unreadable_options() {
        let (sched, dir) = r3tasks_scheduler().await;
        let mut ids = Vec::new();
        for name in ["Readable", "Unreadable"] {
            let library = rustyfin_model::library::Library {
                id: uuid::Uuid::new_v4(),
                name: name.to_string(),
                content_type: rustyfin_model::library::LibraryContentType::Movies,
                paths: vec![dir.join(name).to_string_lossy().to_string()],
            };
            rustyfin_db::repo_library::create_library(&sched.db, &library)
                .await
                .unwrap();
            ids.push(library.id);
        }
        let on = rustyfin_model::library::LibraryOptions {
            enable_trickplay: true,
            ..Default::default()
        };
        rustyfin_db::repo_library::set_library_options(&sched.db, ids[0], &on)
            .await
            .unwrap();
        sqlx::query("INSERT INTO configuration (key, value) VALUES (?1, 'not json')")
            .bind(format!("library_options_{}", ids[1]))
            .execute(&sched.db)
            .await
            .unwrap();
        let picked = sched.libraries_where(|o| o.enable_trickplay).await.unwrap();
        assert_eq!(picked, vec![ids[0].to_string()]);
        // Nor is it read as "everything off" for a task that wants options off
        let off = sched
            .libraries_where(|o| !o.enable_trickplay)
            .await
            .unwrap();
        assert!(off.is_empty(), "{off:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3tasks_image_tasks_say_when_no_library_turns_them_on() {
        let (sched, dir) = r3tasks_scheduler().await;
        let library = rustyfin_model::library::Library {
            id: uuid::Uuid::new_v4(),
            name: "Films".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Movies,
            paths: vec![dir.join("films").to_string_lossy().to_string()],
        };
        rustyfin_db::repo_library::create_library(&sched.db, &library)
            .await
            .unwrap();
        for (id, message) in [
            (TASK_TRICKPLAY, NO_TRICKPLAY_LIBRARY),
            (TASK_CHAPTER_IMAGES, NO_CHAPTER_LIBRARY),
        ] {
            sched.run_video_images(id).await;
            let result = sched.get_task(id).await.unwrap().last_execution_result;
            let result = result.expect("the run has a result");
            assert_eq!(result.status, TaskCompletionStatus::Completed, "{id}");
            assert_eq!(result.error_message.as_deref(), Some(message), "{id}");
        }

        // With the option on, the run reports what it made instead.
        let options = rustyfin_model::library::LibraryOptions {
            enable_trickplay: true,
            ..Default::default()
        };
        rustyfin_db::repo_library::set_library_options(&sched.db, library.id, &options)
            .await
            .unwrap();
        sched.run_video_images(TASK_TRICKPLAY).await;
        let result = sched.get_task(TASK_TRICKPLAY).await.unwrap();
        let message = result.last_execution_result.unwrap().error_message;
        assert_eq!(
            message.as_deref(),
            Some("Made the trickplay images of 0 videos")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3sched_a_stopped_task_waits_for_its_trigger_not_the_next_scan() {
        let (sched, dir) = r3tasks_scheduler().await;
        let library = rustyfin_model::library::Library {
            id: uuid::Uuid::new_v4(),
            name: "Library A".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Movies,
            paths: vec![dir.join("a").to_string_lossy().to_string()],
        };
        rustyfin_db::repo_library::create_library(&sched.db, &library)
            .await
            .unwrap();
        let options = rustyfin_model::library::LibraryOptions {
            enable_trickplay: true,
            extract_trickplay_images_during_library_scan: true,
            ..Default::default()
        };
        rustyfin_db::repo_library::set_library_options(&sched.db, library.id, &options)
            .await
            .unwrap();
        let ran = |sched: TaskScheduler| async move {
            sched
                .get_task(TASK_TRICKPLAY)
                .await
                .unwrap()
                .last_execution_result
                .is_some()
        };
        let wait_for_run = |sched: TaskScheduler| async move {
            for _ in 0..500 {
                if ran(sched.clone()).await {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            false
        };

        // An admin stops a running trickplay run
        let _run = sched.begin_image_run(TASK_TRICKPLAY).await.unwrap();
        assert!(sched.stop_task(TASK_TRICKPLAY).await);
        sched.end_image_run(TASK_TRICKPLAY).await;
        // The next library's scan ends: the task stays idle
        sched.after_scan(library.id).await;
        r3tasks_settle().await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !ran(sched.clone()).await,
            "a scan started the stopped task again"
        );
        assert_eq!(
            sched.get_task(TASK_TRICKPLAY).await.unwrap().state,
            TaskState::Idle
        );

        // Once its trigger or a start by hand runs it, a scan's end starts it again
        sched.release(TASK_TRICKPLAY);
        sched.after_scan(library.id).await;
        assert!(wait_for_run(sched.clone()).await, "a scan's end starts it");
        // A stop by MaxRuntimeTicks doesn't hold it
        let _run = sched.begin_image_run(TASK_CHAPTER_IMAGES).await.unwrap();
        assert!(sched.stop_run(TASK_CHAPTER_IMAGES).await);
        assert!(!sched.is_held(TASK_CHAPTER_IMAGES));
        // A start by hand releases a stopped task
        assert!(sched.stop_task(TASK_CHAPTER_IMAGES).await);
        assert!(sched.is_held(TASK_CHAPTER_IMAGES));
        sched.end_image_run(TASK_CHAPTER_IMAGES).await;
        assert!(sched.start_task(TASK_CHAPTER_IMAGES).await);
        assert!(!sched.is_held(TASK_CHAPTER_IMAGES));
        r3tasks_settle().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Polls task `id` until `done` holds for it, for up to 10 seconds.
    async fn r3sched_until(
        sched: &TaskScheduler,
        id: &str,
        done: impl Fn(&ScheduledTaskInfo) -> bool,
    ) -> ScheduledTaskInfo {
        for _ in 0..1000 {
            let task = sched.get_task(id).await.unwrap();
            if done(&task) {
                return task;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{id} never got there: {:?}", sched.get_task(id).await);
    }

    #[tokio::test]
    async fn r3sched_stop_lets_the_run_end_and_a_start_meanwhile_waits_for_it() {
        let (sched, dir) = r3tasks_scheduler().await;
        let id = TASK_METADATA_REFRESH;
        // A run going on
        let run = sched.claim(id).unwrap();
        sched.set_task_state(id, TaskState::Running).await;
        assert!(sched.stop_task(id).await);
        assert!(run.is_cancelled(), "the run is told to stop");
        let task = sched.get_task(id).await.unwrap();
        assert_eq!(task.state, TaskState::Cancelling, "not Idle while it runs");
        // A restart of the tasks leaves it as it is
        sched.restart().await;
        assert_eq!(
            sched.get_task(id).await.unwrap().state,
            TaskState::Cancelling
        );
        // A start meanwhile waits; nothing runs yet
        assert!(sched.start_task(id).await);
        assert!(sched.start_task(id).await, "a second start joins the first");
        r3tasks_settle().await;
        assert!(
            sched
                .get_task(id)
                .await
                .unwrap()
                .last_execution_result
                .is_none()
        );
        // The run ends: Idle only now, then the waiting start runs once
        sched.end_run(run).await;
        let ran = r3sched_until(&sched, id, |t| {
            t.state == TaskState::Idle && t.last_execution_result.is_some()
        })
        .await;
        assert_eq!(
            ran.last_execution_result.unwrap().status,
            TaskCompletionStatus::Completed,
            "the new run isn't the stopped one"
        );

        // A stop that comes before the run shows Running: it shows Cancelling instead
        let run = sched.claim(TASK_CLEAN_ACTIVITY_LOG).unwrap();
        assert!(sched.stop_task(TASK_CLEAN_ACTIVITY_LOG).await);
        sched
            .set_task_state(TASK_CLEAN_ACTIVITY_LOG, TaskState::Running)
            .await;
        assert_eq!(
            sched.get_task(TASK_CLEAN_ACTIVITY_LOG).await.unwrap().state,
            TaskState::Cancelling
        );
        sched.end_run(run).await;
        // A stop with nothing running changes nothing; an unknown task is refused
        assert!(sched.stop_task(TASK_CLEAN_ACTIVITY_LOG).await);
        assert_eq!(
            sched.get_task(TASK_CLEAN_ACTIVITY_LOG).await.unwrap().state,
            TaskState::Idle
        );
        assert!(!sched.stop_task("NoSuchTask").await);
        assert!(!sched.start_task("NoSuchTask").await);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3sched_a_stopped_library_scan_ends_as_cancelled() {
        let (sched, dir) = r3tasks_scheduler().await;
        let mut ids = Vec::new();
        for name in ["Library A", "Library B"] {
            let path = dir.join(name);
            std::fs::create_dir_all(&path).unwrap();
            let library = rustyfin_model::library::Library {
                id: uuid::Uuid::new_v4(),
                name: name.to_string(),
                content_type: rustyfin_model::library::LibraryContentType::Movies,
                paths: vec![path.to_string_lossy().to_string()],
            };
            rustyfin_db::repo_library::create_library(&sched.db, &library)
                .await
                .unwrap();
            ids.push(library.id);
        }
        // The scheduler's own run, stopped: it ends Cancelled
        let tickets_before = sched.scans().snapshot().len();
        assert_eq!(tickets_before, 0);
        let run = sched.claim(TASK_LIBRARY_SCAN).unwrap();
        sched
            .set_task_state(TASK_LIBRARY_SCAN, TaskState::Running)
            .await;
        assert!(sched.stop_task(TASK_LIBRARY_SCAN).await);
        assert!(run.is_cancelled());
        drop(run);
        sched.release(TASK_LIBRARY_SCAN);
        sched
            .set_task_state(TASK_LIBRARY_SCAN, TaskState::Idle)
            .await;

        // A start by hand runs it again to the end
        assert!(sched.start_task(TASK_LIBRARY_SCAN).await);
        let again = r3sched_until(&sched, TASK_LIBRARY_SCAN, |t| {
            t.last_execution_result
                .as_ref()
                .is_some_and(|r| r.status == TaskCompletionStatus::Completed)
        })
        .await;
        assert!(again.last_execution_result.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn r3sched_stop_ends_a_scan_started_from_a_library() {
        let (sched, dir) = r3tasks_scheduler().await;
        let path = dir.join("Library A");
        std::fs::create_dir_all(path.join("Made Up Film (2001)")).unwrap();
        std::fs::write(
            path.join("Made Up Film (2001)/Made Up Film (2001).mkv"),
            b"x",
        )
        .unwrap();
        let library = rustyfin_model::library::Library {
            id: uuid::Uuid::new_v4(),
            name: "Library A".to_string(),
            content_type: rustyfin_model::library::LibraryContentType::Movies,
            paths: vec![path.to_string_lossy().to_string()],
        };
        rustyfin_db::repo_library::create_library(&sched.db, &library)
            .await
            .unwrap();
        let items = || async {
            rustyfin_db::repo_media::get_items_by_library(&sched.db, library.id)
                .await
                .unwrap()
                .len()
        };
        let scan = |ticket: crate::scan_progress::ScanTicket| {
            let sched = sched.clone();
            let library = library.clone();
            async move {
                crate::scanner::scan_library_with_metadata(
                    &sched.db,
                    &library,
                    &sched.http_client,
                    &sched.plugin_manager,
                    &ticket,
                )
                .await
                .unwrap();
                ticket.wait_end().await;
            }
        };

        // A library's Scan, as the library route queues it: the ring and the card show it
        let ticket = sched.scans().queue(library.id);
        assert!(ticket.claim());
        let card = sched.get_task(TASK_LIBRARY_SCAN).await.unwrap();
        assert_eq!(card.state, TaskState::Running);
        // Stop on the scan task reaches it; the card shows Cancelling until it has ended
        assert!(sched.stop_task(TASK_LIBRARY_SCAN).await);
        assert!(ticket.is_cancelled());
        let card = sched.get_task(TASK_LIBRARY_SCAN).await.unwrap();
        assert_eq!(card.state, TaskState::Cancelling);
        scan(ticket).await;
        assert_eq!(items().await, 0, "the stopped scan read nothing");
        assert!(sched.scans().snapshot().is_empty(), "the ring is idle");
        let ended = r3sched_until(&sched, TASK_LIBRARY_SCAN, |t| {
            t.last_execution_result.is_some()
        })
        .await;
        assert_eq!(ended.state, TaskState::Idle);
        assert_eq!(
            ended.last_execution_result.unwrap().status,
            TaskCompletionStatus::Cancelled
        );

        // A scan queued after the stop isn't part of it, and reads the library
        let ticket = sched.scans().queue(library.id);
        assert!(ticket.claim());
        assert!(!ticket.is_cancelled());
        scan(ticket).await;
        assert_eq!(items().await, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
