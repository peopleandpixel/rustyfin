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
    for line in content.lines() {
        let mut rest = line.trim();
        let mut stamps = Vec::new();
        while let Some(tag_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=tag_end];
            rest = rest[tag_end + 2..].trim_start();
            if let Some(ms) = parse_time(tag) {
                stamps.push(ms);
            } else if let Some((key, value)) = tag.split_once(':') {
                set_header(&mut metadata, key.trim(), value.trim());
            }
        }
        if stamps.is_empty() {
            continue;
        }
        let text = strip_word_stamps(rest);
        for ms in stamps {
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
            start: Some((ms - offset_ms).max(0) * TICKS_PER_MS),
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
                .map(|ms| ms * TICKS_PER_MS)
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
    let frac_ms = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().ok()? * 100,
        2 => frac.parse::<i64>().ok()? * 10,
        _ => frac.parse::<i64>().ok()?,
    };
    Some((min * 60 + sec) * 1000 + frac_ms)
}

fn strip_word_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(close) if parse_time(&rest[open + 1..open + close]).is_some() => {
                out.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
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
                    .is_some_and(|rest| rest.starts_with('.'));
            named_like.then(|| (rank, f.clone()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, f)| f).collect()
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
    keys.sort_by_key(|k| (k.to_ascii_lowercase() != "lyrics", k.to_ascii_lowercase()));
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
    let run = tokio::process::Command::new("ffprobe")
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
                dir.join("01 Song.txt")
            ]
        );
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
