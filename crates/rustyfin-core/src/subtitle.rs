use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

/// Supported external subtitle extensions.
const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt"];

/// OpenSubtitles login response
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OsLoginResponse {
    error: Option<String>,
    token: Option<String>,
}

/// OpenSubtitles search result
#[derive(Debug, Deserialize)]
struct OsSearchResponse {
    data: Option<Vec<OsSubtitleEntry>>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct OsSubtitleEntry {
    id: String,
    SubFileName: String,
    SubLanguageID: String,
    #[allow(dead_code)]
    SubDownloadLink: String,
    #[allow(dead_code)]
    SubVotes: Option<String>,
    #[allow(dead_code)]
    SubSumVotes: Option<String>,
}

/// Configuration for OpenSubtitles API
#[derive(Debug, Clone)]
pub struct OpenSubtitlesConfig {
    pub user_agent: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for OpenSubtitlesConfig {
    fn default() -> Self {
        Self {
            user_agent: format!("rustyfin/{}", env!("CARGO_PKG_VERSION")),
            username: None,
            password: None,
        }
    }
}

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
        } else if let Some(lang) = file_stem_full
            .strip_prefix(stem)
            .and_then(|r| r.strip_prefix('.'))
            && !lang.is_empty()
            && lang.len() <= 10
        {
            let lang = lang.to_string();
            results.push(ExternalSubtitle {
                path,
                language: Some(lang),
                format: ext,
                index: 0,
            });
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

/// Remote subtitle search result
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteSubtitleResult {
    pub id: String,
    pub name: String,
    pub language: String,
    pub format: String,
    pub download_count: Option<i64>,
}

/// Search for remote subtitles using OpenSubtitles API
pub async fn search_remote_subtitles(
    _pool: &SqlitePool,
    name: &str,
    language: Option<&str>,
    production_year: Option<i32>,
) -> Result<Vec<RemoteSubtitleResult>, sqlx::Error> {
    let config = OpenSubtitlesConfig::default();
    search_open_subtitles(&config, name, language, production_year).await
}

/// Search OpenSubtitles API for subtitles
async fn search_open_subtitles(
    config: &OpenSubtitlesConfig,
    name: &str,
    language: Option<&str>,
    year: Option<i32>,
) -> Result<Vec<RemoteSubtitleResult>, sqlx::Error> {
    let client = reqwest::Client::new();

    // Build search query
    let mut query = serde_json::Map::new();
    query.insert("query".to_string(), serde_json::json!(name));
    if let Some(y) = year {
        query.insert("movie_year".to_string(), serde_json::json!(y));
    }
    if let Some(lang) = language {
        query.insert("sublanguageid".to_string(), serde_json::json!(lang));
    }

    let body = serde_json::json!({
        "method": "search",
        "params": query,
    });

    let resp = client
        .post("https://api.opensubtitles.com/api/v1/search")
        .header("User-Agent", &config.user_agent)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("OpenSubtitles search request failed: {}", e);
            return Ok(vec![]);
        }
    };

    let os_resp: OsSearchResponse = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("OpenSubtitles JSON parse failed: {}", e);
            return Ok(vec![]);
        }
    };

    if let Some(ref error) = os_resp.error {
        tracing::warn!("OpenSubtitles API error: {}", error);
        return Ok(vec![]);
    }

    let entries = os_resp.data.unwrap_or_default();
    let mut results = Vec::new();

    for entry in entries {
        let lang_name = match entry.SubLanguageID.as_str() {
            "eng" => "English",
            "deu" => "German",
            "fra" => "French",
            "spa" => "Spanish",
            "ita" => "Italian",
            "por" => "Portuguese",
            "jpn" => "Japanese",
            "kor" => "Korean",
            "chi" | "zho" => "Chinese",
            "rus" => "Russian",
            "nld" => "Dutch",
            "pol" => "Polish",
            "tur" => "Turkish",
            "ara" => "Arabic",
            other => other,
        };

        let download_count = entry
            .SubVotes
            .as_deref()
            .and_then(|v| v.parse::<i64>().ok());

        results.push(RemoteSubtitleResult {
            id: entry.id,
            name: entry.SubFileName,
            language: lang_name.to_string(),
            format: "srt".to_string(),
            download_count,
        });
    }

    Ok(results)
}

/// Download a subtitle from OpenSubtitles by ID and save it to the cache directory
pub async fn download_subtitle(
    config: &OpenSubtitlesConfig,
    subtitle_id: &str,
    cache_dir: &Path,
) -> Result<PathBuf, String> {
    let client = reqwest::Client::new();

    // Ensure cache directory exists
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|e| format!("Failed to create cache directory: {}", e))?;

    // Build download request
    let body = serde_json::json!({
        "method": "download",
        "params": {
            "subid": subtitle_id,
        },
    });

    let resp = client
        .post("https://api.opensubtitles.com/api/v1/download")
        .header("User-Agent", &config.user_agent)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Download failed with status: {}", status));
    }

    // The response is a JSON object with a download link
    let download_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse download response: {}", e))?;

    let download_url = download_resp
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|d| d.get("data"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| "No download URL in response".to_string())?;

    // Fetch the actual subtitle file
    let sub_bytes = client
        .get(download_url)
        .header("User-Agent", &config.user_agent)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch subtitle: {}", e))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read subtitle bytes: {}", e))?;

    // Save to cache
    let filename = format!("{}_en.srt", subtitle_id);
    let file_path = cache_dir.join(&filename);

    tokio::fs::write(&file_path, &sub_bytes)
        .await
        .map_err(|e| format!("Failed to write subtitle file: {}", e))?;

    tracing::info!("Downloaded subtitle {} to {}", subtitle_id, filename);
    Ok(file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── srt_to_vtt ──

    #[test]
    fn test_srt_to_vtt_basic() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nHello world";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:02.000"));
        assert!(vtt.contains("Hello world"));
    }

    #[test]
    fn test_srt_to_vtt_empty() {
        let vtt = srt_to_vtt("");
        assert_eq!(vtt, "WEBVTT\n\n");
    }

    #[test]
    fn test_srt_to_vtt_multiple_cues() {
        let srt =
            "1\n00:00:01,000 --> 00:00:02,000\nFirst\n\n2\n00:00:03,000 --> 00:00:04,000\nSecond";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n"));
        assert!(vtt.contains("First"));
        assert!(vtt.contains("Second"));
    }

    #[test]
    fn test_srt_to_vtt_commas_in_text_preserved() {
        // Commas in subtitle text (not in timestamp lines) should be preserved
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nHello, world";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("Hello, world"));
    }

    #[test]
    fn test_srt_to_vtt_commas_in_timestamp_replaced() {
        let srt = "1\n00:01:02,345 --> 00:03:04,567\nText";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("00:01:02.345 --> 00:03:04.567"));
    }

    #[test]
    fn test_srt_to_vtt_multiline_text() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nLine one\nLine two";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("Line one"));
        assert!(vtt.contains("Line two"));
    }

    #[test]
    fn test_srt_to_vtt_blank_line_preserved() {
        let srt =
            "1\n00:00:01,000 --> 00:00:02,000\nText\n\n2\n00:00:03,000 --> 00:00:04,000\nMore";
        let vtt = srt_to_vtt(srt);
        // The blank line between cues should be preserved
        assert!(vtt.contains("\n\n"));
    }
}
