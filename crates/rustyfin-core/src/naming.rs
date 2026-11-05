//! Names the scanner reads: what it skips, episode numbers, and movie titles and version groups.
//!
//! The ignore rules follow Jellyfin's `IgnorePatterns.cs` (commit 06695fb4942c15c7463c2dc644440d988951ed17).
//! The functions are pure, with no I/O. Names are compared case-insensitively. The scanner applies
//! them only to entries below a library root, and only to the entry's own name (for a symlink,
//! the link's name).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Folder names that are skipped whole, in lower case.
const IGNORED_DIR_NAMES: &[&str] = &[
    "metadata",
    "ps3_update",
    "ps3_vprm",
    "extrafanart",
    "extrathumbs",
    "lost+found",
    "subs",
    "temprec",
    "tempsbe",
    "eadir",
    "#recycle",
    "$recycle.bin",
    "system volume information",
    "sample",
    "minta",
];

/// File names that are skipped whole, in lower case.
const IGNORED_FILE_NAMES: &[&str] = &["small.jpg", "albumart.jpg", "thumbs.db"];

/// File name endings that are skipped, in lower case.
const IGNORED_FILE_SUFFIXES: &[&str] = &[".bts", ".sync", ".trickplay"];

/// Whether the scanner must not enter the folder called `name`.
///
/// Skipped: names that start with `.` or `@`, names in `IGNORED_DIR_NAMES`, and names that end
/// with `.trickplay`.
pub fn is_ignored_dir(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with(['.', '@'])
        || IGNORED_DIR_NAMES.contains(&lower.as_str())
        || lower.ends_with(".trickplay")
}

/// Whether the scanner must not collect the file called `name`.
///
/// Skipped: names that start with `.`, `small.jpg`, `albumart.jpg` and `thumbs.db`, names that end
/// with `.bts`, `.sync` or `.trickplay`, and `sample.X`, `*.sample.X`, `minta.X` and `*.minta.X`,
/// where X is 1 to 5 characters with no dot.
pub fn is_ignored_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with('.')
        || IGNORED_FILE_NAMES.contains(&lower.as_str())
        || IGNORED_FILE_SUFFIXES
            .iter()
            .any(|&suffix| lower.ends_with(suffix))
        || is_sample_file(&lower, "sample")
        || is_sample_file(&lower, "minta")
}

/// Whether the lower-case file name is `word.X` or `*.word.X`, where X is as `is_extension` allows.
fn is_sample_file(lower: &str, word: &str) -> bool {
    // `word.X`: the whole name.
    let whole = lower
        .strip_prefix(word)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(is_extension);
    // `*.word.X`: X has no dot, so the last `.word.` is the one that starts X.
    let marker = format!(".{word}.");
    let suffixed = lower
        .rfind(marker.as_str())
        .is_some_and(|at| is_extension(&lower[at + marker.len()..]));
    whole || suffixed
}

/// Whether `ext` is 1 to 5 characters long and contains no dot.
fn is_extension(ext: &str) -> bool {
    (1..=5).contains(&ext.chars().count()) && !ext.contains('.')
}

/// The season and episode numbers that `parse_episode` reads from a file name.
///
/// `season` is `None` when the name gives only an episode. `ending` is the last episode of a
/// multi-episode file, such as 3 in `Show S01E02E03`, when it counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeNumbers {
    pub season: Option<i32>,
    pub episode: i32,
    pub ending: Option<i32>,
}

/// One episode rule: it reads the numbers from a stem's bytes, if the stem matches it.
type EpisodeRule = fn(&[u8]) -> Option<EpisodeNumbers>;

/// Reads the season and episode numbers from a file name stem. Returns `None` when no rule gives
/// a valid result.
///
/// The rules follow Jellyfin's `EpisodePathParser.cs` and `NamingOptions.cs` (commit
/// 06695fb4942c15c7463c2dc644440d988951ed17). Matching is ASCII and case-insensitive. The rules
/// are tried in order, and within a rule the leftmost match is its result:
///
/// 1. `S01E02`: `s`, digits, any run of `[ ] space . _ -`, `e`, digits. Season and episode.
/// 2. `ep05`: one of `. _ space -`, `ep`, an optional `_`, digits. Episode only.
/// 3. `1x02`: at the start or after one of `. _ space [ ( -`, 1 to 4 digits, `x`, 1 to 3 digits.
///    Season and episode.
/// 4. `Episode 7`: `episode`, optional spaces, digits, and optionally `-M` as the ending. Episode
///    only.
///
/// A rule whose season is invalid gives nothing (see `is_valid_season`). Rules 1 and 3 also read
/// a multi-episode ending (see `repeated_ending`), and rule 4 reads `-M`.
pub fn parse_episode(stem: &str) -> Option<EpisodeNumbers> {
    let bytes = stem.as_bytes();
    let rules: [EpisodeRule; 4] = [rule_season_episode, rule_ep, rule_nxnn, rule_episode_word];
    rules
        .into_iter()
        .find_map(|rule| rule(bytes).filter(|found| found.season.is_none_or(is_valid_season)))
}

/// Rule 1, `S01E02`: `s`, 1+ digits, any run of `[ ] space . _ -`, `e`, 1+ digits.
fn rule_season_episode(bytes: &[u8]) -> Option<EpisodeNumbers> {
    (0..bytes.len()).find_map(|at| {
        if !is_ci_at(bytes, at, b's') {
            return None;
        }
        let season_end = digits_end(bytes, at + 1);
        let season = parse_digits(&bytes[at + 1..season_end])?;
        let mut e_at = season_end;
        while bytes.get(e_at).is_some_and(|b| b"[] ._-".contains(b)) {
            e_at += 1;
        }
        if !is_ci_at(bytes, e_at, b'e') {
            return None;
        }
        let episode_end = digits_end(bytes, e_at + 1);
        let episode = parse_digits(&bytes[e_at + 1..episode_end])?;
        Some(EpisodeNumbers {
            season: Some(season),
            episode,
            ending: repeated_ending(bytes, episode_end, episode),
        })
    })
}

/// Rule 2, `ep05`: one of `. _ space -`, then `ep`, an optional `_`, then 1+ digits.
fn rule_ep(bytes: &[u8]) -> Option<EpisodeNumbers> {
    (0..bytes.len()).find_map(|at| {
        if !matches!(bytes[at], b'.' | b'_' | b' ' | b'-')
            || !(is_ci_at(bytes, at + 1, b'e') && is_ci_at(bytes, at + 2, b'p'))
        {
            return None;
        }
        let mut start = at + 3;
        if bytes.get(start) == Some(&b'_') {
            start += 1;
        }
        let episode = parse_digits(&bytes[start..digits_end(bytes, start)])?;
        Some(EpisodeNumbers {
            season: None,
            episode,
            ending: None,
        })
    })
}

/// Rule 3, `1x02`: at the start or after one of `. _ space [ ( -`, 1 to 4 digits, `x`, 1 to 3
/// digits. The digit runs are whole: `1x1080` is no match, since 1080 has four digits.
fn rule_nxnn(bytes: &[u8]) -> Option<EpisodeNumbers> {
    (0..bytes.len()).find_map(|at| {
        let starts_number = at == 0 || b"._ [(-".contains(&bytes[at - 1]);
        if !starts_number || !bytes[at].is_ascii_digit() {
            return None;
        }
        let season_end = digits_end(bytes, at);
        if !is_ci_at(bytes, season_end, b'x') || !(1..=4).contains(&(season_end - at)) {
            return None;
        }
        let episode_start = season_end + 1;
        let episode_end = digits_end(bytes, episode_start);
        if !(1..=3).contains(&(episode_end - episode_start)) {
            return None;
        }
        let season = parse_digits(&bytes[at..season_end])?;
        let episode = parse_digits(&bytes[episode_start..episode_end])?;
        Some(EpisodeNumbers {
            season: Some(season),
            episode,
            ending: repeated_ending(bytes, episode_end, episode),
        })
    })
}

/// Rule 4, `Episode 7`: `episode`, optional spaces, 1+ digits, and optionally `-M` as the ending.
fn rule_episode_word(bytes: &[u8]) -> Option<EpisodeNumbers> {
    (0..bytes.len()).find_map(|at| {
        if !bytes.get(at..at + 7)?.eq_ignore_ascii_case(b"episode") {
            return None;
        }
        let mut start = at + 7;
        while bytes.get(start) == Some(&b' ') {
            start += 1;
        }
        let end = digits_end(bytes, start);
        let episode = parse_digits(&bytes[start..end])?;
        let ending = if bytes.get(end) == Some(&b'-') {
            let ending_end = digits_end(bytes, end + 1);
            parse_digits(&bytes[end + 1..ending_end])
                .filter(|&number| ending_counts(bytes, ending_end, number, episode))
        } else {
            None
        };
        Some(EpisodeNumbers {
            season: None,
            episode,
            ending,
        })
    })
}

/// Whether a season can be one. Jellyfin rejects 200 to 1927, which read as years in names such as
/// `1920x1080`, and anything above 2500 (`EpisodePathParser.cs` 188-192).
fn is_valid_season(season: i32) -> bool {
    !(200..1928).contains(&season) && season <= 2500
}

/// The multi-episode ending after the episode digits at `pos` (rules 1 and 3), if it counts.
///
/// The forms repeat, and the last one matched gives the number, so `S01E01E02E03` ends at 3. The
/// check in `ending_counts` applies to that last form only.
fn repeated_ending(bytes: &[u8], mut pos: usize, episode: i32) -> Option<i32> {
    let mut last = None;
    while let Some((number, end)) = ending_form(bytes, pos) {
        last = Some((number, end));
        pos = end;
    }
    let (number, end) = last?;
    ending_counts(bytes, end, number, episode).then_some(number)
}

/// One ending form at `pos`, as its number and the index after its digits.
fn ending_form(bytes: &[u8], pos: usize) -> Option<(i32, usize)> {
    // Form (c) is tried first, as the first multi-episode expressions in Jellyfin's
    // `NamingOptions.cs` (773-776) are. So `-1234e05` gives 5, where form (b) would stop at `-123`.
    form_c(bytes, pos)
        .or_else(|| form_a(bytes, pos))
        .or_else(|| form_b(bytes, pos))
}

/// Form (a): an optional `-` or ` - `, then `x` or `e`, then 1 to 3 digits.
fn form_a(bytes: &[u8], pos: usize) -> Option<(i32, usize)> {
    let x_at = dash_end(bytes, pos).unwrap_or(pos);
    if !(is_ci_at(bytes, x_at, b'x') || is_ci_at(bytes, x_at, b'e')) {
        return None;
    }
    ending_digits(bytes, x_at + 1)
}

/// Form (b): `-`, an optional `x`, an optional `e`, then 1 to 3 digits.
fn form_b(bytes: &[u8], pos: usize) -> Option<(i32, usize)> {
    if bytes.get(pos) != Some(&b'-') {
        return None;
    }
    let mut at = pos + 1;
    if is_ci_at(bytes, at, b'x') {
        at += 1;
    }
    if is_ci_at(bytes, at, b'e') {
        at += 1;
    }
    ending_digits(bytes, at)
}

/// Form (c): `-` or ` - `, then 1 to 4 digits, then `e`, `x` or `xe`, then 1 to 3 digits.
fn form_c(bytes: &[u8], pos: usize) -> Option<(i32, usize)> {
    let start = dash_end(bytes, pos)?;
    let digits = digits_end(bytes, start);
    if !(1..=4).contains(&(digits - start)) {
        return None;
    }
    let at = if is_ci_at(bytes, digits, b'x') && is_ci_at(bytes, digits + 1, b'e') {
        digits + 2
    } else if is_ci_at(bytes, digits, b'x') || is_ci_at(bytes, digits, b'e') {
        digits + 1
    } else {
        return None;
    };
    ending_digits(bytes, at)
}

/// Up to 3 digits from `from`, as their number and the index after them. A longer run is cut after
/// 3 here, and `ending_counts` then rejects the ending, since a digit follows.
fn ending_digits(bytes: &[u8], from: usize) -> Option<(i32, usize)> {
    let end = digits_end(bytes, from).min(from + 3);
    Some((parse_digits(&bytes[from..end])?, end))
}

/// Whether an ending counts (`EpisodePathParser.cs` 151-167). The character at `end`, just after
/// its digits, must not be a digit or one of `iIpP` (as in `-1080p`), and the number must be at
/// least the episode.
fn ending_counts(bytes: &[u8], end: usize, number: i32, episode: i32) -> bool {
    let continues = bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_digit() || b"iIpP".contains(b));
    !continues && number >= episode
}

/// The index after a `-` or ` - ` at `pos`, if there is one.
fn dash_end(bytes: &[u8], pos: usize) -> Option<usize> {
    if bytes
        .get(pos..)
        .is_some_and(|rest| rest.starts_with(b" - "))
    {
        Some(pos + 3)
    } else if bytes.get(pos) == Some(&b'-') {
        Some(pos + 1)
    } else {
        None
    }
}

/// The index just past the run of ASCII digits that starts at `from`.
fn digits_end(bytes: &[u8], mut from: usize) -> usize {
    while bytes.get(from).is_some_and(u8::is_ascii_digit) {
        from += 1;
    }
    from
}

/// The number that `digits` spells, or `None` if it is empty or too long for an `i32`.
fn parse_digits(digits: &[u8]) -> Option<i32> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Whether the byte at `at` is `c`, ignoring ASCII case. Non-ASCII bytes never match.
fn is_ci_at(bytes: &[u8], at: usize, c: u8) -> bool {
    bytes.get(at).is_some_and(|b| b.eq_ignore_ascii_case(&c))
}

/// A title and year read from a movie file name, as Jellyfin's `CleanDateTime` and `CleanString` leave them
/// (`NamingOptions.cs:146-160`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanName {
    pub title: String,
    pub year: Option<i32>,
}

/// The files that are versions of one movie. `files[0]` is the primary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionGroup {
    pub title: String,
    pub year: Option<i32>,
    pub files: Vec<PathBuf>,
}

/// Separators that can sit directly before a year (`NamingOptions.cs:148`, rule A).
const YEAR_SEP: &[char] = &['_', '.', '(', ')', '[', ']', '-'];

/// Separators that can run before a year (`NamingOptions.cs:149`, rule B).
const YEAR_RUN_SEP: &[char] = &[' ', '_', '.', '(', ')', '[', ']', '-'];

/// Characters that a title cannot end with before a year (`NamingOptions.cs:148-149`).
const NOT_TITLE_END: &[char] = &['_', ',', '.', '(', ')', '[', ']', '-'];

/// Characters on either side of a tag (`NamingOptions.cs:154`).
const TAG_SEP: &[char] = &[' ', '_', ',', '.', '(', ')', '[', ']', '-'];

/// The tags of `CleanString` expression 1 (`NamingOptions.cs:154`), in lower case. In a tag `.` matches any
/// character and `#` a digit from 1 to 9, for the regex's `read.nfo`, `www.www` and `cd[1-9]`.
const TAGS: &[&str] = &[
    "3d",
    "sbs",
    "tab",
    "hsbs",
    "htab",
    "mvc",
    "hdr",
    "hdc",
    "uhd",
    "ultrahd",
    "4k",
    "ac3",
    "dts",
    "custom",
    "dc",
    "divx",
    "divx5",
    "dsr",
    "dsrip",
    "dutch",
    "dvd",
    "dvdrip",
    "dvdscr",
    "dvdscreener",
    "screener",
    "dvdivx",
    "cam",
    "fragment",
    "fs",
    "hdtv",
    "hdrip",
    "hdtvrip",
    "internal",
    "limited",
    "multi",
    "subs",
    "ntsc",
    "ogg",
    "ogm",
    "pal",
    "pdtv",
    "proper",
    "repack",
    "rerip",
    "retail",
    "cd#",
    "r5",
    "bd5",
    "bd",
    "se",
    "svcd",
    "swedish",
    "german",
    "read.nfo",
    "nfofix",
    "unrated",
    "ws",
    "web-dl",
    "telesync",
    "ts",
    "telecine",
    "tc",
    "brrip",
    "bdrip",
    "480p",
    "480i",
    "576p",
    "576i",
    "720p",
    "720i",
    "1080p",
    "1080i",
    "2160p",
    "hrhd",
    "hrhdtv",
    "hddvd",
    "bluray",
    "blu-ray",
    "x264",
    "x265",
    "h264",
    "h265",
    "xvid",
    "xvidvd",
    "xxx",
    "www.www",
    "aac",
];

/// Whether `c` is a word character, as `\w` matches: a letter, a digit or `_`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The index of the first character at or after `at` that is not whitespace.
fn skip_ws(s: &[char], at: usize) -> usize {
    at + s.iter().skip(at).take_while(|c| c.is_whitespace()).count()
}

/// The lengths of leading whitespace that `^\s*` can take, longest first, because a regex backs off and tries
/// shorter ones.
fn lead_offsets(s: &[char]) -> impl Iterator<Item = usize> {
    (0..=skip_ws(s, 0)).rev()
}

/// The year that starts at `at` if it is `19dd` or `20dd` and the lookahead of `CleanDateTime` allows it: no digit
/// may follow, and no `\W dd \W dd` date may follow.
fn year_at(s: &[char], at: usize) -> Option<i32> {
    let digits = s.get(at..at + 4)?;
    if !matches!(digits[..2], ['1', '9'] | ['2', '0'])
        || !digits[2..].iter().all(char::is_ascii_digit)
    {
        return None;
    }
    let rest = &s[at + 4..];
    let date_follows = rest.len() >= 6
        && !is_word(rest[0])
        && rest[1..3].iter().all(char::is_ascii_digit)
        && !is_word(rest[3])
        && rest[4..6].iter().all(char::is_ascii_digit);
    if rest.first().is_some_and(char::is_ascii_digit) || date_follows {
        return None;
    }
    digits.iter().collect::<String>().parse().ok()
}

/// Where the title ends and the year starts, as Jellyfin's `CleanDateTime` finds it (`NamingOptions.cs:148-149`).
/// Rule A, one separator before the year, is tried at every position before rule B, a run of separators.
/// Positions are tried from the longest title down.
fn split_year(s: &[char]) -> Option<(usize, i32)> {
    let rule_a = (2..s.len()).rev().find_map(|e| {
        if YEAR_SEP.contains(&s[e]) && !NOT_TITLE_END.contains(&s[e - 1]) {
            year_at(s, e + 1).map(|year| (e, year))
        } else {
            None
        }
    });
    rule_a.or_else(|| {
        (2..s.len()).rev().find_map(|e| {
            let run = s[e..]
                .iter()
                .take_while(|&&c| YEAR_RUN_SEP.contains(&c))
                .count();
            if run > 0 && !NOT_TITLE_END.contains(&s[e - 1]) {
                year_at(s, e + run).map(|year| (e, year))
            } else {
                None
            }
        })
    })
}

/// Whether `c` matches the tag character `p`: `.` matches any character, `#` a digit from 1 to 9, and a letter
/// matches either case.
fn tag_char(c: char, p: char) -> bool {
    match p {
        '.' => true,
        '#' => ('1'..='9').contains(&c),
        _ => c.eq_ignore_ascii_case(&p),
    }
}

/// Whether a tag of `TAGS` starts `rest` and ends at a separator or at the end.
fn tag_at(rest: &[char]) -> bool {
    TAGS.iter().any(|tag| {
        let len = tag.chars().count();
        rest.len() >= len
            && tag.chars().zip(rest).all(|(p, &c)| tag_char(c, p))
            && rest.get(len).is_none_or(|c| TAG_SEP.contains(c))
    })
}

/// The index just past the `]` that closes the `[` at `at`, when the group has something inside.
fn bracket_end(t: &[char], at: usize) -> Option<usize> {
    if t.get(at) != Some(&'[') {
        return None;
    }
    let close = t.get(at + 1..)?.iter().position(|&c| c == ']')?;
    (close > 0).then_some(at + close + 2)
}

/// Whether `t` is one or more `[...]` groups, each with spaces around it, then an optional `.ext` and the end.
fn bracket_tail(t: &[char]) -> bool {
    let mut at = 0;
    let mut groups = 0;
    while let Some(end) = bracket_end(t, skip_ws(t, at)) {
        groups += 1;
        at = skip_ws(t, end);
    }
    let rest = &t[at..];
    let ext = rest.first() == Some(&'.')
        && rest.len() > 1
        && !rest[1..].iter().any(|c| c.is_whitespace());
    groups > 0 && (rest.is_empty() || ext)
}

/// Whether `t` is `.` and then word characters to the end, the `\.\w+$` that expression 4 looks ahead for.
fn is_dot_word_end(t: &[char]) -> bool {
    t.first() == Some(&'.') && t.len() > 1 && t[1..].iter().all(|&c| is_word(c))
}

/// Whether `t` is spaces, `-`, spaces, digits and then spaces to the end (`NamingOptions.cs:158`).
fn number_tail(t: &[char]) -> bool {
    let dash = skip_ws(t, 0);
    if dash == 0 || t.get(dash) != Some(&'-') {
        return false;
    }
    let digits = skip_ws(t, dash + 1);
    if digits == dash + 1 {
        return false;
    }
    let end = digits
        + t.iter()
            .skip(digits)
            .take_while(|c| c.is_ascii_digit())
            .count();
    end > digits && skip_ws(t, end) == t.len()
}

/// `CleanString` expression 1 (`NamingOptions.cs:154`): the title before the first separator that starts a tag.
fn strip_tags(s: &[char]) -> Option<&[char]> {
    lead_offsets(s).find_map(|lead| {
        let rest = &s[lead..];
        (1..rest.len())
            .find(|&q| TAG_SEP.contains(&rest[q]) && tag_at(&rest[q + 1..]))
            .map(|q| &rest[..q])
    })
}

/// `CleanString` expression 2 (`NamingOptions.cs:155`): the title before trailing `[...]` groups.
fn strip_bracket_tail(s: &[char]) -> Option<&[char]> {
    lead_offsets(s).find_map(|lead| {
        let rest = &s[lead..];
        (1..rest.len())
            .find(|&q| bracket_tail(&rest[q..]))
            .map(|q| &rest[..q])
    })
}

/// `CleanString` expression 4 (`NamingOptions.cs:157`): what follows a leading `[...]` group, unless only `.ext` does.
fn strip_leading_bracket(s: &[char]) -> Option<&[char]> {
    lead_offsets(s).find_map(|lead| {
        let rest = &s[lead..];
        let after = &rest[bracket_end(rest, 0)?..];
        if is_dot_word_end(after) {
            return None;
        }
        let spaces = skip_ws(after, 0);
        if spaces < after.len() {
            Some(&after[spaces..])
        } else {
            // All spaces: `\s*` gives one back so that `.+` has a character to take.
            after.len().checked_sub(1).map(|last| &after[last..])
        }
    })
}

/// `CleanString` expression 5 (`NamingOptions.cs:158`): the title before a trailing ` - N`.
fn strip_number_tail(s: &[char]) -> Option<&[char]> {
    lead_offsets(s).find_map(|lead| {
        let rest = &s[lead..];
        (1..rest.len())
            .find(|&q| number_tail(&rest[q..]))
            .map(|q| &rest[..q])
    })
}

/// Tidies a title: removes empty brackets and `(0)`, turns dots and underscores into spaces when there is no space,
/// collapses whitespace, and drops trailing whitespace, `-`, `_` and `,`.
fn tidy(title: &str) -> String {
    let mut out = title.replace("()", "").replace("[]", "").replace("(0)", "");
    if !out.contains(' ') {
        out = out.replace(['.', '_'], " ");
    }
    let words: Vec<&str> = out.split_whitespace().collect();
    words
        .join(" ")
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | ','))
        .to_string()
}

/// One `CleanString` expression: takes the title and returns the part it keeps, or `None` when it does not match.
type Strip = fn(&[char]) -> Option<&[char]>;

/// Reads a movie title and year from a file name stem (`NamingOptions.cs:146-160`): the year first, then the four
/// tag expressions, then the tidying. An empty result falls back to the trimmed stem.
pub fn clean_name(stem: &str) -> CleanName {
    let chars: Vec<char> = stem.chars().collect();
    let (mut title, year) = match split_year(&chars) {
        Some((end, year)) => (
            chars[..end]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string(),
            Some(year),
        ),
        None => (stem.to_string(), None),
    };
    let strips: [Strip; 4] = [
        strip_tags,
        strip_bracket_tail,
        strip_leading_bracket,
        strip_number_tail,
    ];
    for strip in strips {
        let current: Vec<char> = title.chars().collect();
        if let Some(cleaned) = strip(&current) {
            title = cleaned.iter().collect::<String>().trim().to_string();
        }
    }
    let title = tidy(&title);
    if title.is_empty() {
        CleanName {
            title: stem.trim().to_string(),
            year,
        }
    } else {
        CleanName { title, year }
    }
}

/// The key that titles share when they differ only in case, spacing and punctuation: the lower-case letters and
/// digits of the title.
pub fn title_key(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The number in the leftmost run of three or more ASCII digits that is followed by `i` or `p`, either case
/// (`VideoListResolver.cs:35`). `None` if there is no such run, or the number does not fit a `u64`.
pub fn resolution(stem: &str) -> Option<u64> {
    let bytes = stem.as_bytes();
    let mut start = 0;
    while let Some(offset) = bytes[start..].iter().position(u8::is_ascii_digit) {
        let from = start + offset;
        let end = digits_end(bytes, from);
        let ip_follows = bytes
            .get(end)
            .is_some_and(|b| matches!(b.to_ascii_lowercase(), b'i' | b'p'));
        if end - from >= 3 && ip_follows {
            return std::str::from_utf8(&bytes[from..end]).ok()?.parse().ok();
        }
        start = end;
    }
    None
}

/// A file being grouped, with what grouping reads from its name.
struct Entry {
    path: PathBuf,
    stem: String,
    name: String,
    clean: CleanName,
    resolution: Option<u64>,
}

impl Entry {
    fn new(path: &Path) -> Self {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            clean: clean_name(&stem),
            resolution: resolution(&stem),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_path_buf(),
            stem,
        }
    }
}

/// Higher resolution first, files without one last, then by file name in byte order (`VideoListResolver.cs:283-300`).
fn version_order(a: &Entry, b: &Entry) -> Ordering {
    b.resolution
        .cmp(&a.resolution)
        .then_with(|| a.name.cmp(&b.name))
}

/// Sorts the versions of one title, then moves the file named like the folder to the front (`VideoListResolver.cs:150`).
fn sort_versions(entries: &mut [Entry], folder_name: Option<&str>) {
    entries.sort_by(version_order);
    if let Some(at) = entries
        .iter()
        .position(|e| Some(e.stem.as_str()) == folder_name)
    {
        entries[..=at].rotate_right(1);
    }
}

/// Groups the video files directly in `folder` into versions of one movie, each with a title and a year. In a
/// library root (`is_library_root`) the folder's own name is never read.
pub fn group_movie_files(
    folder: &Path,
    files: &[PathBuf],
    is_library_root: bool,
) -> Vec<VersionGroup> {
    let folder_name = if is_library_root {
        None
    } else {
        folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    let folder_clean = folder_name.as_deref().map(clean_name);
    let folder_year = folder_clean.as_ref().and_then(|fc| fc.year);

    let mut buckets: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for path in files {
        let entry = Entry::new(path);
        buckets
            .entry(title_key(&entry.clean.title))
            .or_default()
            .push(entry);
    }

    let mut groups: Vec<(Vec<Entry>, Option<i32>)> = Vec::new();
    for bucket in buckets.into_values() {
        let years: BTreeSet<i32> = bucket.iter().filter_map(|e| e.clean.year).collect();
        if years.len() <= 1 {
            groups.push((bucket, years.first().copied()));
        } else {
            // A yearless file joins the folder's year when that year has a group, else the yearless files form one.
            let home = folder_year.filter(|year| years.contains(year));
            let mut by_year: BTreeMap<Option<i32>, Vec<Entry>> = BTreeMap::new();
            for entry in bucket {
                by_year
                    .entry(entry.clean.year.or(home))
                    .or_default()
                    .push(entry);
            }
            groups.extend(by_year.into_iter().map(|(year, members)| (members, year)));
        }
    }
    for (members, _) in &mut groups {
        sort_versions(members, folder_name.as_deref());
    }

    let single = groups.len() == 1;
    let fc = folder_clean.as_ref();
    let mut result: Vec<VersionGroup> = groups
        .into_iter()
        .map(|(members, group_year)| {
            let primary = &members[0].clean.title;
            let keys_match = fc.is_some_and(|f| title_key(&f.title) == title_key(primary));
            let (title, year) = match fc {
                Some(f) if single && (f.year.is_some() || keys_match) => {
                    (f.title.clone(), f.year.or(group_year))
                }
                _ => (
                    primary.clone(),
                    group_year.or(fc.and_then(|f| f.year).filter(|_| keys_match)),
                ),
            };
            VersionGroup {
                title,
                year,
                files: members.into_iter().map(|e| e.path).collect(),
            }
        })
        .collect();
    result.sort_by_key(|group| group.files[0].clone());
    result
}

/// Groups one season's files into versions of one episode, keyed by the season (the file's own, else the folder's)
/// and the episode number. A file with no episode number in its name forms a group of its own.
pub fn group_episode_files(files: &[PathBuf], folder_season: Option<i32>) -> Vec<Vec<PathBuf>> {
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    let mut by_episode: BTreeMap<(Option<i32>, i32), Vec<Entry>> = BTreeMap::new();
    for path in files {
        let entry = Entry::new(path);
        let ep = parse_episode(&entry.stem);
        match ep {
            Some(ep) => by_episode
                .entry((ep.season.or(folder_season), ep.episode))
                .or_default()
                .push(entry),
            None => groups.push(vec![entry]),
        }
    }
    groups.extend(by_episode.into_values());
    let mut out: Vec<Vec<PathBuf>> = groups
        .into_iter()
        .map(|mut group| {
            sort_versions(&mut group, None);
            group.into_iter().map(|e| e.path).collect()
        })
        .collect();
    out.sort_by_key(|group| group[0].clone());
    out
}
