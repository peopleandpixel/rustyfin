//! Track choices (spec 2.16, 2.30): which Rustyfin stream a Jellyfin track choice means.

use rustyfin_db::repo_jellyfin_import::{SourceStreams, StreamRow};

use super::source::{JfStream, StreamKind};

/// A codec name as both sides compare it (spec 2.16): lowercase, with the spellings Jellyfin and
/// ffmpeg use for one codec made the same.
pub fn normalize_codec(codec: &str) -> String {
    let lower = codec.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "pgssub" => Some("hdmv_pgs_subtitle"),
        "dvdsub" => Some("dvd_subtitle"),
        "dvbsub" => Some("dvb_subtitle"),
        "srt" => Some("subrip"),
        _ => None,
    };
    alias.map_or(lower, str::to_string)
}

/// The source a Jellyfin item's track choices refer to (spec 2.16): the one at `path` (the
/// translated Jellyfin path). None when no source is at that path; a choice never falls back to
/// another source.
pub fn choose_source<'a>(
    sources: &'a [SourceStreams],
    path: Option<&str>,
) -> Option<&'a SourceStreams> {
    path.and_then(|path| sources.iter().find(|source| source.path.as_str() == path))
}

/// The Rustyfin stream index that the Jellyfin track `choice` of `kind` maps to (spec 2.16, 2.30).
/// Candidates share the kind, the external flag, the codec, the flags, and the language and title
/// where both sides know them; an external stream also needs the same file name. One candidate is
/// the answer. Several are told apart by ordinal among the embedded streams, and an external stream
/// with several has no answer.
pub fn map_track(choice: i64, kind: StreamKind, jf: &[JfStream], rf: &[StreamRow]) -> Option<i64> {
    let wanted = jf
        .iter()
        .find(|stream| stream.index == choice && stream.kind == kind)?;
    let candidates: Vec<&StreamRow> = rf.iter().filter(|row| is_candidate(wanted, row)).collect();
    match candidates.as_slice() {
        [] => None,
        [only] => Some(only.index),
        _ if wanted.is_external => None,
        _ => {
            let ordinal = embedded_jf(jf, kind)
                .iter()
                .position(|&index| index == wanted.index)?;
            let pick = embedded_rf(rf, kind).get(ordinal)?.index;
            candidates
                .iter()
                .any(|row| row.index == pick)
                .then_some(pick)
        }
    }
}

/// Whether a Rustyfin stream can be the one a Jellyfin stream maps to (spec 2.30).
fn is_candidate(jf: &JfStream, row: &StreamRow) -> bool {
    row.codec_type.eq_ignore_ascii_case(kind_name(jf.kind))
        && row.is_external == jf.is_external
        && same_codec(jf.codec.as_deref(), row.codec.as_deref())
        && row.is_forced == jf.is_forced
        && row.is_default == jf.is_default
        && agrees(
            known_language(jf.language.as_deref()),
            known_language(row.language.as_deref()),
        )
        && agrees(
            known_title(jf.title.as_deref()),
            known_title(row.title.as_deref()),
        )
        && (!jf.is_external || same_file(jf.path.as_deref(), row.path.as_deref()))
}

/// The `codec_type` a stream of `kind` has in Rustyfin's rows.
fn kind_name(kind: StreamKind) -> &'static str {
    match kind {
        StreamKind::Audio => "audio",
        StreamKind::Video => "video",
        StreamKind::Subtitle => "subtitle",
        StreamKind::Other => "other",
    }
}

/// Whether two codecs are the same: both missing, or equal once normalised.
fn same_codec(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => normalize_codec(a) == normalize_codec(b),
        (None, None) => true,
        _ => false,
    }
}

/// A language is known unless it is missing, empty or `und`.
fn known_language(language: Option<&str>) -> Option<&str> {
    language.filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case("und"))
}

/// A title is known unless it is missing or empty.
fn known_title(title: Option<&str>) -> Option<&str> {
    title.filter(|text| !text.is_empty())
}

/// Whether two values agree, compared case-insensitively. An unknown value agrees with anything.
fn agrees(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => true,
    }
}

/// Whether two paths end in the same file name, compared exactly. Two missing paths agree.
fn same_file(a: Option<&str>, b: Option<&str>) -> bool {
    a.map(file_name) == b.map(file_name)
}

/// The last `/` or `\` component of a path.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Jellyfin's embedded streams of `kind`, by index.
fn embedded_jf(jf: &[JfStream], kind: StreamKind) -> Vec<i64> {
    let mut indexes: Vec<i64> = jf
        .iter()
        .filter(|stream| !stream.is_external && stream.kind == kind)
        .map(|stream| stream.index)
        .collect();
    indexes.sort_unstable();
    indexes
}

/// Rustyfin's embedded streams of `kind`, by index.
fn embedded_rf(rf: &[StreamRow], kind: StreamKind) -> Vec<&StreamRow> {
    let mut streams: Vec<&StreamRow> = rf
        .iter()
        .filter(|row| !row.is_external && row.codec_type.eq_ignore_ascii_case(kind_name(kind)))
        .collect();
    streams.sort_by_key(|row| row.index);
    streams
}
