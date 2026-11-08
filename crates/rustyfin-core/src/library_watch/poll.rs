//! Polling for a library path that cannot be watched: one stat per known folder per poll.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::naming::is_ignored_dir;

/// Files of a folder: name -> (size, modified time).
type Listing = BTreeMap<OsString, (u64, SystemTime)>;
/// Device and inode, which tell apart a folder reached under two names.
type Id = (u64, u64);

struct Folder {
    modified: SystemTime,
    id: Option<Id>,
    /// Non-ignored subfolder names, including the ones skipped as already known.
    subfolders: BTreeSet<OsString>,
    /// Files as of the last listing.
    files: Listing,
    /// Files as of the last report; settling and the library path compare against it.
    reported: Listing,
    /// Files are listed at every poll until they hold still.
    settling: bool,
}

/// What `walk` did with a folder.
enum Walked {
    /// Recorded, with the folders under it.
    Recorded,
    /// Skipped by the guard: its `(dev, ino)` is already known under another name (a second
    /// link, a symlink loop), or it is the data dir.
    Known,
    /// Not recorded, since it could not be read.
    Failed,
}

/// Polls the folders under a library path for changes.
pub struct FolderPoller {
    root: PathBuf,
    /// The data dir's `(dev, ino)`, taken once in `new`; `None` when it is unknown.
    data_id: Option<Id>,
    /// The data dir's canonical path, taken once in `new` where there is no `(dev, ino)`.
    data_canon: Option<PathBuf>,
    folders: BTreeMap<PathBuf, Folder>,
    available: bool,
}

impl FolderPoller {
    pub fn new(root: PathBuf, data_dir: Option<PathBuf>) -> Self {
        let data_id = data_dir
            .as_deref()
            .and_then(|dir| fs::metadata(dir).ok())
            .and_then(|meta| identity(&meta));
        let data_canon = match data_id {
            Some(_) => None,
            None => data_dir
                .as_deref()
                .and_then(|dir| fs::canonicalize(dir).ok()),
        };
        Self {
            root,
            data_id,
            data_canon,
            folders: BTreeMap::new(),
            available: true,
        }
    }

    /// One poll. Returns the paths to report, sorted and without duplicates.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        let root = self.root.clone();
        let mut out = Vec::new();
        if !self.has_baseline() {
            // The data dir is in the guard from the start, so the walk leaves it out.
            let mut ids: HashSet<Id> = self.data_id.into_iter().collect();
            if matches!(self.walk(&root, &mut ids, false), Walked::Recorded) {
                self.set_available();
            } else {
                self.set_unavailable();
            }
            return out;
        }

        // Changed and gone folders first, parents before children; the library path is first.
        let mut new = Vec::new();
        let paths: Vec<PathBuf> = self.folders.keys().cloned().collect();
        for path in paths {
            if !self.folders.contains_key(&path) {
                continue; // dropped with an ancestor
            }
            let is_root = path == root;
            let modified = match fs::metadata(&path).and_then(|meta| meta.modified()) {
                Ok(modified) => modified,
                Err(_) if is_root => {
                    self.set_unavailable();
                    return out;
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    self.drop_subtree(&path);
                    out.push(path);
                    continue;
                }
                Err(_) => continue,
            };
            let Some(folder) = self.folders.get(&path) else {
                continue;
            };
            let changed = modified != folder.modified || (is_root && !self.available);
            if !changed && !folder.settling {
                continue;
            }
            let (subs, files) = match self.list(&path, true) {
                Ok(listed) => listed,
                Err(_) if is_root => {
                    self.set_unavailable();
                    return out;
                }
                Err(_) => continue, // kept as it was; checked again at the next poll
            };
            if is_root {
                let folder = &self.folders[&path];
                let emptied = subs.is_empty()
                    && files.is_empty()
                    && (!folder.subfolders.is_empty() || !folder.reported.is_empty());
                if emptied {
                    self.set_unavailable();
                    return out;
                }
                self.set_available();
            }

            if changed {
                let Some(folder) = self.folders.get(&path) else {
                    continue;
                };
                let gone: Vec<OsString> = folder.subfolders.difference(&subs).cloned().collect();
                let added: Vec<OsString> = subs.difference(&folder.subfolders).cloned().collect();
                let report: Vec<PathBuf> = if is_root {
                    changed_names(&folder.reported, &files)
                        .into_iter()
                        .map(|name| path.join(name))
                        .collect()
                } else {
                    vec![path.clone()]
                };
                for name in gone {
                    let child = path.join(&name);
                    if self.folders.contains_key(&child) {
                        self.drop_subtree(&child);
                        out.push(child);
                    }
                }
                new.extend(added.iter().map(|name| path.join(name)));
                out.extend(report);
                if let Some(folder) = self.folders.get_mut(&path) {
                    folder.modified = modified;
                    folder.subfolders = subs;
                    folder.reported = files.clone();
                    folder.files = files;
                    folder.settling = true;
                }
            } else if let Some(folder) = self.folders.get_mut(&path) {
                // A copy tool can put the folder's time back after making a subfolder, so the
                // names are compared here too.
                new.extend(
                    subs.difference(&folder.subfolders)
                        .map(|name| path.join(name)),
                );
                folder.subfolders = subs;
                if files != folder.files {
                    folder.files = files; // still moving; checked again at the next poll
                } else {
                    folder.settling = false;
                    if files != folder.reported {
                        if is_root {
                            let names = changed_names(&folder.reported, &files);
                            out.extend(names.into_iter().map(|name| path.join(name)));
                        } else {
                            out.push(path.clone());
                        }
                        folder.reported = files;
                    }
                    if !is_root {
                        // Only the library path keeps its listings once it holds still.
                        folder.files.clear();
                        folder.reported.clear();
                    }
                }
            }
        }

        // New folders are walked after the drops, so a folder moved within the library is
        // walked under its new path. Known folders stay in the guard.
        if !new.is_empty() {
            let mut ids: HashSet<Id> = self
                .folders
                .values()
                .filter_map(|f| f.id)
                .chain(self.data_id)
                .collect();
            for path in new {
                match self.walk(&path, &mut ids, true) {
                    Walked::Recorded => out.push(path),
                    // A second name for a known folder, or the data dir: it stays known by
                    // name and is not reported.
                    Walked::Known => {}
                    Walked::Failed => self.forget(&path),
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn has_baseline(&self) -> bool {
        !self.folders.is_empty()
    }

    /// Folders in the baseline, the library path included.
    pub fn folders(&self) -> usize {
        self.folders.len()
    }

    /// False while the last poll found the library path unavailable.
    pub fn available(&self) -> bool {
        self.available
    }

    /// Folders whose files are being checked until they hold still.
    pub fn settling(&self) -> usize {
        self.folders.values().filter(|f| f.settling).count()
    }

    /// Records `path` and the folders under it, unless the guard skips it. Folders found after
    /// the baseline (`fresh`) start settling. File listings are kept for the library path and
    /// for folders that start settling; the other folders keep none.
    fn walk(&mut self, path: &Path, ids: &mut HashSet<Id>, fresh: bool) -> Walked {
        let Ok(meta) = fs::metadata(path) else {
            return Walked::Failed;
        };
        let id = identity(&meta);
        // The library path is never skipped, not even when it is the data dir.
        let is_root = path == self.root.as_path();
        if !is_root && self.guarded(path, id, ids) {
            return Walked::Known;
        }
        let Ok(modified) = meta.modified() else {
            return Walked::Failed;
        };
        let Ok((mut subfolders, files)) = self.list(path, fresh || is_root) else {
            return Walked::Failed;
        };
        ids.extend(id);
        let mut failed = Vec::new();
        for name in &subfolders {
            let child = path.join(name);
            if matches!(self.walk(&child, ids, fresh), Walked::Failed) {
                failed.push(name.clone());
            }
        }
        for name in &failed {
            subfolders.remove(name); // retried when the parent changes again
        }
        self.folders.insert(
            path.to_path_buf(),
            Folder {
                modified,
                id,
                subfolders,
                reported: files.clone(),
                files,
                settling: fresh,
            },
        );
        Walked::Recorded
    }

    /// Whether `path` is the data dir, or a folder whose `(dev, ino)` is already in `ids`. Where
    /// there is no `(dev, ino)`, the data dir is recognised by its canonical path.
    fn guarded(&self, path: &Path, id: Option<Id>, ids: &HashSet<Id>) -> bool {
        match id {
            Some(id) => ids.contains(&id),
            None => self.data_canon.is_some() && fs::canonicalize(path).ok() == self.data_canon,
        }
    }

    /// Names of the non-ignored subfolders of `dir`, and its files when `want_files` is set.
    /// Folders are told apart by the entry's type and are not stat'ed. A symlink is stat'ed
    /// once, to follow it. Symlinked folders are followed on Unix only.
    fn list(&self, dir: &Path, want_files: bool) -> io::Result<(BTreeSet<OsString>, Listing)> {
        let mut subfolders = BTreeSet::new();
        let mut files = Listing::new();
        for entry in fs::read_dir(dir)? {
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let symlink = kind.is_symlink();
            // A symlink is stat'ed to follow it. A file is stat'ed only when it is listed.
            let meta = if symlink || (want_files && !kind.is_dir()) {
                let Ok(meta) = fs::metadata(entry.path()) else {
                    continue;
                };
                Some(meta)
            } else {
                None
            };
            let is_dir = meta.as_ref().map_or(kind.is_dir(), Metadata::is_dir);
            if is_dir {
                let ignored = name.to_str().is_some_and(is_ignored_dir);
                if !ignored && !(cfg!(not(unix)) && symlink) {
                    subfolders.insert(name);
                }
            } else if let Some(meta) = meta.filter(|_| want_files) {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                files.insert(name, (meta.len(), modified));
            }
        }
        Ok((subfolders, files))
    }

    /// Drops `path` and everything under it, and forgets its name in the parent.
    fn drop_subtree(&mut self, path: &Path) {
        let under: Vec<PathBuf> = self
            .folders
            .range(path.to_path_buf()..)
            .take_while(|(p, _)| p.starts_with(path))
            .map(|(p, _)| p.clone())
            .collect();
        for p in &under {
            self.folders.remove(p);
        }
        self.forget(path);
    }

    /// Forgets the name of `path` in its parent, so it counts as new if it appears again.
    fn forget(&mut self, path: &Path) {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if let Some(folder) = self.folders.get_mut(parent) {
            folder.subfolders.remove(name);
        }
    }

    fn set_unavailable(&mut self) {
        if self.available {
            tracing::warn!(
                "library monitor: '{}' is missing or unreadable; keeping its last state until it is back",
                self.root.display()
            );
            self.available = false;
        }
    }

    fn set_available(&mut self) {
        if !self.available {
            tracing::info!("library monitor: '{}' is back", self.root.display());
            self.available = true;
        }
    }
}

#[cfg(unix)]
fn identity(meta: &Metadata) -> Option<Id> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity(_meta: &Metadata) -> Option<Id> {
    None
}

/// Names of the files that were added, removed or changed between two listings.
fn changed_names(old: &Listing, new: &Listing) -> Vec<OsString> {
    let names: BTreeSet<&OsString> = old.keys().chain(new.keys()).collect();
    names
        .into_iter()
        .filter(|name| old.get(*name) != new.get(*name))
        .cloned()
        .collect()
}
