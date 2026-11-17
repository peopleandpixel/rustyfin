//! Lyrics of audio items, as Jellyfin reads them (`MediaBrowser.Providers/Lyric`): a lyric file
//! beside the track (`.lrc`, `.elrc` or `.txt`, named like the track), or the track's lyrics tag.
//! An `.lrc` file is read as synced lyrics; a file it can't read, and a `.txt` file, as plain lines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rustyfin_db::repo_media::PathScope;
use rustyfin_model::lyrics::{LyricDto, LyricLine, LyricMetadata};
use sqlx::SqlitePool;
use uuid::Uuid;

/// The extensions of lyric files, synced ones first, as Jellyfin's parsers take them.
pub const LYRIC_EXTENSIONS: [&str; 3] = ["lrc", "elrc", "txt"];

const TICKS_PER_MS: i64 = 10_000;

/// The most timestamps one LRC line is read with; later ones are ignored.
pub const MAX_STAMPS_PER_LINE: usize = 100;
/// The most timed lines a file gives.
pub const MAX_TIMED_LINES: usize = 10_000;
/// The most text, in bytes, that the timed lines of a file hold together, so that a line repeated
/// by many stamps can't grow the lyrics much past the file's own size.
pub const MAX_TIMED_TEXT_BYTES: usize = 4 * 1024 * 1024;
/// How far after a `<` the `>` of a word stamp may be (`<mm:ss.xxx>` is at most a few digits).
const WORD_STAMP_MAX_LEN: usize = 16;

/// Parses the lyric file `name` (only its extension is read) with `content`, as Jellyfin's parsers
/// do: an `.lrc` or `.elrc` file as timed lines when it has any, else every file as plain lines.
/// `None` for another extension.
pub fn parse_file(name: &str, content: &str) -> Option<LyricDto> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    match ext.as_str() {
        "lrc" | "elrc" => Some(parse_lrc(content).unwrap_or_else(|| parse_txt(content))),
        "txt" => Some(parse_txt(content)),
        _ => None,
    }
}

/// Parses lyrics from a tag: timed lines when the tag is in LRC form, else plain lines.
pub fn parse_tag(content: &str) -> LyricDto {
    parse_lrc(content).unwrap_or_else(|| parse_txt(content))
}

/// Plain lyrics: one line per line of `content`, trimmed, with no start.
pub fn parse_txt(content: &str) -> LyricDto {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let lyrics = content
        .split("\r\n")
        .flat_map(|part| part.split(['\r', '\n']))
        .map(|line| LyricLine {
            text: line.trim().to_string(),
            start: None,
        })
        .collect();
    LyricDto {
        metadata: LyricMetadata {
            is_synced: Some(false),
            ..Default::default()
        },
        lyrics,
    }
}

/// Synced lyrics from LRC text: each `[mm:ss.xx]` stamp of a line starts that line, a line may
/// carry several stamps, and the lines are sorted by start. `[ar:…]`-style header tags fill the
/// metadata, and `[offset:±ms]` moves every start earlier by that many milliseconds. Word stamps
/// (`<mm:ss.xx>`, enhanced LRC) are left out of the text. `None` when no line has a stamp.
pub fn parse_lrc(content: &str) -> Option<LyricDto> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut metadata = LyricMetadata {
        is_synced: Some(true),
        ..Default::default()
    };
    let mut timed: Vec<(i64, String)> = Vec::new();
    let mut text_bytes = 0usize;
    'lines: for line in content.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some(tag_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=tag_end];
            rest = rest[tag_end + 2..].trim_start();
            if let Some(ms) = parse_time(tag) {
                if stamps.len() < MAX_STAMPS_PER_LINE {
                    stamps.push(ms);
                }
            } else if let Some((key, value)) = tag.split_once(':') {
                set_header(&mut metadata, key.trim(), value.trim());
            }
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_stamps(rest);
        for ms in stamps {
            if timed.len() >= MAX_TIMED_LINES || text_bytes + text.len() > MAX_TIMED_TEXT_BYTES {
                break 'lines;
            }
            text_bytes += text.len();
            timed.push((ms, text.clone()));
        }
    }
    if timed.is_empty() {
        return None;
    }
    let offset_ms = metadata.offset.map_or(0, |ticks| ticks / TICKS_PER_MS);
    timed.sort_by_key(|(ms, _)| *ms);
    let lyrics = timed
        .into_iter()
        .map(|(ms, text)| LyricLine {
            text,
            start: ms
                .saturating_sub(offset_ms)
                .max(0)
                .checked_mul(TICKS_PER_MS),
        })
        .collect();
    Some(LyricDto { metadata, lyrics })
}

fn set_header(metadata: &mut LyricMetadata, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    let text = || Some(value.to_string());
    match key.to_ascii_lowercase().as_str() {
        "ar" => metadata.artist = text(),
        "al" => metadata.album = text(),
        "ti" => metadata.title = text(),
        "au" => metadata.author = text(),
        "by" => metadata.by = text(),
        "re" => metadata.creator = text(),
        "ve" => metadata.version = text(),
        "length" => metadata.length = parse_time(value).map(|ms| ms * TICKS_PER_MS),
        "offset" => {
            metadata.offset = value
                .trim_start_matches('+')
                .parse::<i64>()
                .ok()
                .and_then(|ms| ms.checked_mul(TICKS_PER_MS))
        }
        _ => {}
    }
}

/// Milliseconds of an LRC time: `mm:ss`, `mm:ss.x` to `mm:ss.xxx`, or `mm:ss:xx`.
fn parse_time(tag: &str) -> Option<i64> {
    let (min, rest) = tag.trim().split_once(':')?;
    if min.is_empty() || !min.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((sec, frac)) => (sec, frac),
        None => (rest, ""),
    };
    if sec.is_empty()
        || !sec.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > 3
    {
        return None;
    }
    let min: i64 = min.parse().ok()?;
    let sec: i64 = sec.parse().ok()?;
    let frac_ms: i64 = frac.parse().unwrap_or(0) * 10_i64.pow(3 - frac.len() as u32);
    min.checked_mul(60)?
        .checked_add(sec)?
        .checked_mul(1000)?
        .checked_add(frac_ms)
        // A start in ticks must fit too.
        .filter(|ms| ms.checked_mul(TICKS_PER_MS).is_some())
}

/// The text without its word stamps. A `>` is looked for only a few bytes after each `<`, so a
/// line of `<` without `>` takes linear time.
fn strip_word_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let close = after
            .char_indices()
            .take(WORD_STAMP_MAX_LEN)
            .find(|(_, c)| *c == '>')
            .map(|(i, _)| i);
        match close {
            Some(close) if parse_time(&after[..close]).is_some() => {
                out.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The lyric files among `files` that belong to the track `audio`: in its folder, named like it
/// (`Song.lrc`, or `Song.en.lrc` with a language), with a lyric extension, compared without case.
/// Synced files come first, then by name.
pub fn sidecars_among(audio: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let Some(stem) = audio.file_stem().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let stem = stem.to_lowercase();
    let dir = audio.parent();
    let mut found: Vec<(usize, PathBuf)> = files
        .iter()
        .filter(|f| f.parent() == dir)
        .filter_map(|f| {
            let name = f.file_name()?.to_str()?.to_lowercase();
            let ext = f.extension()?.to_str()?.to_ascii_lowercase();
            let rank = LYRIC_EXTENSIONS.iter().position(|e| *e == ext)?;
            let base = name.strip_suffix(&format!(".{ext}"))?;
            let named_like = base == stem
                || base
                    .strip_prefix(&stem)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(is_language);
            named_like.then(|| (rank, f.clone()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, f)| f).collect()
}

/// Whether the middle part of a lyric file name is a language: 2 or 3 letters, optionally with a
/// region (`en`, `eng`, `pt-BR`, `zh_Hant`), so that `Song.live.lrc` is not `Song.flac`'s.
fn is_language(part: &str) -> bool {
    let (lang, region) = match part.split_once(['-', '_']) {
        Some((lang, region)) => (lang, Some(region)),
        None => (part, None),
    };
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_alphabetic())
        && region.is_none_or(|r| {
            (2..=4).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric())
        })
}

/// The lyric files beside the track at `audio`, as [`sidecars_among`] picks them. Reads its folder.
pub fn find_sidecars(audio: &Path) -> Vec<PathBuf> {
    let Some(dir) = audio.parent() else {
        return Vec::new();
    };
    sidecars_among(audio, &list_dir(dir))
}

fn list_dir(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// The lyrics in a file's tags, as ffprobe names them: `lyrics` (ID3 USLT, MP4 ©lyr, Vorbis
/// LYRICS), `lyrics-<lang>` (ID3 USLT with a language), or `unsyncedlyrics`. Empty values are
/// left out.
pub fn lyrics_tag(tags: &HashMap<String, String>) -> Option<String> {
    let mut keys: Vec<&String> = tags
        .keys()
        .filter(|k| {
            let k = k.to_ascii_lowercase();
            k == "lyrics" || k.starts_with("lyrics-") || k == "unsyncedlyrics"
        })
        .collect();
    keys.sort_by_key(|k| (!k.eq_ignore_ascii_case("lyrics"), k.to_ascii_lowercase()));
    keys.into_iter()
        .map(|k| tags[k].trim())
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

/// Reads the lyrics tag of the file at `path` with ffprobe.
pub async fn read_lyrics_tag(path: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Output {
        format: Option<Format>,
    }
    #[derive(serde::Deserialize)]
    struct Format {
        tags: Option<HashMap<String, String>>,
    }
    let run = tokio::process::Command::new(crate::media_tools::ffprobe())
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_entries",
            "format_tags",
            "-i",
            path,
        ])
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), run)
        .await
        .ok()?
        .ok()?;
    let parsed: Output = serde_json::from_slice(&output.stdout).ok()?;
    lyrics_tag(&parsed.format?.tags?)
}

/// The uploaded lyric files of `library`'s items, read before a removal for
/// [`remove_orphaned_uploads`]. Empty when the query fails, so nothing is deleted then.
pub async fn uploads_in_library(pool: &SqlitePool, library: Uuid) -> Vec<String> {
    rustyfin_db::repo_lyrics::uploads_in_library(pool, library)
        .await
        .unwrap_or_default()
}

/// Deletes the uploaded lyric files among `paths` that no item has any more: their items were
/// deleted (the `item_lyrics` rows go with them). A failed check keeps the file.
pub async fn remove_orphaned_uploads(pool: &SqlitePool, paths: &[String]) {
    for path in paths {
        if let Ok(false) = rustyfin_db::repo_lyrics::still_uploaded(pool, path).await
            && let Err(e) = tokio::fs::remove_file(path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to delete the lyric file '{}': {}", path, e);
        }
    }
}

/// Records the lyric file beside each audio item of `library` within `scope`, reading each folder
/// once, so that a lyric file added or removed beside an unchanged track shows in its HasLyrics.
pub async fn sync_sidecars(
    pool: &SqlitePool,
    library: Uuid,
    scope: &PathScope,
) -> Result<usize, sqlx::Error> {
    let tracks = rustyfin_db::repo_lyrics::audio_sidecars(pool, library, scope).await?;
    if tracks.is_empty() {
        return Ok(0);
    }
    let changes = tokio::task::spawn_blocking(move || {
        let dirs: HashSet<PathBuf> = tracks
            .values()
            .filter_map(|(path, _)| Path::new(path).parent().map(Path::to_path_buf))
            .collect();
        let listed: HashMap<PathBuf, Vec<PathBuf>> = dirs
            .into_iter()
            .map(|dir| {
                let files = list_dir(&dir);
                (dir, files)
            })
            .collect();
        let mut changes: Vec<(Uuid, Option<String>)> = Vec::new();
        for (id, (path, stored)) in tracks {
            let audio = Path::new(&path);
            let found = audio
                .parent()
                .and_then(|dir| listed.get(dir))
                .and_then(|files| sidecars_among(audio, files).into_iter().next())
                .map(|f| f.to_string_lossy().into_owned());
            if found != stored {
                changes.push((id, found));
            }
        }
        changes
    })
    .await
    .unwrap_or_default();
    rustyfin_db::repo_lyrics::set_sidecars(pool, &changes).await?;
    Ok(changes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, start: Option<i64>) -> LyricLine {
        LyricLine {
            text: text.to_string(),
            start,
        }
    }

    #[test]
    fn r3music2_lrc_lines_sorted_with_ticks_and_headers() {
        let dto = parse_lrc(
            "[ar:Tone Band]\n[ti:Sine]\n[length: 03:20]\n[00:12.50]Second\n[00:01.00][00:20.00]Chorus\n[00:05]Mid <00:05.50>word\nno stamp\n",
        )
        .unwrap();
        assert_eq!(
            dto.lyrics,
            vec![
                line("Chorus", Some(10_000_000)),
                line("Mid word", Some(50_000_000)),
                line("Second", Some(125_000_000)),
                line("Chorus", Some(200_000_000)),
            ]
        );
        assert_eq!(dto.metadata.artist.as_deref(), Some("Tone Band"));
        assert_eq!(dto.metadata.title.as_deref(), Some("Sine"));
        assert_eq!(dto.metadata.length, Some(2_000_000_000));
        assert_eq!(dto.metadata.is_synced, Some(true));
    }

    #[test]
    fn r3music2_lrc_offset_moves_lines_earlier() {
        let dto = parse_lrc("[offset:+500]\n[00:02.00]A\n[00:00.20]B").unwrap();
        assert_eq!(dto.lyrics[0], line("B", Some(0)));
        assert_eq!(dto.lyrics[1], line("A", Some(15_000_000)));
    }

    #[test]
    fn r3music2_unstamped_lrc_and_txt_are_plain_lines() {
        let dto = parse_file("song.LRC", "just words\r\nmore").unwrap();
        assert_eq!(
            dto.lyrics,
            vec![line("just words", None), line("more", None)]
        );
        assert_eq!(dto.metadata.is_synced, Some(false));
        assert!(parse_file("song.mp3", "x").is_none());
        let json = serde_json::to_value(parse_file("a.txt", " one ").unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"Metadata": {"IsSynced": false}, "Lyrics": [{"Text": "one"}]})
        );
    }

    #[test]
    fn r3music2_sidecars_match_the_track_name() {
        let dir = Path::new("/m/Album");
        let files: Vec<PathBuf> = [
            "01 Song.flac",
            "01 Song.txt",
            "01 song.en.LRC",
            "01 Song.lrc",
            "01 Songs.lrc",
            "01 Song.live.lrc",
            "01 Song.pt-BR.txt",
            "02 Other.lrc",
            "cover.jpg",
        ]
        .iter()
        .map(|f| dir.join(f))
        .collect();
        assert_eq!(
            sidecars_among(&dir.join("01 Song.flac"), &files),
            vec![
                dir.join("01 Song.lrc"),
                dir.join("01 song.en.LRC"),
                dir.join("01 Song.pt-BR.txt"),
                dir.join("01 Song.txt")
            ]
        );
    }

    #[test]
    fn r3music2_pathological_lrc_stays_bounded() {
        // ~100k stamps before a long text: the line is kept at most MAX_STAMPS_PER_LINE times.
        let text = "x".repeat(200_000);
        let content = format!("{}{text}\n", "[00:01.00]".repeat(100_000));
        let dto = parse_lrc(&content).unwrap();
        assert_eq!(
            dto.lyrics.len(),
            MAX_STAMPS_PER_LINE.min(MAX_TIMED_TEXT_BYTES / text.len())
        );
        // Many short stamped lines stop at MAX_TIMED_LINES.
        let many = "[00:01]a\n".repeat(MAX_TIMED_LINES + 50);
        assert_eq!(parse_lrc(&many).unwrap().lyrics.len(), MAX_TIMED_LINES);
        // Times and offsets too large for ticks are not read, nor do they overflow.
        let huge =
            parse_lrc("[offset:-99999999999999999]\n[99999999999999:00]big\n[00:01]ok").unwrap();
        assert_eq!(huge.lyrics, vec![line("ok", Some(10_000_000))]);
        assert_eq!(huge.metadata.offset, None);
    }

    #[test]
    fn r3music2_word_stamp_stripping_is_linear() {
        let start = std::time::Instant::now();
        let angles = "<".repeat(1_000_000);
        assert_eq!(strip_word_stamps(&angles).len(), 1_000_000);
        let then_close = format!("{}>", "<".repeat(1_000_000));
        assert_eq!(strip_word_stamps(&then_close).len(), 1_000_001);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(strip_word_stamps("a <b> <00:01.2>c <x"), "a <b> c <x");
    }

    #[test]
    fn r3music2_lyrics_tag_keys() {
        let tags: HashMap<String, String> = [
            ("title".to_string(), "x".to_string()),
            ("lyrics-eng".to_string(), "la la".to_string()),
        ]
        .into();
        assert_eq!(lyrics_tag(&tags).as_deref(), Some("la la"));
        let empty: HashMap<String, String> = [("LYRICS".to_string(), " ".to_string())].into();
        assert_eq!(lyrics_tag(&empty), None);
    }
}
