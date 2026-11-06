//! The pure parts of the *arr Webhook. Sonarr, Radarr and Lidarr post one JSON event per change:
//! `changed_paths` lists the paths that an event names, and `PathMap` maps the apps' paths to this
//! server's. The endpoints are in `routes::library_monitor`.

use std::collections::HashSet;

use serde_json::Value;

/// The paths that a Webhook event names, each once, at its first place. The order depends on the
/// event type. An unknown event, or a payload that is not an object, names none.
pub fn changed_paths(payload: &Value) -> Vec<String> {
    let mut paths = Paths::default();
    match payload["eventType"].as_str() {
        Some("Download") => {
            add_path(&mut paths, &payload["episodeFile"]["path"]);
            add_paths(&mut paths, &payload["episodeFiles"], &["path"]);
            add_path(&mut paths, &payload["movieFile"]["path"]);
            add_paths(&mut paths, &payload["trackFiles"], &["path"]);
            add_paths(&mut paths, &payload["deletedFiles"], &["path"]);
        }
        Some("Rename") => {
            add_paths(
                &mut paths,
                &payload["renamedEpisodeFiles"],
                &["previousPath", "path"],
            );
            add_paths(
                &mut paths,
                &payload["renamedMovieFiles"],
                &["previousPath", "path"],
            );
            add_paths(
                &mut paths,
                &payload["renamedTrackFiles"],
                &["previousPath", "path"],
            );
        }
        Some("EpisodeFileDelete") => add_path(&mut paths, &payload["episodeFile"]["path"]),
        Some("MovieFileDelete") => add_path(&mut paths, &payload["movieFile"]["path"]),
        Some("SeriesDelete") => add_path(&mut paths, &payload["series"]["path"]),
        Some("MovieDelete") => add_path(&mut paths, &payload["movie"]["folderPath"]),
        Some("ArtistDelete" | "AlbumDelete") => add_path(&mut paths, &payload["artist"]["path"]),
        _ => {}
    }
    paths.list
}

/// The paths of one event, each once, in the order in which they were first added.
#[derive(Default)]
struct Paths {
    list: Vec<String>,
    /// The paths of `list`, so that a path is looked up without scanning `list`.
    seen: HashSet<String>,
}

/// Adds `value` to `paths` when it is a non-empty string that `paths` does not hold yet.
fn add_path(paths: &mut Paths, value: &Value) {
    let Some(path) = value.as_str() else {
        return;
    };
    if !path.is_empty() && !paths.seen.contains(path) {
        paths.seen.insert(path.to_string());
        paths.list.push(path.to_string());
    }
}

/// Adds the values at `keys` of each entry of the array `list`, entry by entry. A value that is not
/// an array adds nothing.
fn add_paths(paths: &mut Paths, list: &Value, keys: &[&str]) {
    for entry in list.as_array().into_iter().flatten() {
        for key in keys {
            add_path(paths, &entry[*key]);
        }
    }
}

/// The `from` and `to` pairs of a Webhook URL. A path that starts with a `from` takes the `to` in
/// its place.
#[derive(Debug, Clone)]
pub struct PathMap {
    pairs: Vec<(String, String)>,
}

impl PathMap {
    /// The pairs of the query string `query`: the i-th `from` pairs with the i-th `to`, and every
    /// other key is ignored. Each `from` loses its trailing `/` and `\`. Unequal counts, or a `from`
    /// that is empty after that, are errors.
    pub fn from_query(query: &str) -> Result<Self, String> {
        let mut froms = Vec::new();
        let mut tos = Vec::new();
        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            match &*key {
                "from" => froms.push(value.into_owned()),
                "to" => tos.push(value.into_owned()),
                _ => {}
            }
        }
        if froms.len() != tos.len() {
            return Err(format!(
                "the URL has {} from and {} to values; each from needs one to",
                froms.len(),
                tos.len()
            ));
        }
        let mut pairs = Vec::with_capacity(froms.len());
        for (from, to) in froms.into_iter().zip(tos) {
            let trimmed = from.trim_end_matches(['/', '\\']);
            if trimmed.is_empty() {
                return Err(format!("the from {from:?} names no path"));
            }
            pairs.push((trimmed.to_string(), to));
        }
        Ok(Self { pairs })
    }

    /// `path` with the pair applied that matches it, or `path` unchanged when no pair does. A pair
    /// matches when its `from` is the whole path or is followed by `/` or `\`. The longest `from`
    /// wins, and of equal ones the first. When `to` starts with `/`, the `\` after the `from` become
    /// `/`.
    pub fn apply(&self, path: &str) -> String {
        let matches = self.pairs.iter().filter_map(|(from, to)| {
            let rest = path.strip_prefix(from.as_str())?;
            (rest.is_empty() || rest.starts_with(['/', '\\'])).then_some((
                from.len(),
                to.as_str(),
                rest,
            ))
        });
        match matches.min_by_key(|(len, _, _)| std::cmp::Reverse(*len)) {
            Some((_, to, rest)) if to.starts_with('/') => {
                format!("{to}{}", rest.replace('\\', "/"))
            }
            Some((_, to, rest)) => format!("{to}{rest}"),
            None => path.to_string(),
        }
    }
}
