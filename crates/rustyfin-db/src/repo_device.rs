use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub user_id: Option<String>,
    pub app_name: String,
    pub app_version: String,
    pub device_name: String,
    pub last_activity_date: String,
    pub image_path: String,
}

pub async fn get_devices(pool: &SqlitePool) -> Result<Vec<DeviceRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, user_id, app_name, app_version, device_name, last_activity_date, image_path FROM devices",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_device(
    pool: &SqlitePool,
    device_id: &str,
) -> Result<Option<DeviceRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, user_id, app_name, app_version, device_name, last_activity_date, image_path FROM devices WHERE id = ?1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn get_device_by_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<DeviceRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT id, name, user_id, app_name, app_version, device_name, last_activity_date, image_path FROM devices WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_device(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    user_id: Option<&str>,
    app_name: &str,
    app_version: &str,
    device_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO devices (id, name, user_id, app_name, app_version, device_name, last_activity_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
    )
    .bind(id)
    .bind(name)
    .bind(user_id)
    .bind(app_name)
    .bind(app_version)
    .bind(device_name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_device(pool: &SqlitePool, device_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM devices WHERE id = ?1")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_device_image(
    pool: &SqlitePool,
    device_id: &str,
    image_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE devices SET image_path = ?1 WHERE id = ?2")
        .bind(image_path)
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_device_activity(pool: &SqlitePool, device_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE devices SET last_activity_date = datetime('now') WHERE id = ?1")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}
