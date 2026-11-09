use rustyfin_model::library::{Library, LibraryContentType, LibraryOptions};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn create_library(pool: &SqlitePool, lib: &Library) -> Result<(), sqlx::Error> {
    let id = lib.id.to_string();
    let ct = serde_json::to_string(&lib.content_type).unwrap_or_default();
    let paths = serde_json::to_string(&lib.paths).unwrap_or_default();

    sqlx::query("INSERT INTO libraries (id, name, content_type, paths) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(&lib.name)
        .bind(&ct)
        .bind(&paths)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn get_all_libraries(pool: &SqlitePool) -> Result<Vec<Library>, sqlx::Error> {
    let rows = sqlx::query_as::<_, LibraryRow>("SELECT * FROM libraries")
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_library_by_id(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<Library>, sqlx::Error> {
    let row = sqlx::query_as::<_, LibraryRow>("SELECT * FROM libraries WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

pub async fn delete_library(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM media_items WHERE library_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM libraries WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_library_paths(
    pool: &SqlitePool,
    id: Uuid,
    paths: &[String],
) -> Result<(), sqlx::Error> {
    let paths_json = serde_json::to_string(paths).unwrap_or_default();
    sqlx::query("UPDATE libraries SET paths = ? WHERE id = ?")
        .bind(&paths_json)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// The options saved for a library, or the defaults when none are saved.
pub async fn get_library_options(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<LibraryOptions, sqlx::Error> {
    let value: Option<String> =
        sqlx::query_scalar("SELECT value FROM configuration WHERE key = ?1")
            .bind(format!("library_options_{id}"))
            .fetch_optional(pool)
            .await?;

    match value {
        Some(json) => serde_json::from_str(&json).map_err(|e| sqlx::Error::Decode(Box::new(e))),
        None => Ok(LibraryOptions::default()),
    }
}

/// Saves the options of a library, replacing the ones saved before.
pub async fn set_library_options(
    pool: &SqlitePool,
    id: Uuid,
    options: &LibraryOptions,
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_string(options).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT INTO configuration (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(format!("library_options_{id}"))
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct LibraryRow {
    id: String,
    name: String,
    content_type: String,
    paths: String,
}

impl From<LibraryRow> for Library {
    fn from(r: LibraryRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_default(),
            name: r.name,
            content_type: serde_json::from_str(&r.content_type)
                .unwrap_or(LibraryContentType::Mixed),
            paths: serde_json::from_str(&r.paths).unwrap_or_default(),
        }
    }
}
