use std::path::{Path, PathBuf};

/// Supported external subtitle extensions.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt"];

/// Represents an external subtitle file found alongside a media file.
#[derive(Debug, Clone)]
pub struct ExternalSubtitle {
    pub path: PathBuf,
    pub language: Option<String>,
    pub format: String,
    pub index: usize,
}

/// Discover external subtitle files next to the given media file.
///
/// Looks for files matching `<basename>.<ext>` and `<basename>.<lang>.<ext>`
/// where `<ext>` is one of srt, ass, ssa, vtt.
pub fn discover_external_subtitles(media_path: &Path) -> Vec<ExternalSubtitle> {
    let parent = match media_path.parent() {
        Some(p) => p,
        None => return vec![],
    };
    let stem = match media_path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return vec![],
    };

    let mut results = Vec::new();
    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => continue,
        };
        if !SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let file_stem_full = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };

        // Match <stem> or <stem>.<lang>
        if file_stem_full == stem {
            results.push(ExternalSubtitle {
                path,
                language: None,
                format: ext,
                index: 0, // assigned later
            });
        } else if let Some(lang) = file_stem_full.strip_prefix(stem).and_then(|r| r.strip_prefix('.')) {
            if !lang.is_empty() && lang.len() <= 10 {
                let lang = lang.to_string();
                results.push(ExternalSubtitle {
                    path,
                    language: Some(lang),
                    format: ext,
                    index: 0,
                });
            }
        }
    }

    results.sort_by(|a, b| a.path.cmp(&b.path));
    for (i, sub) in results.iter_mut().enumerate() {
        sub.index = i;
    }
    results
}

/// Convert SRT content to WebVTT format.
pub fn srt_to_vtt(srt: &str) -> String {
    let mut out = String::with_capacity(srt.len() + 16);
    out.push_str("WEBVTT\n\n");

    for line in srt.lines() {
        // Replace SRT timestamp commas with VTT dots
        if line.contains(" --> ") && line.contains(',') {
            out.push_str(&line.replace(',', "."));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}
