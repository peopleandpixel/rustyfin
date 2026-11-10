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
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_lowercase(),
            None => continue,
        };
        if !SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        if !path.is_file() {
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

/// Whether a subtitle codec is a picture subtitle, which is overlaid rather than drawn as text.
/// Matched without case: any codec containing "pgs", or one of the DVD, DVB, VobSub, XSUB and SUP
/// names.
pub fn is_image_subtitle(codec: &str) -> bool {
    let codec = codec.to_ascii_lowercase();
    codec.contains("pgs")
        || matches!(
            codec.as_str(),
            "dvd_subtitle" | "dvdsub" | "vobsub" | "dvb_subtitle" | "dvbsub" | "xsub" | "sup"
        )
}

/// Moves the cues of a WebVTT text back by `start_ticks` (100 ns units), so that a player starting
/// there gets them. A cue that has ended by then is dropped; the others start at 0 at the earliest.
/// The other blocks (the header, NOTE, STYLE and REGION) are kept as they are. Blocks are joined by
/// one blank line and the text ends with a newline. A start of 0 or less returns the text unchanged.
pub fn shift_vtt(vtt: &str, start_ticks: i64) -> String {
    if start_ticks <= 0 {
        return vtt.to_string();
    }
    let shift = start_ticks / 10_000;
    let mut out = String::with_capacity(vtt.len());
    for block in vtt_blocks(vtt) {
        let Some(lines) = shift_block(&block, shift) else {
            continue;
        };
        if !out.is_empty() {
            out.push('\n');
        }
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Takes the ASS override and comment blocks out of a WebVTT text, as no ASS renderer shows them.
/// In a cue, each `{` ... `}` run on a text line after the timing line is cut out; a `{` with no
/// `}` after it stays. A text line left empty or only whitespace is removed, and a cue left with no
/// text line is dropped. The header, the NOTE, STYLE and REGION blocks, the cue identifiers and the
/// timings stay as they are. Blocks are joined by one blank line and the text ends with a newline.
pub fn strip_ass_blocks(vtt: &str) -> String {
    let mut out = String::with_capacity(vtt.len());
    for block in vtt_blocks(vtt) {
        let Some(lines) = strip_block(&block) else {
            continue;
        };
        if !out.is_empty() {
            out.push('\n');
        }
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The lines of a block with the brace runs cut from its text, or None for a cue left with no text.
/// A block with no " --> " in its first two lines is kept as it is.
fn strip_block(block: &[&str]) -> Option<Vec<String>> {
    let Some(at) = block.iter().take(2).position(|line| line.contains(" --> ")) else {
        return Some(block.iter().map(|line| line.to_string()).collect());
    };
    let text: Vec<String> = block[at + 1..]
        .iter()
        .copied()
        .map(remove_brace_runs)
        .filter(|line| !line.trim().is_empty())
        .collect();
    if text.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = block[..=at].iter().map(|line| line.to_string()).collect();
    lines.extend(text);
    Some(lines)
}

/// The text with each `{` ... `}` run on it cut out. A `{` with no `}` after it stays.
fn remove_brace_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let Some(len) = rest[open..].find('}') else {
            break;
        };
        out.push_str(&rest[..open]);
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out
}

/// The blocks of a WebVTT text: runs of lines between blank lines, without their line endings.
fn vtt_blocks(vtt: &str) -> Vec<Vec<&str>> {
    let mut blocks = Vec::new();
    let mut block = Vec::new();
    for line in vtt.lines() {
        if !line.is_empty() {
            block.push(line);
        } else if !block.is_empty() {
            blocks.push(std::mem::take(&mut block));
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    blocks
}

/// The lines of a block moved back by `shift` milliseconds, or None when the block is a cue that
/// has ended. A cue's timing line is the first or second line; a line before it is an identifier.
fn shift_block(block: &[&str], shift: i64) -> Option<Vec<String>> {
    let mut lines: Vec<String> = block.iter().map(|line| line.to_string()).collect();
    let Some(at) = block.iter().take(2).position(|line| line.contains(" --> ")) else {
        return Some(lines);
    };
    let Some((start, end, settings)) = parse_timing(block[at]) else {
        return Some(lines);
    };
    if end <= shift {
        return None;
    }
    lines[at] = format!(
        "{} --> {}{settings}",
        format_vtt_time((start - shift).max(0)),
        format_vtt_time((end - shift).max(0)),
    );
    Some(lines)
}

/// A timing line `start --> end settings` as the start and end in milliseconds, and the settings
/// after the end time with their leading space. None when a time does not read.
fn parse_timing(line: &str) -> Option<(i64, i64, &str)> {
    let (start, rest) = line.split_once(" --> ")?;
    let rest = rest.trim_start();
    let end_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (end, settings) = rest.split_at(end_len);
    Some((
        parse_vtt_time(start.trim())?,
        parse_vtt_time(end)?,
        settings,
    ))
}

/// Reads `hh:mm:ss.ttt` (hours of one or more digits) or `mm:ss.ttt` as milliseconds.
fn parse_vtt_time(text: &str) -> Option<i64> {
    let (clock, millis) = text.split_once('.')?;
    if millis.len() != 3 {
        return None;
    }
    let parts: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (digits(h)?, digits(m)?, digits(s)?),
        [m, s] => (0, digits(m)?, digits(s)?),
        _ => return None,
    };
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    total.checked_mul(1000)?.checked_add(digits(millis)?)
}

/// The number `text` is when it is one or more ASCII digits.
fn digits(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Writes milliseconds as `HH:MM:SS.mmm`, with at least two digits for the hours.
fn format_vtt_time(ms: i64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1_000) % 60,
        ms % 1_000
    )
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
    client: &reqwest::Client,
    _pool: &SqlitePool,
    name: &str,
    language: Option<&str>,
    production_year: Option<i32>,
) -> Result<Vec<RemoteSubtitleResult>, sqlx::Error> {
    let config = OpenSubtitlesConfig::default();
    search_open_subtitles(client, &config, name, language, production_year).await
}

/// Search OpenSubtitles API for subtitles
async fn search_open_subtitles(
    client: &reqwest::Client,
    config: &OpenSubtitlesConfig,
    name: &str,
    language: Option<&str>,
    year: Option<i32>,
) -> Result<Vec<RemoteSubtitleResult>, sqlx::Error> {
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
    client: &reqwest::Client,
    config: &OpenSubtitlesConfig,
    subtitle_id: &str,
    cache_dir: &Path,
) -> Result<PathBuf, String> {
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

    // ── is_image_subtitle ──

    #[test]
    fn test_picture_subtitle_codecs_are_images() {
        // ffprobe's names and Jellyfin's, matched without case
        for codec in [
            "hdmv_pgs_subtitle",
            "PGSSUB",
            "dvd_subtitle",
            "DVDSUB",
            "vobsub",
            "dvb_subtitle",
            "DVBSUB",
            "xsub",
            "sup",
        ] {
            assert!(is_image_subtitle(codec), "{codec}");
        }
        for codec in [
            "subrip",
            "srt",
            "ass",
            "ssa",
            "webvtt",
            "vtt",
            "mov_text",
            "text",
            "dvb_teletext",
            "",
        ] {
            assert!(!is_image_subtitle(codec), "{codec}");
        }
    }

    // ── shift_vtt ──

    #[test]
    fn test_shift_vtt_without_a_start_returns_the_input() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000\nOne\n\n";
        assert_eq!(shift_vtt(vtt, 0), vtt);
        assert_eq!(shift_vtt(vtt, -10_000_000), vtt);
    }

    #[test]
    fn test_shift_vtt_drops_ended_cues_and_moves_the_rest() {
        let vtt = "WEBVTT\n\n\
                   00:00:01.000 --> 00:00:02.500\nOne\n\n\
                   00:00:09.000 --> 00:00:11.000\nTwo\n\n\
                   00:01:05.250 --> 00:01:07.000 align:start line:0\nThree\nlines\n\n\
                   01:00:00.000 --> 01:00:02.000\nFour\n\n";
        // 10 s: the first cue has ended, the second started before and is clamped at 0
        let shifted = shift_vtt(vtt, 100_000_000);
        assert_eq!(
            shifted,
            "WEBVTT\n\n\
             00:00:00.000 --> 00:00:01.000\nTwo\n\n\
             00:00:55.250 --> 00:00:57.000 align:start line:0\nThree\nlines\n\n\
             00:59:50.000 --> 00:59:52.000\nFour\n"
        );
    }

    #[test]
    fn test_shift_vtt_drops_a_cue_that_ends_at_the_start() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000\nOne\n\n00:02.000 --> 00:03.000\nTwo\n";
        assert_eq!(
            shift_vtt(vtt, 20_000_000),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nTwo\n"
        );
        // Ticks below a millisecond are dropped: 2.0009 s starts at 2.000 s
        assert_eq!(
            shift_vtt(vtt, 20_009_999),
            "WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nTwo\n"
        );
        // Every cue has ended: the header alone
        assert_eq!(shift_vtt(vtt, 30_000_000), "WEBVTT\n");
    }

    #[test]
    fn test_shift_vtt_keeps_the_header_notes_and_cue_ids() {
        // ffmpeg writes mm:ss.ttt below an hour; files may use CRLF
        let vtt = "WEBVTT\r\nKind: captions\r\n\r\n\
                   NOTE made by hand\r\n\r\n\
                   intro\r\n00:04.000 --> 00:06.000\r\n<i>Hello</i>\r\n\r\n\
                   12:34.567 --> 12:35.000\r\nLater\r\n";
        assert_eq!(
            shift_vtt(vtt, 30_000_000),
            "WEBVTT\nKind: captions\n\n\
             NOTE made by hand\n\n\
             intro\n00:00:01.000 --> 00:00:03.000\n<i>Hello</i>\n\n\
             00:12:31.567 --> 00:12:32.000\nLater\n"
        );
    }

    #[test]
    fn test_shift_vtt_writes_long_hours() {
        let vtt = "WEBVTT\n\n100:00:00.000 --> 100:00:01.000\nLong\n";
        assert_eq!(
            shift_vtt(vtt, 36_000_000_000),
            "WEBVTT\n\n99:00:00.000 --> 99:00:01.000\nLong\n"
        );
    }

    // ── strip_ass_blocks ──

    #[test]
    fn test_strip_ass_blocks_cuts_each_brace_run() {
        let vtt = "WEBVTT\n\n\
                   00:01.000 --> 00:02.000\n{=2}Doom\n\n\
                   00:03.000 --> 00:04.000\n{*\\c&H0000FF&}Red\n\n\
                   00:05.000 --> 00:06.000\nHello {note} world\n";
        assert_eq!(
            strip_ass_blocks(vtt),
            "WEBVTT\n\n\
             00:01.000 --> 00:02.000\nDoom\n\n\
             00:03.000 --> 00:04.000\nRed\n\n\
             00:05.000 --> 00:06.000\nHello  world\n"
        );
    }

    #[test]
    fn test_strip_ass_blocks_keeps_a_brace_with_no_close() {
        let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000\na { b\n";
        assert_eq!(strip_ass_blocks(vtt), vtt);
    }

    #[test]
    fn test_strip_ass_blocks_drops_a_cue_left_with_no_text() {
        let vtt = "WEBVTT\n\n\
                   00:01.000 --> 00:02.000\nOne\n\n\
                   00:02.000 --> 00:03.000\n{=5}\n\n\
                   00:03.000 --> 00:04.000\n{=5}\nTwo\n\n\
                   00:04.000 --> 00:05.000\nThree\n";
        assert_eq!(
            strip_ass_blocks(vtt),
            "WEBVTT\n\n\
             00:01.000 --> 00:02.000\nOne\n\n\
             00:03.000 --> 00:04.000\nTwo\n\n\
             00:04.000 --> 00:05.000\nThree\n"
        );
    }

    #[test]
    fn test_strip_ass_blocks_keeps_the_header_notes_ids_and_timings() {
        let vtt = "WEBVTT\r\nKind: captions\r\n\r\n\
                   STYLE\r\n::cue { color: red }\r\n\r\n\
                   NOTE {kept} by hand\r\n\r\n\
                   intro\r\n00:04.000 --> 00:06.000 line:90%\r\n{=2}Doom\r\n";
        assert_eq!(
            strip_ass_blocks(vtt),
            "WEBVTT\nKind: captions\n\n\
             STYLE\n::cue { color: red }\n\n\
             NOTE {kept} by hand\n\n\
             intro\n00:04.000 --> 00:06.000 line:90%\nDoom\n"
        );
    }
}
