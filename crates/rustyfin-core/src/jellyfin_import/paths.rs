//! Path pairs (spec 2.8): translation by the longest component prefix, and the suggested pairs
//! from Jellyfin's library folders.

use std::collections::HashMap;
use std::path::Path;

use rustyfin_model::library::Library;
use serde::{Deserialize, Serialize};

use super::source::LibraryFolder;

/// One path pair: Jellyfin's `From` prefix stands for Rustyfin's `To`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PathMapping {
    pub from: String,
    pub to: String,
}

/// The pair with the longest `From` that is a component-wise prefix of `path` (trailing slashes
/// ignored; the first of equal lengths), and the rest of `path` below that prefix.
pub(super) fn pair_for<'a>(
    path: &str,
    pairs: &'a [PathMapping],
) -> Option<(&'a PathMapping, String)> {
    let path = path.trim_end_matches('/');
    let mut best: Option<(&PathMapping, usize, &str)> = None;
    for pair in pairs.iter().filter(|pair| !pair.from.is_empty()) {
        let from = pair.from.trim_end_matches('/');
        let Some(rest) = strip_components(path, from) else {
            continue;
        };
        if best.is_none_or(|(_, longest, _)| from.len() > longest) {
            best = Some((pair, from.len(), rest));
        }
    }
    best.map(|(pair, _, rest)| (pair, rest.to_string()))
}

/// Translates a Jellyfin path by the pair with the longest `From` that is a component-wise prefix
/// of `path` (trailing slashes ignored); `None` when no pair applies.
pub fn translate_path(path: &str, pairs: &[PathMapping]) -> Option<String> {
    let (pair, rest) = pair_for(path, pairs)?;
    let translated = format!("{}{rest}", pair.to.trim_end_matches('/'));
    Some(if translated.is_empty() {
        "/".to_string()
    } else {
        translated
    })
}

/// The rest of `path` below `from`, when `from` is a whole-component prefix of it.
fn strip_components<'a>(path: &'a str, from: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(from)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

/// The suggested pairs (spec 2.8) and the Jellyfin library paths that none of them pairs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestions {
    pub pairs: Vec<PathMapping>,
    pub unpaired: Vec<String>,
}

/// Suggests the pairs for `folders` (Jellyfin's `root/default/<Library>/`, sorted by name), then
/// `%MetadataPath%` and `%AppDataPath%`.
pub fn suggest(
    folders: &[LibraryFolder],
    libraries: &[Library],
    program_data: &Path,
    data_path: &Path,
) -> Suggestions {
    let mut pairs = Vec::new();
    let mut unpaired = Vec::new();
    for folder in folders {
        let library = choose_library(folder, libraries);
        for link in &folder.mblinks {
            match library.and_then(|library| pick_path(library, link)) {
                Some(to) => pairs.push(PathMapping {
                    from: link.clone(),
                    to,
                }),
                None => unpaired.push(link.clone()),
            }
        }
    }
    pairs.push(PathMapping {
        from: "%MetadataPath%".to_string(),
        to: program_data.join("metadata").display().to_string(),
    });
    pairs.push(PathMapping {
        from: "%AppDataPath%".to_string(),
        to: data_path.display().to_string(),
    });
    Suggestions { pairs, unpaired }
}

/// The Rustyfin library a Jellyfin library folder pairs with: the one of the same name (ambiguous
/// names give none), else the only one whose content type is the folder's `<type>.collection`.
fn choose_library<'a>(folder: &LibraryFolder, libraries: &'a [Library]) -> Option<&'a Library> {
    let wanted = folder.name.to_lowercase();
    let same_name: Vec<&Library> = libraries
        .iter()
        .filter(|library| library.name.to_lowercase() == wanted)
        .collect();
    match same_name.as_slice() {
        [only] => return Some(*only),
        [_, _, ..] => return None,
        [] => {}
    }
    let collection = folder.collection_type.as_deref()?;
    let typed: Vec<&Library> = libraries
        .iter()
        .filter(|library| library.content_type.jellyfin_collection_type() == collection)
        .collect();
    match typed.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The path of `library` that a Jellyfin media path pairs with: the one whose last component
/// matches (case-insensitively), else the library's only path.
fn pick_path(library: &Library, link: &str) -> Option<String> {
    let last = last_component(link).to_lowercase();
    let same_last: Vec<&String> = library
        .paths
        .iter()
        .filter(|path| last_component(path).to_lowercase() == last)
        .collect();
    if let [only] = same_last.as_slice() {
        return Some((*only).clone());
    }
    match library.paths.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// The last component of a path, ignoring trailing slashes.
fn last_component(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
}

/// The pair for Jellyfin's metadata folder (spec 5.9): among `image_paths` (absolute, without `..`),
/// the prefix up to and including the first component named `metadata` that most paths share (ties:
/// the shortest, then the smallest). Only when `<program data>/metadata` is a folder and no pair
/// covers that prefix.
pub fn suggest_metadata(
    image_paths: &[&str],
    program_data: &Path,
    pairs: &[PathMapping],
) -> Option<PathMapping> {
    let local = program_data.join("metadata");
    if !local.is_dir() {
        return None;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for path in image_paths {
        if !path.starts_with('/') || path.split('/').any(|part| part == "..") {
            continue;
        }
        let parts: Vec<&str> = path.split('/').collect();
        if let Some(index) = parts.iter().position(|part| *part == "metadata") {
            *counts.entry(parts[..=index].join("/")).or_default() += 1;
        }
    }
    let (prefix, _) = counts.into_iter().max_by(|(a, count_a), (b, count_b)| {
        count_a
            .cmp(count_b)
            .then_with(|| b.len().cmp(&a.len()))
            .then_with(|| b.cmp(a))
    })?;
    if translate_path(&prefix, pairs).is_some() {
        return None;
    }
    Some(PathMapping {
        from: prefix,
        to: local.display().to_string(),
    })
}
