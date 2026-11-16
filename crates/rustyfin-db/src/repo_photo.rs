//! What a scan read from each photo's header and EXIF block (`photo_metadata`).

use std::collections::HashMap;

use crate::repo_media::PathScope;
use chrono::{DateTime, Datelike, Utc};
use rustyfin_model::photo::PhotoInfo;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct PhotoRow {
    item_id: String,
    width: Option<i32>,
    height: Option<i32>,
    camera_make: Option<String>,
    camera_model: Option<String>,
    software: Option<String>,
    orientation: Option<String>,
    exposure_time: Option<f64>,
    focal_length: Option<f64>,
    aperture: Option<f64>,
    iso_speed_rating: Option<i32>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude: Option<f64>,
    date_taken: Option<String>,
}

impl From<PhotoRow> for PhotoInfo {
    fn from(r: PhotoRow) -> Self {
        Self {
            width: r.width,
            height: r.height,
            camera_make: r.camera_make,
            camera_model: r.camera_model,
            software: r.software,
            orientation: r.orientation,
            exposure_time: r.exposure_time,
            focal_length: r.focal_length,
            aperture: r.aperture,
            iso_speed_rating: r.iso_speed_rating,
            latitude: r.latitude,
            longitude: r.longitude,
            altitude: r.altitude,
            date_taken: r.date_taken.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
        }
    }
}

const SELECT: &str = "SELECT item_id, width, height, camera_make, camera_model, software, orientation, \
     exposure_time, focal_length, aperture, iso_speed_rating, latitude, longitude, altitude, date_taken \
     FROM photo_metadata";

/// The stored metadata of one photo; `None` when no scan has read it.
pub async fn get_photo_info(
    pool: &SqlitePool,
    item_id: Uuid,
) -> Result<Option<PhotoInfo>, sqlx::Error> {
    let sql = format!("{SELECT} WHERE item_id = ?");
    let row = sqlx::query_as::<_, PhotoRow>(sqlx::AssertSqlSafe(sql))
        .bind(item_id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.map(PhotoInfo::from))
}

/// The stored metadata of each of `item_ids` that has some, by item id.
pub async fn get_photo_infos(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<HashMap<Uuid, PhotoInfo>, sqlx::Error> {
    let mut found = HashMap::new();
    // In chunks, well under SQLite's limit on bound parameters.
    for chunk in item_ids.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!("{SELECT} WHERE item_id IN ({placeholders})");
        // Only placeholders are added to the fixed SQL.
        let mut query = sqlx::query_as::<_, PhotoRow>(sqlx::AssertSqlSafe(sql));
        for id in chunk {
            query = query.bind(id.to_string());
        }
        for row in query.fetch_all(pool).await? {
            if let Ok(id) = Uuid::parse_str(&row.item_id) {
                found.insert(id, PhotoInfo::from(row));
            }
        }
    }
    Ok(found)
}

/// A photo of a library, with the file version its metadata was read at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPhoto {
    pub item_id: Uuid,
    pub path: String,
    /// `None` when the photo was never read; `Some(None)` when it was read with no stamp.
    pub stamp: Option<Option<i64>>,
    /// Whether the photo has a Primary image.
    pub has_primary: bool,
}

/// Every photo of a library in `scope`, with the stamp its metadata was read at.
pub async fn library_photos(
    pool: &SqlitePool,
    library_id: Uuid,
    scope: &PathScope,
) -> Result<Vec<StoredPhoto>, sqlx::Error> {
    let sql = format!(
        "SELECT m.id, m.path, p.item_id, p.stamp, EXISTS (SELECT 1 FROM item_images i \
         WHERE i.item_id = m.id AND i.image_type = 'Primary') FROM media_items m \
         LEFT JOIN photo_metadata p ON p.item_id = m.id \
         WHERE m.library_id = ?1 AND m.media_type = 'Photo' AND {}",
        scope.sql("m.path", 2)
    );
    // `PathScope::sql` adds a fixed condition; the folder is bound.
    let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<i64>, bool)>(
        sqlx::AssertSqlSafe(sql),
    )
    .bind(library_id.to_string())
    .bind(scope.param())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, path, read, stamp, has_primary)| {
            Some(StoredPhoto {
                item_id: Uuid::parse_str(&id).ok()?,
                path,
                stamp: read.map(|_| stamp),
                has_primary,
            })
        })
        .collect())
}

/// Stores what a photo's file says, read at `stamp`. A date taken also becomes the item's premiere
/// date and production year, as in Jellyfin.
pub async fn save_photo_info(
    pool: &SqlitePool,
    item_id: Uuid,
    stamp: Option<i64>,
    info: &PhotoInfo,
) -> Result<(), sqlx::Error> {
    let id = item_id.to_string();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query(
        "INSERT OR REPLACE INTO photo_metadata (item_id, stamp, width, height, camera_make, \
         camera_model, software, orientation, exposure_time, focal_length, aperture, \
         iso_speed_rating, latitude, longitude, altitude, date_taken) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(stamp)
    .bind(info.width)
    .bind(info.height)
    .bind(&info.camera_make)
    .bind(&info.camera_model)
    .bind(&info.software)
    .bind(&info.orientation)
    .bind(info.exposure_time)
    .bind(info.focal_length)
    .bind(info.aperture)
    .bind(info.iso_speed_rating)
    .bind(info.latitude)
    .bind(info.longitude)
    .bind(info.altitude)
    .bind(info.date_taken.map(|d| d.to_rfc3339()))
    .execute(&mut *tx)
    .await?;
    if let Some(taken) = info.date_taken {
        sqlx::query("UPDATE media_items SET premiere_date = ?, production_year = ? WHERE id = ?")
            .bind(taken.to_rfc3339())
            .bind(taken.year())
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}
