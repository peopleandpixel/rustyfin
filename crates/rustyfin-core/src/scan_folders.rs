//! The `Folder` items of a photo (home videos) or mixed library, as Jellyfin lists a library's
//! subfolders: one per folder under a library path that holds media somewhere below it. Each file
//! and each folder points at the folder holding it through `parent_id`; one directly in a library
//! path has none, so it is listed at the top of the library. Movie and TV libraries have none.
//!
//! A scan runs `sync_folders` after it has removed what it no longer collects: it adds the folders
//! that are missing, moves files and folders under the right one, and deletes the folders that no
//! longer hold anything. A folder's id comes from its library and path (UUID v5), so it is the same
//! whenever the folder is listed again, even after it briefly held nothing.
//!
//! A scan of part of a library (one folder, or the files directly in a library path) looks only
//! at the files in that part, the folders in it and the folders above it, and deletes only folders
//! in that part.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use rustyfin_db::repo_media::PathScope;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Whether a library of this content type lists its subfolders as `Folder` items
pub fn browses_folders(content_type: &LibraryContentType) -> bool {
    matches!(
        content_type,
        LibraryContentType::Photos | LibraryContentType::Mixed
    )
}

/// The folder holding `path`, when that folder is under one of `roots` and isn't a root itself
/// (a file directly in a library path has no parent folder)
fn holding_folder(roots: &[PathBuf], path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    roots
        .iter()
        .any(|root| dir != root && dir.starts_with(root))
        .then(|| dir.to_path_buf())
}

/// What a folder pass changes: the folders to add (parents before their children, so a parent's
/// row exists first), each item's new parent, and the folders to delete (children first)
#[derive(Debug, Default, PartialEq)]
pub struct FolderPlan {
    pub add: Vec<PathBuf>,
    /// (item id, new parent folder path, or None for the library's top)
    pub move_to: Vec<(String, Option<PathBuf>)>,
    pub delete: Vec<String>,
}

/// Plans the folders of a library with these `roots`, from its `files` (id, path, parent folder's
/// path today) and its stored `folders` (id, path, parent folder's path today). Only a folder that
/// `deletable` accepts is deleted when no file needs it; any other is left as it is.
///
/// As Jellyfin 12.4 resolves a mixed library (`MovieResolver.FindMovie`), a folder that holds one
/// movie (an id in `movies`) and nothing else, here or below, is that movie rather than a folder:
/// it gets no Folder item, and the movie is listed in the folder above.
pub fn plan_folders(
    roots: &[PathBuf],
    files: &[(String, PathBuf, Option<PathBuf>)],
    folders: &[(String, PathBuf, Option<PathBuf>)],
    movies: &HashSet<String>,
    deletable: impl Fn(&Path) -> bool,
) -> FolderPlan {
    // The folders that stand for their movie: each holds exactly one item, a movie, and no file
    // is below it
    let mut direct: HashMap<PathBuf, (usize, bool)> = HashMap::new();
    let mut deeper: HashSet<PathBuf> = HashSet::new();
    for (id, path, _) in files {
        let Some(holder) = holding_folder(roots, path) else {
            continue;
        };
        let entry = direct.entry(holder.clone()).or_insert((0, true));
        entry.0 += 1;
        entry.1 &= movies.contains(id);
        let mut dir = holding_folder(roots, &holder);
        while let Some(folder) = dir {
            dir = holding_folder(roots, &folder);
            deeper.insert(folder);
        }
    }
    let movie_folder = |dir: &PathBuf| direct.get(dir) == Some(&(1, true)) && !deeper.contains(dir);

    let mut wanted: BTreeSet<PathBuf> = BTreeSet::new();
    let mut plan = FolderPlan::default();
    for (id, path, parent) in files {
        let mut holder = holding_folder(roots, path);
        if holder.as_ref().is_some_and(movie_folder) {
            holder = holder.and_then(|dir| holding_folder(roots, &dir));
        }
        let mut dir = holder.clone();
        while let Some(folder) = dir {
            dir = holding_folder(roots, &folder);
            wanted.insert(folder);
        }
        if *parent != holder {
            plan.move_to.push((id.clone(), holder));
        }
    }
    let stored: HashMap<&PathBuf, &String> = folders.iter().map(|(id, p, _)| (p, id)).collect();
    // A BTreeSet walks a parent before its children, as a path sorts before its extensions
    plan.add = wanted
        .iter()
        .filter(|p| !stored.contains_key(p))
        .cloned()
        .collect();
    let mut gone: Vec<&(String, PathBuf, Option<PathBuf>)> = Vec::new();
    for folder @ (id, path, parent) in folders {
        if !wanted.contains(path) {
            if deletable(path) {
                gone.push(folder);
            }
            continue;
        }
        let holder = holding_folder(roots, path);
        if *parent != holder {
            plan.move_to.push((id.clone(), holder));
        }
    }
    // Deepest first, so a folder never outlives a child that is deleted after it
    gone.sort_by_key(|(_, path, _)| std::cmp::Reverse(path.components().count()));
    plan.delete = gone.into_iter().map(|(id, _, _)| id.clone()).collect();
    plan
}

type Row = (String, PathBuf, Option<PathBuf>);

/// The library's files and folders that a pass over `scope` reads, in one query: the items in
/// scope, and the folders above it. Each is (id, path, path of the item it points at). The ids of
/// the files that are movies come third.
async fn rows(
    pool: &SqlitePool,
    library: &Library,
    scope: &PathScope,
) -> Result<(Vec<Row>, Vec<Row>, HashSet<String>), sqlx::Error> {
    let types = crate::scan_sync::scanned_types(&library.content_type);
    let rows: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT m.id, m.path, m.media_type, p.path FROM media_items AS m
             LEFT JOIN media_items AS p ON p.id = m.parent_id
             WHERE m.library_id = ?1 AND m.extra_type IS NULL
               AND ({in_scope}
                    OR (m.media_type = 'Folder' AND substr(?2, 1, length(m.path) + 1) = m.path || '/'))",
            in_scope = scope.sql("m.path", 2),
        )))
        .bind(library.id.to_string())
        .bind(scope.param())
        .fetch_all(pool)
        .await?;
    let (mut files, mut folders, mut movies) = (Vec::new(), Vec::new(), HashSet::new());
    for (id, path, kind, parent) in rows {
        if kind == "Movie" {
            movies.insert(id.clone());
        }
        let row = (id, PathBuf::from(path), parent.map(PathBuf::from));
        if kind == "Folder" {
            folders.push(row);
        } else if types.contains(&kind.as_str()) {
            files.push(row);
        }
    }
    Ok((files, folders, movies))
}

/// A folder's id: a UUID v5 of its path, in the namespace of its library's id
pub fn folder_id(library: Uuid, path: &Path) -> Uuid {
    Uuid::new_v5(&library, path.to_string_lossy().as_bytes())
}

/// Brings the library's `Folder` items in line with its files (see the module comment). Returns how
/// many folders were added and deleted. Does nothing for a library that doesn't browse folders.
pub async fn sync_folders(
    pool: &SqlitePool,
    library: &Library,
    scope: &PathScope,
) -> Result<(u32, u32), sqlx::Error> {
    if !browses_folders(&library.content_type) {
        return Ok((0, 0));
    }
    let roots: Vec<PathBuf> = library
        .paths
        .iter()
        .map(|p| PathBuf::from(p.trim_end_matches('/')))
        .collect();
    let (files, folders, movies) = rows(pool, library, scope).await?;
    // The files directly in a library path have no folder: a pass over them deletes none
    let plan = plan_folders(&roots, &files, &folders, &movies, |path| match scope {
        PathScope::All => true,
        PathScope::Under(_) => scope.contains(&path.to_string_lossy()),
        PathScope::DirectlyIn(_) => false,
    });

    let mut ids: HashMap<PathBuf, Uuid> = folders
        .iter()
        .filter_map(|(id, path, _)| Some((path.clone(), id.parse().ok()?)))
        .collect();
    let mut new_folders = Vec::with_capacity(plan.add.len());
    for path in &plan.add {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let parent_id = holding_folder(&roots, path).and_then(|p| ids.get(&p).copied());
        let item = MediaItem {
            id: folder_id(library.id, path),
            sort_name: crate::scanner::auto_sort_name(MediaType::Folder, &name, None, None),
            name,
            media_type: MediaType::Folder,
            path: path.to_string_lossy().into_owned(),
            parent_id,
            library_id: library.id,
            date_created: crate::scanner::date_added(path).await,
            ..MediaItem::default_dto_fields()
        };
        ids.insert(path.clone(), item.id);
        new_folders.push(item);
    }
    if !new_folders.is_empty() {
        crate::scanner::upsert_items(pool, &new_folders).await?;
    }

    if !plan.move_to.is_empty() {
        let mut tx = rustyfin_db::begin_write(pool).await?;
        for (id, parent) in &plan.move_to {
            let parent = parent
                .as_ref()
                .and_then(|p| ids.get(p))
                .map(Uuid::to_string);
            sqlx::query("UPDATE media_items SET parent_id = ?1 WHERE id = ?2")
                .bind(parent)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
    }

    let mut deleted = 0;
    for id in &plan.delete {
        if crate::scan_sync::delete_item(pool, id).await? {
            deleted += 1;
        }
    }
    Ok((new_folders.len() as u32, deleted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn file(id: &str, path: &str, parent: Option<&str>) -> (String, PathBuf, Option<PathBuf>) {
        (id.to_string(), p(path), parent.map(p))
    }

    #[test]
    fn folders_are_added_parents_first_and_files_moved_under_them() {
        let roots = [p("/lib"), p("/more")];
        let files = [
            file("a", "/lib/top.jpg", None),
            file("b", "/lib/Trips/2024/x.jpg", None),
            file("c", "/lib/Trips/y.mp4", None),
            file("d", "/more/Family/z.jpg", None),
        ];
        let plan = plan_folders(&roots, &files, &[], &HashSet::new(), |_| true);
        assert_eq!(
            plan.add,
            [p("/lib/Trips"), p("/lib/Trips/2024"), p("/more/Family")]
        );
        assert_eq!(
            plan.move_to,
            [
                ("b".to_string(), Some(p("/lib/Trips/2024"))),
                ("c".to_string(), Some(p("/lib/Trips"))),
                ("d".to_string(), Some(p("/more/Family"))),
            ]
        );
        assert!(plan.delete.is_empty());
    }

    #[test]
    fn stored_folders_are_kept_moved_or_deleted() {
        let roots = [p("/lib")];
        let files = [
            file("a", "/lib/Trips/2024/x.jpg", Some("/lib/Trips/2024")),
            file("b", "/lib/moved.jpg", Some("/lib/Old")),
        ];
        let folders = [
            file("f1", "/lib/Trips", None),
            // Parent wrong: it is under Trips
            file("f2", "/lib/Trips/2024", None),
            file("f3", "/lib/Old", None),
            file("f4", "/lib/Old/Deeper", Some("/lib/Old")),
        ];
        let plan = plan_folders(&roots, &files, &folders, &HashSet::new(), |_| true);
        assert!(plan.add.is_empty());
        assert_eq!(
            plan.move_to,
            [
                ("b".to_string(), None),
                ("f2".to_string(), Some(p("/lib/Trips"))),
            ]
        );
        assert_eq!(plan.delete, ["f4", "f3"]);
    }

    #[test]
    fn a_file_outside_every_root_gets_no_folder() {
        let plan = plan_folders(
            &[p("/lib")],
            &[file("a", "/elsewhere/x/y.jpg", None)],
            &[],
            &HashSet::new(),
            |_| true,
        );
        assert_eq!(plan, FolderPlan::default());
    }

    #[test]
    fn a_folder_holding_one_movie_is_that_movie() {
        // As Jellyfin 12.4 resolves a mixed library: Film/ holds one movie and is the movie;
        // Two/ holds two movies, and Mixed/ a movie and a photo, so both are folders; Old/ held
        // nothing but its movie, so its stored Folder goes
        let roots = [p("/lib")];
        let files = [
            file("m1", "/lib/Film (2001)/Film (2001).mkv", None),
            file("m2", "/lib/Two/a.mkv", None),
            file("m3", "/lib/Two/b.mkv", None),
            file("m4", "/lib/Mixed/c.mkv", None),
            file("p1", "/lib/Mixed/d.jpg", None),
            file("m5", "/lib/Old/e.mkv", Some("/lib/Old")),
            file("m6", "/lib/Shelf/Inner (2003)/f.mkv", None),
        ];
        let folders = [file("f1", "/lib/Old", None)];
        let movies: HashSet<String> = ["m1", "m2", "m3", "m4", "m5", "m6"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let plan = plan_folders(&roots, &files, &folders, &movies, |_| true);
        assert_eq!(plan.add, [p("/lib/Mixed"), p("/lib/Shelf"), p("/lib/Two")]);
        assert_eq!(plan.delete, ["f1"]);
        let moved: HashMap<String, Option<PathBuf>> = plan.move_to.into_iter().collect();
        assert_eq!(moved.get("m1"), None, "stays at the top");
        assert_eq!(moved["m5"], None, "moves up to the top");
        assert_eq!(moved["m2"], Some(p("/lib/Two")));
        assert_eq!(moved["m4"], Some(p("/lib/Mixed")));
        assert_eq!(
            moved["m6"],
            Some(p("/lib/Shelf")),
            "its own folder stands for it"
        );
    }

    #[test]
    fn a_scoped_pass_deletes_only_folders_in_its_scope() {
        // A scan of /lib/Trips/2024 sees no files: 2024 goes, Trips above it stays
        let folders = [
            file("f1", "/lib/Trips", None),
            file("f2", "/lib/Trips/2024", Some("/lib/Trips")),
        ];
        let scope = PathScope::Under("/lib/Trips/2024".to_string());
        let plan = plan_folders(&[p("/lib")], &[], &folders, &HashSet::new(), |path| {
            scope.contains(&path.to_string_lossy())
        });
        assert_eq!(plan.delete, ["f2"]);
        assert!(plan.add.is_empty() && plan.move_to.is_empty());
    }

    #[test]
    fn a_folder_id_follows_its_library_and_path() {
        let (lib, other) = (Uuid::new_v4(), Uuid::new_v4());
        let id = folder_id(lib, Path::new("/lib/Trips"));
        assert_eq!(id, folder_id(lib, Path::new("/lib/Trips")));
        assert_ne!(id, folder_id(lib, Path::new("/lib/Trips/2024")));
        assert_ne!(id, folder_id(other, Path::new("/lib/Trips")));
    }

    #[test]
    fn only_photo_and_mixed_libraries_browse_folders() {
        assert!(browses_folders(&LibraryContentType::Photos));
        assert!(browses_folders(&LibraryContentType::Mixed));
        assert!(!browses_folders(&LibraryContentType::Movies));
        assert!(!browses_folders(&LibraryContentType::TvShows));
        assert!(!browses_folders(&LibraryContentType::Music));
    }
}
