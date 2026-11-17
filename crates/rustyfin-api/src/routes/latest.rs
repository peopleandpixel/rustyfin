//! The "Latest" row (`/Items/Latest`, `/Users/{userId}/Items/Latest`), as Jellyfin 12.4 builds
//! it: `UserLibraryController.GetLatestMedia`, `UserViewManager.GetLatestItems` and
//! `GetItemsForLatestItems`, and `BaseItemRepository.GetLatestItemList`.
use std::collections::HashMap;

use rustyfin_db::repo_media::{LatestFilter, LatestOrder, LatestRow};
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::query::{bool_param, count_param, item_kinds, parent_param};
use crate::state::AppState;

/// Episodes added this close to a series' newest one count as one addition.
const RECENT_ADDITION_WINDOW_HOURS: i64 = 24;

/// What a library's grouping path is, from its collection type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    TvShows,
    Movies,
    Music,
    /// A mixed library (no collection type).
    Mixed,
    /// Any other collection type (home videos and photos): no grouping query.
    Other,
}

fn grouping(library: &Library) -> Grouping {
    match library.content_type {
        LibraryContentType::TvShows => Grouping::TvShows,
        LibraryContentType::Movies => Grouping::Movies,
        LibraryContentType::Music => Grouping::Music,
        LibraryContentType::Mixed => Grouping::Mixed,
        LibraryContentType::Photos => Grouping::Other,
    }
}

/// The row's entries for `user`, each with its `ChildCount` (0: leave the item's own).
///
/// Without `includeItemTypes` the kinds follow the libraries' media types (music: tracks; home
/// videos and photos: photos and videos; others: videos), folders left out. With `groupItems`
/// (default true) the first library with a collection type picks the path, as in Jellyfin:
/// - TV: per series with new episodes (newest first, `limit` of them), the episodes added
///   within 24 hours of its newest: one gives that episode, several in one season give the
///   season when the series has more than one season (else the series), several seasons give
///   the series. A single new episode that is all of its season counts as several. A season or
///   series the user may not see gives its newest episode instead.
/// - Movies: the newest `limit` items.
/// - Music: the newest `limit` albums (their tracks' played state is not read).
/// - Mixed (no collection type): the newest `limit` items that aren't episodes, and the TV
///   result, merged by date added.
///
/// Otherwise the newest `limit * 2` items. Then, as `GetLatestItems` does, new tracks and
/// episodes are grouped under their album and series in first-seen order, up to `limit`
/// entries; a group of several (or any album) is shown as its container with `ChildCount`.
pub(crate) async fn latest_entries(
    state: &AppState,
    user: Uuid,
    params: &[(String, String)],
) -> Result<(Vec<MediaItem>, Vec<i32>), ApiError> {
    let limit = count_param(params, "limit")?.unwrap_or(20);
    let group_items = bool_param(params, "groupItems")?.unwrap_or(true);
    let parent_id = parent_param(params, "parentId")?;
    let config = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user)
        .await?
        .map(|u| rustyfin_model::user::jellyfin_configuration(&u.configuration))
        .unwrap_or_default();
    let mut is_played = bool_param(params, "isPlayed")?;
    if is_played.is_none() && config["HidePlayedInLatest"].as_bool() != Some(false) {
        is_played = Some(false);
    }

    // Jellyfin's parents: the library asked for, else the user's libraries less their
    // `LatestItemsExcludes`.
    let policy = crate::parental::user_policy(state, user).await?;
    let libraries: Vec<Library> = rustyfin_db::repo_library::get_all_libraries(&state.db_ro)
        .await?
        .into_iter()
        .filter(|l| policy.allows_folder(&l.id.to_string()))
        .collect();
    let mut parents: Vec<&Library> = Vec::new();
    if let Some(parent_id) = parent_id {
        match libraries.iter().find(|l| l.id == parent_id) {
            Some(library) => parents.push(library),
            // Another folder: Rustyfin's row reads whole libraries only.
            None if rustyfin_db::repo_media::get_item_by_id(&state.db_ro, parent_id)
                .await?
                .is_some() =>
            {
                return Ok((Vec::new(), Vec::new()));
            }
            // Not an item: Jellyfin reads the user's libraries.
            None => {}
        }
    }
    // Only an explicit music library turns the played filter off.
    if parents
        .iter()
        .any(|l| l.content_type == LibraryContentType::Music)
    {
        is_played = None;
    }
    if parents.is_empty() {
        let excluded: Vec<Uuid> = config["LatestItemsExcludes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().and_then(|id| Uuid::parse_str(id).ok()))
            .collect();
        parents = libraries
            .iter()
            .filter(|l| !excluded.contains(&l.id))
            .collect();
    }
    if parents.is_empty() || limit == 0 {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut item_types: Vec<String> = item_kinds(params, "includeItemTypes")
        .into_iter()
        .map(str::to_string)
        .collect();
    if item_types.is_empty() {
        for library in &parents {
            let kinds: &[&str] = match library.content_type {
                LibraryContentType::Music => &["Audio"],
                LibraryContentType::Photos => &["Photo", "Movie", "Episode", "Video"],
                _ => &["Movie", "Episode", "Video"],
            };
            for kind in kinds {
                if !item_types.iter().any(|k| k == kind) {
                    item_types.push(kind.to_string());
                }
            }
        }
    }
    let hidden = HiddenItems::for_user(state, user).await?;
    let filter = LatestFilter {
        user_id: Some(user),
        item_types,
        library_ids: parents.iter().map(|l| l.id).collect(),
        is_played,
        hidden_ids_json: hidden.json(),
    };

    let limit_i = limit as i64;
    let path = parents
        .iter()
        .map(|l| grouping(l))
        .find(|g| *g != Grouping::Mixed)
        .unwrap_or(Grouping::Mixed);
    let rows: Vec<Candidate> = match (group_items, path) {
        (true, Grouping::TvShows) => latest_tv(state, &filter, &hidden, limit_i).await?,
        (true, Grouping::Movies) => {
            candidates(state, &filter, LatestOrder::AddedThenId, false, limit_i).await?
        }
        (true, Grouping::Music) => {
            rustyfin_db::repo_media::get_latest_albums(&state.db_ro, &filter, limit_i)
                .await?
                .into_iter()
                .map(Candidate::alone)
                .collect()
        }
        (true, Grouping::Mixed) => {
            let mut all =
                candidates(state, &filter, LatestOrder::AddedThenId, true, limit_i).await?;
            all.extend(latest_tv(state, &filter, &hidden, limit_i).await?);
            all.sort_by(|a, b| {
                b.item
                    .date_created
                    .cmp(&a.item.date_created)
                    .then_with(|| b.item.id.to_string().cmp(&a.item.id.to_string()))
            });
            all.truncate(limit);
            all
        }
        _ => {
            candidates(
                state,
                &filter,
                LatestOrder::Added,
                false,
                limit_i.saturating_mul(2),
            )
            .await?
        }
    };

    // GetLatestItems: group by container in first-seen order, up to `limit` entries.
    let mut groups: Vec<(Option<Uuid>, Vec<MediaItem>)> = Vec::new();
    for row in rows {
        let container = row
            .container
            .filter(|_| group_items && !row.item.media_type.is_folder());
        match container {
            Some(id) => match groups.iter_mut().find(|(c, _)| *c == Some(id)) {
                Some((_, members)) => members.push(row.item),
                None => groups.push((Some(id), vec![row.item])),
            },
            None => groups.push((None, vec![row.item])),
        }
        if groups.len() >= limit {
            break;
        }
    }
    let mut items = Vec::with_capacity(groups.len());
    let mut child_counts = Vec::with_capacity(groups.len());
    for (container, mut members) in groups {
        let container = match container {
            Some(id) => rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id).await?,
            None => None,
        };
        match container {
            Some(c) if members.len() > 1 || c.media_type == MediaType::MusicAlbum => {
                child_counts.push(members.len() as i32);
                items.push(c);
            }
            _ => {
                child_counts.push(0);
                items.push(members.swap_remove(0));
            }
        }
    }
    Ok((items, child_counts))
}

/// A row entry before grouping: the item and what `GetLatestItems` groups it under.
struct Candidate {
    item: MediaItem,
    container: Option<Uuid>,
}

impl Candidate {
    fn alone(item: MediaItem) -> Self {
        Self {
            item,
            container: None,
        }
    }

    fn of(row: LatestRow) -> Self {
        let container = match row.item.media_type {
            MediaType::Episode => row.series_id,
            MediaType::Audio => row.album_id,
            _ => None,
        };
        Self {
            item: row.item,
            container,
        }
    }
}

async fn candidates(
    state: &AppState,
    filter: &LatestFilter,
    order: LatestOrder,
    without_series: bool,
    limit: i64,
) -> Result<Vec<Candidate>, ApiError> {
    Ok(rustyfin_db::repo_media::get_latest_items(
        &state.db_ro,
        filter,
        order,
        without_series,
        None,
        limit,
    )
    .await?
    .into_iter()
    .map(Candidate::of)
    .collect())
}

/// Jellyfin 12.4's `GetLatestTvShowItems`: one entry per series with new episodes.
async fn latest_tv(
    state: &AppState,
    filter: &LatestFilter,
    hidden: &HiddenItems,
    limit: i64,
) -> Result<Vec<Candidate>, ApiError> {
    let db = &state.db_ro;
    let series = rustyfin_db::repo_media::get_latest_series(db, filter, limit).await?;
    if series.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = series.iter().map(|(id, _)| *id).collect();
    let episodes = rustyfin_db::repo_media::get_latest_items(
        db,
        filter,
        LatestOrder::AddedThenId,
        false,
        Some(&ids),
        -1,
    )
    .await?;
    let mut by_series: HashMap<Uuid, Vec<LatestRow>> = HashMap::new();
    for row in episodes {
        if let Some(id) = row.series_id {
            by_series.entry(id).or_default().push(row);
        }
    }

    // Per series: how many were added recently, and in which seasons.
    struct Analysis {
        series: Uuid,
        newest: LatestRow,
        recent: usize,
        seasons: Vec<Uuid>,
    }
    let mut analysis = Vec::new();
    for series_id in &ids {
        let Some(mut rows) = by_series.remove(series_id) else {
            continue;
        };
        let newest_date = rows[0].item.date_created;
        let cutoff = newest_date - chrono::Duration::hours(RECENT_ADDITION_WINDOW_HOURS);
        let mut seasons = Vec::new();
        let mut recent = 0;
        for row in rows.iter().filter(|r| r.item.date_created >= cutoff) {
            recent += 1;
            if let Some(season) = row.season_id
                && !seasons.contains(&season)
            {
                seasons.push(season);
            }
        }
        analysis.push(Analysis {
            series: *series_id,
            newest: rows.swap_remove(0),
            recent,
            seasons,
        });
    }
    let all_seasons: Vec<Uuid> = analysis.iter().flat_map(|a| a.seasons.clone()).collect();
    let episodes_per_season =
        rustyfin_db::repo_media::count_children_of_kind(db, &all_seasons, "Episode").await?;
    let seasons_per_series =
        rustyfin_db::repo_media::count_children_of_kind(db, &ids, "Season").await?;

    let mut results: Vec<(MediaItem, chrono::DateTime<chrono::Utc>, Option<Uuid>)> = Vec::new();
    for a in analysis {
        let library = a.newest.item.library_id;
        let newest_date = a.newest.item.date_created;
        let container = match a.seasons.as_slice() {
            [season] => {
                let total = episodes_per_season.get(season).copied().unwrap_or(0);
                let seasons = seasons_per_series.get(&a.series).copied().unwrap_or(1);
                let several = a.recent > 1 || a.recent as i64 == total;
                if seasons > 1 && several {
                    Some(*season)
                } else if several {
                    Some(a.series)
                } else {
                    None
                }
            }
            [] => None,
            _ => Some(a.series),
        };
        // A container the user may not see gives its newest episode instead.
        let container = container.filter(|id| !hidden.hides(*id, library));
        let entity = match container {
            Some(id) => rustyfin_db::repo_media::get_item_by_id(db, id).await?,
            None => None,
        };
        match entity {
            Some(entity) => results.push((entity, newest_date, None)),
            None => results.push((a.newest.item, newest_date, Some(a.series))),
        }
    }
    results.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.id.to_string().cmp(&a.0.id.to_string()))
    });
    results.truncate(limit.max(0) as usize);
    Ok(results
        .into_iter()
        .map(|(item, _, container)| Candidate { item, container })
        .collect())
}
