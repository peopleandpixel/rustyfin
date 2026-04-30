use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NotificationDto {
    pub id: i64,
    pub user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub level: String,
    pub is_read: bool,
    pub date: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NotificationResult {
    pub notifications: Vec<NotificationDto>,
    pub total_record_count: i64,
}
