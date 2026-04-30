use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ScheduledTaskInfo {
    pub id: String,
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
    pub max_runtime_ticks: Option<u64>,
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
