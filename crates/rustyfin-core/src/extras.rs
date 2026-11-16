//! Extras: local trailers, special features and theme media, found the way Jellyfin finds them
//! (`NamingOptions.VideoExtraRules`). A scan stores each one as an item with an extra type and an owner
//! (the movie, series or season it belongs to); library listings never show them.
//!
//! Movies: `<movie>-trailer.mkv`, `trailer.mkv` and the other suffixes next to the movie, `theme.mp3`, and
//! the files in its extras folders (`trailers/`, `featurettes/`, `extras/`, `theme-music/`, `backdrops/`
//! and the rest). Shows: the extras folders of a series or season folder, and `theme.mp3` and `trailer.mkv`
//! in it. A show's `Extras` folder stays its specials season (`naming::is_specials_folder`).

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
    /// The paths its owner may be stored at, set by the scan from what is on disk: the files of the movie it
    /// belongs to, or the series or season folder. Empty: it has no owner.
    pub owner_paths: Vec<PathBuf>,
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
        // `Show/Extras` is the specials season.
        if n == 3 && naming::is_specials_folder(&parts[1], &parts[0]) {
            return Classified::No;
        }
        return match n {
            3 | 4 => in_extras_dir(file, parent.parent().unwrap_or(root), kind),
            _ => Classified::Skip {
                owner_dir: parent.to_path_buf(),
            },
        };
    }
    // A file directly in a series or season folder.
    if n == 2 || n == 3 {
        return by_name(file, parent, false);
    }
    Classified::No
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

    let mut new_items: Vec<(MediaItem, String, &'static str)> = Vec::new();
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
            (None, _) => {
                tracing::debug!("{path} looks like an extra but has no owner; left as it is");
            }
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
        // The specials season, and episodes with suffix-like names, are not extras.
        assert_eq!(c("/tv/Show/Extras/S00E01.mkv"), Classified::No);
        assert_eq!(c("/tv/Show/Season 1/Show S01E01-short.mkv"), Classified::No);
        assert_eq!(c("/tv/Show/Season 1/S01E01.mkv"), Classified::No);
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
