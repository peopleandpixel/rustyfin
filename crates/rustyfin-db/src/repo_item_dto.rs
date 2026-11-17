//! The stored item fields that both item DTO builders (`/Items/{id}` and the lists) send, read in
//! one batch so that the two can't disagree: the metadata columns outside `media_items`
//! (OriginalTitle, CriticRating, the IMDb id, ForcedSortName, the locks), a split movie's part
//! count and a folder's DateLastMediaAdded.

use std::collections::HashMap;

use sqlx::SqlitePool;
use uuid::Uuid;

/// One item's stored fields; each is `None` (or 0) when the item has none.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StoredFields {
    pub original_title: Option<String>,
    pub critic_rating: Option<f64>,
    pub imdb_id: Option<String>,
    pub forced_sort_name: Option<String>,
    pub lock_data: bool,
    /// Jellyfin's LockedFields, pipe-joined as stored.
    pub locked_fields: String,
    /// The extra parts of a split movie (part 1 not counted).
    pub additional_parts: i64,
    /// For a folder: when its newest child was added.
    pub last_media_added: Option<chrono::DateTime<chrono::Utc>>,
}

/// The stored fields of `items`, by id; `folders` (among them) also get their DateLastMediaAdded.
/// Items with none of them are left out. Each id list is bound once, as a JSON array.
pub async fn stored_fields(
    pool: &SqlitePool,
    items: &[Uuid],
    folders: &[Uuid],
) -> Result<HashMap<Uuid, StoredFields>, sqlx::Error> {
    let json = |ids: &[Uuid]| {
        serde_json::Value::from(ids.iter().map(Uuid::to_string).collect::<Vec<_>>()).to_string()
    };
    let mut fields: HashMap<Uuid, StoredFields> = HashMap::new();
    if items.is_empty() {
        return Ok(fields);
    }
    let ids = json(items);
    type ExtrasRow = (
        String,
        Option<String>,
        Option<f64>,
        Option<String>,
        Option<bool>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<ExtrasRow> = sqlx::query_as(
        "SELECT item_id, original_title, critic_rating, imdb_id, lock_data, locked_fields, \
         forced_sort_name FROM item_metadata_extras \
         WHERE item_id IN (SELECT value FROM json_each(?))",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for (id, original_title, critic_rating, imdb_id, lock_data, locked_fields, forced_sort_name) in
        rows
    {
        let Ok(id) = Uuid::parse_str(&id) else {
            continue;
        };
        let entry = fields.entry(id).or_default();
        entry.original_title = original_title;
        entry.critic_rating = critic_rating;
        entry.imdb_id = imdb_id.filter(|imdb| !imdb.trim().is_empty());
        entry.forced_sort_name = forced_sort_name.filter(|name| !name.trim().is_empty());
        entry.lock_data = lock_data.unwrap_or(false);
        entry.locked_fields = locked_fields.unwrap_or_default();
    }
    let parts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT owner_id, COUNT(*) FROM media_items WHERE extra_type = 'Part' \
         AND owner_id IN (SELECT value FROM json_each(?)) GROUP BY owner_id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for (id, count) in parts {
        if let Ok(id) = Uuid::parse_str(&id) {
            fields.entry(id).or_default().additional_parts = count;
        }
    }
    if !folders.is_empty() {
        // As RFC 3339 in UTC, whichever form the date was stored in (with or without a zone).
        let added: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT parent_id, strftime('%Y-%m-%dT%H:%M:%fZ', MAX(julianday(date_created))) \
             FROM media_items WHERE parent_id IN (SELECT value FROM json_each(?)) \
             GROUP BY parent_id",
        )
        .bind(json(folders))
        .fetch_all(pool)
        .await?;
        for (id, date) in added {
            if let (Ok(id), Some(Ok(date))) = (Uuid::parse_str(&id), date.map(|d| d.parse())) {
                fields.entry(id).or_default().last_media_added = Some(date);
            }
        }
    }
    Ok(fields)
}
