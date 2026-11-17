//! The environment for an ffmpeg that opens a GPU, so its driver keeps its shader cache in the
//! server's data dir. Mesa caches compiled shaders under `$XDG_CACHE_HOME` (or `~/.cache`);
//! in a container whose `HOME` is `/` it can't, prints "Failed to create //.cache for shader
//! cache (Permission denied)", and compiles them again in every process. Shared by everything
//! that runs ffmpeg on a device.

use std::path::{Path, PathBuf};

/// The folder drivers cache into: `<data dir>/cache/gpu`.
pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("gpu")
}

/// The variables that point a GPU driver's caches at `cache_dir(data_dir)`, which is made if it
/// isn't there. Empty when it can't be made: the driver then works without its cache.
pub fn vars(data_dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let dir = cache_dir(data_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(dir = %dir.display(), error = %e, "no cache folder for the GPU driver");
        return Vec::new();
    }
    vec![
        ("XDG_CACHE_HOME", dir.clone()),
        ("MESA_SHADER_CACHE_DIR", dir.join("mesa_shader_cache")),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3trickplay_driver_caches_go_under_the_data_dir() {
        let data = std::env::temp_dir().join(format!("r3trickplay-env-{}", uuid::Uuid::new_v4()));
        let vars = vars(&data);
        assert_eq!(
            vars,
            [
                ("XDG_CACHE_HOME", data.join("cache/gpu")),
                (
                    "MESA_SHADER_CACHE_DIR",
                    data.join("cache/gpu/mesa_shader_cache")
                ),
            ]
        );
        assert!(data.join("cache/gpu").is_dir());
        let _ = std::fs::remove_dir_all(&data);
    }
}
