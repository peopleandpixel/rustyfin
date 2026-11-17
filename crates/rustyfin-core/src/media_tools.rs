//! Where ffmpeg and ffprobe are: the one place every spawn and probe takes its program from, so
//! what the dashboard reports (`EncoderLocation`, `EncoderAppPath`) is what runs.
//!
//! `RUSTYFIN_FFMPEG_PATH` names ffmpeg ("Custom"), else `ffmpeg` is looked up on the PATH
//! ("System"). ffprobe is `RUSTYFIN_FFPROBE_PATH`, else the `ffprobe` next to a custom ffmpeg
//! when there is one (as Jellyfin finds it), else `ffprobe` on the PATH.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// The two programs, and whether ffmpeg was named by the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub custom: bool,
}

impl MediaTools {
    /// The programs for the variables' values (`None` when unset or empty); `exists` says
    /// whether a file is there.
    pub fn resolve(
        ffmpeg_var: Option<String>,
        ffprobe_var: Option<String>,
        exists: impl Fn(&Path) -> bool,
    ) -> Self {
        let set = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let ffmpeg = set(ffmpeg_var).map(PathBuf::from);
        let ffprobe = set(ffprobe_var).map(PathBuf::from).or_else(|| {
            let ffmpeg = ffmpeg.as_ref()?;
            let name = if ffmpeg.extension().is_some_and(|e| e == "exe") {
                "ffprobe.exe"
            } else {
                "ffprobe"
            };
            let beside = ffmpeg.parent()?.join(name);
            (ffmpeg.parent() != Some(Path::new("")) && exists(&beside)).then_some(beside)
        });
        Self {
            custom: ffmpeg.is_some(),
            ffmpeg: ffmpeg.unwrap_or_else(|| PathBuf::from("ffmpeg")),
            ffprobe: ffprobe.unwrap_or_else(|| PathBuf::from("ffprobe")),
        }
    }

    /// The programs the environment names.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var("RUSTYFIN_FFMPEG_PATH").ok(),
            std::env::var("RUSTYFIN_FFPROBE_PATH").ok(),
            Path::is_file,
        )
    }
}

fn cell() -> &'static RwLock<MediaTools> {
    static TOOLS: OnceLock<RwLock<MediaTools>> = OnceLock::new();
    TOOLS.get_or_init(|| RwLock::new(MediaTools::from_env()))
}

/// The programs in use (read from the environment the first time).
pub fn current() -> MediaTools {
    cell().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Replaces the programs in use, for every spawn after it. Tests point them at fake scripts.
pub fn set(tools: MediaTools) {
    *cell().write().unwrap_or_else(|e| e.into_inner()) = tools;
}

/// The ffmpeg to run.
pub fn ffmpeg() -> PathBuf {
    current().ffmpeg
}

/// The ffprobe to run.
pub fn ffprobe() -> PathBuf {
    current().ffprobe
}

/// Whether the operator named ffmpeg (`RUSTYFIN_FFMPEG_PATH`): Jellyfin's "Custom" location.
pub fn is_custom() -> bool {
    current().custom
}

/// `program` ("ffmpeg" or "ffprobe") as it runs; any other name is returned as it is.
pub fn program(program: &str) -> PathBuf {
    match program {
        "ffmpeg" => ffmpeg(),
        "ffprobe" => ffprobe(),
        other => PathBuf::from(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points `media_tools` at wrapper scripts that note each call's arguments in a log, then
    /// run the real ffmpeg and ffprobe on the PATH, so tests running at the same time are
    /// unaffected. Each spawner gets an input path naming it, and the log shows which program
    /// it ran.
    #[cfg(unix)]
    #[tokio::test]
    async fn r3tools_every_spawner_runs_the_configured_programs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("r3tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let wrap = |name: &str| {
            let path = dir.join(format!("custom-{name}"));
            let tmp = dir.join(format!("{name}.tmp"));
            let script = format!(
                "#!/bin/sh\nprintf '{name} %s\\n' \"$*\" >> '{}'\nexec {name} \"$@\"\n",
                log.display()
            );
            std::fs::write(&tmp, script).unwrap();
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::rename(&tmp, &path).unwrap();
            path
        };
        let tools = MediaTools {
            ffmpeg: wrap("ffmpeg"),
            ffprobe: wrap("ffprobe"),
            custom: true,
        };
        set(tools.clone());
        assert_eq!(ffmpeg(), tools.ffmpeg);
        assert_eq!(program("ffprobe"), tools.ffprobe);

        let input = |who: &str| {
            dir.join(format!("{who}.mkv"))
                .to_string_lossy()
                .into_owned()
        };
        let _ = crate::probe::probe_file(&input("probe")).await;
        let _ = crate::probe::read_chapters(&input("chapters")).await;
        let _ = crate::lyrics::read_lyrics_tag(&input("lyrics")).await;
        let cancel = crate::trickplay::Cancel::new();
        let _ = crate::trickplay::run_ffmpeg(&["-i", &input("trickplay")], &cancel).await;
        let _ = crate::probe::ffprobe_runs().await;
        // The transcoder and its capability probe take the same ffmpeg
        let manager = crate::transcode::TranscodeManager::new(&dir);
        assert_eq!(manager.ffmpeg(), tools.ffmpeg.as_path());

        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        for (program, who) in [
            ("ffprobe", "probe"),
            ("ffprobe", "chapters"),
            ("ffprobe", "lyrics"),
            ("ffmpeg", "trickplay"),
        ] {
            assert!(
                calls
                    .lines()
                    .any(|l| l.starts_with(&format!("{program} ")) && l.contains(&input(who))),
                "{program} for {who} not in {calls}"
            );
        }
        assert!(calls.lines().any(|l| l == "ffprobe -version"), "{calls}");
        // The wrappers stay: another test may still be running them
    }

    #[test]
    fn r3tools_resolve_env_then_path() {
        let none = MediaTools::resolve(None, None, |_| true);
        assert_eq!(none.ffmpeg, PathBuf::from("ffmpeg"));
        assert_eq!(none.ffprobe, PathBuf::from("ffprobe"));
        assert!(!none.custom);
        // An empty variable is unset
        assert!(!MediaTools::resolve(Some(" ".into()), None, |_| true).custom);
        // A custom ffmpeg brings the ffprobe beside it, when there is one
        let beside = MediaTools::resolve(Some("/opt/ff/bin/ffmpeg".into()), None, |p| {
            p == Path::new("/opt/ff/bin/ffprobe")
        });
        assert!(beside.custom);
        assert_eq!(beside.ffmpeg, PathBuf::from("/opt/ff/bin/ffmpeg"));
        assert_eq!(beside.ffprobe, PathBuf::from("/opt/ff/bin/ffprobe"));
        let alone = MediaTools::resolve(Some("/opt/ff/bin/ffmpeg".into()), None, |_| false);
        assert_eq!(alone.ffprobe, PathBuf::from("ffprobe"));
        // RUSTYFIN_FFPROBE_PATH wins
        let both = MediaTools::resolve(
            Some("/opt/ff/bin/ffmpeg".into()),
            Some("/usr/bin/ffprobe".into()),
            |_| true,
        );
        assert_eq!(both.ffprobe, PathBuf::from("/usr/bin/ffprobe"));
    }
}
