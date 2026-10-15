use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHint {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub media_type: Option<String>,
    pub production_year: Option<i32>,
    pub run_time_ticks: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHintResult {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: i64,
}
