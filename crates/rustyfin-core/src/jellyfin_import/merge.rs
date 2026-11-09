//! Merging Jellyfin's watch data with Rustyfin's rows (spec 2.12-2.14, 2.29).

use chrono::{DateTime, Utc};
use rustyfin_db::repo_jellyfin_import::UserDataRow;

use super::source::JfUserData;

/// The Rustyfin track indexes a Jellyfin row's track choices map to (spec 2.16), each with the key
/// of the stream it names (see `StreamIdentity`). `None` where a choice has no counterpart. A
/// subtitle of `Some(-1)` means subtitles off, which names no stream and so has no key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MappedTracks {
    pub audio: Option<i64>,
    pub audio_key: Option<String>,
    pub subtitle: Option<i64>,
    pub subtitle_key: Option<String>,
}

/// A Jellyfin rating as Rustyfin's whole rating from 0 to 10 (spec 2.13): rounded half away from
/// zero and clamped. NaN has no rating.
pub fn convert_rating(rating: f64) -> Option<i64> {
    if rating.is_nan() {
        None
    } else {
        Some(rating.round().clamp(0.0, 10.0) as i64)
    }
}

/// Reduces Jellyfin's rows to one per user and item (spec 2.12). The rows are sorted by user, then
/// item. In each group the first row that no later row beats is kept: a later date beats an earlier
/// one (no date is the oldest), then more plays, then a larger position.
pub fn harmonize(mut rows: Vec<JfUserData>) -> Vec<JfUserData> {
    rows.sort_by_key(|row| (row.user, row.item));
    let mut kept: Vec<JfUserData> = Vec::with_capacity(rows.len());
    for row in rows {
        match kept.last_mut() {
            Some(last) if last.user == row.user && last.item == row.item => {
                if progress(&row) > progress(last) {
                    *last = row;
                }
            }
            _ => kept.push(row),
        }
    }
    kept
}

/// How far along a row is: its date (no date is oldest), its plays, then its position.
fn progress(row: &JfUserData) -> (Option<DateTime<Utc>>, i64, i64) {
    (row.last_played, row.play_count, row.position_ticks)
}

/// Merges a Jellyfin row into the Rustyfin row of the same user and item, if there is one (spec
/// 2.14). Played and favorite are either side's; the play count is the larger; the date is the
/// later. A rating and likes Rustyfin has are kept, else Jellyfin's. The position and the mapped
/// tracks come from Jellyfin only when its row is newer: when there is no Rustyfin row, when
/// Rustyfin's row has no date and Jellyfin's has one, or when Jellyfin's date is later. A track's
/// index and key move together: a track Jellyfin maps to nothing keeps Rustyfin's pair.
pub fn merge(existing: Option<&UserDataRow>, jf: &JfUserData, mapped: MappedTracks) -> UserDataRow {
    let stored = existing.cloned().unwrap_or_default();
    // `Option` orders None below Some, so this is also true for a date where Rustyfin has none.
    let jellyfin_newer = existing.is_none() || jf.last_played > stored.last_played;
    let (audio, audio_key) = match mapped.audio {
        Some(index) if jellyfin_newer => (Some(index), mapped.audio_key),
        _ => (stored.audio, stored.audio_key),
    };
    let (subtitle, subtitle_key) = match mapped.subtitle {
        Some(index) if jellyfin_newer => (Some(index), mapped.subtitle_key),
        _ => (stored.subtitle, stored.subtitle_key),
    };
    UserDataRow {
        played: stored.played || jf.played,
        play_count: stored.play_count.max(jf.play_count),
        position_ticks: if jellyfin_newer {
            jf.position_ticks
        } else {
            stored.position_ticks
        },
        last_played: stored.last_played.max(jf.last_played),
        is_favorite: stored.is_favorite || jf.is_favorite,
        user_rating: stored
            .user_rating
            .or_else(|| jf.rating.and_then(convert_rating)),
        likes: stored.likes.or(jf.likes),
        audio,
        audio_key,
        subtitle,
        subtitle_key,
    }
}
