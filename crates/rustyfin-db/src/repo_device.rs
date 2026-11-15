use sqlx::SqlitePool;

/// A signed-in device, from the `devices` table that sign-ins fill (`repo_session`), with the
/// name of its last user.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub custom_name: Option<String>,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub app_name: String,
    pub app_version: String,
    /// When the device was last used, ISO 8601 UTC.
    pub last_seen: String,
}

/// Every device, most recently used first; only `user_id`'s when given.
pub async fn get_devices(
    pool: &SqlitePool,
    user_id: Option<&str>,
) -> Result<Vec<DeviceRow>, sqlx::Error> {
    sqlx::query_as::<_, DeviceRow>(
        "SELECT d.id, d.name, d.custom_name, d.user_id, u.name AS user_name,
                d.app_name, d.app_version, strftime('%Y-%m-%dT%H:%M:%fZ', d.last_seen) AS last_seen
         FROM devices d LEFT JOIN users u ON u.id = d.user_id
         WHERE ?1 IS NULL OR d.user_id = ?1
         ORDER BY d.last_seen DESC, d.id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn get_device(
    pool: &SqlitePool,
    device_id: &str,
) -> Result<Option<DeviceRow>, sqlx::Error> {
    sqlx::query_as::<_, DeviceRow>(
        "SELECT d.id, d.name, d.custom_name, d.user_id, u.name AS user_name,
                d.app_name, d.app_version, strftime('%Y-%m-%dT%H:%M:%fZ', d.last_seen) AS last_seen
         FROM devices d LEFT JOIN users u ON u.id = d.user_id
         WHERE d.id = ?1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await
}

/// Sets a device's custom name (Jellyfin's device options); a blank name clears it. Returns
/// false when there is no such device.
pub async fn set_custom_name(
    pool: &SqlitePool,
    device_id: &str,
    custom_name: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let custom_name = custom_name.map(str::trim).filter(|n| !n.is_empty());
    let done = sqlx::query("UPDATE devices SET custom_name = ?1 WHERE id = ?2")
        .bind(custom_name)
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Adds or replaces a device by hand (`POST /Devices`); sign-ins add theirs themselves.
pub async fn create_device(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    user_id: Option<&str>,
    app_name: &str,
    app_version: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO devices (id, name, user_id, app_name, app_version, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           user_id = excluded.user_id,
           app_name = excluded.app_name,
           app_version = excluded.app_version,
           last_seen = excluded.last_seen",
    )
    .bind(id)
    .bind(name)
    .bind(user_id)
    .bind(app_name)
    .bind(app_version)
    .execute(pool)
    .await?;
    Ok(())
}
