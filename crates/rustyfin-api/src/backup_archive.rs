//! Server backups, as Jellyfin 10.11's `/Backup` API lists them. A backup is one SQLite file in
//! `<data dir>/backups`: a consistent copy of the database (`VACUUM INTO`) with a table
//! `rustyfin_backup` that holds the manifest and the settings files of the data directory. Images,
//! trickplay tiles and subtitles are not included.
//!
//! A restore is staged (`restore.pending` names the backup) and done at the next start, before the
//! database is opened: the current database moves to `backups/pre-restore-<time>.db`, the backup
//! takes its place and its settings files are written back. A restore that fails part way puts the
//! database and settings back as they were; if even that fails, the server must not start (an
//! empty database would let anyone claim the owner account through the setup wizard).
//!
//! The backup folder is readable only by the server's user (0700, its files 0600). The newest
//! `KEEP_BACKUPS` backups and `KEEP_PRE_RESTORE` replaced databases are kept.

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
const PRE_RESTORE_PREFIX: &str = "pre-restore-";
/// How many backups `create` keeps, newest first.
pub const KEEP_BACKUPS: usize = 10;
/// How many replaced databases (`pre-restore-*.db`) a restore keeps, newest first.
pub const KEEP_PRE_RESTORE: usize = 2;

/// Sets a file's or folder's Unix permission bits; nothing elsewhere.
fn restrict(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Creates the backup folder, readable only by the server's user.
fn ensure_backup_dir(data_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = backup_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    restrict(&dir, 0o700)?;
    Ok(dir)
}

/// Deletes all but the newest `keep` database files in the backup folder whose names start with
/// `prefix` (newest by modification time, then by name), with their journal files.
fn prune(data_dir: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(backup_dir(data_dir)) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let ok = name.starts_with(prefix) && name.ends_with(&format!(".{EXTENSION}"));
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            ok.then(|| (modified, name, e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, name, path) in files.into_iter().skip(keep) {
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", path.display()));
            if let Err(e) = std::fs::remove_file(&file)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("could not delete the old backup file {name}{suffix}: {e}");
            }
        }
    }
}

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
    let dir = ensure_backup_dir(data_dir).map_err(|e| e.to_string())?;
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
    if let Err(e) = restrict(&path, 0o600) {
        tokio::fs::remove_file(&path).await.ok();
        return Err(e.to_string());
    }
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
    prune(data_dir, PREFIX, KEEP_BACKUPS);
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

/// Why a staged restore didn't happen.
#[derive(Debug)]
pub enum RestoreError {
    /// It failed, and the database and settings are as they were: the server can start.
    RolledBack(String),
    /// It failed, and the database could not be put back: the server must not start.
    Fatal(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RolledBack(e) => write!(f, "{e}; the database and settings are unchanged"),
            Self::Fatal(e) => write!(f, "{e}"),
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

/// Does a staged restore, at the start, before `db_path` (the database in the data directory) is
/// opened. The marker is removed first, so a restore that fails is never tried again. Returns
/// the name of the backup restored.
pub async fn apply_pending_restore(
    data_dir: &Path,
    db_path: &Path,
) -> Result<Option<String>, RestoreError> {
    let Some(name) = pending_restore(data_dir) else {
        return Ok(None);
    };
    let rolled_back = RestoreError::RolledBack;
    std::fs::remove_file(data_dir.join(PENDING_FILE)).map_err(|e| rolled_back(e.to_string()))?;
    let source = backup_path(data_dir, &name)
        .ok_or_else(|| rolled_back(format!("backup {name} not found")))?;
    if manifest(&source).await.is_none() {
        return Err(rolled_back(format!("{name} is not a Rustyfin backup")));
    }
    let dir = ensure_backup_dir(data_dir).map_err(|e| rolled_back(e.to_string()))?;
    // The current database, with its journal files, is kept beside the backups
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let mut kept = dir.join(format!("{PRE_RESTORE_PREFIX}{stamp}.{EXTENSION}"));
    let mut n = 1;
    while kept.exists() {
        n += 1;
        kept = dir.join(format!("{PRE_RESTORE_PREFIX}{stamp}-{n}.{EXTENSION}"));
    }
    // The settings files as they are now, to put back if the restore fails
    let settings_before: Vec<(&str, Option<String>)> = SETTINGS_FILES
        .iter()
        .map(|f| (*f, std::fs::read_to_string(data_dir.join(f)).ok()))
        .collect();
    let mut moved = Vec::new();
    // Set once every original file is moved aside and the backup copied in: only then are the
    // files at `db_path` the restore's own, to delete on failure
    let mut copied = false;
    let result = async {
        for suffix in ["", "-wal", "-shm"] {
            let from = with_suffix(db_path, suffix);
            if from.exists() {
                let to = with_suffix(&kept, suffix);
                std::fs::rename(&from, &to).map_err(|e| e.to_string())?;
                moved.push(suffix);
                restrict(&to, 0o600).map_err(|e| e.to_string())?;
            }
        }
        copied = true;
        std::fs::copy(&source, db_path).map_err(|e| e.to_string())?;
        let mut conn = open(db_path, false).await.map_err(|e| e.to_string())?;
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT key, value FROM rustyfin_backup WHERE key LIKE 'file:%'")
                .fetch_all(&mut conn)
                .await
                .map_err(|e| e.to_string())?;
        sqlx::query("DROP TABLE rustyfin_backup")
            .execute(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        conn.close().await.map_err(|e| e.to_string())?;
        for (key, text) in rows {
            let file = key.trim_start_matches("file:");
            if SETTINGS_FILES.contains(&file) {
                std::fs::write(data_dir.join(file), text).map_err(|e| e.to_string())?;
            }
        }
        Ok::<(), String>(())
    }
    .await;
    let Err(error) = result else {
        prune(data_dir, PRE_RESTORE_PREFIX, KEEP_PRE_RESTORE);
        return Ok(Some(name));
    };
    // Put everything back: the settings files, then the database and its journal files
    for (file, before) in &settings_before {
        let path = data_dir.join(file);
        let put_back = match before {
            Some(text) => std::fs::write(&path, text),
            None => std::fs::remove_file(&path).or_else(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            }),
        };
        if let Err(e) = put_back {
            tracing::error!("could not put {file} back after the failed restore: {e}");
        }
    }
    let mut fatal = Vec::new();
    for suffix in ["", "-wal", "-shm"].into_iter().filter(|_| copied) {
        let current = with_suffix(db_path, suffix);
        if let Err(e) = std::fs::remove_file(&current)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            fatal.push(format!("remove {}: {e}", current.display()));
        }
    }
    for suffix in &moved {
        let from = with_suffix(&kept, suffix);
        let to = with_suffix(db_path, suffix);
        if let Err(e) = std::fs::rename(&from, &to) {
            fatal.push(format!("move {} back: {e}", from.display()));
        }
    }
    if fatal.is_empty() {
        Err(RestoreError::RolledBack(format!(
            "restoring {name} failed ({error})"
        )))
    } else {
        Err(RestoreError::Fatal(format!(
            "restoring {name} failed ({error}), and the database could not be put back ({}); \
             it is in {}",
            fatal.join("; "),
            kept.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("r3dash3-backup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn pool(dir: &Path) -> sqlx::SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(dir.join("source.db"))
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE users (name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO users VALUES ('owner')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn files(dir: &Path, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(backup_dir(dir))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(prefix))
            .collect();
        names.sort();
        names
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn r3dash3_failed_restore_puts_the_database_back() {
        let dir = temp_dir();
        std::fs::write(dir.join("config.json"), "{\"ServerName\":\"then\"}").unwrap();
        let name = create(&pool(&dir).await, &dir).await.unwrap()["Path"]
            .as_str()
            .unwrap()
            .to_string();
        let db = dir.join("live.db");
        std::fs::write(&db, b"the live database").unwrap();
        std::fs::write(with_suffix(&db, "-wal"), b"its journal").unwrap();
        std::fs::write(dir.join("branding.json"), "{\"LoginDisclaimer\":\"now\"}").unwrap();
        // config.json can't be written back: the restore fails after the database moved aside
        std::fs::remove_file(dir.join("config.json")).unwrap();
        std::fs::create_dir(dir.join("config.json")).unwrap();
        stage_restore(&dir, &name).await.unwrap();

        let err = apply_pending_restore(&dir, &db).await.unwrap_err();
        assert!(matches!(err, RestoreError::RolledBack(_)), "{err}");
        assert_eq!(std::fs::read(&db).unwrap(), b"the live database");
        assert_eq!(
            std::fs::read(with_suffix(&db, "-wal")).unwrap(),
            b"its journal"
        );
        assert!(!with_suffix(&db, "-shm").exists());
        assert!(files(&dir, PRE_RESTORE_PREFIX).is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("branding.json")).unwrap(),
            "{\"LoginDisclaimer\":\"now\"}"
        );
        // Never tried again
        assert!(apply_pending_restore(&dir, &db).await.unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn r3dash3_restore_keeps_the_replaced_database() {
        let dir = temp_dir();
        let name = create(&pool(&dir).await, &dir).await.unwrap()["Path"]
            .as_str()
            .unwrap()
            .to_string();
        let db = dir.join("live.db");
        for round in 0..3 {
            std::fs::write(&db, format!("live database {round}")).unwrap();
            stage_restore(&dir, &name).await.unwrap();
            let done = apply_pending_restore(&dir, &db).await.unwrap();
            assert_eq!(done.as_deref(), Some(name.as_str()));
        }
        let kept = files(&dir, PRE_RESTORE_PREFIX);
        assert_eq!(kept.len(), KEEP_PRE_RESTORE, "{kept:?}");
        #[cfg(unix)]
        for file in &kept {
            assert_eq!(mode(&backup_dir(&dir).join(file)), 0o600, "{file}");
        }
        let mut conn = open(&db, true).await.unwrap();
        let users: Vec<String> = sqlx::query_scalar("SELECT name FROM users")
            .fetch_all(&mut conn)
            .await
            .unwrap();
        assert_eq!(users, ["owner"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn r3dash3_backups_are_private_and_the_newest_ten_kept() {
        let dir = temp_dir();
        let pool = pool(&dir).await;
        for _ in 0..KEEP_BACKUPS + 2 {
            create(&pool, &dir).await.unwrap();
        }
        let names = files(&dir, PREFIX);
        assert_eq!(names.len(), KEEP_BACKUPS, "{names:?}");
        assert_eq!(list(&dir).await.len(), KEEP_BACKUPS);
        #[cfg(unix)]
        {
            assert_eq!(mode(&backup_dir(&dir)), 0o700);
            for name in &names {
                assert_eq!(mode(&backup_dir(&dir).join(name)), 0o600, "{name}");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
