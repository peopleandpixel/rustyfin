use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionDto {
    pub id: Uuid,
    pub name: String,
    pub user_id: Uuid,
    pub child_count: i64,
    pub date_created: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateCollectionRequest {
    pub name: String,
    #[serde(default)]
    pub ids: Option<Vec<Uuid>>,
}
