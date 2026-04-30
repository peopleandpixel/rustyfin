use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActivityLogEntry {
    pub id: i64,
    pub name: String,
    #[serde(rename = "Type")]
    pub entry_type: String,
    pub user_id: Option<String>,
    pub item_id: Option<String>,
    pub severity: String,
    pub overview: Option<String>,
    pub date: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActivityLogResult {
    pub items: Vec<ActivityLogEntry>,
    pub total_record_count: i64,
}
