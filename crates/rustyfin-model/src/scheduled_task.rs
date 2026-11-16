use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScheduledTaskInfo {
    pub id: String,
    /// Jellyfin's stable task key (`RefreshLibrary`, ...). Jellyfin Web finds tasks by it.
    #[serde(default)]
    pub key: String,
    /// Hidden tasks are left out of `GET /ScheduledTasks?isHidden=false`. None is hidden.
    #[serde(default)]
    pub is_hidden: bool,
    pub name: String,
    pub description: String,
    pub category: String,
    pub state: TaskState,
    pub current_progress_percentage: Option<f64>,
    pub triggers: Vec<TaskTriggerInfo>,
    pub last_execution_result: Option<TaskResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskState {
    Idle,
    Running,
    Cancelling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    #[serde(rename = "Type")]
    pub trigger_type: String,
    pub interval_ticks: Option<u64>,
    pub time_of_day_ticks: Option<u64>,
    /// The day of a `WeeklyTrigger`, as Jellyfin's `DayOfWeek` names it (`Sunday` … `Saturday`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_week: Option<String>,
    pub max_runtime_ticks: Option<u64>,
}

/// Jellyfin's `DayOfWeek` names, Sunday first.
pub const DAYS_OF_WEEK: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Ticks (100 ns) in a day: a `TimeOfDayTicks` is below it.
pub const TICKS_PER_DAY: u64 = 24 * 60 * 60 * 10_000_000;

/// The shortest `IntervalTicks` accepted: one minute.
pub const MIN_INTERVAL_TICKS: u64 = 60 * 10_000_000;

impl TaskTriggerInfo {
    /// A trigger that runs the task every `ticks`.
    pub fn interval(ticks: u64) -> Self {
        Self {
            trigger_type: "IntervalTrigger".to_string(),
            interval_ticks: Some(ticks),
            time_of_day_ticks: None,
            day_of_week: None,
            max_runtime_ticks: None,
        }
    }

    /// Checks the trigger the way Jellyfin's `ScheduledTaskWorker` reads it: a known type with the
    /// fields that type needs. Returns the sentence for a trigger the scheduler cannot use.
    pub fn validate(&self) -> Result<(), String> {
        let time_of_day = || match self.time_of_day_ticks {
            Some(t) if t < TICKS_PER_DAY => Ok(()),
            Some(_) => Err("TimeOfDayTicks must be less than a day.".to_string()),
            None => Err(format!("A {} needs TimeOfDayTicks.", self.trigger_type)),
        };
        match self.trigger_type.as_str() {
            "IntervalTrigger" => match self.interval_ticks {
                Some(t) if t >= MIN_INTERVAL_TICKS => Ok(()),
                Some(_) => Err("IntervalTicks must be at least one minute.".to_string()),
                None => Err("An IntervalTrigger needs IntervalTicks.".to_string()),
            },
            "DailyTrigger" => time_of_day(),
            "WeeklyTrigger" => {
                time_of_day()?;
                match self.day_of_week.as_deref() {
                    Some(d) if DAYS_OF_WEEK.contains(&d) => Ok(()),
                    _ => Err("A WeeklyTrigger needs DayOfWeek (Sunday to Saturday).".to_string()),
                }
            }
            "StartupTrigger" => Ok(()),
            other => Err(format!("Unknown trigger type \"{other}\".")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TaskResult {
    pub status: TaskCompletionStatus,
    pub error_message: Option<String>,
    pub start_time_utc: String,
    pub end_time_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskCompletionStatus {
    Completed,
    Failed,
    Cancelled,
    Aborted,
}
