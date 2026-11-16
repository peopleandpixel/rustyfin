//! Extras: local trailers, special features and theme media, found the way Jellyfin finds them
//! (`NamingOptions.VideoExtraRules`). A scan stores each one as an item with an extra type and an owner
//! (the movie, series or season it belongs to); library listings never show them.
//!
//! Movies: `<movie>-trailer.mkv`, `trailer.mkv` and the other suffixes next to the movie, `theme.mp3`, and
//! the files in its extras folders (`trailers/`, `featurettes/`, `extras/`, `theme-music/`, `backdrops/`
//! and the rest). Shows: the extras folders of a series or season folder, and `theme.mp3` and `trailer.mkv`
//! in it; an episode's suffix files (`S01E01-featurette.mkv`) and its episode folder's extras folders. A
//! show's `Extras` folder holds the series' extras, as in Jellyfin; `Specials` is its season 0. See
//! [`classify_tv_file`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rustyfin_db::repo_extras;
use rustyfin_db::repo_media::PathScope;
use rustyfin_model::library::Library;
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::naming::{self, ExtraKind};
use crate::probe::ProbeStep;
use crate::scanner::{AUDIO_EXTENSIONS, VIDEO_EXTENSIONS};

/// An extra file that a scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundExtra {
    pub path: PathBuf,
    /// The folder of its owner: the movie's folder, or the series or season folder.
    pub owner_dir: PathBuf,
    pub kind: ExtraKind,
    /// The owner's name that a suffix names (`Movie (2020)` in `Movie (2020)-trailer.mkv`), else empty.
    pub owner_name: String,
    /// The paths its owner may be stored at, set by the scan from what is on disk: the files of the movie or
    /// episode it belongs to, or the series or season folder. Empty: it has no owner.
    pub owner_paths: Vec<PathBuf>,
    /// Whether it belongs to an episode in `owner_dir` (see [`owning_episode`]) rather than to the folder.
    pub of_episode: bool,
}

/// What a file is to a scan.
#[derive(Debug, PartialEq, Eq)]
pub enum Classified {
    /// An extra.
    Extra(FoundExtra),
    /// A file in an extras folder that is not that folder's kind of media (a video in `theme-music/`). Next to
    /// a movie (in `owner_dir`) it is neither an extra nor a library item; otherwise the scan reads it as
    /// before.
    Skip { owner_dir: PathBuf },
    /// Not an extra: the scan reads it as before.
    No,
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn is_video(path: &Path) -> bool {
    VIDEO_EXTENSIONS.contains(&extension(path).as_str())
}

fn is_audio(path: &Path) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(path).as_str())
}

/// The extra in the extras folder `dir` of kind `kind`, owned by `owner_dir`, when the file's media fits.
fn in_extras_dir(file: &Path, owner_dir: &Path, kind: ExtraKind) -> Classified {
    let fits = if kind.is_audio() {
        is_audio(file)
    } else {
        is_video(file)
    };
    if !fits {
        return Classified::Skip {
            owner_dir: owner_dir.to_path_buf(),
        };
    }
    Classified::Extra(FoundExtra {
        path: file.to_path_buf(),
        owner_dir: owner_dir.to_path_buf(),
        kind,
        owner_name: String::new(),
        owner_paths: Vec::new(),
        of_episode: false,
    })
}

/// A file by its own name: `theme.mp3`, or (`suffixes` false) only `trailer.mkv` and `sample.mkv`, or
/// (`suffixes` true) also `<name>-trailer.mkv` and the other suffixes.
fn by_name(file: &Path, owner_dir: &Path, suffixes: bool) -> Classified {
    if is_audio(file) && naming::is_theme_song_file(file) {
        return Classified::Extra(FoundExtra {
            path: file.to_path_buf(),
            owner_dir: owner_dir.to_path_buf(),
            kind: ExtraKind::ThemeSong,
            owner_name: String::new(),
            owner_paths: Vec::new(),
            of_episode: false,
        });
    }
    if !is_video(file) {
        return Classified::No;
    }
    match naming::extra_file_kind(file) {
        Some((kind, owner_name)) if suffixes || owner_name.is_empty() => {
            Classified::Extra(FoundExtra {
                path: file.to_path_buf(),
                owner_dir: owner_dir.to_path_buf(),
                kind,
                owner_name,
                owner_paths: Vec::new(),
                of_episode: false,
            })
        }
        _ => Classified::No,
    }
}

/// What `file`, under the movie library root `root`, is by its path alone. An extras folder directly in the
/// root (`Movies/Shorts/`) is a folder of movies, not of extras. The scan keeps an extra only when its owner
/// folder holds a movie of its own (see `scanner::scan_flat_library`).
pub fn classify_movie_file(file: &Path, root: &Path) -> Classified {
    let Some(parent) = file.parent() else {
        return Classified::No;
    };
    if parent != root
        && let Some(kind) = parent
            .file_name()
            .and_then(|name| naming::extra_dir_kind(&name.to_string_lossy()))
        && let Some(owner_dir) = parent.parent()
        && owner_dir != root
        && owner_dir.starts_with(root)
    {
        return in_extras_dir(file, owner_dir, kind);
    }
    by_name(file, parent, true)
}

/// What `file`, under the TV library root `root`, is.
///
/// - `Show/<extras folder>/x`: the series' extra. Jellyfin never reads a folder named like an extras folder
///   as a season (`CoreResolutionIgnoreRule`), so `Show/Extras` holds the series' extras, while `Specials`
///   and `Show Extras` are its specials season.
/// - `Show/Season 1/<extras folder>/x`: the season's extra.
/// - `Show/Season 1/<episode folder>/<extras folder>/x`: an extra of the episode in that folder.
/// - `theme.mp3`, `trailer.mkv` and `sample.mkv` in a series or season folder: the folder's extra.
/// - `S01E01-featurette.mkv` and the other suffixes, where the name before the suffix reads as an episode
///   (`naming::parse_episode`), next to the episodes of a series, season or episode folder: an extra of
///   that episode. The scan finds the episode among the files next to it ([`owning_episode`]) and reads
///   the file as an episode, as before, when there is none.
pub fn classify_tv_file(file: &Path, root: &Path) -> Classified {
    let Ok(rel) = file.strip_prefix(root) else {
        return Classified::No;
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let n = parts.len();
    let Some(parent) = file.parent() else {
        return Classified::No;
    };
    if n >= 3
        && let Some(kind) = naming::extra_dir_kind(&parts[n - 2])
    {
        let owner_dir = parent.parent().unwrap_or(root);
        return match (n, in_extras_dir(file, owner_dir, kind)) {
            (3 | 4, found) => found,
            (5, Classified::Extra(found)) => Classified::Extra(FoundExtra {
                of_episode: true,
                ..found
            }),
            _ => Classified::Skip {
                owner_dir: parent.to_path_buf(),
            },
        };
    }
    // A file directly in a series or season folder.
    if n == 2 || n == 3 {
        let found = by_name(file, parent, false);
        if found != Classified::No {
            return found;
        }
    }
    // An episode's extra by its suffix, in a series, season or episode folder.
    if (2..=4).contains(&n)
        && is_video(file)
        && let Some((kind, owner_name)) = naming::extra_file_kind(file)
        && naming::parse_episode(owner_name.trim()).is_some()
    {
        return Classified::Extra(FoundExtra {
            path: file.to_path_buf(),
            owner_dir: parent.to_path_buf(),
            kind,
            owner_name,
            owner_paths: Vec::new(),
            of_episode: true,
        });
    }
    Classified::No
}

/// The episode files among `episodes` (the episode files in `extra.owner_dir`) that the episode extra `extra`
/// belongs to. A suffix file (`S01E01-featurette.mkv`) belongs to the files whose stem is the name before its
/// suffix, in any case, else to those with that name's season and episode numbers. A file in an episode
/// folder's extras folder belongs to the folder's episode: its files, when they are all one episode. Empty
/// when there is no such episode, or when the name fits several.
pub fn owning_episode(extra: &FoundExtra, episodes: &[PathBuf]) -> Vec<PathBuf> {
    let numbers = |file: &PathBuf| {
        file.file_stem()
            .and_then(|stem| naming::parse_episode(&stem.to_string_lossy()))
            .map(|n| (n.season, n.episode))
    };
    let name = extra.owner_name.trim();
    if name.is_empty() {
        let first = episodes.first().map(numbers);
        let one = episodes.len() == 1
            || first.flatten().is_some() && episodes.iter().all(|f| Some(numbers(f)) == first);
        return if one { episodes.to_vec() } else { Vec::new() };
    }
    let lower = name.to_lowercase();
    let named: Vec<PathBuf> = episodes
        .iter()
        .filter(|f| {
            f.file_stem()
                .is_some_and(|s| s.to_string_lossy().to_lowercase() == lower)
        })
        .cloned()
        .collect();
    if !named.is_empty() {
        return named;
    }
    let Some(wanted) = naming::parse_episode(name).map(|n| (n.season, n.episode)) else {
        return Vec::new();
    };
    episodes
        .iter()
        .filter(|f| numbers(f) == Some(wanted))
        .cloned()
        .collect()
}

/// The movie that `extra` belongs to among the movies its folder holds on disk (`groups`, as
/// `naming::group_movie_files` makes them; `is_root` as given to it).
///
/// - A folder-wide extra (`trailer.mkv`, `theme.mp3`, an extras folder) belongs to the folder's movie when it
///   holds exactly one, as in Jellyfin; among several it belongs to none.
/// - A suffix file (`Name-trailer.mkv`) belongs only to the movie it names: the one with a file of exactly
///   that name, else the one whose title and year the name reads as. This is stricter than Jellyfin, which
///   gives a suffix file in a one-movie folder to that movie whatever its name; here a real film named like
///   `B-short.mkv` is never handed to another film, however many films its folder holds.
///
/// A file with no owner here is read by the scan as a movie of its own.
pub fn owning_group<'a>(
    extra: &FoundExtra,
    groups: &'a [naming::VersionGroup],
    is_root: bool,
) -> Option<&'a naming::VersionGroup> {
    let name = extra.owner_name.trim();
    match groups {
        [] => None,
        [only] if name.is_empty() => Some(only),
        _ if name.is_empty() => None,
        _ => {
            let lower = name.to_lowercase();
            let stem_is = |file: &PathBuf| {
                file.file_stem()
                    .is_some_and(|s| s.to_string_lossy().to_lowercase() == lower)
            };
            if let Some(group) = groups.iter().find(|g| g.files.iter().any(stem_is)) {
                return Some(group);
            }
            let named = naming::group_movie_files(
                &extra.owner_dir,
                &[extra.owner_dir.join(format!("{name}.mkv"))],
                is_root,
            );
            let wanted = named.first()?;
            groups
                .iter()
                .find(|g| g.title.eq_ignore_ascii_case(&wanted.title) && g.year == wanted.year)
        }
    }
}

/// Stores the extras that a scan of `library` found in `scope`. Each one's owner is the stored movie, series
/// or season at one of its `owner_paths` (files and folders the scan just found, so a stale row whose file is
/// gone never owns anything). A new extra is added, with a probe plan in the result; a stored item at an
/// extra's path (an extra, or a movie that an older scan made of a trailer) is updated. An extra with no
/// stored owner is left as it is: this never deletes a row. The items whose files are gone are the removal's
/// (`scan_sync::remove_not_collected`).
pub async fn sync_extras(
    pool: &SqlitePool,
    library: &Library,
    scope: &PathScope,
    found: &[FoundExtra],
) -> Result<Vec<(Uuid, Vec<ProbeStep>)>, sqlx::Error> {
    let mut plans = Vec::new();
    if found.is_empty() {
        return Ok(plans);
    }
    let owner_paths: Vec<String> = found
        .iter()
        .flat_map(|e| e.owner_paths.iter())
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let owners = repo_extras::owners_at(pool, library.id, &owner_paths).await?;
    let stored = repo_extras::items_by_path(pool, library.id, scope).await?;
    // The stored extras whose file is gone: a new extra file that is one of them renamed or moved takes it
    // over (`heir_extra`), so it keeps its id and user data. The others are the removal's.
    let found_paths: HashSet<String> = found
        .iter()
        .map(|e| e.path.to_string_lossy().into_owned())
        .collect();
    let mut gone: Vec<(&str, &repo_extras::PathRow)> = Vec::new();
    for (path, row) in &stored {
        if row.extra_type.is_some() && !found_paths.contains(path) && is_missing(path).await {
            gone.push((path.as_str(), row));
        }
    }

    let mut new_items: Vec<(MediaItem, String, &'static str)> = Vec::new();
    // The extras whose owner isn't stored (a show folder whose series or season isn't): warned about once.
    let mut unowned: Vec<String> = Vec::new();
    let mut seen: HashSet<&Path> = HashSet::new();
    for extra in found {
        if !seen.insert(extra.path.as_path()) {
            continue;
        }
        let path = extra.path.to_string_lossy().into_owned();
        let owner = extra
            .owner_paths
            .iter()
            .find_map(|p| owners.get(p.to_string_lossy().as_ref()))
            .map(String::as_str);
        let kind = extra.kind.as_str();
        let media_type = if extra.kind.is_audio() {
            MediaType::Audio
        } else {
            MediaType::Video
        };
        let name = extra
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match (owner, stored.get(&path)) {
            // No owner (a show folder whose series isn't stored): the file is left as it is, and a stored
            // row is never deleted here.
            (None, _) => unowned.push(path),
            (Some(owner), Some(row)) => {
                if row.extra_type.as_deref() != Some(kind) || row.owner_id.as_deref() != Some(owner)
                {
                    if row.extra_type.is_none() {
                        // A movie or episode that an older scan made of this file leaves the listings.
                        tracing::info!(
                            "{path} is now a {kind} extra of item {owner}; it was a library item"
                        );
                    }
                    repo_extras::set_extra(
                        pool,
                        &row.id,
                        owner,
                        kind,
                        media_type.item_kind(),
                        &name,
                    )
                    .await?;
                }
            }
            (Some(owner), None) => {
                let stamp = crate::scan_sync::file_stamp(&path);
                if let Some(at) = heir_extra(&gone, &extra.path, stamp, owner) {
                    let (old_path, row) = gone.swap_remove(at);
                    tracing::info!(
                        "extra {old_path} is now at {path}; it keeps its id and user data"
                    );
                    let old_name = Path::new(old_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    repo_extras::move_extra(
                        pool,
                        &row.id,
                        old_path,
                        &old_name,
                        &path,
                        owner,
                        kind,
                        media_type.item_kind(),
                        &name,
                    )
                    .await?;
                    if let Ok(id) = Uuid::parse_str(&row.id) {
                        plans.push((id, vec![ProbeStep::Primary(path.clone())]));
                    }
                    continue;
                }
                let item = MediaItem {
                    id: Uuid::new_v4(),
                    name: name.clone(),
                    sort_name: Some(name.clone()),
                    overview: None,
                    media_type,
                    path: path.clone(),
                    parent_id: None,
                    library_id: library.id,
                    production_year: None,
                    community_rating: None,
                    official_rating: None,
                    genres: vec![],
                    date_created: crate::scanner::date_added(&extra.path).await,
                    premiere_date: None,
                    runtime_ticks: None,
                    index_number: None,
                    parent_index_number: None,
                    media_sources: Vec::new(),
                    ..MediaItem::default_dto_fields()
                };
                new_items.push((item, owner.to_string(), kind));
            }
        }
    }
    if !unowned.is_empty() {
        const SHOWN: usize = 10;
        let more = unowned.len().saturating_sub(SHOWN);
        tracing::warn!(
            "library '{}': {} extras have no stored series, season or movie to belong to and stay hidden \
             until one is scanned: {}{}",
            library.name,
            unowned.len(),
            unowned[..unowned.len().min(SHOWN)].join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        );
    }
    if new_items.is_empty() {
        return Ok(plans);
    }
    // Each row is written with its owner and extra type, all in one transaction.
    rustyfin_db::busy::retry_busy("saving extras", || {
        repo_extras::insert_extras(pool, &new_items)
    })
    .await?;
    for (item, _, _) in &new_items {
        plans.push((item.id, vec![ProbeStep::Primary(item.path.clone())]));
    }
    tracing::info!("library '{}': {} new extras", library.name, new_items.len());
    Ok(plans)
}

/// Whether nothing is at `path` any more. A file that can't be read for another reason is still there.
async fn is_missing(path: &str) -> bool {
    matches!(tokio::fs::metadata(path).await, Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// The index in `gone` (stored extras whose file is gone, with their old path) of the one that the new extra
/// file `file`, with the size and modification time `stamp`, of the owner `owner`, is after a rename or a
/// move: the only one whose source has that stamp and the same kind of media (a rename or move within a
/// disk keeps both, even to another owner), else the only one of the same owner with the same file name (a
/// file moved into another extras folder) whose stamp isn't known to differ: a re-downloaded trailer with
/// the old one's name is a new file and starts with no user data. Main items follow their files the same
/// way (`scan_sync::takes_over`).
fn heir_extra(
    gone: &[(&str, &repo_extras::PathRow)],
    file: &Path,
    stamp: Option<(i64, i64)>,
    owner: &str,
) -> Option<usize> {
    let only = |hits: Vec<usize>| match hits[..] {
        [one] => Some(one),
        _ => None,
    };
    if let Some(stamp) = stamp {
        let audio = is_audio(file);
        let hits = (0..gone.len())
            .filter(|&i| {
                let (old, row) = gone[i];
                row.stamp == Some(stamp) && is_audio(Path::new(old)) == audio
            })
            .collect();
        if let Some(one) = only(hits) {
            return Some(one);
        }
    }
    let name = file.file_name()?;
    only(
        (0..gone.len())
            .filter(|&i| {
                let (old, row) = gone[i];
                let differs = matches!((row.stamp, stamp), (Some(was), Some(is)) if was != is);
                row.owner_id.as_deref() == Some(owner)
                    && Path::new(old).file_name() == Some(name)
                    && !differs
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extra(path: &str, owner_dir: &str, kind: ExtraKind, owner_name: &str) -> Classified {
        Classified::Extra(FoundExtra {
            path: PathBuf::from(path),
            owner_dir: PathBuf::from(owner_dir),
            kind,
            owner_name: owner_name.to_string(),
            owner_paths: Vec::new(),
            of_episode: false,
        })
    }

    #[test]
    fn r3extras_movie_files_are_read_by_jellyfins_extra_rules() {
        let root = Path::new("/m");
        let c = |p: &str| classify_movie_file(Path::new(p), root);
        assert_eq!(
            c("/m/A (2020)/A (2020)-trailer.mkv"),
            extra(
                "/m/A (2020)/A (2020)-trailer.mkv",
                "/m/A (2020)",
                ExtraKind::Trailer,
                "A (2020)"
            )
        );
        assert_eq!(
            c("/m/A (2020)/trailer2.mp4"),
            extra(
                "/m/A (2020)/trailer2.mp4",
                "/m/A (2020)",
                ExtraKind::Trailer,
                ""
            )
        );
        assert_eq!(
            c("/m/A/A-deletedscene.mkv"),
            extra(
                "/m/A/A-deletedscene.mkv",
                "/m/A",
                ExtraKind::DeletedScene,
                "A"
            )
        );
        assert_eq!(
            c("/m/A/Trailers/one.mkv"),
            extra("/m/A/Trailers/one.mkv", "/m/A", ExtraKind::Trailer, "")
        );
        assert_eq!(
            c("/m/A/behind the scenes/x.mkv"),
            extra(
                "/m/A/behind the scenes/x.mkv",
                "/m/A",
                ExtraKind::BehindTheScenes,
                ""
            )
        );
        assert_eq!(
            c("/m/A/featurettes/x.mkv"),
            extra("/m/A/featurettes/x.mkv", "/m/A", ExtraKind::Featurette, "")
        );
        assert_eq!(
            c("/m/A/extras/x.mkv"),
            extra("/m/A/extras/x.mkv", "/m/A", ExtraKind::Unknown, "")
        );
        assert_eq!(
            c("/m/A/A-featurette.mkv"),
            extra("/m/A/A-featurette.mkv", "/m/A", ExtraKind::Featurette, "A")
        );
        assert_eq!(
            c("/m/A/theme.mp3"),
            extra("/m/A/theme.mp3", "/m/A", ExtraKind::ThemeSong, "")
        );
        assert_eq!(
            c("/m/A/theme-music/song.flac"),
            extra(
                "/m/A/theme-music/song.flac",
                "/m/A",
                ExtraKind::ThemeSong,
                ""
            )
        );
        assert_eq!(
            c("/m/A/backdrops/loop.mp4"),
            extra("/m/A/backdrops/loop.mp4", "/m/A", ExtraKind::ThemeVideo, "")
        );
        assert_eq!(
            c("/m/A/theme-music/clip.mkv"),
            Classified::Skip {
                owner_dir: PathBuf::from("/m/A")
            }
        );
        // An extras folder in the root holds movies.
        assert_eq!(c("/m/Shorts/Film.mkv"), Classified::No);
        assert_eq!(c("/m/Extras/Film (2001).mkv"), Classified::No);
        // A grouping folder's extras folder is read as extras here; the scan checks for a movie next to it.
        assert_eq!(
            c("/m/Collection/Featurettes/x.mkv"),
            extra(
                "/m/Collection/Featurettes/x.mkv",
                "/m/Collection",
                ExtraKind::Featurette,
                ""
            )
        );
        assert_eq!(c("/m/A/A (2020).mkv"), Classified::No);
        assert_eq!(c("/m/A/notes.mp3"), Classified::No);
        // An extras folder that is the library root itself is a movie folder.
        assert_eq!(
            classify_movie_file(Path::new("/m/extras/A.mkv"), Path::new("/m/extras")),
            Classified::No
        );
    }

    #[test]
    fn r3extras_show_files_are_extras_in_extras_folders_and_by_name_only() {
        let root = Path::new("/tv");
        let c = |p: &str| classify_tv_file(Path::new(p), root);
        assert_eq!(
            c("/tv/Show/featurettes/x.mkv"),
            extra(
                "/tv/Show/featurettes/x.mkv",
                "/tv/Show",
                ExtraKind::Featurette,
                ""
            )
        );
        assert_eq!(
            c("/tv/Show/Season 1/deleted scenes/x.mkv"),
            extra(
                "/tv/Show/Season 1/deleted scenes/x.mkv",
                "/tv/Show/Season 1",
                ExtraKind::DeletedScene,
                ""
            )
        );
        assert_eq!(
            c("/tv/Show/theme.mp3"),
            extra("/tv/Show/theme.mp3", "/tv/Show", ExtraKind::ThemeSong, "")
        );
        assert_eq!(
            c("/tv/Show/trailer.mkv"),
            extra("/tv/Show/trailer.mkv", "/tv/Show", ExtraKind::Trailer, "")
        );
        assert_eq!(c("/tv/Show/Season 1/S01E01.mkv"), Classified::No);
    }

    fn episode_extra(path: &str, owner_dir: &str, kind: ExtraKind, owner_name: &str) -> Classified {
        match extra(path, owner_dir, kind, owner_name) {
            Classified::Extra(found) => Classified::Extra(FoundExtra {
                of_episode: true,
                ..found
            }),
            other => other,
        }
    }

    #[test]
    fn r3extras2_show_extras_folders_and_episode_extras_follow_jellyfin() {
        let root = Path::new("/tv");
        let c = |p: &str| classify_tv_file(Path::new(p), root);
        // `Extras` in a show is its extras folder, as Jellyfin skips it as a season; `Specials` and
        // `Show Extras` are season 0.
        assert_eq!(
            c("/tv/Show/Extras/Making of.mkv"),
            extra(
                "/tv/Show/Extras/Making of.mkv",
                "/tv/Show",
                ExtraKind::Unknown,
                ""
            )
        );
        assert_eq!(
            c("/tv/Show/Behind The Scenes/b.mkv"),
            extra(
                "/tv/Show/Behind The Scenes/b.mkv",
                "/tv/Show",
                ExtraKind::BehindTheScenes,
                ""
            )
        );
        assert_eq!(c("/tv/Show/Specials/S00E01.mkv"), Classified::No);
        assert_eq!(c("/tv/Show/Show Extras/S00E01.mkv"), Classified::No);
        assert_eq!(
            c("/tv/Show/Specials/trailers/t.mkv"),
            extra(
                "/tv/Show/Specials/trailers/t.mkv",
                "/tv/Show/Specials",
                ExtraKind::Trailer,
                ""
            )
        );
        // An episode's suffix files, in a season, series or episode folder.
        assert_eq!(
            c("/tv/Show/Season 1/Show S01E01-short.mkv"),
            episode_extra(
                "/tv/Show/Season 1/Show S01E01-short.mkv",
                "/tv/Show/Season 1",
                ExtraKind::Short,
                "Show S01E01"
            )
        );
        assert_eq!(
            c("/tv/Show/S01E02-featurette.mkv"),
            episode_extra(
                "/tv/Show/S01E02-featurette.mkv",
                "/tv/Show",
                ExtraKind::Featurette,
                "S01E02"
            )
        );
        assert_eq!(
            c("/tv/Show/Season 1/S01E03/S01E03-deleted.mkv"),
            episode_extra(
                "/tv/Show/Season 1/S01E03/S01E03-deleted.mkv",
                "/tv/Show/Season 1/S01E03",
                ExtraKind::DeletedScene,
                "S01E03"
            )
        );
        // A suffix whose name is not an episode's stays an episode file.
        assert_eq!(c("/tv/Show/Season 1/Pilot-short.mkv"), Classified::No);
        // An episode folder's extras folder.
        assert_eq!(
            c("/tv/Show/Season 1/S01E03/featurettes/f.mkv"),
            episode_extra(
                "/tv/Show/Season 1/S01E03/featurettes/f.mkv",
                "/tv/Show/Season 1/S01E03",
                ExtraKind::Featurette,
                ""
            )
        );
        assert_eq!(
            c("/tv/Show/Season 1/S01E03/A/featurettes/f.mkv"),
            Classified::Skip {
                owner_dir: PathBuf::from("/tv/Show/Season 1/S01E03/A/featurettes")
            }
        );
    }

    #[test]
    fn r3extras2_an_episode_extra_goes_to_the_episode_it_names() {
        let dir = Path::new("/tv/Show/Season 1");
        let files =
            |names: &[&str]| -> Vec<PathBuf> { names.iter().map(|n| dir.join(n)).collect() };
        let found = |name: &str| FoundExtra {
            path: dir.join("x.mkv"),
            owner_dir: dir.to_path_buf(),
            kind: ExtraKind::Featurette,
            owner_name: name.to_string(),
            owner_paths: Vec::new(),
            of_episode: true,
        };
        let season = files(&["Show S01E01.mkv", "Show S01E02.mkv"]);
        // By stem, in any case, else by season and episode numbers.
        assert_eq!(
            owning_episode(&found("show s01e01"), &season),
            files(&["Show S01E01.mkv"])
        );
        assert_eq!(
            owning_episode(&found("S01E02"), &season),
            files(&["Show S01E02.mkv"])
        );
        assert!(owning_episode(&found("S01E03"), &season).is_empty());
        assert!(owning_episode(&found("S02E01"), &season).is_empty());
        // An episode folder's extras: its one episode, in all its files, or none among several.
        assert!(owning_episode(&found(""), &season).is_empty());
        let one = files(&["S01E03 - part1.mkv", "S01E03 - part2.mkv"]);
        assert_eq!(owning_episode(&found(""), &one), one);
        let single = files(&["Pilot.mkv"]);
        assert_eq!(owning_episode(&found(""), &single), single);
        assert!(owning_episode(&found(""), &[]).is_empty());
    }

    #[test]
    fn r3extras2_a_renamed_or_moved_extra_is_found_by_its_stamp_or_name() {
        let row = |owner: &str, stamp: Option<(i64, i64)>| repo_extras::PathRow {
            id: "id".into(),
            extra_type: Some("Featurette".into()),
            owner_id: Some(owner.into()),
            name: "n".into(),
            stamp,
        };
        let a = row("A", Some((100, 7)));
        let b = row("A", Some((200, 7)));
        let c = row("B", None);
        let gone = [
            ("/m/A/featurettes/one.mkv", &a),
            ("/m/A/two-featurette.mkv", &b),
            ("/m/B/extras/three.mkv", &c),
        ];
        let heir = |file: &str, stamp, owner| heir_extra(&gone, Path::new(file), stamp, owner);
        // Renamed: the same size and time, even under another owner.
        assert_eq!(
            heir("/m/A/featurettes/uno.mkv", Some((100, 7)), "A"),
            Some(0)
        );
        assert_eq!(heir("/m/C/extras/dos.mkv", Some((200, 7)), "C"), Some(1));
        // A theme song is never a video's heir.
        assert_eq!(heir("/m/A/theme.mp3", Some((100, 7)), "A"), None);
        // Moved into another extras folder of its owner, its old stamp unknown: by its name.
        assert_eq!(
            heir("/m/B/featurettes/three.mkv", Some((1, 1)), "B"),
            Some(2)
        );
        assert_eq!(heir("/m/C/three.mkv", None, "C"), None, "another owner");
        // A new file is no one's heir.
        assert_eq!(heir("/m/A/featurettes/new.mkv", Some((5, 5)), "A"), None);
        // A new file with a gone one's name and another modification time (a re-download) is not its heir;
        // with no stamp on either side, the name is enough.
        assert_eq!(heir("/m/A/trailers/one.mkv", Some((100, 8)), "A"), None);
        assert_eq!(heir("/m/A/trailers/one.mkv", None, "A"), Some(0));
        // Two gone extras with the stamp: neither.
        let twin = row("A", Some((100, 7)));
        let gone = [("/m/A/x.mkv", &a), ("/m/A/y.mkv", &twin)];
        assert_eq!(
            heir_extra(&gone, Path::new("/m/A/z.mkv"), Some((100, 7)), "A"),
            None
        );
    }

    #[test]
    fn r3extras_an_extra_goes_to_the_only_movie_or_the_one_its_suffix_names() {
        let dir = Path::new("/m/Films");
        let groups = |names: &[&str]| {
            let files: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
            naming::group_movie_files(dir, &files, false)
        };
        let found = |name: &str| FoundExtra {
            path: dir.join("x.mkv"),
            owner_dir: dir.to_path_buf(),
            kind: ExtraKind::Short,
            owner_name: name.to_string(),
            owner_paths: Vec::new(),
            of_episode: false,
        };
        let owner = |groups: &[naming::VersionGroup], name: &str| {
            owning_group(&found(name), groups, false).map(|g| g.title.clone())
        };
        // One movie owns the folder-wide extras next to it, and the suffix files that name it.
        let one = groups(&["Movie (2020) 1080p.mkv"]);
        assert_eq!(owner(&one, ""), Some("Movie".to_string()));
        assert_eq!(owner(&one, "Movie (2020)"), Some("Movie".to_string()));
        assert_eq!(
            owner(&one, "Other"),
            None,
            "Other-short.mkv is a film of its own"
        );
        assert_eq!(owner(&[], ""), None);
        // Several: only a name that is one of them picks it, by file stem or by title and year.
        let several = groups(&["A.mkv", "C.mkv", "Beta (2001) 1080p.mkv"]);
        assert_eq!(owner(&several, "A"), Some("A".to_string()));
        assert_eq!(owner(&several, "beta (2001)"), Some("Beta".to_string()));
        // B-short.mkv next to A.mkv and C.mkv is a film of its own, not A's extra.
        assert_eq!(owner(&several, "B"), None);
        assert_eq!(
            owner(&several, ""),
            None,
            "trailer.mkv belongs to no one of several"
        );
    }
}
