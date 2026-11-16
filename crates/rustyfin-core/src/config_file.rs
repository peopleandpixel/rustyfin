//! Reads and writes of `config.json`. The file is replaced in one rename, so a crash or a full
//! disk never leaves a half-written config behind, and a file that can't be read is never
//! replaced.

use std::io::Write;
use std::path::{Path, PathBuf};

use rustyfin_model::server_info::ServerConfiguration;

/// Write the config as pretty JSON to a temporary file of its own in the same directory, flush it
/// to disk, then rename it over `path`. The temporary file is left behind only if the rename
/// fails. The file gets this build's `RustyfinConfigVersion`, so what it holds is never upgraded
/// again.
pub fn write_config_atomic(path: &Path, config: &ServerConfiguration) -> std::io::Result<()> {
    let mut json = serde_json::to_value(config).map_err(std::io::Error::other)?;
    json["RustyfinConfigVersion"] = rustyfin_model::server_info::CONFIG_VERSION.into();
    let json = serde_json::to_string_pretty(&json).map_err(std::io::Error::other)?;
    // Two saves at once each write their own file, so neither renames the other's half-written one
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let tmp = PathBuf::from(tmp_name);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Why `config.json` wasn't loaded. The file is left as it was.
#[derive(Debug)]
pub enum LoadError {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, serde_json::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Read(path, e) => write!(f, "could not read {}: {e}", path.display()),
            LoadError::Parse(path, e) => write!(f, "{} is not valid: {e}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {}

/// Loads the server configuration from `path`. A missing file gives the defaults, which are
/// saved. A file from an older Rustyfin is brought up to date once, and saved so. A file that
/// can't be read or parsed is an error and is left untouched, so a typo from a hand edit never
/// costs the settings it holds; fields the file doesn't have take their defaults.
pub fn load_config(path: &Path) -> Result<ServerConfiguration, LoadError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let config = ServerConfiguration::default();
            if let Err(e) = write_config_atomic(path, &config) {
                tracing::warn!("could not save the default {}: {e}", path.display());
            }
            return Ok(config);
        }
        Err(e) => return Err(LoadError::Read(path.to_path_buf(), e)),
    };
    let mut config: ServerConfiguration =
        serde_json::from_str(&content).map_err(|e| LoadError::Parse(path.to_path_buf(), e))?;
    tracing::info!("loaded configuration from {}", path.display());
    if config.upgrade()
        && let Err(e) = write_config_atomic(path, &config)
    {
        tracing::warn!("could not save the upgraded {}: {e}", path.display());
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3admin_written_config_carries_the_version() {
        let dir = std::env::temp_dir().join(format!("rf-config-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        // An in-memory config parsed from a post without the version
        let config: ServerConfiguration =
            serde_json::from_str(r#"{"QuickConnectAvailable":false}"#).unwrap();
        write_config_atomic(&path, &config).unwrap();
        let mut back: ServerConfiguration =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(!back.upgrade(), "a file this build wrote is current");
        assert!(!back.quick_connect_available);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_file_writes_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("rf-config-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let mut config = ServerConfiguration {
            server_name: "first-name".to_string(),
            ..ServerConfiguration::default()
        };
        write_config_atomic(&path, &config).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("first-name"), "{text}");

        config.server_name = "second-name".to_string();
        write_config_atomic(&path, &config).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("second-name") && !text.contains("first-name"),
            "{text}"
        );
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["config.json"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn r3rel_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rf-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn r3rel_a_config_that_fails_to_parse_is_an_error_and_kept() {
        let dir = r3rel_dir("config-bad");
        let path = dir.join("config.json");
        for bad in ["{\"ServerName\": \"Mine\",, }", "{\"ServerName\": 5}", ""] {
            std::fs::write(&path, bad).unwrap();
            let err = load_config(&path).unwrap_err();
            assert!(matches!(err, LoadError::Parse(..)), "{bad}: {err}");
            assert!(err.to_string().contains("config.json"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn r3rel_a_missing_config_is_the_defaults_and_saved_and_new_fields_default() {
        let dir = r3rel_dir("config-missing");
        let path = dir.join("config.json");
        let config = load_config(&path).unwrap();
        assert_eq!(
            config.server_name,
            ServerConfiguration::default().server_name
        );
        assert!(path.exists());

        // A file without the fields a later version added keeps what it has.
        std::fs::write(
            &path,
            r#"{"ServerName":"Mine","IsStartupWizardCompleted":true}"#,
        )
        .unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(config.server_name, "Mine");
        assert!(config.is_startup_wizard_completed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn r3rel_a_config_that_cannot_be_read_is_an_error() {
        let dir = r3rel_dir("config-unreadable");
        // A folder where the file should be can't be read as one, for root too
        let path = dir.join("config.json");
        std::fs::create_dir_all(&path).unwrap();
        let err = load_config(&path).unwrap_err();
        assert!(matches!(err, LoadError::Read(..)), "{err}");
        assert!(path.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
