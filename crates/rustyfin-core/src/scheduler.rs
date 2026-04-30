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

/// Known task IDs
pub const TASK_LIBRARY_SCAN: &str = "LibraryScan";
pub const TASK_METADATA_REFRESH: &str = "MetadataRefresh";
pub const TASK_TRANSCODE_CLEANUP: &str = "TranscodeCacheCleanup";

/// Default interval: 12 hours in .NET ticks (1 tick = 100ns)
const TICKS_12H: u64 = 12 * 60 * 60 * 10_000_000;
/// Default interval: 24 hours in .NET ticks
const TICKS_24H: u64 = 24 * 60 * 60 * 10_000_000;

fn ticks_to_duration(ticks: u64) -> std::time::Duration {
    std::time::Duration::from_nanos(ticks * 100)
}

#[derive(Clone)]
struct TaskEntry {
    info: ScheduledTaskInfo,
}

#[derive(Clone)]
pub struct TaskScheduler {
    tasks: Arc<RwLock<HashMap<String, TaskEntry>>>,
    db: SqlitePool,
    data_dir: Arc<PathBuf>,
}

impl TaskScheduler {
    pub fn new(db: SqlitePool, data_dir: Arc<PathBuf>) -> Self {
        let mut tasks = HashMap::new();

        // Library Scan
        tasks.insert(
            TASK_LIBRARY_SCAN.to_string(),
            TaskEntry {
                info: ScheduledTaskInfo {
                    id: TASK_LIBRARY_SCAN.to_string(),
                    name: "Scan Media Library".to_string(),
                    description: "Scans all libraries for new and updated media files.".to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo {
                        trigger_type: "IntervalTrigger".to_string(),
                        interval_ticks: Some(TICKS_12H),
                        time_of_day_ticks: None,
                        max_runtime_ticks: None,
                    }],
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
                    name: "Refresh Metadata".to_string(),
                    description: "Updates metadata for all media items from local and remote sources.".to_string(),
                    category: "Library".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo {
                        trigger_type: "IntervalTrigger".to_string(),
                        interval_ticks: Some(TICKS_24H),
                        time_of_day_ticks: None,
                        max_runtime_ticks: None,
                    }],
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
                    name: "Transcode Cache Cleanup".to_string(),
                    description: "Removes old transcoded files from the cache directory.".to_string(),
                    category: "Maintenance".to_string(),
                    state: TaskState::Idle,
                    current_progress_percentage: None,
                    triggers: vec![TaskTriggerInfo {
                        trigger_type: "IntervalTrigger".to_string(),
                        interval_ticks: Some(TICKS_24H),
                        time_of_day_ticks: None,
                        max_runtime_ticks: None,
                    }],
                    last_execution_result: None,
                },
            },
        );

        Self {
            tasks: Arc::new(RwLock::new(tasks)),
            db,
            data_dir,
        }
    }

    pub async fn get_all_tasks(&self) -> Vec<ScheduledTaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.values().map(|e| e.info.clone()).collect()
    }

    pub async fn get_task(&self, id: &str) -> Option<ScheduledTaskInfo> {
        let tasks = self.tasks.read().await;
        tasks.get(id).map(|e| e.info.clone())
    }

    pub async fn set_triggers(&self, id: &str, triggers: Vec<TaskTriggerInfo>) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(entry) = tasks.get_mut(id) {
            entry.info.triggers = triggers;
            true
        } else {
            false
        }
    }

    async fn set_task_state(&self, id: &str, state: TaskState) {
        let mut tasks = self.tasks.write().await;
        if let Some(entry) = tasks.get_mut(id) {
            entry.info.state = state;
            if entry.info.state == TaskState::Idle {
                entry.info.current_progress_percentage = None;
            }
        }
    }

    async fn set_task_result(&self, id: &str, result: TaskResult) {
        let mut tasks = self.tasks.write().await;
        if let Some(entry) = tasks.get_mut(id) {
            entry.info.last_execution_result = Some(result);
        }
    }

    async fn get_interval_for_task(&self, id: &str) -> Option<std::time::Duration> {
        let tasks = self.tasks.read().await;
        let entry = tasks.get(id)?;
        for trigger in &entry.info.triggers {
            if trigger.trigger_type == "IntervalTrigger" {
                if let Some(ticks) = trigger.interval_ticks {
                    return Some(ticks_to_duration(ticks));
                }
            }
        }
        None
    }

    /// Run the library scan task once
    async fn run_library_scan(&self) {
        self.set_task_state(TASK_LIBRARY_SCAN, TaskState::Running).await;
        let start = Utc::now();

        let result = match rustyfin_db::repo_library::get_all_libraries(&self.db).await {
            Ok(libraries) => {
                let mut errors = Vec::new();
                for lib in &libraries {
                    if let Err(e) = crate::scanner::scan_library(&self.db, lib).await {
                        errors.push(format!("{}: {}", lib.name, e));
                    }
                }
                let end = Utc::now();
                if errors.is_empty() {
                    TaskResult {
                        status: TaskCompletionStatus::Completed,
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

        self.set_task_result(TASK_LIBRARY_SCAN, result).await;
        self.set_task_state(TASK_LIBRARY_SCAN, TaskState::Idle).await;
    }

    /// Run metadata refresh for all items across all libraries
    async fn run_metadata_refresh(&self) {
        self.set_task_state(TASK_METADATA_REFRESH, TaskState::Running).await;
        let start = Utc::now();

        let result = match rustyfin_db::repo_library::get_all_libraries(&self.db).await {
            Ok(libraries) => {
                let mut errors = Vec::new();
                for lib in &libraries {
                    match rustyfin_db::repo_media::get_items_by_library(&self.db, lib.id).await {
                        Ok(items) => {
                            for item in &items {
                                if let Err(e) = crate::metadata::refresh_item_metadata(
                                    &self.db, item, None, None,
                                ).await {
                                    errors.push(format!("{}: {}", item.name, e));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!("library {}: {}", lib.name, e));
                        }
                    }
                }
                let end = Utc::now();
                if errors.is_empty() {
                    TaskResult {
                        status: TaskCompletionStatus::Completed,
                        error_message: None,
                        start_time_utc: start.to_rfc3339(),
                        end_time_utc: end.to_rfc3339(),
                    }
                } else {
                    tracing::warn!("metadata refresh had {} errors", errors.len());
                    TaskResult {
                        status: TaskCompletionStatus::Completed,
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

        self.set_task_result(TASK_METADATA_REFRESH, result).await;
        self.set_task_state(TASK_METADATA_REFRESH, TaskState::Idle).await;
    }

    /// Clean up old transcode cache files
    async fn run_transcode_cleanup(&self) {
        self.set_task_state(TASK_TRANSCODE_CLEANUP, TaskState::Running).await;
        let start = Utc::now();

        let cache_dir = self.data_dir.join("cache").join("transcode");
        let result = if cache_dir.exists() {
            match cleanup_transcode_cache(&cache_dir).await {
                Ok(removed) => {
                    if removed > 0 {
                        tracing::info!("transcode cache cleanup: removed {} files", removed);
                    }
                    let end = Utc::now();
                    TaskResult {
                        status: TaskCompletionStatus::Completed,
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
        self.set_task_state(TASK_TRANSCODE_CLEANUP, TaskState::Idle).await;
    }

    /// Start all scheduled task loops. Call once at startup.
    pub fn start(self, mut shutdown_rx: watch::Receiver<bool>) {
        // Library scan loop
        let sched = self.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            // Initial delay before first run
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                sched.run_library_scan().await;
                let interval = sched
                    .get_interval_for_task(TASK_LIBRARY_SCAN)
                    .await
                    .unwrap_or(std::time::Duration::from_secs(12 * 3600));
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = rx.wait_for(|v| *v) => break,
                }
            }
        });

        // Metadata refresh loop
        let sched = self.clone();
        let mut rx = shutdown_rx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
            loop {
                sched.run_metadata_refresh().await;
                let interval = sched
                    .get_interval_for_task(TASK_METADATA_REFRESH)
                    .await
                    .unwrap_or(std::time::Duration::from_secs(24 * 3600));
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = rx.wait_for(|v| *v) => break,
                }
            }
        });

        // Transcode cache cleanup loop
        let sched = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            loop {
                sched.run_transcode_cleanup().await;
                let interval = sched
                    .get_interval_for_task(TASK_TRANSCODE_CLEANUP)
                    .await
                    .unwrap_or(std::time::Duration::from_secs(24 * 3600));
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = shutdown_rx.wait_for(|v| *v) => break,
                }
            }
        });
    }
}

/// Remove transcode cache files older than 24 hours
async fn cleanup_transcode_cache(cache_dir: &std::path::Path) -> Result<u32, String> {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 3600))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let mut removed = 0u32;
    let mut rd = tokio::fs::read_dir(cache_dir)
        .await
        .map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = rd.next_entry().await {
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
