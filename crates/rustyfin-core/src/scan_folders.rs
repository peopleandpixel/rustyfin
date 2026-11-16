//! The `Folder` items of a photo (home videos) or mixed library, as Jellyfin lists a library's
//! subfolders: one per folder under a library path that holds media somewhere below it. Each file
//! and each folder points at the folder holding it through `parent_id`; one directly in a library
//! path has none, so it is listed at the top of the library. Movie and TV libraries have none.
//!
//! A scan runs `sync_folders` after it has removed what it no longer collects: it adds the folders
//! that are missing, moves files and folders under the right one, and deletes the folders that no
//! longer hold anything. A folder keeps its id while its path stays.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

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
/// path today) and its stored `folders` (id, path, parent folder's path today)
pub fn plan_folders(
    roots: &[PathBuf],
    files: &[(String, PathBuf, Option<PathBuf>)],
    folders: &[(String, PathBuf, Option<PathBuf>)],
) -> FolderPlan {
    let mut wanted: BTreeSet<PathBuf> = BTreeSet::new();
    let mut plan = FolderPlan::default();
    for (id, path, parent) in files {
        let holder = holding_folder(roots, path);
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
            gone.push(folder);
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

/// The library's items of `types` (or its folders): id, path and the path of the item it points at
async fn rows(
    pool: &SqlitePool,
    library: &Library,
    folders: bool,
) -> Result<Vec<(String, PathBuf, Option<PathBuf>)>, sqlx::Error> {
    let types = crate::scan_sync::scanned_types(&library.content_type);
    let wanted = |kind: &str| {
        if folders {
            kind == "Folder"
        } else {
            types.contains(&kind)
        }
    };
    let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT m.id, m.path, m.media_type, p.path FROM media_items AS m
         LEFT JOIN media_items AS p ON p.id = m.parent_id
         WHERE m.library_id = ?1 AND m.extra_type IS NULL",
    )
    .bind(library.id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, kind, _)| wanted(kind))
        .map(|(id, path, _, parent)| (id, PathBuf::from(path), parent.map(PathBuf::from)))
        .collect())
}

/// Brings the library's `Folder` items in line with its files (see the module comment). Returns how
/// many folders were added and deleted. Does nothing for a library that doesn't browse folders.
pub async fn sync_folders(pool: &SqlitePool, library: &Library) -> Result<(u32, u32), sqlx::Error> {
    if !browses_folders(&library.content_type) {
        return Ok((0, 0));
    }
    let roots: Vec<PathBuf> = library
        .paths
        .iter()
        .map(|p| PathBuf::from(p.trim_end_matches('/')))
        .collect();
    let files = rows(pool, library, false).await?;
    let folders = rows(pool, library, true).await?;
    let plan = plan_folders(&roots, &files, &folders);

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
            id: Uuid::new_v4(),
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
        let plan = plan_folders(&roots, &files, &[]);
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
        let plan = plan_folders(&roots, &files, &folders);
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
        let plan = plan_folders(&[p("/lib")], &[file("a", "/elsewhere/x/y.jpg", None)], &[]);
        assert_eq!(plan, FolderPlan::default());
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
