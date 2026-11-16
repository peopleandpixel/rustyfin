//! Writes of `config.json`. The file is replaced in one rename, so a crash or a full disk never
//! leaves a half-written config behind.

use std::path::{Path, PathBuf};

use rustyfin_model::server_info::ServerConfiguration;

/// Write the config as pretty JSON to `<path>.tmp` in the same directory, then rename it over
/// `path`. The temporary file is left behind only if the rename fails. The file gets this build's
/// `RustyfinConfigVersion`, so what it holds is never upgraded again.
pub fn write_config_atomic(path: &Path, config: &ServerConfiguration) -> std::io::Result<()> {
    let mut json = serde_json::to_value(config).map_err(std::io::Error::other)?;
    json["RustyfinConfigVersion"] = rustyfin_model::server_info::CONFIG_VERSION.into();
    let json = serde_json::to_string_pretty(&json).map_err(std::io::Error::other)?;
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
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
        assert!(!dir.join("config.json.tmp").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
