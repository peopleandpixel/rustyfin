//! Writes of `config.json`. The file is replaced in one rename, so a crash or a full disk never
//! leaves a half-written config behind.

use std::path::{Path, PathBuf};

use rustyfin_model::server_info::ServerConfiguration;

/// Write the config as pretty JSON to `<path>.tmp` in the same directory, then rename it over
/// `path`. The temporary file is left behind only if the rename fails.
pub fn write_config_atomic(path: &Path, config: &ServerConfiguration) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
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
