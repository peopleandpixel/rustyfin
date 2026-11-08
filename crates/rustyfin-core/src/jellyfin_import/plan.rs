//! Matching users and items (spec 2.5-2.11).

use std::collections::{HashMap, HashSet};

use rustyfin_db::repo_jellyfin_import::ImportTarget;
use rustyfin_model::media::MediaType;
use uuid::Uuid;

use super::paths::{PathMapping, translate_path};
use super::source::{JfItem, JfUser};
use super::{
    ConfirmedMatch, ItemReport, SkipReason, SuggestedMatch, UnmatchedSample, UserAction, UserRow,
};

/// The most unmatched items the report samples (spec 2.20).
const SAMPLE_LIMIT: usize = 200;

/// How a Jellyfin user name matches Rustyfin's users (spec 2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMatch {
    Matched(Uuid),
    Ambiguous,
    NoMatch,
}

/// Matches `name` against `users` (id, name): case-insensitively; an exact-case match wins; two
/// case-insensitive matches and no exact one are ambiguous.
pub fn match_user_name(name: &str, users: &[(Uuid, String)]) -> UserMatch {
    if let Some((id, _)) = users.iter().find(|(_, user)| user == name) {
        return UserMatch::Matched(*id);
    }
    let folded = name.to_lowercase();
    let mut hits = users
        .iter()
        .filter(|(_, user)| user.to_lowercase() == folded)
        .map(|(id, _)| *id);
    match (hits.next(), hits.next()) {
        (None, _) => UserMatch::NoMatch,
        (Some(id), None) => UserMatch::Matched(id),
        (Some(_), Some(_)) => UserMatch::Ambiguous,
    }
}

/// Plans what the import does with each Jellyfin user (spec 2.5-2.7, 2.20). A user outside
/// `picked` (when given) is skipped as not selected. A picked user whose name matches a Rustyfin
/// user gets that account as its suggested match, and is created when no name matches and skipped
/// as ambiguous when several do. The rows are sorted by name, case-insensitively.
pub fn plan_users(
    jf_users: &[JfUser],
    picked: Option<&[Uuid]>,
    confirmed: &[ConfirmedMatch],
    rustyfin_users: &[(Uuid, String)],
) -> Vec<UserRow> {
    let mut rows: Vec<UserRow> = jf_users
        .iter()
        .map(|user| {
            let found = if picked.is_some_and(|ids| !ids.contains(&user.id)) {
                None
            } else {
                Some(match_user_name(&user.name, rustyfin_users))
            };
            let (action, reason, user_id) = match found {
                None => (UserAction::Skip, Some(SkipReason::NotSelected), None),
                Some(found) => plan_picked_user(user.id, found, confirmed),
            };
            let suggested_match = match found {
                Some(UserMatch::Matched(id)) => rustyfin_users
                    .iter()
                    .find(|(account, _)| *account == id)
                    .map(|(account, name)| SuggestedMatch {
                        user_id: *account,
                        name: name.clone(),
                    }),
                _ => None,
            };
            UserRow {
                jellyfin_id: user.id,
                name: user.name.clone(),
                was_administrator: user.is_admin,
                action,
                reason,
                user_id,
                suggested_match,
                code: None,
            }
        })
        .collect();
    rows.sort_by_cached_key(|row| (row.name.to_lowercase(), row.jellyfin_id));
    rows
}

/// The action for a picked Jellyfin user (R2). Its data goes into the account its name matches
/// only when `confirmed` pairs the two by that account's id; unconfirmed, it's skipped. A
/// confirmation that names any other account, or comes with no match, is refused, so neither a
/// rename nor a wrong id redirects the data. Ambiguous names stay skipped.
fn plan_picked_user(
    jellyfin_id: Uuid,
    found: UserMatch,
    confirmed: &[ConfirmedMatch],
) -> (UserAction, Option<SkipReason>, Option<Uuid>) {
    let accounts: Vec<Uuid> = confirmed
        .iter()
        .filter(|pair| pair.jellyfin_id == jellyfin_id)
        .map(|pair| pair.user_id)
        .collect();
    match found {
        UserMatch::Ambiguous => (UserAction::Skip, Some(SkipReason::Ambiguous), None),
        _ if accounts.iter().any(|&id| found != UserMatch::Matched(id)) => (
            UserAction::Skip,
            Some(SkipReason::ConfirmedOtherAccount),
            None,
        ),
        UserMatch::Matched(id) if !accounts.is_empty() => (UserAction::Match, None, Some(id)),
        UserMatch::Matched(_) => (UserAction::Skip, Some(SkipReason::NotConfirmed), None),
        UserMatch::NoMatch => (UserAction::Create, None, None),
    }
}

/// How a Jellyfin item matched: by path, or by one of the ID passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    ByPath,
    ById,
}

/// A matched item: how it matched, and the index of its Rustyfin target.
type Hit = (Found, usize);

/// Rustyfin's targets by the keys of the passes (spec 2.10). A key only counts with exactly one
/// target.
#[derive(Default)]
struct TargetIndex {
    by_path: HashMap<(&'static str, String), Vec<usize>>,
    by_tmdb: HashMap<(&'static str, i64), Vec<usize>>,
    by_tvdb: HashMap<(&'static str, i64), Vec<usize>>,
    /// Seasons by (series, season number).
    by_season: HashMap<(Uuid, i64), Vec<usize>>,
    /// Episodes by (series, season number, episode number).
    by_episode: HashMap<(Uuid, i64, i64), Vec<usize>>,
}

impl TargetIndex {
    fn new(targets: &[ImportTarget]) -> Self {
        // A season's series is its parent; an episode's series is its season's parent.
        let series_of_season: HashMap<Uuid, Uuid> = targets
            .iter()
            .filter(|target| target.media_type == MediaType::Season)
            .filter_map(|target| Some((target.id, target.parent_id?)))
            .collect();
        let mut index = Self::default();
        for (n, target) in targets.iter().enumerate() {
            let kind = target.media_type.item_kind();
            if let Some(path) = &target.path {
                index
                    .by_path
                    .entry((kind, path.clone()))
                    .or_default()
                    .push(n);
            }
            if let Some(tmdb) = target.tmdb_id {
                index.by_tmdb.entry((kind, tmdb)).or_default().push(n);
            }
            if let Some(tvdb) = target.tvdb_id {
                index.by_tvdb.entry((kind, tvdb)).or_default().push(n);
            }
            match target.media_type {
                MediaType::Season => {
                    if let Some(key) = target.parent_id.zip(target.index_number) {
                        index.by_season.entry(key).or_default().push(n);
                    }
                }
                MediaType::Episode => {
                    let series = target
                        .parent_id
                        .and_then(|season| series_of_season.get(&season).copied());
                    let key = series
                        .zip(target.parent_index_number)
                        .zip(target.index_number)
                        .map(|((series, season), episode)| (series, season, episode));
                    if let Some(key) = key {
                        index.by_episode.entry(key).or_default().push(n);
                    }
                }
                _ => {}
            }
        }
        index
    }
}

/// The one target index a key holds, when it holds exactly one.
fn only(hits: Option<&Vec<usize>>) -> Option<usize> {
    match hits?.as_slice() {
        [n] => Some(*n),
        _ => None,
    }
}

/// Each Jellyfin item's hit (spec 2.10), in the order of `items`, by four passes: by path; Movies
/// and Series by TMDB, then TVDB; Seasons through their series; Episodes through their series.
/// Each pass only sees the items the passes before it left unmatched.
fn match_found(
    items: &[JfItem],
    targets: &[ImportTarget],
    pairs: &[PathMapping],
) -> Vec<Option<Hit>> {
    let index = TargetIndex::new(targets);
    let mut found: Vec<Option<Hit>> = vec![None; items.len()];

    // 1. By path: the translated path and the media type, exactly one target.
    for (slot, item) in found.iter_mut().zip(items) {
        let Some(path) = item
            .path
            .as_deref()
            .and_then(|path| translate_path(path, pairs))
        else {
            continue;
        };
        let key = (item.media_type.item_kind(), path);
        *slot = only(index.by_path.get(&key)).map(|n| (Found::ByPath, n));
    }

    // 2. Movies and Series: TMDB, then TVDB when TMDB gives no single target.
    for (slot, item) in found.iter_mut().zip(items) {
        if slot.is_some() || !matches!(item.media_type, MediaType::Movie | MediaType::Series) {
            continue;
        }
        let kind = item.media_type.item_kind();
        let by_tmdb = item
            .providers
            .tmdb
            .and_then(|id| only(index.by_tmdb.get(&(kind, id))));
        let hit = by_tmdb.or_else(|| {
            item.providers
                .tvdb
                .and_then(|id| only(index.by_tvdb.get(&(kind, id))))
        });
        *slot = hit.map(|n| (Found::ById, n));
    }

    // The Rustyfin series of each matched Jellyfin series, for the two passes below.
    let matched_series: HashMap<Uuid, Uuid> = items
        .iter()
        .zip(&found)
        .filter(|(item, _)| item.media_type == MediaType::Series)
        .filter_map(|(item, hit)| {
            let (_, n) = (*hit)?;
            Some((item.id, targets[n].id))
        })
        .collect();

    // 3. Seasons: the matched series (SeriesId, else ParentId) and the season number.
    for (slot, item) in found.iter_mut().zip(items) {
        if slot.is_some() || item.media_type != MediaType::Season {
            continue;
        }
        let series = item
            .series_id
            .or(item.parent_id)
            .and_then(|id| matched_series.get(&id).copied());
        let wanted = series.zip(item.index_number);
        *slot = only(wanted.and_then(|key| index.by_season.get(&key))).map(|n| (Found::ById, n));
    }

    // 4. Episodes: the matched series (SeriesId), the season number and the episode number.
    for (slot, item) in found.iter_mut().zip(items) {
        if slot.is_some() || item.media_type != MediaType::Episode {
            continue;
        }
        let series = item
            .series_id
            .and_then(|id| matched_series.get(&id).copied());
        let wanted = series
            .zip(item.parent_index_number)
            .zip(item.index_number)
            .map(|((series, season), episode)| (series, season, episode));
        *slot = only(wanted.and_then(|key| index.by_episode.get(&key))).map(|n| (Found::ById, n));
    }

    found
}

/// Matches the Jellyfin items to Rustyfin's targets (spec 2.10) and counts the matches for the
/// report.
pub fn match_items(
    items: &[JfItem],
    targets: &[ImportTarget],
    pairs: &[PathMapping],
    watched: &HashSet<Uuid>,
) -> ItemReport {
    let found = match_found(items, targets, pairs);
    let count = |how: Found| found.iter().flatten().filter(|hit| hit.0 == how).count();
    let matched_by_path = count(Found::ByPath);
    let matched_by_id = count(Found::ById);
    let unmatched = found.iter().filter(|hit| hit.is_none()).count();

    // Samples: unmatched items that hold a row of an imported user, by path then name.
    let mut samples: Vec<&JfItem> = items
        .iter()
        .zip(&found)
        .filter(|(item, hit)| hit.is_none() && watched.contains(&item.id))
        .map(|(item, _)| item)
        .collect();
    samples.sort_by(|a, b| sample_order(a).cmp(&sample_order(b)));
    let unmatched_samples = samples
        .into_iter()
        .take(SAMPLE_LIMIT)
        .map(|item| UnmatchedSample {
            item_type: item.type_name.clone(),
            name: item.name.clone().unwrap_or_default(),
            path: item.path.clone().unwrap_or_default(),
        })
        .collect();

    ItemReport {
        total: items.len(),
        matched_by_path,
        matched_by_id,
        unmatched,
        unmatched_samples,
    }
}

/// Each matched Jellyfin item's Rustyfin item (spec 2.32): Jellyfin item id to target id, by the
/// same passes as `match_items`.
pub fn matched_items(
    items: &[JfItem],
    targets: &[ImportTarget],
    pairs: &[PathMapping],
) -> HashMap<Uuid, Uuid> {
    let found = match_found(items, targets, pairs);
    items
        .iter()
        .zip(&found)
        .filter_map(|(item, hit)| {
            let (_, n) = (*hit)?;
            Some((item.id, targets[n].id))
        })
        .collect()
}

/// The ids of the Jellyfin items that match by path: pass 1 of `match_found` (spec 5.15).
pub fn matched_by_path(
    items: &[JfItem],
    targets: &[ImportTarget],
    pairs: &[PathMapping],
) -> HashSet<Uuid> {
    let found = match_found(items, targets, pairs);
    items
        .iter()
        .zip(&found)
        .filter(|(_, hit)| matches!(hit, Some((Found::ByPath, _))))
        .map(|(item, _)| item.id)
        .collect()
}

/// The sample order: by path, then by name. A missing path or name sorts as "".
fn sample_order(item: &JfItem) -> (&str, &str) {
    (
        item.path.as_deref().unwrap_or_default(),
        item.name.as_deref().unwrap_or_default(),
    )
}
