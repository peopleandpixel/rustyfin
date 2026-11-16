//! Server backups, as Jellyfin 10.11's `/Backup` API lists them. A backup is one SQLite file in
//! `<data dir>/backups`: a consistent copy of the database (`VACUUM INTO`) with a table
//! `rustyfin_backup` that holds the manifest and the settings files of the data directory. Images,
//! trickplay tiles and subtitles are not included.
//!
//! A restore is staged (`restore.pending` names the backup) and done at the next start, before the
//! database is opened: the current database moves to `backups/pre-restore-<time>.db`, the backup
//! takes its place and its settings files are written back.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

pub const BACKUP_DIR: &str = "backups";
const PENDING_FILE: &str = "restore.pending";
const PREFIX: &str = "rustyfin-backup-";
const EXTENSION: &str = "db";
/// The settings files a backup carries, in the data directory.
const SETTINGS_FILES: [&str; 4] = [
    "config.json",
    "encoding.json",
    "network.json",
    "branding.json",
];
const ENGINE_VERSION: &str = "1.0.0";

pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_DIR)
}

/// The backup file a client names, when it is a plain file name of a backup in the backup folder.
/// Anything with a folder in it is refused.
pub fn backup_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let name = Path::new(name).file_name()?.to_str()?;
    let plain = !name.contains(['/', '\\']) && name.starts_with(PREFIX);
    let path = backup_dir(data_dir).join(name);
    (plain && name.ends_with(&format!(".{EXTENSION}")) && path.is_file()).then_some(path)
}

async fn open(path: &Path, read_only: bool) -> Result<SqliteConnection, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(read_only)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options).await
}

/// Writes a new backup of `pool`'s database and the settings files. Returns its manifest.
pub async fn create(pool: &sqlx::SqlitePool, data_dir: &Path) -> Result<Value, String> {
    let dir = backup_dir(data_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let mut name = format!("{PREFIX}{}.{EXTENSION}", now.format("%Y%m%d%H%M%S"));
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("{PREFIX}{}-{n}.{EXTENSION}", now.format("%Y%m%d%H%M%S"));
    }
    let path = dir.join(&name);
    sqlx::query("VACUUM INTO ?1")
        .bind(path.to_string_lossy().to_string())
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    let manifest = json!({
        "ServerVersion": env!("CARGO_PKG_VERSION"),
        "BackupEngineVersion": ENGINE_VERSION,
        "DateCreated": now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "Path": name,
        "Options": { "Metadata": false, "Trickplay": false, "Subtitles": false, "Database": true },
    });
    let result = async {
        let mut conn = open(&path, false).await?;
        sqlx::query(
            "CREATE TABLE rustyfin_backup (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)",
        )
        .execute(&mut conn)
        .await?;
        let mut rows = vec![("manifest".to_string(), manifest.to_string())];
        for file in SETTINGS_FILES {
            if let Ok(text) = tokio::fs::read_to_string(data_dir.join(file)).await {
                rows.push((format!("file:{file}"), text));
            }
        }
        for (key, value) in rows {
            sqlx::query("INSERT INTO rustyfin_backup (key, value) VALUES (?1, ?2)")
                .bind(key)
                .bind(value)
                .execute(&mut conn)
                .await?;
        }
        conn.close().await
    }
    .await;
    if let Err(e) = result {
        tokio::fs::remove_file(&path).await.ok();
        return Err(e.to_string());
    }
    Ok(manifest)
}

/// The manifest of a backup file; `None` for a file that isn't one.
pub async fn manifest(path: &Path) -> Option<Value> {
    let mut conn = open(path, true).await.ok()?;
    let text: Option<String> =
        sqlx::query_scalar("SELECT value FROM rustyfin_backup WHERE key = 'manifest'")
            .fetch_optional(&mut conn)
            .await
            .ok()
            .flatten();
    conn.close().await.ok();
    let mut manifest: Value = serde_json::from_str(&text?).ok()?;
    // The file's name now, should it have been renamed
    let name = path.file_name()?.to_string_lossy().to_string();
    manifest["Path"] = json!(name);
    Some(manifest)
}

/// Every backup's manifest, newest first.
pub async fn list(data_dir: &Path) -> Vec<Value> {
    let mut found = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(backup_dir(data_dir)).await else {
        return found;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(path) = backup_path(data_dir, &name)
            && let Some(m) = manifest(&path).await
        {
            found.push(m);
        }
    }
    found.sort_by(|a, b| {
        b["DateCreated"]
            .as_str()
            .cmp(&a["DateCreated"].as_str())
            .then_with(|| b["Path"].as_str().cmp(&a["Path"].as_str()))
    });
    found
}

/// Stages the backup `name` for a restore at the next start.
pub async fn stage_restore(data_dir: &Path, name: &str) -> Result<(), String> {
    tokio::fs::write(data_dir.join(PENDING_FILE), name)
        .await
        .map_err(|e| e.to_string())
}

/// The backup staged for a restore, if any.
pub fn pending_restore(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(data_dir.join(PENDING_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Does a staged restore, at the start, before `db_path` (the database in the data directory) is
/// opened. The marker is removed first, so a restore that fails is never tried again. Returns
/// the name of the backup restored.
pub async fn apply_pending_restore(
    data_dir: &Path,
    db_path: &Path,
) -> Result<Option<String>, String> {
    let Some(name) = pending_restore(data_dir) else {
        return Ok(None);
    };
    std::fs::remove_file(data_dir.join(PENDING_FILE)).map_err(|e| e.to_string())?;
    let source = backup_path(data_dir, &name).ok_or(format!("backup {name} not found"))?;
    if manifest(&source).await.is_none() {
        return Err(format!("{name} is not a Rustyfin backup"));
    }
    // The current database, with its journal files, is kept beside the backups
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let kept = backup_dir(data_dir).join(format!("pre-restore-{stamp}.db"));
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", db_path.display()));
        if from.exists() {
            let to = PathBuf::from(format!("{}{suffix}", kept.display()));
            std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    std::fs::copy(&source, db_path).map_err(|e| e.to_string())?;
    let mut conn = open(db_path, false).await.map_err(|e| e.to_string())?;
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM rustyfin_backup WHERE key LIKE 'file:%'")
            .fetch_all(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
    for (key, text) in rows {
        let file = key.trim_start_matches("file:");
        if SETTINGS_FILES.contains(&file) {
            std::fs::write(data_dir.join(file), text).map_err(|e| e.to_string())?;
        }
    }
    sqlx::query("DROP TABLE rustyfin_backup")
        .execute(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    conn.close().await.map_err(|e| e.to_string())?;
    Ok(Some(name))
}
