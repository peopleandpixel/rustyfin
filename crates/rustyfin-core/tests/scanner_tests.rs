//! Library scans of real files in a temp dir: ignored folders and files (R1-12), symlink
//! loops (M-19), removal of items that a scan no longer collects, episode numbers (R1-13),
//! movie titles (R1-27), and the versions of a movie or an episode as one item (R1-20), which
//! keeps the user data of every copy and does not start on a version that cannot be probed
//! (G9b).
//!
//! The tests marked "ffmpeg" write real clips and need ffmpeg and ffprobe; without them they
//! print a line and pass.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustyfin_core::scan_progress::{ScanRegistry, ScanState, ScanStatus, ScanTicket, ScanUpdate};
use rustyfin_core::scanner::{ScanChanges, ScanScope};
use rustyfin_core::scheduler::{TASK_LIBRARY_SCAN, TaskScheduler};
use rustyfin_core::tmdb_client::{TmdbApi, TmdbClient, TmdbKey, TmdbSettings};
use rustyfin_core::{probe, scanner};
use rustyfin_model::library::{Library, LibraryContentType};
use sqlx::SqlitePool;
use uuid::Uuid;

/// A temp dir holding a database and the test media; removed on drop.
struct Fixture {
    pool: SqlitePool,
    dir: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_scan_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
        Self { pool, dir }
    }

    /// `<temp dir>/<rel>`.
    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    /// Creates a library whose one root is `<temp dir>/<root>`.
    async fn library(&self, root: &str, content_type: LibraryContentType) -> Library {
        let root = self.path(root);
        std::fs::create_dir_all(&root).unwrap();
        let lib = Library {
            id: Uuid::new_v4(),
            name: root.file_name().unwrap().to_str().unwrap().to_string(),
            content_type,
            paths: vec![root.to_str().unwrap().to_string()],
        };
        rustyfin_db::repo_library::create_library(&self.pool, &lib)
            .await
            .unwrap();
        lib
    }

    /// A second database, `<temp dir>/<file>`, holding the same library row as `lib`.
    async fn second_db(&self, file: &str, lib: &Library) -> SqlitePool {
        let db = self.path(file);
        let pool = rustyfin_db::init_db(db.to_str().unwrap()).await.unwrap();
        rustyfin_db::repo_library::create_library(&pool, lib)
            .await
            .unwrap();
        pool
    }

    /// A real clip of `secs` seconds under `<temp dir>/clips`, written by ffmpeg on first use.
    fn template(&self, secs: u32) -> PathBuf {
        let path = self.path(&format!("clips/{secs}s.mkv"));
        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let video = format!("testsrc=size=64x48:rate=5:duration={secs}");
            let audio = format!("sine=duration={secs}");
            let status = Command::new("ffmpeg")
                .args(["-nostdin", "-loglevel", "error", "-y"])
                .args(["-f", "lavfi", "-i", &video, "-f", "lavfi", "-i", &audio])
                .args(["-c:v", "mpeg4", "-c:a", "aac"])
                .arg(&path)
                .status()
                .expect("run ffmpeg");
            assert!(
                status.success(),
                "ffmpeg could not write {}",
                path.display()
            );
        }
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn root_of(lib: &Library) -> PathBuf {
    PathBuf::from(&lib.paths[0])
}

/// Creates an empty file at `<root>/<rel>`, with its parent folders.
fn touch(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"").unwrap();
    path
}

async fn scan(pool: &SqlitePool, lib: &Library) -> u32 {
    scanner::scan_library(pool, lib)
        .await
        .unwrap_or_else(|e| panic!("scan of {}: {e}", lib.name))
}

/// The paths of the library's items of `media_type`, relative to its root, sorted.
async fn paths(pool: &SqlitePool, lib: &Library, media_type: &str) -> Vec<String> {
    let root = root_of(lib);
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT path FROM media_items WHERE library_id = ?1 AND media_type = ?2",
    )
    .bind(lib.id.to_string())
    .bind(media_type)
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<String> = rows.iter().map(|p| rel(&root, p)).collect();
    out.sort();
    out
}

/// `path` relative to `root`, or unchanged if it is outside.
fn rel(root: &Path, path: &str) -> String {
    match Path::new(path).strip_prefix(root) {
        Ok(r) => r.to_str().unwrap().to_string(),
        Err(_) => path.to_string(),
    }
}

/// Runs `sql` with `binds` and panics on error.
async fn exec(pool: &SqlitePool, sql: &'static str, binds: &[&str]) {
    let mut query = sqlx::query(sql);
    for b in binds {
        query = query.bind(b.to_string());
    }
    query
        .execute(pool)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn count(pool: &SqlitePool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Inserts an item the way the old scanner stored it, and returns its id.
async fn insert_item(
    pool: &SqlitePool,
    lib: &Library,
    media_type: &str,
    path: &Path,
    parent: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    let name = path.file_stem().unwrap().to_str().unwrap();
    sqlx::query(
        "INSERT INTO media_items (id, name, sort_name, media_type, path, library_id, parent_id)
         VALUES (?1, ?2, lower(?2), ?3, ?4, ?5, ?6)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(media_type)
    .bind(path.to_str().unwrap())
    .bind(lib.id.to_string())
    .bind(parent.map(|p| p.to_string()))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// One episode: its path relative to the root, `index_number`, `parent_index_number`, and the
/// name of its season item.
type EpisodeRow = (String, Option<i64>, Option<i64>, String);

/// The library's episodes, sorted by path.
async fn episodes(pool: &SqlitePool, lib: &Library) -> Vec<EpisodeRow> {
    let root = root_of(lib);
    let rows: Vec<EpisodeRow> = sqlx::query_as(
        "SELECT e.path, e.index_number, e.parent_index_number, s.name
         FROM media_items AS e JOIN media_items AS s ON s.id = e.parent_id
         WHERE e.library_id = ?1 AND e.media_type = 'Episode'",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    let mut rows: Vec<EpisodeRow> = rows
        .into_iter()
        .map(|(path, index, season, season_name)| {
            let rel = Path::new(&path).strip_prefix(&root).unwrap();
            (
                rel.to_str().unwrap().to_string(),
                index,
                season,
                season_name,
            )
        })
        .collect();
    rows.sort();
    rows
}

async fn insert_user(pool: &SqlitePool) -> String {
    let id = Uuid::new_v4().to_string();
    let name = format!("user-{id}");
    exec(
        pool,
        "INSERT INTO users (id, name, password_hash) VALUES (?1, ?2, 'x')",
        &[&id, &name],
    )
    .await;
    id
}

/// True when ffmpeg and ffprobe both run.
fn have_ffmpeg() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Copies a real clip of `secs` seconds to `<root>/<rel>`, with its parent folders.
fn clip(fx: &Fixture, root: &Path, rel: &str, secs: u32) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(fx.template(secs), &path).unwrap();
    path
}

/// Scans, then waits until the scan's probes have finished.
async fn scan_idle(pool: &SqlitePool, lib: &Library) {
    scan(pool, lib).await;
    scanner::wait_for_idle().await;
}

/// One item, with the fields that an upgraded library must share with a fresh scan (AC 12).
/// Paths are relative to the library root; `sources` are the source paths in rowid order.
#[derive(Debug, PartialEq)]
struct ItemState {
    media_type: String,
    path: String,
    name: String,
    sort_name: Option<String>,
    year: Option<i64>,
    index: Option<i64>,
    parent_index: Option<i64>,
    parent_path: Option<String>,
    runtime_ticks: Option<i64>,
    sources: Vec<String>,
}

type ItemRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<i64>,
);

/// The library's items, sorted by path.
async fn snapshot(pool: &SqlitePool, lib: &Library) -> Vec<ItemState> {
    let root = root_of(lib);
    let rows: Vec<ItemRow> = sqlx::query_as(
        "SELECT i.id, i.media_type, i.path, i.name, i.sort_name, i.production_year,
                i.index_number, i.parent_index_number, p.path, i.runtime_ticks
         FROM media_items AS i LEFT JOIN media_items AS p ON p.id = i.parent_id
         WHERE i.library_id = ?1",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    let mut items = Vec::new();
    for (id, media_type, path, name, sort_name, year, index, parent_index, parent, ticks) in rows {
        let sources: Vec<String> =
            sqlx::query_scalar("SELECT path FROM media_sources WHERE item_id = ?1 ORDER BY rowid")
                .bind(&id)
                .fetch_all(pool)
                .await
                .unwrap();
        items.push(ItemState {
            media_type,
            path: rel(&root, &path),
            name,
            sort_name,
            year,
            index,
            parent_index,
            parent_path: parent.map(|p| rel(&root, &p)),
            runtime_ticks: ticks,
            sources: sources.iter().map(|s| rel(&root, s)).collect(),
        });
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

/// Asserts that the library's movies and episodes are exactly `groups`, as (item path, source
/// paths in rowid order), and that each one's runtime is that of its first source (AC 10).
async fn assert_groups(pool: &SqlitePool, lib: &Library, groups: &[(&str, &[&str])]) {
    let items: Vec<ItemState> = snapshot(pool, lib)
        .await
        .into_iter()
        .filter(|i| matches!(i.media_type.as_str(), "Movie" | "Episode"))
        .collect();
    let got: Vec<(&str, Vec<&str>)> = items
        .iter()
        .map(|i| {
            (
                i.path.as_str(),
                i.sources.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let want: Vec<(&str, Vec<&str>)> = groups.iter().map(|(p, s)| (*p, s.to_vec())).collect();
    assert_eq!(got, want);

    let root = root_of(lib);
    let firsts: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT i.path, (SELECT s.run_time_ticks FROM media_sources AS s
                         WHERE s.item_id = i.id ORDER BY s.rowid LIMIT 1)
         FROM media_items AS i
         WHERE i.library_id = ?1 AND i.media_type IN ('Movie', 'Episode')",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    for (path, first) in firsts {
        let path = rel(&root, &path);
        let item = items.iter().find(|i| i.path == path).unwrap();
        assert!(first.is_some(), "{path}: its first source has no runtime");
        assert_eq!(
            item.runtime_ticks, first,
            "{path}: the runtime is not its primary's"
        );
    }
}

/// Every (item id, source id) of the library, by item path and then rowid.
async fn source_ids(pool: &SqlitePool, lib: &Library) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT s.item_id, s.id FROM media_sources AS s JOIN media_items AS i ON i.id = s.item_id
         WHERE i.library_id = ?1 ORDER BY i.path, s.rowid",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The id of the library's item at `<root>/<rel>`.
async fn item_at(pool: &SqlitePool, lib: &Library, rel: &str) -> Uuid {
    let path = root_of(lib).join(rel);
    let id: String =
        sqlx::query_scalar("SELECT id FROM media_items WHERE library_id = ?1 AND path = ?2")
            .bind(lib.id.to_string())
            .bind(path.to_str().unwrap())
            .fetch_one(pool)
            .await
            .unwrap_or_else(|e| panic!("no item at {rel}: {e}"));
    id.parse().unwrap()
}

/// (path, name, sort_name, year) of each item.
type Names = (String, String, Option<String>, Option<i64>);

fn names(items: &[ItemState]) -> Vec<Names> {
    items
        .iter()
        .map(|i| (i.path.clone(), i.name.clone(), i.sort_name.clone(), i.year))
        .collect()
}

/// The names that a scan gives the movie at `path` titled `title`.
fn titled(path: &str, title: &str, year: Option<i64>) -> Names {
    let sort_name = Some(title.to_lowercase());
    (path.to_string(), title.to_string(), sort_name, year)
}

/// A movie row as a test inserts it. Names left `None` take the file stem and its lowercase
/// form, the way the old scanner stored them.
#[derive(Default)]
struct Movie<'a> {
    name: Option<&'a str>,
    sort_name: Option<&'a str>,
    year: Option<i64>,
    /// RFC 3339, the way the scanner stores it.
    date_created: Option<&'a str>,
}

async fn insert_movie(pool: &SqlitePool, lib: &Library, path: &Path, movie: Movie<'_>) -> Uuid {
    let id = Uuid::new_v4();
    let stem = path.file_stem().unwrap().to_str().unwrap();
    let sort_name = match movie.sort_name {
        Some(s) => s.to_string(),
        None => stem.to_lowercase(),
    };
    sqlx::query(
        "INSERT INTO media_items
           (id, name, sort_name, media_type, path, library_id, production_year, date_created)
         VALUES (?1, ?2, ?3, 'Movie', ?4, ?5, ?6, ?7)",
    )
    .bind(id.to_string())
    .bind(movie.name.unwrap_or(stem))
    .bind(sort_name)
    .bind(path.to_str().unwrap())
    .bind(lib.id.to_string())
    .bind(movie.year)
    .bind(movie.date_created.unwrap_or("2024-01-01T00:00:00+00:00"))
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Probes `files`, relative to the library root, into `item`'s sources the way the old
/// scanner did: the first through `probe_and_save`, each later one through
/// `probe_and_add_source`, which also set the item's runtime to that file's.
async fn old_probe(pool: &SqlitePool, lib: &Library, item: Uuid, files: &[&str]) {
    for (i, file) in files.iter().enumerate() {
        let path = root_of(lib).join(file);
        let path = path.to_str().unwrap();
        let probed = if i == 0 {
            probe::probe_and_save(pool, &item, path).await
        } else {
            probe::probe_and_add_source(pool, &item, path).await
        };
        probed.unwrap_or_else(|e| panic!("probe of {path}: {e}"));
    }
}

/// `root` and every entry under it, with its size and modification time, sorted.
fn tree(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut entries = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let meta = std::fs::symlink_metadata(&dir).unwrap();
        entries.push((dir.clone(), meta.len(), meta.modified().unwrap()));
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                dirs.push(path);
            } else {
                entries.push((path, meta.len(), meta.modified().unwrap()));
            }
        }
    }
    entries.sort();
    entries
}

/// Writes the movies of `fresh_scan_merges_versions` and `rescans_add_no_sources`: two
/// versions of Argo, one file of Heat, and one movie at the library root.
fn write_films(fx: &Fixture, root: &Path) {
    clip(fx, root, "Argo (2012)/Argo.mkv", 2);
    clip(fx, root, "Argo (2012)/Argo (2012) Bluray-2160p.mkv", 3);
    clip(fx, root, "Heat (1995)/Heat (1995).mkv", 2);
    clip(fx, root, "The Room.mkv", 3);
}

/// The items that `write_films` must give. The 2160p Argo is the primary; it is the longer
/// clip, so a runtime taken from `Argo.mkv` fails `assert_groups`.
const FILM_GROUPS: &[(&str, &[&str])] = &[
    (
        "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
        &[
            "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
            "Argo (2012)/Argo.mkv",
        ],
    ),
    (
        "Heat (1995)/Heat (1995).mkv",
        &["Heat (1995)/Heat (1995).mkv"],
    ),
    ("The Room.mkv", &["The Room.mkv"]),
];

/// Every (path, id) of the library's items, by path.
async fn item_ids(pool: &SqlitePool, lib: &Library) -> Vec<(String, String)> {
    sqlx::query_as("SELECT path, id FROM media_items WHERE library_id = ?1 ORDER BY path")
        .bind(lib.id.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
}

const SEV_E1_SD: &str = "Severance/Season 1/Severance S01E01 720p.mkv";
const SEV_E1_HD: &str = "Severance/Season 1/Severance S01E01 1080p.mkv";
const SEV_E2: &str = "Severance/Season 1/Severance S01E02.mkv";
const SEV_E5: &str = "Severance/Season 1/Severance S01E05.mkv";
const SEV_E5_SD: &str = "Severance/Season 1/Severance S01E05 720p.mkv";
/// The fifth episode of season 2, filed in Season 1.
const SEV_MISFILED: &str = "Severance/Season 1/Severance S02E05.mkv";
const SEV_BONUS: &str = "Severance/Season 1/Bonus.mkv";
const SEV_S2E1: &str = "Severance/Season 2/Severance S02E01.mkv";
/// Directly in the series folder.
const OTHER_E1: &str = "Other/S03E01.mkv";

/// Writes the episodes of `episode_versions_are_one_item` and `tv_upgrade_matches_fresh_scan`.
/// Each version is longer or shorter than its primary, so a runtime taken from the wrong file
/// shows.
fn write_tv(fx: &Fixture, root: &Path) {
    for (file, secs) in [
        (SEV_E1_SD, 2),
        (SEV_E1_HD, 3),
        (SEV_E2, 2),
        (SEV_E5, 3),
        (SEV_E5_SD, 2),
        (SEV_MISFILED, 2),
        (SEV_BONUS, 2),
        (SEV_S2E1, 2),
        (OTHER_E1, 2),
    ] {
        clip(fx, root, file, secs);
    }
}

/// The episodes that `write_tv` must give: one per (season, episode) of a season, the file
/// with the highest resolution first. The misfiled episode's key has its own season, so it is
/// not merged with the fifth episode of Season 1.
const TV_GROUPS: &[(&str, &[&str])] = &[
    (OTHER_E1, &[OTHER_E1]),
    (SEV_BONUS, &[SEV_BONUS]),
    (SEV_E1_HD, &[SEV_E1_HD, SEV_E1_SD]),
    (SEV_E2, &[SEV_E2]),
    (SEV_E5_SD, &[SEV_E5_SD, SEV_E5]),
    (SEV_MISFILED, &[SEV_MISFILED]),
    (SEV_S2E1, &[SEV_S2E1]),
];

/// (media type, path, name, sort_name, index, parent index, parent path) of an item.
type TvFields = (
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn tv_fields(items: &[ItemState]) -> Vec<TvFields> {
    items
        .iter()
        .map(|i| {
            (
                i.media_type.clone(),
                i.path.clone(),
                i.name.clone(),
                i.sort_name.clone(),
                i.index,
                i.parent_index,
                i.parent_path.clone(),
            )
        })
        .collect()
}

/// The Series at `path`, as a scan names it.
fn series_row(path: &str) -> TvFields {
    let name = path.to_string();
    let sort_name = Some(path.to_lowercase());
    (
        "Series".into(),
        path.into(),
        name,
        sort_name,
        None,
        None,
        None,
    )
}

/// Season `index` of `series`, as a scan names it.
fn season_row(series: &str, index: i64) -> TvFields {
    (
        "Season".into(),
        format!("{series}/Season {index}"),
        format!("Season {index}"),
        Some(format!("season {index:04}")),
        Some(index),
        None,
        Some(series.into()),
    )
}

/// The Episode at `path`, named after its file stem, in season `season` at `season_path`.
fn episode_row(path: &str, index: Option<i64>, season: i64, season_path: &str) -> TvFields {
    let stem = Path::new(path).file_stem().unwrap().to_str().unwrap();
    (
        "Episode".into(),
        path.into(),
        stem.into(),
        Some(stem.to_lowercase()),
        index,
        Some(season),
        Some(season_path.into()),
    )
}

/// A TV item as a test inserts it. Names left `None` take the last path component (the file
/// stem, for an episode) and its lowercase form.
#[derive(Default)]
struct Tv<'a> {
    name: Option<&'a str>,
    sort_name: Option<&'a str>,
    index: Option<i64>,
    parent_index: Option<i64>,
    /// RFC 3339, the way the scanner stores it.
    date_created: Option<&'a str>,
}

/// Inserts the item at `<root>/<rel>`, and returns its id.
async fn insert_tv(
    pool: &SqlitePool,
    lib: &Library,
    media_type: &str,
    rel: &str,
    parent: Option<Uuid>,
    tv: Tv<'_>,
) -> Uuid {
    let id = Uuid::new_v4();
    let path = root_of(lib).join(rel);
    let last = if media_type == "Episode" {
        path.file_stem()
    } else {
        path.file_name()
    };
    let name = tv.name.unwrap_or(last.unwrap().to_str().unwrap());
    let sort_name = tv
        .sort_name
        .map_or_else(|| name.to_lowercase(), str::to_string);
    sqlx::query(
        "INSERT INTO media_items
           (id, name, sort_name, media_type, path, library_id, parent_id, index_number,
            parent_index_number, date_created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(sort_name)
    .bind(media_type)
    .bind(path.to_str().unwrap())
    .bind(lib.id.to_string())
    .bind(parent.map(|p| p.to_string()))
    .bind(tv.index)
    .bind(tv.parent_index)
    .bind(tv.date_created.unwrap_or("2024-01-01T00:00:00+00:00"))
    .execute(pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn scan_skips_ignored_dirs_and_files() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&film);
    for rel in [
        "Argo (2012)/Argo (2012).mkv",
        "Argo (2012)/.@__thumb/Argo (2012).mkv",
        "Argo (2012)/@eaDir/Argo (2012).mkv",
        "Argo (2012)/._Argo (2012).mkv",
        "Argo (2012)/Argo (2012).sample.mkv",
        "Argo (2012)/sample.mkv",
        "Argo (2012)/Sample/Argo (2012).mkv",
        "Argo (2012)/metadata/Argo (2012).mkv",
        "Argo (2012)/Argo (2012).trickplay/Argo (2012).mkv",
        "Amadeus (1984)/Amadeus (1984).mkv",
        "Amadeus (1984)/Featurettes/Making Of.mkv",
        "Sample Movie (2010)/Sample Movie (2010).mkv",
        "#recycle/Old (2001)/Old (2001).mkv",
        "@Recycle/Old (2002).mkv",
        "lost+found/Lost (2003).mkv",
        ".hidden/Hidden (2004).mkv",
        "$RECYCLE.BIN/Bin (2005).mkv",
        "System Volume Information/Svi (2006).mkv",
    ] {
        touch(&root, rel);
    }
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let tv_root = root_of(&tv);
    for rel in [
        "Show/Season 1/Show - S01E01.mkv",
        "Show/Season 1/.@__thumb/Show - S01E01.mkv",
        "Show/Season 1/Show - S01E01.sample.mkv",
        "Show/.@__thumb/Show - S01E02.mkv",
        "Show/@eaDir/Show - S01E03.mkv",
        "@Recycle/Old Show/Season 1/Old Show - S01E01.mkv",
    ] {
        touch(&tv_root, rel);
    }

    scan(&fx.pool, &film).await;
    scan(&fx.pool, &tv).await;

    assert_eq!(
        paths(&fx.pool, &film, "Movie").await,
        [
            "Amadeus (1984)/Amadeus (1984).mkv",
            "Amadeus (1984)/Featurettes/Making Of.mkv",
            "Argo (2012)/Argo (2012).mkv",
            "Sample Movie (2010)/Sample Movie (2010).mkv",
        ]
    );
    assert_eq!(
        paths(&fx.pool, &tv, "Episode").await,
        ["Show/Season 1/Show - S01E01.mkv"]
    );
    assert_eq!(paths(&fx.pool, &tv, "Season").await, ["Show/Season 1"]);
    assert_eq!(paths(&fx.pool, &tv, "Series").await, ["Show"]);
}

#[tokio::test]
async fn scan_root_may_start_with_dot() {
    let fx = Fixture::new().await;
    let lib = fx
        .library(".media/@films", LibraryContentType::Movies)
        .await;
    touch(&root_of(&lib), "Argo (2012)/Argo (2012).mkv");

    scan(&fx.pool, &lib).await;

    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        ["Argo (2012)/Argo (2012).mkv"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn scan_skips_symlink_loop() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;

    // A folder holding a link to itself. The old scanner stored nested copies through it.
    let own = fx.library("loop-self", LibraryContentType::Movies).await;
    let own_root = root_of(&own);
    touch(&own_root, "Loop Test (2020)/Loop Test (2020).mkv");
    symlink(".", own_root.join("Loop Test (2020)/loop")).unwrap();
    for rel in [
        "Loop Test (2020)/loop/Loop Test (2020).mkv",
        "Loop Test (2020)/loop/loop/Loop Test (2020).mkv",
    ] {
        insert_item(&fx.pool, &own, "Movie", &own_root.join(rel), None).await;
    }

    // A folder holding a link back to the library root.
    let up = fx.library("loop-root", LibraryContentType::Movies).await;
    let up_root = root_of(&up);
    touch(&up_root, "Other (2001)/Other (2001).mkv");
    symlink(&up_root, up_root.join("Other (2001)/root")).unwrap();

    scan(&fx.pool, &own).await;
    scan(&fx.pool, &up).await;

    assert_eq!(
        paths(&fx.pool, &own, "Movie").await,
        ["Loop Test (2020)/Loop Test (2020).mkv"]
    );
    assert_eq!(
        paths(&fx.pool, &up, "Movie").await,
        ["Other (2001)/Other (2001).mkv"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rescan_removes_items_deep_in_symlink_loop() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Loop Test (2020)/Loop Test (2020).mkv");
    symlink(".", root.join("Loop Test (2020)/loop")).unwrap();
    // A copy stored through the loop so many times that the path no longer resolves.
    let deep = root
        .join("Loop Test (2020)")
        .join("loop/".repeat(45))
        .join("Loop Test (2020).mkv");
    assert!(
        std::fs::metadata(&deep).is_err(),
        "the test needs a path that does not resolve: {}",
        deep.display()
    );
    insert_item(&fx.pool, &lib, "Movie", &deep, None).await;

    scan(&fx.pool, &lib).await;

    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        ["Loop Test (2020)/Loop Test (2020).mkv"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn scan_follows_two_links_to_one_dir() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;
    let store = fx.path("store");
    touch(&store, "Shared (2005)/Shared (2005).mkv");
    touch(&store, "clip.mkv");
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    for dir in ["A", "B"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        symlink(
            store.join("Shared (2005)"),
            root.join(dir).join("Shared (2005)"),
        )
        .unwrap();
    }
    symlink(store.join("clip.mkv"), root.join("Linked (2006).mkv")).unwrap();

    scan(&fx.pool, &lib).await;

    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        [
            "A/Shared (2005)/Shared (2005).mkv",
            "B/Shared (2005)/Shared (2005).mkv",
            "Linked (2006).mkv",
        ]
    );
}

#[tokio::test]
async fn rescan_removes_items_now_ignored() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Argo (2012)/Argo (2012).mkv");
    let thumb = touch(&root, "Argo (2012)/.@__thumb/Argo (2012).mkv");
    let junk = touch(&root, "#recycle/Junk (2001).mkv");
    // Items the old scanner made for these files, and rows that point at them.
    let thumb_id = insert_item(&fx.pool, &lib, "Movie", &thumb, None)
        .await
        .to_string();
    let junk_id = insert_item(&fx.pool, &lib, "Movie", &junk, None)
        .await
        .to_string();
    let user = insert_user(&fx.pool).await;
    let pool = &fx.pool;
    exec(
        pool,
        "INSERT INTO user_item_data (user_id, item_id) VALUES (?1, ?2)",
        &[&user, &thumb_id],
    )
    .await;
    exec(
        pool,
        "INSERT INTO people (id, name) VALUES ('p1', 'Someone')",
        &[],
    )
    .await;
    exec(
        pool,
        "INSERT INTO item_people (item_id, person_id) VALUES (?1, 'p1')",
        &[&thumb_id],
    )
    .await;
    exec(
        pool,
        "INSERT INTO studios (id, name) VALUES ('s1', 'Studio')",
        &[],
    )
    .await;
    exec(
        pool,
        "INSERT INTO item_studios (item_id, studio_id) VALUES (?1, 's1')",
        &[&junk_id],
    )
    .await;

    for round in 1..=2 {
        scan(pool, &lib).await;
        assert_eq!(
            paths(pool, &lib, "Movie").await,
            ["Argo (2012)/Argo (2012).mkv"],
            "after scan {round}"
        );
    }
    assert_eq!(count(pool, "SELECT COUNT(*) FROM user_item_data").await, 0);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM item_people").await, 0);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM item_studios").await, 0);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM people").await, 1);
}

/// B18 criterion 5: the item of a missing file goes, since its library path still lists
/// something. An item whose path is now a folder stays.
#[tokio::test]
async fn rescan_removes_items_with_missing_files() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Gone (1999)/Gone (1999).mkv");
    touch(&root, "Here (2000)/Here (2000).mkv");
    let odd = touch(&root, "Odd (2003)/Odd (2003).mkv");
    // An item outside the library's roots, whose file exists.
    let elsewhere = touch(&fx.path("elsewhere"), "Old (1990).mkv");
    insert_item(&fx.pool, &lib, "Movie", &elsewhere, None).await;
    scan(&fx.pool, &lib).await;

    std::fs::remove_dir_all(root.join("Gone (1999)")).unwrap();
    std::fs::remove_file(&odd).unwrap();
    std::fs::create_dir(&odd).unwrap();
    scan(&fx.pool, &lib).await;

    let mut expected = vec![
        "Here (2000)/Here (2000).mkv".to_string(),
        "Odd (2003)/Odd (2003).mkv".to_string(),
        elsewhere.to_str().unwrap().to_string(),
    ];
    expected.sort();
    assert_eq!(paths(&fx.pool, &lib, "Movie").await, expected);
}

#[cfg(unix)]
#[tokio::test]
async fn rescan_keeps_items_in_unreadable_dirs() {
    use std::os::unix::fs::PermissionsExt;

    /// Sets a folder's permission bits on drop, so the temp dir can be removed.
    struct Mode(PathBuf, u32);
    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
        }
    }

    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Argo (2012)/Argo (2012).mkv");
    touch(&root, "Locked (2001)/Locked (2001).mkv");
    scan(&fx.pool, &lib).await;

    // The folder cannot be listed, but its file can still be opened, as on a share that
    // fails part way through a scan.
    let locked = root.join("Locked (2001)");
    let _restore = Mode(locked.clone(), 0o755);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o100)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        eprintln!("skip: the folder is still readable (running as root?)");
        return;
    }
    scan(&fx.pool, &lib).await;

    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        [
            "Argo (2012)/Argo (2012).mkv",
            "Locked (2001)/Locked (2001).mkv"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_rescans_remove_items_in_both_libraries() {
    let fx = Fixture::new().await;
    let user = insert_user(&fx.pool).await;
    let mut libs = Vec::new();
    for name in ["film-a", "film-b"] {
        let lib = fx.library(name, LibraryContentType::Movies).await;
        let root = root_of(&lib);
        // The real file is already known, so the rescans only remove.
        let argo = touch(&root, "Argo (2012)/Argo (2012).mkv");
        insert_item(&fx.pool, &lib, "Movie", &argo, None).await;
        for i in 0..20 {
            let junk = touch(&root, &format!("Argo (2012)/.@__thumb/Argo {i}.mkv"));
            let id = insert_item(&fx.pool, &lib, "Movie", &junk, None)
                .await
                .to_string();
            exec(
                &fx.pool,
                "INSERT INTO user_item_data (user_id, item_id) VALUES (?1, ?2)",
                &[&user, &id],
            )
            .await;
        }
        libs.push(lib);
    }

    tokio::join!(scan(&fx.pool, &libs[0]), scan(&fx.pool, &libs[1]));

    for lib in &libs {
        assert_eq!(
            paths(&fx.pool, lib, "Movie").await,
            ["Argo (2012)/Argo (2012).mkv"],
            "library {}",
            lib.name
        );
    }
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM user_item_data").await,
        0
    );
}

#[tokio::test]
async fn rescan_removes_empty_tv_seasons_and_series() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    touch(&root, "Show/Season 1/Show - S01E01.mkv");
    let thumb = touch(&root, "Show/.@__thumb/Show - S01E01.mkv");
    let junk = touch(&root, "#recycle/Junk.mkv");
    // What the old scanner made of the two junk files.
    let pool = &fx.pool;
    let show = insert_item(pool, &lib, "Series", &root.join("Show"), None).await;
    let thumb_season = insert_item(
        pool,
        &lib,
        "Season",
        &root.join("Show/.@__thumb"),
        Some(show),
    )
    .await;
    insert_item(pool, &lib, "Episode", &thumb, Some(thumb_season)).await;
    let recycle = insert_item(pool, &lib, "Series", &root.join("#recycle"), None).await;
    let recycle_season = insert_item(
        pool,
        &lib,
        "Season",
        &root.join("#recycle/Season 1"),
        Some(recycle),
    )
    .await;
    let junk_ep = insert_item(pool, &lib, "Episode", &junk, Some(recycle_season)).await;
    let user = insert_user(pool).await;
    exec(
        pool,
        "INSERT INTO pinned_episodes (user_id, series_id, episode_id, season_id, created_date)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))",
        &[
            &user,
            &recycle.to_string(),
            &junk_ep.to_string(),
            &recycle_season.to_string(),
        ],
    )
    .await;
    exec(
        pool,
        "INSERT INTO user_item_data (user_id, item_id) VALUES (?1, ?2)",
        &[&user, &junk_ep.to_string()],
    )
    .await;

    scan(pool, &lib).await;

    assert_eq!(
        paths(pool, &lib, "Episode").await,
        ["Show/Season 1/Show - S01E01.mkv"]
    );
    assert_eq!(paths(pool, &lib, "Season").await, ["Show/Season 1"]);
    assert_eq!(paths(pool, &lib, "Series").await, ["Show"]);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM pinned_episodes").await, 0);
    assert_eq!(count(pool, "SELECT COUNT(*) FROM user_item_data").await, 0);
}

#[tokio::test]
async fn tv_scan_sets_episode_and_season_numbers() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    for rel in [
        "Severance/Season 2/Severance - S02E01 - Hello, Ms. Cobel.mkv",
        "Severance/Season 2/Severance - S02E02E03 - Goodbye.mkv",
        // The season folder's number wins over the file's.
        "Show/Season 1/Show S03E04.mkv",
        // A season folder without a number: the file's season.
        "Show/Specials/Show S00E01.mkv",
        // Directly in the series folder: the season is the file's, else 1.
        "Flat Show/Flat Show 3x07.mkv",
        "Flat Show/Flat Show - Episode 9.mkv",
        "Flat Show/Pilot.mkv",
    ] {
        touch(&root, rel);
    }

    scan(&fx.pool, &lib).await;

    let row = |path: &str, index: Option<i64>, season: Option<i64>, season_name: &str| {
        (path.to_string(), index, season, season_name.to_string())
    };
    assert_eq!(
        episodes(&fx.pool, &lib).await,
        [
            row(
                "Flat Show/Flat Show - Episode 9.mkv",
                Some(9),
                Some(1),
                "Season 1"
            ),
            row("Flat Show/Flat Show 3x07.mkv", Some(7), Some(3), "Season 3"),
            row("Flat Show/Pilot.mkv", None, Some(1), "Season 1"),
            row(
                "Severance/Season 2/Severance - S02E01 - Hello, Ms. Cobel.mkv",
                Some(1),
                Some(2),
                "Season 2"
            ),
            row(
                "Severance/Season 2/Severance - S02E02E03 - Goodbye.mkv",
                Some(2),
                Some(2),
                "Season 2"
            ),
            row(
                "Show/Season 1/Show S03E04.mkv",
                Some(4),
                Some(1),
                "Season 1"
            ),
            row(
                "Show/Specials/Show S00E01.mkv",
                Some(1),
                Some(0),
                "Specials"
            ),
        ]
    );
}

#[tokio::test]
async fn movie_titles_are_clean() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&film);
    let files = [
        (
            "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
            "Argo",
            Some(2012),
        ),
        (
            "Holy.Motors.2012.BRRip.XviD.5rFF/5rFF-hmot480p.avi",
            "Holy Motors",
            Some(2012),
        ),
        (
            "The Iron Claw ()/The Iron Claw (2023) WEBDL-2160p.mkv",
            "The Iron Claw",
            Some(2023),
        ),
        (
            "The Machine (2013)/the.machine.2013.720p.bluray.x264.dts-nohate.mkv",
            "The Machine",
            Some(2013),
        ),
        ("The Room.avi", "The Room", None),
    ];
    for (rel, _, _) in files {
        touch(&root, rel);
    }

    scan_idle(&fx.pool, &film).await;

    let want: Vec<Names> = files.iter().map(|(p, t, y)| titled(p, t, *y)).collect();
    assert_eq!(names(&snapshot(&fx.pool, &film).await), want);

    // In a mixed library a movie is named the same way, music and photos keep their stems, and
    // a second scan adds nothing.
    let mixed = fx.library("mixed", LibraryContentType::Mixed).await;
    let root = root_of(&mixed);
    for rel in [
        "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
        "Beach (2012).jpg",
        "Live (2012).mp3",
    ] {
        touch(&root, rel);
    }
    scan_idle(&fx.pool, &mixed).await;
    scan_idle(&fx.pool, &mixed).await;
    let items = snapshot(&fx.pool, &mixed).await;
    let got: Vec<(&str, &str, &str)> = items
        .iter()
        .map(|i| (i.path.as_str(), i.media_type.as_str(), i.name.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("Argo (2012)/Argo (2012) Bluray-2160p.mkv", "Movie", "Argo"),
            ("Beach (2012).jpg", "Photo", "Beach (2012)"),
            ("Live (2012).mp3", "Audio", "Live (2012)"),
        ]
    );
}

/// ffmpeg.
#[tokio::test]
async fn fresh_scan_merges_versions() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    write_films(&fx, &root);
    let before = tree(&root);

    scan_idle(&fx.pool, &lib).await;

    assert_groups(&fx.pool, &lib, FILM_GROUPS).await;
    assert_eq!(
        names(&snapshot(&fx.pool, &lib).await),
        [
            titled(FILM_GROUPS[0].0, "Argo", Some(2012)),
            titled(FILM_GROUPS[1].0, "Heat", Some(1995)),
            titled(FILM_GROUPS[2].0, "The Room", None),
        ]
    );
    // AC 15: the scan wrote nothing inside the library.
    assert_eq!(tree(&root), before);
}

/// ffmpeg.
#[tokio::test]
async fn rescans_add_no_sources() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    write_films(&fx, &root_of(&lib));

    // The second scan starts while the probes of the first may still run. It must wait for
    // them, then find every source in place and probe nothing.
    scan(&fx.pool, &lib).await;
    scan(&fx.pool, &lib).await;
    assert_groups(&fx.pool, &lib, FILM_GROUPS).await;

    let ids = source_ids(&fx.pool, &lib).await;
    for _ in 0..2 {
        scan_idle(&fx.pool, &lib).await;
        assert_eq!(
            source_ids(&fx.pool, &lib).await,
            ids,
            "a rescan probed again"
        );
    }
}

/// ffmpeg. A library as the old scanner left it, with split versions, a duplicate source, a
/// runtime taken from a later version, stem names and a NAS thumbnail, ends the same as a
/// fresh scan of the same files, and keeps the items that users know (AC 12).
#[tokio::test]
async fn upgrade_from_old_state_matches_fresh_scan() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let argo = "Argo (2012)/Argo.mkv";
    let argo_uhd = "Argo (2012)/Argo (2012) Bluray-2160p.mkv";
    let santa = "Bad Santa (2003)/Bad Santa (2003).mkv";
    let santa_cut = "Bad Santa (2003)/Bad Santa.mkv";
    let kane_uhd = "Citizen Kane (1941)/Citizen Kane (1941) Bluray-2160p.mkv";
    let kane_cd1 = "Citizen Kane (1941)/Citizen Kane cd1.mkv";
    let kane_cd2 = "Citizen Kane (1941)/Citizen Kane cd2.mkv";
    let seven = "Se7en (1995)/Se7en (1995).mkv";
    let thumb = "Se7en (1995)/.@__thumb/Se7en (1995).mkv";
    let machine = "The Machine (2013)/The Machine (2013) Bluray-1080p.mkv";
    let scene = "The Machine (2013)/the.machine.2013.720p.bluray.x264.dts-nohate.mkv";
    // Every other version is longer or shorter than its primary, so a runtime taken from the
    // wrong file shows.
    for (file, secs) in [
        (argo, 2),
        (argo_uhd, 3),
        (santa, 2),
        (santa_cut, 3),
        (kane_uhd, 2),
        (kane_cd1, 3),
        (kane_cd2, 3),
        (seven, 2),
        (machine, 2),
        (scene, 3),
    ] {
        clip(&fx, &root, file, secs);
    }
    touch(&root, thumb);

    scan_idle(&fx.pool, &lib).await;
    let fresh = snapshot(&fx.pool, &lib).await;
    assert_groups(
        &fx.pool,
        &lib,
        &[
            (argo_uhd, &[argo_uhd, argo]),
            (santa, &[santa, santa_cut]),
            (kane_uhd, &[kane_uhd, kane_cd1, kane_cd2]),
            (seven, &[seven]),
            (machine, &[machine, scene]),
        ],
    )
    .await;

    // The same files as the old scanner left them, in a second database.
    let old = fx.second_db("old.db", &lib).await;
    let user1 = insert_user(&old).await;
    let user2 = insert_user(&old).await;
    let year = |y| Movie {
        year: Some(y),
        ..Movie::default()
    };

    // Argo: the versions were split into two items. user1 played the first; user2 played the
    // first too, and has an unplayed row of its own on the second. The rows combine, so both
    // users have played Argo (G9b).
    let a1 = insert_movie(&old, &lib, &root.join(argo), Movie::default()).await;
    old_probe(&old, &lib, a1, &[argo]).await;
    let a2 = insert_movie(&old, &lib, &root.join(argo_uhd), year(2012)).await;
    old_probe(&old, &lib, a2, &[argo_uhd]).await;
    for (user, item, played) in [(&user1, a1, "1"), (&user2, a1, "1"), (&user2, a2, "0")] {
        exec(
            &old,
            "INSERT INTO user_item_data (user_id, item_id, played) VALUES (?1, ?2, ?3)",
            &[user, &item.to_string(), played],
        )
        .await;
    }

    // Bad Santa: one item with both files, but the runtime of the second.
    let b = insert_movie(&old, &lib, &root.join(santa), year(2003)).await;
    old_probe(&old, &lib, b, &[santa, santa_cut]).await;

    // Citizen Kane: two items. The second, made first, survives; the first holds the 2160p
    // file as a version and is in a collection.
    let made = |date| Movie {
        date_created: Some(date),
        ..Movie::default()
    };
    let c1 = insert_movie(
        &old,
        &lib,
        &root.join(kane_cd1),
        made("2021-01-01T00:00:00+00:00"),
    )
    .await;
    old_probe(&old, &lib, c1, &[kane_cd1, kane_uhd]).await;
    let c2 = insert_movie(
        &old,
        &lib,
        &root.join(kane_cd2),
        made("2020-01-01T00:00:00+00:00"),
    )
    .await;
    old_probe(&old, &lib, c2, &[kane_cd2]).await;
    let set = Uuid::new_v4().to_string();
    exec(
        &old,
        "INSERT INTO collections (id, name, user_id) VALUES (?1, 'Kane', ?2)",
        &[&set, &user1],
    )
    .await;
    exec(
        &old,
        "INSERT INTO collection_items (collection_id, item_id) VALUES (?1, ?2)",
        &[&set, &c1.to_string()],
    )
    .await;

    // Se7en: right but for its name, so it must not be probed again. Its NAS thumbnail was
    // an item too.
    let s = insert_movie(&old, &lib, &root.join(seven), year(1995)).await;
    old_probe(&old, &lib, s, &[seven]).await;
    insert_movie(&old, &lib, &root.join(thumb), year(1995)).await;

    // The Machine: the scene version was added again by each rescan.
    let m = insert_movie(&old, &lib, &root.join(machine), year(2013)).await;
    old_probe(&old, &lib, m, &[machine, scene, scene]).await;

    let s_id = s.to_string();
    let sources_of_s = |ids: Vec<(String, String)>| -> Vec<(String, String)> {
        ids.into_iter().filter(|(item, _)| *item == s_id).collect()
    };
    let s_before = sources_of_s(source_ids(&old, &lib).await);

    scan_idle(&old, &lib).await;

    assert_eq!(snapshot(&old, &lib).await, fresh);
    for (file, id) in [
        (argo_uhd, a2),
        (santa, b),
        (kane_uhd, c2),
        (seven, s),
        (machine, m),
    ] {
        assert_eq!(item_at(&old, &lib, file).await, id, "the item at {file}");
    }
    let mut want = vec![
        (user1.clone(), a2.to_string(), 1),
        (user2.clone(), a2.to_string(), 1),
    ];
    want.sort();
    let got: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT user_id, item_id, played FROM user_item_data ORDER BY user_id")
            .fetch_all(&old)
            .await
            .unwrap();
    assert_eq!(got, want);
    let in_set: Vec<String> = sqlx::query_scalar("SELECT item_id FROM collection_items")
        .fetch_all(&old)
        .await
        .unwrap();
    assert_eq!(in_set, [c2.to_string()]);
    assert_eq!(
        sources_of_s(source_ids(&old, &lib).await),
        s_before,
        "Se7en was probed again"
    );

    // A second rescan changes nothing.
    let ids = source_ids(&old, &lib).await;
    scan_idle(&old, &lib).await;
    assert_eq!(
        source_ids(&old, &lib).await,
        ids,
        "the second rescan probed again"
    );
}

/// Two scans of one library at once, and a TV scan beside them, all succeed and add each
/// item once (AC 14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_scans_both_succeed() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&film);
    for rel in [
        "Argo (2012)/Argo.mkv",
        "Argo (2012)/Argo (2012).mkv",
        "Heat (1995)/Heat (1995).mkv",
    ] {
        touch(&root, rel);
    }
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&tv);
    for rel in [
        "Show/Season 1/Show S01E01.mkv",
        "Show/Season 1/Show S01E02.mkv",
    ] {
        touch(&root, rel);
    }

    tokio::join!(
        scan(&fx.pool, &film),
        scan(&fx.pool, &film),
        scan(&fx.pool, &tv)
    );
    scanner::wait_for_idle().await;

    assert_eq!(
        paths(&fx.pool, &film, "Movie").await,
        ["Argo (2012)/Argo (2012).mkv", "Heat (1995)/Heat (1995).mkv"]
    );
    assert_eq!(paths(&fx.pool, &tv, "Series").await, ["Show"]);
    assert_eq!(paths(&fx.pool, &tv, "Season").await, ["Show/Season 1"]);
    assert_eq!(
        paths(&fx.pool, &tv, "Episode").await,
        [
            "Show/Season 1/Show S01E01.mkv",
            "Show/Season 1/Show S01E02.mkv"
        ]
    );
}

/// A rescan keeps a name, sort name or year that differs from what the old scanner made from
/// the file stem, since metadata set it, and rewrites the rest.
#[tokio::test]
async fn rescan_keeps_names_from_metadata() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let argo = touch(&root, "Argo (2012)/Argo (2012) Bluray-2160p.mkv");
    let heat = touch(&root, "Heat (1995)/Heat (1995).mkv");
    let seven = touch(&root, "Se7en (1995)/Se7en (1995).mkv");
    let from_metadata = Movie {
        name: Some("Argo: Declassified"),
        sort_name: Some("argo declassified"),
        year: Some(2011),
        ..Movie::default()
    };
    insert_movie(&fx.pool, &lib, &argo, from_metadata).await;
    // Stem names and no year.
    insert_movie(&fx.pool, &lib, &heat, Movie::default()).await;
    // Stem names; only the year came from metadata.
    let year_only = Movie {
        year: Some(1996),
        ..Movie::default()
    };
    insert_movie(&fx.pool, &lib, &seven, year_only).await;

    scan_idle(&fx.pool, &lib).await;

    let argo_kept = (
        "Argo (2012)/Argo (2012) Bluray-2160p.mkv".to_string(),
        "Argo: Declassified".to_string(),
        Some("argo declassified".to_string()),
        Some(2011),
    );
    assert_eq!(
        names(&snapshot(&fx.pool, &lib).await),
        [
            argo_kept,
            titled("Heat (1995)/Heat (1995).mkv", "Heat", Some(1995)),
            titled("Se7en (1995)/Se7en (1995).mkv", "Se7en", Some(1996)),
        ]
    );
}

/// ffmpeg. The versions of an episode are one item, numbered from its primary, and rescans
/// change nothing (AC 13, AC 11, AC 15).
#[tokio::test]
async fn episode_versions_are_one_item() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    write_tv(&fx, &root);
    let before = tree(&root);

    scan_idle(&fx.pool, &lib).await;

    assert_groups(&fx.pool, &lib, TV_GROUPS).await;
    let season_1 = "Severance/Season 1";
    assert_eq!(
        tv_fields(&snapshot(&fx.pool, &lib).await),
        [
            series_row("Other"),
            episode_row(OTHER_E1, Some(1), 3, "Other/Season 3"),
            season_row("Other", 3),
            series_row("Severance"),
            season_row("Severance", 1),
            episode_row(SEV_BONUS, None, 1, season_1),
            episode_row(SEV_E1_HD, Some(1), 1, season_1),
            episode_row(SEV_E2, Some(2), 1, season_1),
            episode_row(SEV_E5_SD, Some(5), 1, season_1),
            // The season folder's number wins over the file's.
            episode_row(SEV_MISFILED, Some(5), 1, season_1),
            season_row("Severance", 2),
            episode_row(SEV_S2E1, Some(1), 2, "Severance/Season 2"),
        ]
    );

    let ids = source_ids(&fx.pool, &lib).await;
    let items = item_ids(&fx.pool, &lib).await;
    let state = snapshot(&fx.pool, &lib).await;
    for _ in 0..2 {
        scan_idle(&fx.pool, &lib).await;
        assert_eq!(
            source_ids(&fx.pool, &lib).await,
            ids,
            "a rescan probed again"
        );
        assert_eq!(
            item_ids(&fx.pool, &lib).await,
            items,
            "a rescan replaced an item"
        );
        assert_eq!(snapshot(&fx.pool, &lib).await, state);
    }
    // AC 15: the scans wrote nothing inside the library.
    assert_eq!(tree(&root), before);
}

/// ffmpeg. A TV library as the old scanner left it, with split versions, a duplicate source,
/// no episode numbers and the episodes of a series folder in Season 1, ends the same as a
/// fresh scan of the same files, and keeps the items that users know (AC 12).
#[tokio::test]
async fn tv_upgrade_matches_fresh_scan() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    write_tv(&fx, &root);

    // The same files as the old scanner left them, in a second database.
    let old = fx.second_db("old.db", &lib).await;
    let user = insert_user(&old).await;
    let season = |index, sort_name| Tv {
        sort_name: Some(sort_name),
        index: Some(index),
        ..Tv::default()
    };
    let in_season = |parent_index| Tv {
        parent_index: Some(parent_index),
        ..Tv::default()
    };
    let sev = insert_tv(&old, &lib, "Series", "Severance", None, Tv::default()).await;
    let s1 = insert_tv(
        &old,
        &lib,
        "Season",
        "Severance/Season 1",
        Some(sev),
        season(1, "season 0001"),
    )
    .await;
    let s2 = insert_tv(
        &old,
        &lib,
        "Season",
        "Severance/Season 2",
        Some(sev),
        season(2, "season 0002"),
    )
    .await;
    let other = insert_tv(&old, &lib, "Series", "Other", None, Tv::default()).await;
    let other_s1 = insert_tv(
        &old,
        &lib,
        "Season",
        "Other/Season 1",
        Some(other),
        season(1, "season 0001"),
    )
    .await;

    // S01E01: the versions were split into two items. The older one, which is not the
    // primary, was played and is pinned as the series' next episode.
    let e1_sd = insert_tv(
        &old,
        &lib,
        "Episode",
        SEV_E1_SD,
        Some(s1),
        Tv {
            parent_index: Some(1),
            date_created: Some("2020-01-01T00:00:00+00:00"),
            ..Tv::default()
        },
    )
    .await;
    old_probe(&old, &lib, e1_sd, &[SEV_E1_SD]).await;
    let e1_hd = insert_tv(
        &old,
        &lib,
        "Episode",
        SEV_E1_HD,
        Some(s1),
        Tv {
            parent_index: Some(1),
            date_created: Some("2021-01-01T00:00:00+00:00"),
            ..Tv::default()
        },
    )
    .await;
    old_probe(&old, &lib, e1_hd, &[SEV_E1_HD]).await;
    exec(
        &old,
        "INSERT INTO user_item_data (user_id, item_id, played) VALUES (?1, ?2, 1)",
        &[&user, &e1_sd.to_string()],
    )
    .await;
    const PIN: &str = "INSERT INTO pinned_episodes
                         (user_id, series_id, episode_id, season_id, created_date)
                       VALUES (?1, ?2, ?3, ?4, datetime('now'))";
    exec(
        &old,
        PIN,
        &[&user, &sev.to_string(), &e1_sd.to_string(), &s1.to_string()],
    )
    .await;

    // S01E02: its name came from metadata.
    let e2 = insert_tv(
        &old,
        &lib,
        "Episode",
        SEV_E2,
        Some(s1),
        Tv {
            name: Some("Good News About Hell"),
            sort_name: Some("good news about hell"),
            parent_index: Some(1),
            ..Tv::default()
        },
    )
    .await;
    old_probe(&old, &lib, e2, &[SEV_E2]).await;

    // S01E05: the 720p version was added again by each rescan, and its runtime replaced the
    // item's.
    let e5 = insert_tv(&old, &lib, "Episode", SEV_E5, Some(s1), in_season(1)).await;
    old_probe(&old, &lib, e5, &[SEV_E5, SEV_E5_SD, SEV_E5_SD]).await;

    // Right but for their numbers, so they must not be probed again. The episode in the series
    // folder went into Season 1, whatever its name said.
    let misfiled = insert_tv(&old, &lib, "Episode", SEV_MISFILED, Some(s1), in_season(1)).await;
    old_probe(&old, &lib, misfiled, &[SEV_MISFILED]).await;
    let bonus = insert_tv(&old, &lib, "Episode", SEV_BONUS, Some(s1), in_season(1)).await;
    old_probe(&old, &lib, bonus, &[SEV_BONUS]).await;
    let s2e1 = insert_tv(&old, &lib, "Episode", SEV_S2E1, Some(s2), in_season(2)).await;
    old_probe(&old, &lib, s2e1, &[SEV_S2E1]).await;
    let o1 = insert_tv(
        &old,
        &lib,
        "Episode",
        OTHER_E1,
        Some(other_s1),
        in_season(1),
    )
    .await;
    old_probe(&old, &lib, o1, &[OTHER_E1]).await;
    // Pinned as the next episode of its series. Its old season goes, and the pin must not.
    exec(
        &old,
        PIN,
        &[
            &user,
            &other.to_string(),
            &o1.to_string(),
            &other_s1.to_string(),
        ],
    )
    .await;

    let kept = [e2, misfiled, bonus, s2e1, o1].map(|id| id.to_string());
    let sources_of_kept = |ids: Vec<(String, String)>| -> Vec<(String, String)> {
        ids.into_iter()
            .filter(|(item, _)| kept.contains(item))
            .collect()
    };
    let kept_before = sources_of_kept(source_ids(&old, &lib).await);

    scan_idle(&fx.pool, &lib).await;
    assert_groups(&fx.pool, &lib, TV_GROUPS).await;
    // The rescan keeps the name that metadata gave S01E02.
    let mut want = snapshot(&fx.pool, &lib).await;
    let named = want.iter_mut().find(|i| i.path == SEV_E2).unwrap();
    named.name = "Good News About Hell".into();
    named.sort_name = Some("good news about hell".into());

    scan_idle(&old, &lib).await;

    assert_eq!(snapshot(&old, &lib).await, want);
    for (rel, id) in [
        ("Other", other),
        ("Severance", sev),
        ("Severance/Season 1", s1),
        ("Severance/Season 2", s2),
        (SEV_E1_HD, e1_hd),
        (SEV_E2, e2),
        (SEV_E5_SD, e5),
        (SEV_MISFILED, misfiled),
        (SEV_BONUS, bonus),
        (SEV_S2E1, s2e1),
        (OTHER_E1, o1),
    ] {
        assert_eq!(item_at(&old, &lib, rel).await, id, "the item at {rel}");
    }
    let played: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT user_id, item_id, played FROM user_item_data")
            .fetch_all(&old)
            .await
            .unwrap();
    assert_eq!(played, [(user, e1_hd.to_string(), 1)]);
    // Each pin moves to the surviving episode, and to the season it is in now.
    let season_3 = item_at(&old, &lib, "Other/Season 3").await;
    let mut want_pins =
        [(sev, e1_hd, s1), (other, o1, season_3)].map(|(series, episode, season)| {
            (series.to_string(), episode.to_string(), season.to_string())
        });
    want_pins.sort();
    let pins: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT series_id, episode_id, season_id FROM pinned_episodes ORDER BY series_id",
    )
    .fetch_all(&old)
    .await
    .unwrap();
    assert_eq!(pins, want_pins);
    assert_eq!(
        sources_of_kept(source_ids(&old, &lib).await),
        kept_before,
        "an episode whose sources were right was probed again"
    );

    // A second rescan changes nothing.
    let ids = source_ids(&old, &lib).await;
    scan_idle(&old, &lib).await;
    assert_eq!(
        source_ids(&old, &lib).await,
        ids,
        "the second rescan probed again"
    );
}

// G9b: merged copies keep user data

/// One user's `user_item_data` row, without its keys.
#[derive(Clone, Debug, Default, PartialEq)]
struct Marks {
    position: i64,
    plays: i64,
    favorite: bool,
    played: bool,
    last_played: Option<String>,
    rating: Option<i64>,
    likes: Option<i64>,
}

/// Marks with only a position and a last played date.
fn at(position: i64, last_played: Option<&str>) -> Marks {
    Marks {
        position,
        last_played: last_played.map(str::to_string),
        ..Marks::default()
    }
}

const EARLY: &str = "2024-03-01T10:00:00+00:00";
const LATE: &str = "2024-06-01T10:00:00+00:00";

async fn insert_marks(pool: &SqlitePool, user: &str, item: Uuid, marks: &Marks) {
    sqlx::query(
        "INSERT INTO user_item_data (user_id, item_id, playback_position_ticks, play_count,
           is_favorite, played, last_played_date, user_rating, likes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(user)
    .bind(item.to_string())
    .bind(marks.position)
    .bind(marks.plays)
    .bind(i64::from(marks.favorite))
    .bind(i64::from(marks.played))
    .bind(marks.last_played.as_deref())
    .bind(marks.rating)
    .bind(marks.likes)
    .execute(pool)
    .await
    .unwrap();
}

type MarksRow = (i64, i64, i64, i64, Option<String>, Option<i64>, Option<i64>);

/// `user`'s row for `item`, if there is one.
async fn marks_of(pool: &SqlitePool, user: &str, item: Uuid) -> Option<Marks> {
    let row: Option<MarksRow> = sqlx::query_as(
        "SELECT playback_position_ticks, play_count, is_favorite, played, last_played_date,
                user_rating, likes
         FROM user_item_data WHERE user_id = ?1 AND item_id = ?2",
    )
    .bind(user)
    .bind(item.to_string())
    .fetch_optional(pool)
    .await
    .unwrap();
    row.map(
        |(position, plays, favorite, played, last_played, rating, likes)| Marks {
            position,
            plays,
            favorite: favorite != 0,
            played: played != 0,
            last_played,
            rating,
            likes,
        },
    )
}

/// Two empty copies of the movie in `<folder>`, as the old scanner stored them: a 2160p one,
/// which survives the merge, and a 1080p one. Returns their ids.
async fn two_copies(pool: &SqlitePool, lib: &Library, folder: &str) -> (Uuid, Uuid) {
    let root = root_of(lib);
    let uhd = touch(&root, &format!("{folder}/{folder} Bluray-2160p.mkv"));
    let hd = touch(&root, &format!("{folder}/{folder} Bluray-1080p.mkv"));
    let survivor = insert_movie(pool, lib, &uhd, Movie::default()).await;
    let other = insert_movie(pool, lib, &hd, Movie::default()).await;
    (survivor, other)
}

/// When both copies of a movie have a row for a user, the rows combine: a flag is set if
/// either is, plays add up, the later play gives the date and position, and the survivor's
/// rating and likes win when set (G9b AC 1).
#[tokio::test]
async fn merged_copies_combine_user_data() {
    // (folder, the survivor's row, the other copy's row, the row after the merge)
    let cases: Vec<(&str, Marks, Marks, Marks)> = vec![
        // Flags and plays. The survivor has likes but no rating.
        (
            "Alpha (2001)",
            Marks {
                plays: 1,
                played: true,
                likes: Some(1),
                ..at(0, Some(EARLY))
            },
            Marks {
                plays: 2,
                favorite: true,
                rating: Some(8),
                likes: Some(0),
                ..at(0, None)
            },
            Marks {
                plays: 3,
                favorite: true,
                played: true,
                rating: Some(8),
                likes: Some(1),
                ..at(0, Some(EARLY))
            },
        ),
        // The other copy was played later.
        (
            "Bravo (2002)",
            Marks {
                plays: 1,
                played: true,
                ..at(100, Some(EARLY))
            },
            at(500, Some(LATE)),
            Marks {
                plays: 1,
                played: true,
                ..at(500, Some(LATE))
            },
        ),
        // The survivor was played later, and its rating wins.
        (
            "Charlie (2003)",
            Marks {
                rating: Some(3),
                ..at(100, Some(LATE))
            },
            Marks {
                rating: Some(9),
                likes: Some(0),
                ..at(500, Some(EARLY))
            },
            Marks {
                rating: Some(3),
                likes: Some(0),
                ..at(100, Some(LATE))
            },
        ),
        // A missing date is the oldest.
        (
            "Delta (2004)",
            at(100, Some(EARLY)),
            Marks {
                favorite: true,
                ..at(500, None)
            },
            Marks {
                favorite: true,
                ..at(100, Some(EARLY))
            },
        ),
        (
            "Echo (2005)",
            at(100, None),
            at(500, Some(EARLY)),
            at(500, Some(EARLY)),
        ),
        // A tie keeps the survivor's position, unless that is 0.
        (
            "Foxtrot (2006)",
            at(100, Some(LATE)),
            at(500, Some(LATE)),
            at(100, Some(LATE)),
        ),
        ("Golf (2007)", at(0, None), at(500, None), at(500, None)),
        ("Hotel (2008)", at(100, None), at(500, None), at(100, None)),
        // Dates compare as instants, not as text: 11:00+02:00 is before 10:00Z, and
        // 10:00+02:00 is before 09:00Z.
        (
            "India (2009)",
            at(100, Some("2024-06-01T10:00:00+00:00")),
            at(500, Some("2024-06-01T11:00:00+02:00")),
            at(100, Some("2024-06-01T10:00:00+00:00")),
        ),
        (
            "Juliett (2010)",
            at(100, Some("2024-06-01T10:00:00+02:00")),
            at(500, Some("2024-06-01T09:00:00+00:00")),
            at(500, Some("2024-06-01T09:00:00+00:00")),
        ),
        // One instant written two ways is a tie: the survivor's date, and the other's
        // position, since the survivor's is 0.
        (
            "Kilo (2011)",
            at(0, Some("2024-06-01T10:00:00+00:00")),
            at(500, Some("2024-06-01T12:00:00+02:00")),
            at(500, Some("2024-06-01T10:00:00+00:00")),
        ),
        // A date that does not parse is the oldest.
        (
            "Lima (2012)",
            at(100, Some("yesterday")),
            at(500, Some(EARLY)),
            at(500, Some(EARLY)),
        ),
    ];
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let user = insert_user(&fx.pool).await;
    let mut survivors = Vec::new();
    for (folder, kept, merged, _) in &cases {
        let (survivor, other) = two_copies(&fx.pool, &lib, folder).await;
        insert_marks(&fx.pool, &user, survivor, kept).await;
        insert_marks(&fx.pool, &user, other, merged).await;
        survivors.push(survivor);
    }

    scan_idle(&fx.pool, &lib).await;

    for ((folder, _, _, want), survivor) in cases.iter().zip(survivors) {
        let uhd = format!("{folder}/{folder} Bluray-2160p.mkv");
        assert_eq!(item_at(&fx.pool, &lib, &uhd).await, survivor, "{folder}");
        let got = marks_of(&fx.pool, &user, survivor).await;
        assert_eq!(got.as_ref(), Some(want), "{folder}");
    }
    let rows = count(&fx.pool, "SELECT COUNT(*) FROM user_item_data").await;
    assert_eq!(rows, cases.len() as i64);
}

/// Each user's rows combine on their own, and a row on only one copy ends on the survivor as
/// it was (G9b AC 2, 3).
#[tokio::test]
async fn merged_copies_keep_each_users_data() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let uhd = "Argo (2012)/Argo (2012) Bluray-2160p.mkv";
    let (survivor, other) = two_copies(pool, &lib, "Argo (2012)").await;
    let both = insert_user(pool).await;
    let other_only = insert_user(pool).await;
    let survivor_only = insert_user(pool).await;
    // `both` played the survivor, then watched part of the other copy later.
    let played = Marks {
        plays: 1,
        played: true,
        ..at(0, Some(EARLY))
    };
    insert_marks(pool, &both, survivor, &played).await;
    let partial = Marks {
        favorite: true,
        ..at(300, Some(LATE))
    };
    insert_marks(pool, &both, other, &partial).await;
    let on_other = Marks {
        plays: 2,
        played: true,
        rating: Some(7),
        ..at(0, Some(EARLY))
    };
    insert_marks(pool, &other_only, other, &on_other).await;
    let on_survivor = Marks {
        favorite: true,
        ..Marks::default()
    };
    insert_marks(pool, &survivor_only, survivor, &on_survivor).await;

    scan_idle(pool, &lib).await;

    assert_eq!(paths(pool, &lib, "Movie").await, [uhd]);
    assert_eq!(item_at(pool, &lib, uhd).await, survivor);
    let combined = Marks {
        plays: 1,
        played: true,
        favorite: true,
        ..at(300, Some(LATE))
    };
    assert_eq!(marks_of(pool, &both, survivor).await, Some(combined));
    assert_eq!(marks_of(pool, &other_only, survivor).await, Some(on_other));
    assert_eq!(
        marks_of(pool, &survivor_only, survivor).await,
        Some(on_survivor)
    );
    assert_eq!(count(pool, "SELECT COUNT(*) FROM user_item_data").await, 3);
}

/// Three copies merge one at a time in owner order: the survivor, then by creation date. The
/// 720p copy, made first, merges before the 1080p one, so its rating is the one kept
/// (G9b AC 4).
#[tokio::test]
async fn three_copies_merge_in_owner_order() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let made = |date| Movie {
        date_created: Some(date),
        ..Movie::default()
    };
    let uhd = "Heat (1995)/Heat (1995) Bluray-2160p.mkv";
    let hd_file = "Heat (1995)/Heat (1995) Bluray-1080p.mkv";
    let sd_file = "Heat (1995)/Heat (1995) Bluray-720p.mkv";
    let survivor = insert_movie(
        pool,
        &lib,
        &touch(&root, uhd),
        made("2022-01-01T00:00:00+00:00"),
    )
    .await;
    let hd = insert_movie(
        pool,
        &lib,
        &touch(&root, hd_file),
        made("2021-01-01T00:00:00+00:00"),
    )
    .await;
    let sd = insert_movie(
        pool,
        &lib,
        &touch(&root, sd_file),
        made("2020-01-01T00:00:00+00:00"),
    )
    .await;
    let user1 = insert_user(pool).await;
    let user2 = insert_user(pool).await;
    // user1 has a row on each copy. The 720p copy merges first and gives its position, date
    // and rating; the 1080p copy then gives a later date and position, but not its rating.
    let rows1 = [
        (
            survivor,
            Marks {
                plays: 1,
                ..Marks::default()
            },
        ),
        (
            hd,
            Marks {
                plays: 1,
                rating: Some(4),
                ..at(200, Some(LATE))
            },
        ),
        (
            sd,
            Marks {
                plays: 1,
                favorite: true,
                rating: Some(9),
                ..at(100, Some(EARLY))
            },
        ),
    ];
    for (item, marks) in &rows1 {
        insert_marks(pool, &user1, *item, marks).await;
    }
    // user2 has rows only on the two copies that go.
    let rows2 = [
        (
            hd,
            Marks {
                played: true,
                ..at(0, Some(EARLY))
            },
        ),
        (
            sd,
            Marks {
                favorite: true,
                ..at(50, Some(LATE))
            },
        ),
    ];
    for (item, marks) in &rows2 {
        insert_marks(pool, &user2, *item, marks).await;
    }

    scan_idle(pool, &lib).await;

    assert_eq!(paths(pool, &lib, "Movie").await, [uhd]);
    assert_eq!(item_at(pool, &lib, uhd).await, survivor);
    let want1 = Marks {
        plays: 3,
        favorite: true,
        rating: Some(9),
        ..at(200, Some(LATE))
    };
    assert_eq!(marks_of(pool, &user1, survivor).await, Some(want1));
    let want2 = Marks {
        favorite: true,
        played: true,
        ..at(50, Some(LATE))
    };
    assert_eq!(marks_of(pool, &user2, survivor).await, Some(want2));
    assert_eq!(count(pool, "SELECT COUNT(*) FROM user_item_data").await, 2);
}

/// Collections and playlists that hold either copy hold the survivor once, and an activity
/// entry, a session's playing item and a recording follow the survivor (G9b AC 6, 7).
#[tokio::test]
async fn merged_copies_keep_memberships_and_references() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let (argo, argo_hd) = two_copies(pool, &lib, "Argo (2012)").await;
    let (heat, heat_hd) = two_copies(pool, &lib, "Heat (1995)").await;
    let user = insert_user(pool).await;
    let new_id = || Uuid::new_v4().to_string();
    let (both_set, other_set, both_list, other_list) = (new_id(), new_id(), new_id(), new_id());
    for set in [&both_set, &other_set] {
        exec(
            pool,
            "INSERT INTO collections (id, name, user_id) VALUES (?1, 'Set', ?2)",
            &[set, &user],
        )
        .await;
    }
    for list in [&both_list, &other_list] {
        exec(
            pool,
            "INSERT INTO playlists (id, name, user_id) VALUES (?1, 'List', ?2)",
            &[list, &user],
        )
        .await;
    }
    const IN_SET: &str =
        "INSERT INTO collection_items (collection_id, item_id, sort_order) VALUES (?1, ?2, ?3)";
    const IN_LIST: &str =
        "INSERT INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?1, ?2, ?3)";
    for (sql, container, item, order) in [
        (IN_SET, &both_set, argo, "1"),
        (IN_SET, &both_set, argo_hd, "2"),
        (IN_SET, &other_set, heat_hd, "5"),
        (IN_LIST, &both_list, heat, "3"),
        (IN_LIST, &both_list, heat_hd, "4"),
        (IN_LIST, &other_list, argo_hd, "7"),
    ] {
        exec(pool, sql, &[container, &item.to_string(), order]).await;
    }
    exec(
        pool,
        "INSERT INTO activity_log (name, type, user_id, item_id)
         VALUES ('G9b played', 'VideoPlayback', ?1, ?2)",
        &[&user, &argo_hd.to_string()],
    )
    .await;
    let session = new_id();
    exec(
        pool,
        "INSERT INTO sessions (id, user_id, device_id, now_playing_item_id)
         VALUES (?1, ?2, 'G9b device', ?3)",
        &[&session, &user, &heat_hd.to_string()],
    )
    .await;
    let channel = new_id();
    exec(
        pool,
        "INSERT INTO live_tv_channels (id, name) VALUES (?1, 'G9b channel')",
        &[&channel],
    )
    .await;
    exec(
        pool,
        "INSERT INTO live_tv_recordings (id, media_item_id, channel_id, name)
         VALUES (?1, ?2, ?3, 'G9b recording')",
        &[&new_id(), &argo_hd.to_string(), &channel],
    )
    .await;

    scan_idle(pool, &lib).await;

    assert_eq!(
        paths(pool, &lib, "Movie").await,
        [
            "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
            "Heat (1995)/Heat (1995) Bluray-2160p.mkv"
        ]
    );
    let mut want_sets = vec![
        (both_set, argo.to_string(), 1),
        (other_set, heat.to_string(), 5),
    ];
    want_sets.sort();
    let sets: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT collection_id, item_id, sort_order FROM collection_items
         ORDER BY collection_id, item_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(sets, want_sets);
    let mut want_lists = vec![
        (both_list, heat.to_string(), 3),
        (other_list, argo.to_string(), 7),
    ];
    want_lists.sort();
    let lists: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT playlist_id, item_id, sort_order FROM playlist_items
         ORDER BY playlist_id, item_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(lists, want_lists);
    let activity: Vec<String> =
        sqlx::query_scalar("SELECT item_id FROM activity_log WHERE name = 'G9b played'")
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(activity, [argo.to_string()]);
    let playing: Vec<String> =
        sqlx::query_scalar("SELECT now_playing_item_id FROM sessions WHERE id = ?1")
            .bind(&session)
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(playing, [heat.to_string()]);
    let recorded: Vec<String> = sqlx::query_scalar(
        "SELECT media_item_id FROM live_tv_recordings WHERE name = 'G9b recording'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(recorded, [argo.to_string()]);
}

/// Copies of an episode merge the same way, and the pins of both copies end on the survivor
/// in its season (G9b AC 5).
#[tokio::test]
async fn merged_episode_copies_keep_pins_and_user_data() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    touch(&root, SEV_E1_SD);
    touch(&root, SEV_E1_HD);
    let sev = insert_tv(pool, &lib, "Series", "Severance", None, Tv::default()).await;
    let season_1 = Tv {
        sort_name: Some("season 0001"),
        index: Some(1),
        ..Tv::default()
    };
    let s1 = insert_tv(
        pool,
        &lib,
        "Season",
        "Severance/Season 1",
        Some(sev),
        season_1,
    )
    .await;
    let made = |date| Tv {
        parent_index: Some(1),
        date_created: Some(date),
        ..Tv::default()
    };
    let sd = insert_tv(
        pool,
        &lib,
        "Episode",
        SEV_E1_SD,
        Some(s1),
        made("2020-01-01T00:00:00+00:00"),
    )
    .await;
    let hd = insert_tv(
        pool,
        &lib,
        "Episode",
        SEV_E1_HD,
        Some(s1),
        made("2021-01-01T00:00:00+00:00"),
    )
    .await;
    let user1 = insert_user(pool).await;
    let user2 = insert_user(pool).await;
    // user1 pinned the 720p copy, and user2 the 1080p one, which survives.
    for (user, episode) in [(&user1, sd), (&user2, hd)] {
        exec(
            pool,
            "INSERT INTO pinned_episodes (user_id, series_id, episode_id, season_id, created_date)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            &[
                user,
                &sev.to_string(),
                &episode.to_string(),
                &s1.to_string(),
            ],
        )
        .await;
    }
    let played = Marks {
        plays: 1,
        played: true,
        ..at(0, Some(EARLY))
    };
    insert_marks(pool, &user1, sd, &played).await;
    insert_marks(pool, &user1, hd, &at(400, Some(LATE))).await;
    let on_sd = Marks {
        favorite: true,
        ..Marks::default()
    };
    insert_marks(pool, &user2, sd, &on_sd).await;

    scan_idle(pool, &lib).await;

    assert_eq!(item_at(pool, &lib, SEV_E1_HD).await, hd);
    let episodes = count(
        pool,
        "SELECT COUNT(*) FROM media_items WHERE media_type = 'Episode'",
    )
    .await;
    assert_eq!(episodes, 1);
    let season = item_at(pool, &lib, "Severance/Season 1").await;
    let mut want_pins = [&user1, &user2].map(|user| {
        (
            user.clone(),
            sev.to_string(),
            hd.to_string(),
            season.to_string(),
        )
    });
    want_pins.sort();
    let pins: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT user_id, series_id, episode_id, season_id FROM pinned_episodes ORDER BY user_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(pins, want_pins);
    let combined = Marks {
        plays: 1,
        played: true,
        ..at(400, Some(LATE))
    };
    assert_eq!(marks_of(pool, &user1, hd).await, Some(combined));
    assert_eq!(marks_of(pool, &user2, hd).await, Some(on_sd));
    assert_eq!(count(pool, "SELECT COUNT(*) FROM user_item_data").await, 2);
}

// G9b: a version that cannot be probed is not the primary

/// A `failed_at` older than any scan, so that a new probe of the file shows.
const LONG_AGO: &str = "2000-01-01T00:00:00+00:00";

/// The size of `path` and its modification time in whole seconds since the Unix epoch.
fn stat(path: &Path) -> (i64, i64) {
    let meta = std::fs::metadata(path).unwrap();
    let modified = meta.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    (
        i64::try_from(meta.len()).unwrap(),
        i64::try_from(modified.as_secs()).unwrap(),
    )
}

/// The probe failure recorded for `path`, as (size, modified, failed_at).
async fn failure_of(pool: &SqlitePool, path: &Path) -> Option<(i64, i64, String)> {
    sqlx::query_as("SELECT size, modified, failed_at FROM probe_failures WHERE path = ?1")
        .bind(path.to_str().unwrap())
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// The record of a failed probe of `path` as the file is now, failed at `failed_at`.
fn failure(path: &Path, failed_at: &str) -> Option<(i64, i64, String)> {
    let (size, modified) = stat(path);
    Some((size, modified, failed_at.to_string()))
}

/// ffmpeg. When the primary version cannot be probed, the next version that can is the item's
/// path, first source and runtime. Rescans probe nothing and keep the item, and once the broken
/// file is fixed it is the primary again, on the same item (G9b AC 9, AC 11-14).
#[tokio::test]
async fn unprobeable_primary_gives_way() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let uhd = "Alien vs Predator (2004)/Alien vs Predator (2004) Bluray-2160p.mkv";
    let hd = "Alien vs Predator (2004)/Alien vs Predator (2004) Bluray-1080p.mkv";
    let broken = touch(&root, uhd);
    clip(&fx, &root, hd, 2);

    scan_idle(pool, &lib).await;

    // AC 13: the 2160p failed as the primary, so the 1080p took its place.
    assert_groups(pool, &lib, &[(hd, &[hd])]).await;
    let (_, _, failed_at) = failure_of(pool, &broken)
        .await
        .expect("the failed probe is recorded");
    assert_eq!(
        failure_of(pool, &broken).await,
        failure(&broken, &failed_at)
    );

    // AC 12, AC 14: rescans probe neither file and keep the item.
    let item = item_at(pool, &lib, hd).await;
    let ids = source_ids(pool, &lib).await;
    exec(
        pool,
        "UPDATE probe_failures SET failed_at = ?1",
        &[LONG_AGO],
    )
    .await;
    for _ in 0..2 {
        scan_idle(pool, &lib).await;
        assert_eq!(source_ids(pool, &lib).await, ids, "a rescan probed again");
        assert_eq!(
            failure_of(pool, &broken).await,
            failure(&broken, LONG_AGO),
            "a rescan probed the broken file again"
        );
        assert_eq!(item_at(pool, &lib, hd).await, item);
    }

    // AC 14: the fixed file is the primary again, on the same item, and its record is gone.
    clip(&fx, &root, uhd, 3);
    scan_idle(pool, &lib).await;
    assert_groups(pool, &lib, &[(uhd, &[uhd, hd])]).await;
    assert_eq!(item_at(pool, &lib, uhd).await, item);
    assert_eq!(failure_of(pool, &broken).await, None);
}

/// ffmpeg. A file that cannot be probed is recorded with its size and modification time, and
/// scans probe it again only when it changes. A probe that works removes the record. A group
/// whose files all fail keeps its first file as the path (G9b AC 9-12).
#[tokio::test]
async fn failed_probes_are_not_repeated() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let broken = "Broken (2010)/Broken (2010).mkv";
    let twice_uhd = "Twice (2011)/Twice (2011) Bluray-2160p.mkv";
    let twice_hd = "Twice (2011)/Twice (2011) Bluray-1080p.mkv";
    let files = [broken, twice_uhd, twice_hd].map(|rel| touch(&root, rel));

    scan_idle(pool, &lib).await;

    // AC 9: every failed probe is recorded with the file's size and modification time.
    for file in &files {
        let (_, _, failed_at) = failure_of(pool, file)
            .await
            .unwrap_or_else(|| panic!("{}: the failed probe is not recorded", file.display()));
        assert_eq!(failure_of(pool, file).await, failure(file, &failed_at));
    }
    // AC 11: a group whose files all fail starts on its first file.
    assert_eq!(paths(pool, &lib, "Movie").await, [broken, twice_uhd]);

    // AC 10, AC 12: files that have not changed are not probed again.
    exec(
        pool,
        "UPDATE probe_failures SET failed_at = ?1",
        &[LONG_AGO],
    )
    .await;
    for _ in 0..2 {
        scan_idle(pool, &lib).await;
        for file in &files {
            assert_eq!(
                failure_of(pool, file).await,
                failure(file, LONG_AGO),
                "{}: a rescan probed it again",
                file.display()
            );
        }
    }
    assert_eq!(paths(pool, &lib, "Movie").await, [broken, twice_uhd]);

    // AC 10: a changed file is probed again. It still fails, and its record follows the file.
    std::fs::write(&files[0], b"not a video file\n").unwrap();
    scan_idle(pool, &lib).await;
    let (_, _, failed_at) = failure_of(pool, &files[0]).await.unwrap();
    assert_ne!(failed_at, LONG_AGO, "the changed file was not probed");
    assert_eq!(
        failure_of(pool, &files[0]).await,
        failure(&files[0], &failed_at)
    );
    scan_idle(pool, &lib).await;
    assert_eq!(
        failure_of(pool, &files[0]).await,
        failure(&files[0], &failed_at),
        "a rescan probed the changed file again"
    );

    // AC 9: once the file can be probed, it is the item's source and its record is gone.
    clip(&fx, &root, broken, 2);
    scan_idle(pool, &lib).await;
    assert_eq!(failure_of(pool, &files[0]).await, None);
    let items = snapshot(pool, &lib).await;
    let fixed = items.iter().find(|i| i.path == broken).unwrap();
    assert_eq!(fixed.sources, [broken]);
    assert!(
        fixed.runtime_ticks.is_some(),
        "the fixed file gave no runtime"
    );
}

/// ffmpeg. A new movie or episode does not start on a version whose probe failed before, such
/// as in a library that was removed and added again: its path is the first version that can
/// be probed, and the known failure is not probed again (G9b AC 11, AC 12).
#[tokio::test]
async fn new_items_skip_known_failures() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let uhd = "Alien vs Predator (2004)/Alien vs Predator (2004) Bluray-2160p.mkv";
    let hd = "Alien vs Predator (2004)/Alien vs Predator (2004) Bluray-1080p.mkv";
    let episode_uhd = "Show/Season 1/Show S01E01 2160p.mkv";
    let episode_hd = "Show/Season 1/Show S01E01 1080p.mkv";
    let broken = [
        touch(&root_of(&film), uhd),
        touch(&root_of(&tv), episode_uhd),
    ];
    clip(&fx, &root_of(&film), hd, 2);
    clip(&fx, &root_of(&tv), episode_hd, 2);
    for file in &broken {
        let (size, modified) = stat(file);
        exec(
            pool,
            "INSERT INTO probe_failures (path, size, modified, failed_at) VALUES (?1, ?2, ?3, ?4)",
            &[
                file.to_str().unwrap(),
                &size.to_string(),
                &modified.to_string(),
                LONG_AGO,
            ],
        )
        .await;
    }

    scan_idle(pool, &film).await;
    scan_idle(pool, &tv).await;

    assert_groups(pool, &film, &[(hd, &[hd])]).await;
    assert_groups(pool, &tv, &[(episode_hd, &[episode_hd])]).await;
    for file in &broken {
        assert_eq!(
            failure_of(pool, file).await,
            failure(file, LONG_AGO),
            "{}: a known failure was probed again",
            file.display()
        );
    }
}

// B23: date added. A new item is dated the way Jellyfin dates it on Linux: by its path's creation
// time, which .NET reads there as the older of the modification and status-change times. A file
// that is a symlink is dated by its final target, as Jellyfin resolves one; a folder link keeps its
// own times. A movie by its video file; a series, season, artist or album by its folder; an
// episode, track or photo by its file.

/// Sets the modification time of a file or folder to `secs` after the epoch (which sets its
/// status-change time to now).
#[cfg(target_os = "linux")]
fn b23_set_mtime(path: &Path, secs: u64) {
    std::fs::File::open(path)
        .and_then(|f| f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)))
        .unwrap_or_else(|e| panic!("set the time of {}: {e}", path.display()));
}

/// The stored date_created of the item at `rel`, in whole seconds since the epoch.
#[cfg(target_os = "linux")]
async fn b23_date_created(pool: &SqlitePool, lib: &Library, rel: &str) -> i64 {
    let id = item_at(pool, lib, rel).await;
    let stored: String = sqlx::query_scalar("SELECT date_created FROM media_items WHERE id = ?1")
        .bind(id.to_string())
        .fetch_one(pool)
        .await
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(&stored)
        .unwrap_or_else(|e| panic!("date_created of {rel} is {stored}: {e}"))
        .timestamp()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b23_new_items_are_dated_by_their_files() {
    // A second early: file times come from the kernel's coarse clock, which can lag a few ms.
    let before = chrono::Utc::now().timestamp() - 1;
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let music = fx.library("music", LibraryContentType::Music).await;
    let photos = fx.library("photos", LibraryContentType::Photos).await;
    let old_film = "Old Film (1999)/Old Film (1999).mkv";
    let new_film = "New Film (2001)/New Film (2001).mkv";
    let episode = "Show/Season 1/Show S01E01.mkv";
    let track = "Artist A/Album A/01 Song.mp3";
    touch(&root_of(&film), old_film);
    touch(&root_of(&film), new_film);
    touch(&root_of(&tv), episode);
    touch(&root_of(&music), track);
    touch(&root_of(&photos), "Beach.jpg");
    // A series folder outside the library, linked in: the link's own times count.
    touch(
        &fx.path("elsewhere"),
        "Linked Show/Season 1/Linked Show S01E01.mkv",
    );
    std::os::unix::fs::symlink(
        fx.path("elsewhere/Linked Show"),
        root_of(&tv).join("Linked Show"),
    )
    .unwrap();
    // A film whose video file is a link to a link to a file outside the library: the final file's
    // times count.
    let linked_film = "Linked Film (2003)/Linked Film (2003).mkv";
    touch(&fx.path("elsewhere"), "Linked Film.mkv");
    std::os::unix::fs::symlink(
        fx.path("elsewhere/Linked Film.mkv"),
        fx.path("elsewhere/hop.mkv"),
    )
    .unwrap();
    std::fs::create_dir_all(root_of(&film).join("Linked Film (2003)")).unwrap();
    std::os::unix::fs::symlink(
        fx.path("elsewhere/hop.mkv"),
        root_of(&film).join(linked_film),
    )
    .unwrap();
    // Old modification times, set once every file exists, since a new file changes its folder's.
    let old = [
        (root_of(&film).join(old_film), 1_000_000_000),
        (root_of(&tv).join(episode), 1_000_000_100),
        (root_of(&tv).join("Show/Season 1"), 1_000_000_200),
        (root_of(&tv).join("Show"), 1_000_000_300),
        (root_of(&music).join(track), 1_000_000_400),
        (root_of(&music).join("Artist A/Album A"), 1_000_000_500),
        (root_of(&music).join("Artist A"), 1_000_000_600),
        (root_of(&photos).join("Beach.jpg"), 1_000_000_700),
        (fx.path("elsewhere/Linked Show"), 1_000_000_800),
        (fx.path("elsewhere/Linked Film.mkv"), 1_000_000_900),
    ];
    for (path, secs) in &old {
        b23_set_mtime(path, *secs);
    }
    // A modification time in the future: the status-change time, now, is the older one.
    b23_set_mtime(&root_of(&film).join(new_film), 4_000_000_000);
    for lib in [&film, &tv, &music, &photos] {
        scan(&fx.pool, lib).await;
    }
    let after = chrono::Utc::now().timestamp();

    for (lib, rel, secs) in [
        (&film, old_film, 1_000_000_000),
        (&tv, episode, 1_000_000_100),
        (&tv, "Show/Season 1", 1_000_000_200),
        (&tv, "Show", 1_000_000_300),
        (&music, track, 1_000_000_400),
        (&music, "Artist A/Album A", 1_000_000_500),
        (&music, "Artist A", 1_000_000_600),
        (&photos, "Beach.jpg", 1_000_000_700),
        (&film, linked_film, 1_000_000_900),
    ] {
        assert_eq!(b23_date_created(&fx.pool, lib, rel).await, secs, "{rel}");
    }
    for (lib, rel) in [(&film, new_film), (&tv, "Linked Show")] {
        let date = b23_date_created(&fx.pool, lib, rel).await;
        assert!(
            (before..=after).contains(&date),
            "{rel}: {date} is not in {before}..={after}"
        );
    }
}

// B28: date added backfill. Once after the update, an item already in a library gets the earlier
// of its stored date and its path's date by B23's rules, since a scan before B23 stored the time of
// the scan. An earlier or equal stored date is kept, and so is the date of an item whose path can't
// be read. Once done it doesn't run again, and a run that stops part way goes on at the next start.

/// The date_created stored for the item at `rel`, as stored.
#[cfg(target_os = "linux")]
async fn b28_stored(pool: &SqlitePool, lib: &Library, rel: &str) -> String {
    let id = item_at(pool, lib, rel).await;
    sqlx::query_scalar("SELECT date_created FROM media_items WHERE id = ?1")
        .bind(id.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The date_created stored for the item at `rel`.
#[cfg(target_os = "linux")]
async fn b28_date(pool: &SqlitePool, lib: &Library, rel: &str) -> chrono::DateTime<chrono::Utc> {
    let stored = b28_stored(pool, lib, rel).await;
    chrono::DateTime::parse_from_rfc3339(&stored)
        .unwrap_or_else(|e| panic!("date_created of {rel} is {stored}: {e}"))
        .with_timezone(&chrono::Utc)
}

/// `secs` seconds after the epoch.
#[cfg(target_os = "linux")]
fn b28_at(secs: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs as i64, 0).unwrap()
}

/// Stores `date` for every item in the library, as a scan before B23 stored the time of the scan.
#[cfg(target_os = "linux")]
async fn b28_date_all(pool: &SqlitePool, lib: &Library, date: chrono::DateTime<chrono::Utc>) {
    sqlx::query("UPDATE media_items SET date_created = ?1 WHERE library_id = ?2")
        .bind(date.to_rfc3339())
        .bind(lib.id.to_string())
        .execute(pool)
        .await
        .unwrap();
}

/// Stores `date`, as given, for the item at `rel`.
#[cfg(target_os = "linux")]
async fn b28_set_date(pool: &SqlitePool, lib: &Library, rel: &str, date: &str) {
    let id = item_at(pool, lib, rel).await;
    sqlx::query("UPDATE media_items SET date_created = ?1 WHERE id = ?2")
        .bind(date)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
}

/// B23's date for a path that isn't a link to a file: the older of its own modification and
/// status-change times.
#[cfg(target_os = "linux")]
fn b28_own_date(path: &Path) -> chrono::DateTime<chrono::Utc> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).unwrap();
    let (secs, nanos) = std::cmp::min(
        (meta.mtime(), meta.mtime_nsec()),
        (meta.ctime(), meta.ctime_nsec()),
    );
    chrono::DateTime::from_timestamp(secs, nanos as u32).unwrap()
}

/// Makes the backfill pending again from the start, as the update's migration does.
#[cfg(target_os = "linux")]
async fn b28_arm(pool: &SqlitePool) {
    sqlx::query(
        "INSERT OR REPLACE INTO configuration (key, value) VALUES ('date_added_backfill', '')",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Runs the whole backfill, which must be pending.
#[cfg(target_os = "linux")]
async fn b28_run(pool: &SqlitePool) -> rustyfin_core::date_backfill::BackfillRun {
    rustyfin_core::date_backfill::run_pending(pool, |_| {})
        .await
        .unwrap()
        .expect("a pending backfill")
}

/// What a run did: (checked, moved, unreadable, finished).
#[cfg(target_os = "linux")]
fn b28_counts(run: rustyfin_core::date_backfill::BackfillRun) -> (u64, u64, u64, bool) {
    (run.checked, run.moved, run.unreadable, run.finished)
}

/// How many of `films` have their file's date, film n's being 1_000_000_000 + n seconds after the
/// epoch; the others must still have `scanned`.
#[cfg(target_os = "linux")]
async fn b28_dated(
    pool: &SqlitePool,
    lib: &Library,
    films: &[String],
    scanned: chrono::DateTime<chrono::Utc>,
) -> usize {
    let mut dated = 0;
    for (n, rel) in films.iter().enumerate() {
        let date = b28_date(pool, lib, rel).await;
        if date == b28_at(1_000_000_000 + n as u64) {
            dated += 1;
        } else {
            assert_eq!(date, scanned, "{rel}");
        }
    }
    dated
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_items_dated_by_their_scan_move_to_their_paths_dates() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let music = fx.library("music", LibraryContentType::Music).await;
    let photos = fx.library("photos", LibraryContentType::Photos).await;
    let movie = "Old Film (1999)/Old Film (1999).mkv";
    let episode = "Show/Season 1/Show S01E01.mkv";
    let track = "Artist A/Album A/01 Song.mp3";
    touch(&root_of(&film), movie);
    touch(&root_of(&tv), episode);
    touch(&root_of(&music), track);
    touch(&root_of(&photos), "Beach.jpg");
    // Old modification times, set once every file exists, since a new file changes its folder's.
    let dated = [
        (&film, movie, 1_000_000_000),
        (&tv, episode, 1_000_000_100),
        (&tv, "Show/Season 1", 1_000_000_200),
        (&tv, "Show", 1_000_000_300),
        (&music, track, 1_000_000_400),
        (&music, "Artist A/Album A", 1_000_000_500),
        (&music, "Artist A", 1_000_000_600),
        (&photos, "Beach.jpg", 1_000_000_700),
    ];
    for (lib, rel, secs) in dated {
        b23_set_mtime(&root_of(lib).join(rel), secs);
    }
    for lib in [&film, &tv, &music, &photos] {
        scan(&fx.pool, lib).await;
        b28_date_all(&fx.pool, lib, chrono::Utc::now()).await;
    }

    let run = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(run), (8, 8, 0, true));
    for (lib, rel, secs) in dated {
        assert_eq!(b28_date(&fx.pool, lib, rel).await, b28_at(secs), "{rel}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_an_earlier_or_equal_stored_date_is_kept() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let earlier = "Earlier (2000)/Earlier (2000).mkv";
    let equal = "Equal (2001)/Equal (2001).mkv";
    let later = "Later (2002)/Later (2002).mkv";
    let unparsed = "Unparsed (2003)/Unparsed (2003).mkv";
    let films = [earlier, equal, later, unparsed];
    for rel in films {
        touch(&root_of(&film), rel);
    }
    for rel in films {
        b23_set_mtime(&root_of(&film).join(rel), 1_000_000_000);
    }
    scan(&fx.pool, &film).await;
    // Dates an import or an older server could have stored: before the file's, the file's own,
    // after it (the time of a scan before B23), and one in the column's default format, which has
    // no offset, so the server reads it as the epoch.
    b28_set_date(&fx.pool, &film, earlier, "2000-01-01T00:00:00+00:00").await;
    b28_set_date(&fx.pool, &film, equal, "2001-09-09T01:46:40+00:00").await;
    b28_set_date(&fx.pool, &film, later, &chrono::Utc::now().to_rfc3339()).await;
    b28_set_date(&fx.pool, &film, unparsed, "2024-01-01 00:00:00").await;

    let run = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(run), (4, 1, 0, true));
    assert_eq!(
        b28_date(&fx.pool, &film, earlier).await,
        b28_at(946_684_800)
    );
    assert_eq!(
        b28_date(&fx.pool, &film, equal).await,
        b28_at(1_000_000_000)
    );
    assert_eq!(
        b28_date(&fx.pool, &film, later).await,
        b28_at(1_000_000_000)
    );
    assert_eq!(
        b28_stored(&fx.pool, &film, unparsed).await,
        "2024-01-01 00:00:00"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_stored_date_with_an_offset_is_compared_as_a_time() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let rel = "Old Film (1999)/Old Film (1999).mkv";
    touch(&root_of(&film), rel);
    b23_set_mtime(&root_of(&film).join(rel), 1_000_000_000);
    scan(&fx.pool, &film).await;
    // An hour before the file's date, written with +02:00: earlier as a time, later as text.
    let stored = "2001-09-09T02:46:40+02:00";
    assert!(stored > b28_at(1_000_000_000).to_rfc3339().as_str());
    b28_set_date(&fx.pool, &film, rel, stored).await;

    let run = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(run), (1, 0, 0, true));
    assert_eq!(b28_stored(&fx.pool, &film, rel).await, stored);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_path_that_cant_be_read_keeps_its_date() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let gone = "Gone (1999)/Gone (1999).mkv";
    let kept = "Kept (2001)/Kept (2001).mkv";
    // An episode outside a season folder: the scanner makes up "Flat Show/Season 1", which has no
    // folder.
    let episode = "Flat Show/Flat Show S01E01.mkv";
    touch(&root_of(&film), gone);
    touch(&root_of(&film), kept);
    touch(&root_of(&tv), episode);
    let dated = [
        (&film, kept, 1_000_000_000),
        (&tv, episode, 1_000_000_100),
        (&tv, "Flat Show", 1_000_000_300),
    ];
    for (lib, rel, secs) in dated {
        b23_set_mtime(&root_of(lib).join(rel), secs);
    }
    scan(&fx.pool, &film).await;
    scan(&fx.pool, &tv).await;
    let scanned = chrono::Utc::now();
    b28_date_all(&fx.pool, &film, scanned).await;
    b28_date_all(&fx.pool, &tv, scanned).await;
    // A file gone since the scan, as on a share that's offline or a drive that's unplugged.
    std::fs::remove_file(root_of(&film).join(gone)).unwrap();

    let run = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(run), (5, 3, 2, true));
    for (lib, rel) in [(&film, gone), (&tv, "Flat Show/Season 1")] {
        assert_eq!(b28_date(&fx.pool, lib, rel).await, scanned, "{rel}");
    }
    for (lib, rel, secs) in dated {
        assert_eq!(b28_date(&fx.pool, lib, rel).await, b28_at(secs), "{rel}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_second_run_changes_nothing_and_a_stopped_run_finishes_on_the_next_start() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let films: Vec<String> = (0..5)
        .map(|n| format!("Film {n} (200{n})/Film {n} (200{n}).mkv"))
        .collect();
    for rel in &films {
        touch(&root_of(&film), rel);
    }
    for (n, rel) in films.iter().enumerate() {
        b23_set_mtime(&root_of(&film).join(rel), 1_000_000_000 + n as u64);
    }
    scan(&fx.pool, &film).await;
    let scanned = chrono::Utc::now();
    b28_date_all(&fx.pool, &film, scanned).await;

    // A run stopped after its first batch, as by a shutdown, has dated that batch only.
    let stopped = rustyfin_core::date_backfill::run_some(&fx.pool, 2, 1, |_| {})
        .await
        .unwrap()
        .expect("a pending backfill");
    assert_eq!(b28_counts(stopped), (2, 2, 0, false));
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 2);
    // The next start goes on from there.
    let next = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(next), (3, 3, 0, true));
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 5);
    // Done: later starts and scans don't run it.
    assert_eq!(
        rustyfin_core::date_backfill::run_pending(&fx.pool, |_| {})
            .await
            .unwrap(),
        None
    );
    scan(&fx.pool, &film).await;
    assert_eq!(
        rustyfin_core::date_backfill::run_pending(&fx.pool, |_| {})
            .await
            .unwrap(),
        None
    );
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 5);
    // Run again over items already done, it changes nothing.
    b28_arm(&fx.pool).await;
    let again = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(again), (5, 0, 0, true));
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 5);
}

#[cfg(target_os = "linux")]
#[derive(Clone, Default)]
struct B28Logs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(target_os = "linux")]
impl std::io::Write for B28Logs {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The backfill's log lines, from every test in this binary since the first call. A subscriber
/// for one thread misses events from a place another thread logged from first.
#[cfg(target_os = "linux")]
fn b28_logged() -> Vec<String> {
    static LOGS: std::sync::OnceLock<B28Logs> = std::sync::OnceLock::new();
    let logs = LOGS.get_or_init(|| {
        let logs = B28Logs::default();
        let sink = logs.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(move || sink.clone())
                .finish(),
        )
        .expect("no other global subscriber in this test binary");
        logs
    });
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    text.lines()
        .filter(|line| line.contains("date added backfill: "))
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_run_stopped_by_an_error_logs_its_end() {
    b28_logged();
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let films: Vec<String> = (0..4)
        .map(|n| format!("Film {n} (200{n})/Film {n} (200{n}).mkv"))
        .collect();
    for rel in &films {
        touch(&root_of(&film), rel);
    }
    for (n, rel) in films.iter().enumerate() {
        b23_set_mtime(&root_of(&film).join(rel), 1_000_000_000 + n as u64);
    }
    scan(&fx.pool, &film).await;
    let scanned = chrono::Utc::now();
    b28_date_all(&fx.pool, &film, scanned).await;
    // The run goes in id order, two items a batch. The last item's move fails after the second
    // batch has moved the item before it, so that whole batch is undone.
    let last: String = sqlx::query_scalar(
        "SELECT MAX(id) FROM media_items WHERE library_id = ?1 AND media_type = 'Movie'",
    )
    .bind(film.id.to_string())
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    let fail = format!(
        "CREATE TRIGGER b28_fail BEFORE UPDATE OF date_created ON media_items \
         WHEN OLD.id = '{last}' BEGIN SELECT RAISE(ABORT, 'b28: a failed write'); END"
    );
    sqlx::query(sqlx::AssertSqlSafe(fail))
        .execute(&fx.pool)
        .await
        .unwrap();

    let stopped = rustyfin_core::date_backfill::run_some(&fx.pool, 2, u32::MAX, |_| {}).await;
    assert!(stopped.is_err(), "{stopped:?}");
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 2);
    let logged = b28_logged();
    assert!(
        logged
            .iter()
            .any(|line| line.ends_with("date added backfill: started")),
        "{logged:#?}"
    );
    let end = logged
        .iter()
        .find(|line| line.contains("date added backfill: stopped by an error in "))
        .unwrap_or_else(|| panic!("no end line in {logged:#?}"));
    assert!(
        end.ends_with("; 4 items checked, 2 moved to their path's date, 0 unreadable"),
        "{end}"
    );

    // The next start redoes the batch that failed.
    exec(&fx.pool, "DROP TRIGGER b28_fail", &[]).await;
    let next = b28_run(&fx.pool).await;
    assert_eq!(b28_counts(next), (2, 2, 0, true));
    assert_eq!(b28_dated(&fx.pool, &film, &films, scanned).await, 4);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b28_each_batch_is_reported_once_it_has_committed() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let films: Vec<String> = (0..5)
        .map(|n| format!("Film {n} (200{n})/Film {n} (200{n}).mkv"))
        .collect();
    for rel in &films {
        touch(&root_of(&film), rel);
    }
    for (n, rel) in films.iter().enumerate() {
        b23_set_mtime(&root_of(&film).join(rel), 1_000_000_000 + n as u64);
    }
    scan(&fx.pool, &film).await;
    let scanned = chrono::Utc::now();
    b28_date_all(&fx.pool, &film, scanned).await;

    // At each report: the run so far, and how many films another connection sees moved. A report
    // made before its batch commits would wait on the batch's lock, so that read gives up at 10 s.
    let mut reports = Vec::new();
    let run = rustyfin_core::date_backfill::run_some(&fx.pool, 2, u32::MAX, |run| {
        let seen = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let read = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM media_items WHERE library_id = ?1 \
                     AND media_type = 'Movie' AND date_created <> ?2",
                )
                .bind(film.id.to_string())
                .bind(scanned.to_rfc3339())
                .fetch_one(&fx.pool);
                match tokio::time::timeout(Duration::from_secs(10), read).await {
                    Ok(count) => count.unwrap(),
                    Err(_) => -1,
                }
            })
        });
        reports.push((run.checked, run.moved, seen));
    })
    .await
    .unwrap()
    .expect("a pending backfill");
    assert_eq!(b28_counts(run), (5, 5, 0, true));
    assert_eq!(reports, [(2, 2, 2), (4, 4, 4), (5, 5, 5)]);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_file_link_is_dated_by_its_target_and_a_folder_link_by_its_own_times() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    // As in B23's test: a series folder outside the library, linked in, and a film whose video
    // file is a link to a link to a file outside the library.
    touch(
        &fx.path("elsewhere"),
        "Linked Show/Season 1/Linked Show S01E01.mkv",
    );
    std::os::unix::fs::symlink(
        fx.path("elsewhere/Linked Show"),
        root_of(&tv).join("Linked Show"),
    )
    .unwrap();
    let linked_film = "Linked Film (2003)/Linked Film (2003).mkv";
    touch(&fx.path("elsewhere"), "Linked Film.mkv");
    std::os::unix::fs::symlink(
        fx.path("elsewhere/Linked Film.mkv"),
        fx.path("elsewhere/hop.mkv"),
    )
    .unwrap();
    std::fs::create_dir_all(root_of(&film).join("Linked Film (2003)")).unwrap();
    std::os::unix::fs::symlink(
        fx.path("elsewhere/hop.mkv"),
        root_of(&film).join(linked_film),
    )
    .unwrap();
    b23_set_mtime(&fx.path("elsewhere/Linked Show"), 1_000_000_800);
    b23_set_mtime(&fx.path("elsewhere/Linked Film.mkv"), 1_000_000_900);
    scan(&fx.pool, &film).await;
    scan(&fx.pool, &tv).await;
    let scanned = chrono::Utc::now();
    b28_date_all(&fx.pool, &film, scanned).await;
    b28_date_all(&fx.pool, &tv, scanned).await;

    b28_run(&fx.pool).await;
    assert_eq!(
        b28_date(&fx.pool, &film, linked_film).await,
        b28_at(1_000_000_900)
    );
    assert_eq!(
        b28_date(&fx.pool, &tv, "Linked Show").await,
        b28_own_date(&root_of(&tv).join("Linked Show"))
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_saving_an_item_read_before_the_backfill_keeps_its_new_date() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let movie = "Old Film (1999)/Old Film (1999).mkv";
    touch(&root_of(&film), movie);
    b23_set_mtime(&root_of(&film).join(movie), 1_000_000_000);
    scan(&fx.pool, &film).await;
    b28_date_all(&fx.pool, &film, chrono::Utc::now()).await;
    // A metadata refresh reads its items, then saves each whole item once it has new metadata. By
    // then the copy it read can hold the date the backfill has replaced.
    let id = item_at(&fx.pool, &film, movie).await;
    let mut refreshed = rustyfin_db::repo_media::get_item_by_id(&fx.pool, id)
        .await
        .unwrap()
        .expect("the film");
    b28_run(&fx.pool).await;
    refreshed.overview = Some("Found by the refresh.".to_string());
    rustyfin_db::repo_media::upsert_media_item(&fx.pool, &refreshed)
        .await
        .unwrap();
    assert_eq!(
        b28_date(&fx.pool, &film, movie).await,
        b28_at(1_000_000_000)
    );
    let saved = rustyfin_db::repo_media::get_item_by_id(&fx.pool, id)
        .await
        .unwrap()
        .expect("the film");
    assert_eq!(saved.overview.as_deref(), Some("Found by the refresh."));
    // A new item still gets the date it's saved with.
    let mut new = refreshed.clone();
    new.id = Uuid::new_v4();
    new.path = root_of(&film)
        .join("New Film (2005)/New Film (2005).mkv")
        .to_str()
        .unwrap()
        .to_string();
    new.date_created = b28_at(1_100_000_000);
    rustyfin_db::repo_media::upsert_media_item(&fx.pool, &new)
        .await
        .unwrap();
    let stored = rustyfin_db::repo_media::get_item_by_id(&fx.pool, new.id)
        .await
        .unwrap()
        .expect("the new film");
    assert_eq!(stored.date_created, b28_at(1_100_000_000));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_new_date_fires_no_trigger() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let movie = "Alpha/Alpha.mkv";
    touch(&root_of(&film), movie);
    scan(&fx.pool, &film).await;
    let id = item_at(&fx.pool, &film, movie).await.to_string();
    // A year and a rating, which the filter tables count.
    exec(
        &fx.pool,
        "UPDATE media_items SET production_year = 1999, official_rating = 'PG' WHERE id = ?1",
        &[id.as_str()],
    )
    .await;
    // Each item the backfill moves is one UPDATE of date_created. Before migration 061 that also
    // fired the filter trigger, which recounted the item's year and rating over the whole table,
    // and the search trigger, which rewrote the item's search entry.
    let mut conn = fx.pool.acquire().await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT total_changes()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE media_items SET date_created = ?1 WHERE id = ?2")
        .bind(b28_at(1_000_000_000).to_rfc3339())
        .bind(&id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let after: i64 = sqlx::query_scalar("SELECT total_changes()")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    // The item's own row, and no row written by a trigger.
    assert_eq!(after - before, 1);
}

/// The rows of the rating filter table, as (rating, item count), by rating.
#[cfg(target_os = "linux")]
async fn b28_filter_ratings(pool: &SqlitePool) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT rating, item_count FROM filter_rating_index ORDER BY rating")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_new_year_or_rating_still_moves_the_filter_counts() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let films = ["Alpha/Alpha.mkv", "Bravo/Bravo.mkv", "Charlie/Charlie.mkv"];
    for rel in films {
        touch(&root_of(&film), rel);
    }
    scan(&fx.pool, &film).await;
    let mut ids = Vec::new();
    for rel in films {
        ids.push(item_at(&fx.pool, &film, rel).await.to_string());
    }
    let year = "UPDATE media_items SET production_year = ?1 WHERE id = ?2";
    let rating = "UPDATE media_items SET official_rating = ?1 WHERE id = ?2";
    for id in &ids {
        exec(&fx.pool, year, &["1999", id.as_str()]).await;
        exec(&fx.pool, rating, &["PG", id.as_str()]).await;
    }
    // No year or rating loses its last item here. When one does, the trigger keeps that value's
    // old count until the filter tables are rebuilt.
    assert_eq!(b18_filter_years(&fx.pool).await, [(1999, 3)]);
    assert_eq!(b28_filter_ratings(&fx.pool).await, [("PG".to_string(), 3)]);
    exec(&fx.pool, year, &["2001", ids[0].as_str()]).await;
    assert_eq!(b18_filter_years(&fx.pool).await, [(1999, 2), (2001, 1)]);
    exec(&fx.pool, year, &["2001", ids[1].as_str()]).await;
    assert_eq!(b18_filter_years(&fx.pool).await, [(1999, 1), (2001, 2)]);
    exec(&fx.pool, rating, &["R", ids[0].as_str()]).await;
    assert_eq!(
        b28_filter_ratings(&fx.pool).await,
        [("PG".to_string(), 2), ("R".to_string(), 1)]
    );
    exec(&fx.pool, rating, &["R", ids[1].as_str()]).await;
    assert_eq!(
        b28_filter_ratings(&fx.pool).await,
        [("PG".to_string(), 1), ("R".to_string(), 2)]
    );
}

/// The ids of the items a search for `query` finds.
#[cfg(target_os = "linux")]
async fn b28_found(pool: &SqlitePool, query: &str) -> Vec<Uuid> {
    rustyfin_db::repo_search::search_hints(pool, query, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|hint| hint.id)
        .collect()
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn b28_a_new_name_still_changes_what_search_finds() {
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let movie = "Alpha/Alpha.mkv";
    touch(&root_of(&film), movie);
    scan(&fx.pool, &film).await;
    let id = item_at(&fx.pool, &film, movie).await;
    assert_eq!(b28_found(&fx.pool, "Alpha").await, [id]);
    let id_text = id.to_string();
    exec(
        &fx.pool,
        "UPDATE media_items SET name = ?1 WHERE id = ?2",
        &["Zulu", id_text.as_str()],
    )
    .await;
    assert_eq!(b28_found(&fx.pool, "Zulu").await, [id]);
    assert_eq!(b28_found(&fx.pool, "Alpha").await, Vec::<Uuid>::new());
}

/// The backfill over `count` generated films, each stored with a date a day after its file's, as
/// by a scan before B23, so that every one moves. Prints the time of the first batch (and of its
/// read and its write transaction alone) and of the whole run, the UPDATE triggers on media_items,
/// and the query plans of the statements each moved item runs, its triggers' included.
#[cfg(target_os = "linux")]
async fn b28_bench(count: u32) {
    use rustyfin_core::date_backfill::{BATCH_SIZE, run_some};
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&film);
    for n in 0..count {
        let folder = root.join(format!("{:03}", n / 1000));
        if n % 1000 == 0 {
            std::fs::create_dir_all(&folder).unwrap();
        }
        std::fs::File::create(folder.join(format!("Film {n}.mkv"))).unwrap();
    }
    // The rows go in without the INSERT triggers, which recount a year and a rating over the whole
    // table for each row. The search index and the filter tables are then built once.
    let inserts: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master WHERE type = 'trigger' \
         AND name IN ('trg_media_items_insert', 'media_items_ai')",
    )
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    assert_eq!(inserts.len(), 2);
    for (name, _) in &inserts {
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TRIGGER {name}")))
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    let scanned = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?1) \
         INSERT INTO media_items (id, name, media_type, path, library_id, production_year, \
         official_rating, genres, date_created) \
         SELECT printf('%08x-0000-4000-8000-000000000000', i), 'Film ' || i, 'Movie', \
         printf('%s/%03d/Film %d.mkv', ?2, i / 1000, i), ?3, 1950 + i % 75, \
         CASE i % 5 WHEN 0 THEN NULL WHEN 1 THEN 'PG' WHEN 2 THEN 'PG-13' WHEN 3 THEN 'R' \
         ELSE 'TV-MA' END, 'Drama|Thriller', ?4 FROM n",
    )
    .bind(i64::from(count))
    .bind(root.to_str().unwrap())
    .bind(film.id.to_string())
    .bind(&scanned)
    .execute(&fx.pool)
    .await
    .unwrap();
    for (_, sql) in &inserts {
        sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
            .execute(&fx.pool)
            .await
            .unwrap();
    }
    exec(
        &fx.pool,
        "INSERT INTO media_items_fts(media_items_fts) VALUES ('rebuild')",
        &[],
    )
    .await;
    rustyfin_db::repo_media::rebuild_filter_indexes(&fx.pool)
        .await
        .unwrap();
    // The setup's writes leave the WAL first, so no commit timed below copies them back.
    exec(&fx.pool, "PRAGMA wal_checkpoint(TRUNCATE)", &[]).await;

    // The first batch's read alone, then its write transaction alone: each row's guarded UPDATE
    // sets the date it already has, which fires what a move fires.
    let batch_sql = "SELECT id, path, date_created FROM media_items WHERE id > ?1 AND media_type IN \
         ('Movie','Series','Season','Episode','Audio','MusicAlbum','Photo') ORDER BY id LIMIT ?2";
    let started = std::time::Instant::now();
    let rows: Vec<(String, String, String)> = sqlx::query_as(batch_sql)
        .bind("")
        .bind(i64::from(BATCH_SIZE))
        .fetch_all(&fx.pool)
        .await
        .unwrap();
    let read = started.elapsed();
    let started = std::time::Instant::now();
    let mut tx = fx.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    for (id, path, stored) in &rows {
        sqlx::query(
            "UPDATE media_items SET date_created = ?1 \
             WHERE id = ?2 AND path = ?3 AND date_created = ?4",
        )
        .bind(stored)
        .bind(id)
        .bind(path)
        .bind(stored)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let write = started.elapsed();

    let started = std::time::Instant::now();
    let first = run_some(&fx.pool, BATCH_SIZE, 1, |_| {})
        .await
        .unwrap()
        .expect("a pending backfill");
    let batch = started.elapsed();
    let rest = b28_run(&fx.pool).await;
    let run = started.elapsed();
    assert_eq!(first.moved, u64::from(BATCH_SIZE));
    assert_eq!(first.moved + rest.moved, u64::from(count));
    println!(
        "b28 bench, {count} items: the first batch of {BATCH_SIZE} in {batch:.2?} (its read alone \
         in {read:.2?}, its write transaction alone in {write:.2?}), the whole run in {run:.2?}"
    );
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master WHERE type = 'trigger' \
         AND tbl_name = 'media_items' AND sql LIKE '%AFTER UPDATE%' ORDER BY name",
    )
    .fetch_all(&fx.pool)
    .await
    .unwrap();
    for (name, sql) in triggers {
        let head = sql.split("BEGIN").next().unwrap_or_default();
        println!(
            "  trigger {name}: {}",
            head.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }
    // The backfill's statements, and those the filter trigger runs for the old and the new year
    // and rating of each item it fires for.
    let statements = [
        ("batch", batch_sql, 2),
        (
            "move",
            "UPDATE media_items SET date_created = ?1 \
             WHERE id = ?2 AND path = ?3 AND date_created = ?4",
            4,
        ),
        (
            "year recount",
            "INSERT OR REPLACE INTO filter_year_index (year, item_count) \
             SELECT ?1, COUNT(*) FROM media_items WHERE production_year = ?1 GROUP BY ?1",
            1,
        ),
        (
            "year delete",
            "DELETE FROM filter_year_index WHERE year = ?1 AND item_count = 0",
            1,
        ),
        (
            "rating recount",
            "INSERT OR REPLACE INTO filter_rating_index (rating, item_count) \
             SELECT ?1, COUNT(*) FROM media_items WHERE official_rating = ?1 GROUP BY ?1",
            1,
        ),
        (
            "rating delete",
            "DELETE FROM filter_rating_index WHERE rating = ?1 AND item_count = 0",
            1,
        ),
    ];
    for (what, sql, params) in statements {
        let mut query = sqlx::query_as::<_, (i64, i64, i64, String)>(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {sql}"
        )));
        for _ in 0..params {
            query = query.bind("PG");
        }
        let plan = query.fetch_all(&fx.pool).await.unwrap();
        let steps: Vec<&str> = plan.iter().map(|step| step.3.as_str()).collect();
        println!("  plan, {what}: {}", steps.join("; "));
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "a bench: run it by name, with --ignored --nocapture"]
async fn b28_bench_10k_items() {
    b28_bench(10_000).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "a bench: run it by name, with --ignored --nocapture"]
async fn b28_bench_50k_items() {
    b28_bench(50_000).await;
}

// B17: scan progress. A library's scan is listed from the moment it is queued until it ends, and
// its updates go 0, then up to at most 99, then 100 once its probes have finished. B18 adds the
// state to each update: Queued, Active from the start, and Idle with the 100.

/// The updates sent so far, in order.
fn b17_sent(rx: &mut tokio::sync::broadcast::Receiver<ScanUpdate>) -> Vec<(Uuid, u8, ScanStatus)> {
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push((update.library_id, update.progress, update.status));
    }
    updates
}

fn b17_state(status: ScanStatus, progress: Option<u8>) -> Option<ScanState> {
    Some(ScanState { status, progress })
}

#[tokio::test]
async fn b17_registry_follows_a_scan_from_queue_to_end() {
    let scans = ScanRegistry::new();
    let mut rx = scans.subscribe();
    let lib = Uuid::new_v4();
    assert_eq!(scans.state(lib), None);

    let ticket = scans.queue(lib);
    assert_eq!(scans.state(lib), b17_state(ScanStatus::Queued, None));
    assert_eq!(
        scans.snapshot(),
        [(
            lib,
            ScanState {
                status: ScanStatus::Queued,
                progress: None
            }
        )]
    );
    ticket.start();
    assert_eq!(scans.state(lib), b17_state(ScanStatus::Active, None));
    ticket.add_total(4);
    assert_eq!(scans.state(lib), b17_state(ScanStatus::Active, Some(0)));
    ticket.advance(1);
    assert_eq!(scans.state(lib), b17_state(ScanStatus::Active, Some(25)));
    ticket.advance(1);
    ticket.advance(2);
    assert_eq!(
        scans.state(lib),
        b17_state(ScanStatus::Active, Some(99)),
        "all work done, but the scan has not ended"
    );
    ticket.add_total(4);
    assert_eq!(
        scans.state(lib),
        b17_state(ScanStatus::Active, Some(99)),
        "4 of 8 is 50%, but the percent never goes down"
    );
    ticket.advance(4);
    drop(ticket);
    assert_eq!(scans.state(lib), None);
    assert!(scans.snapshot().is_empty());
    use ScanStatus::{Active, Idle, Queued};
    assert_eq!(
        b17_sent(&mut rx),
        [
            (lib, 0, Queued),
            (lib, 0, Active),
            (lib, 25, Active),
            (lib, 50, Active),
            (lib, 99, Active),
            (lib, 100, Idle)
        ]
    );
}

#[tokio::test]
async fn b17_tickets_of_one_library_share_its_scan() {
    let scans = ScanRegistry::new();
    let mut rx = scans.subscribe();
    let (lib, other) = (Uuid::new_v4(), Uuid::new_v4());
    let first = scans.queue(lib);
    let second = scans.queue(lib);
    let clone = first.clone();
    let other_ticket = scans.queue(other);

    drop(first);
    assert!(scans.state(lib).is_some(), "a clone keeps the scan");
    drop(clone);
    assert!(
        scans.state(lib).is_some(),
        "the second ticket keeps the scan"
    );
    second.start();
    second.add_total(2);
    second.advance(1);
    drop(second);
    assert_eq!(scans.state(lib), None);
    assert_eq!(scans.state(other), b17_state(ScanStatus::Queued, None));
    drop(other_ticket);
    use ScanStatus::{Active, Idle, Queued};
    assert_eq!(
        b17_sent(&mut rx),
        [
            (lib, 0, Queued),
            (other, 0, Queued),
            (lib, 0, Active),
            (lib, 50, Active),
            (lib, 100, Idle),
            (other, 100, Idle)
        ]
    );
}

#[tokio::test]
async fn b17_untracked_ticket_ends_at_once() {
    let ticket = ScanTicket::untracked();
    ticket.start();
    ticket.add_total(2);
    ticket.advance(1);
    tokio::time::timeout(Duration::from_secs(5), ticket.clone().wait_end())
        .await
        .expect("an untracked ticket has nothing to wait for");
}

#[tokio::test]
async fn b17_wait_end_returns_when_the_last_ticket_drops() {
    let scans = ScanRegistry::new();
    let lib = Uuid::new_v4();
    let ticket = scans.queue(lib);
    let probes = ticket.clone();
    let waiter = tokio::spawn(ticket.wait_end());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!waiter.is_finished(), "the probes still hold the scan");
    assert!(scans.state(lib).is_some());

    drop(probes);
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("wait_end returns once the scan ends")
        .unwrap();
    assert_eq!(scans.state(lib), None);
}

#[tokio::test]
async fn b17_scheduler_scan_reports_progress_until_its_probes_finish() {
    let fx = Fixture::new().await;
    let lib = fx.library("B17 Film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let files: Vec<PathBuf> = (1..=4)
        .map(|i| {
            touch(
                &root,
                &format!("B17 Movie {i} (2000)/B17 Movie {i} (2000).mkv"),
            )
        })
        .collect();
    let sched = TaskScheduler::new(fx.pool.clone(), Arc::new(fx.dir.clone()));
    let mut rx = sched.scans().subscribe();
    assert!(sched.start_task(TASK_LIBRARY_SCAN).await);

    // Other tests' scans hold the scan lock now and then, so this one may wait its turn.
    let mut progress = Vec::new();
    while progress.last() != Some(&100) {
        let update = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("the scan did not end within 60 s: {progress:?}"))
            .unwrap();
        assert_eq!(update.library_id, lib.id, "{progress:?}");
        progress.push(update.progress);
    }
    assert_eq!(progress[0], 0, "a queued scan sends 0 first: {progress:?}");
    assert!(
        progress.windows(2).all(|w| w[0] <= w[1]),
        "the percent only goes up: {progress:?}"
    );
    assert!(
        progress[..progress.len() - 1].iter().all(|&p| p <= 99),
        "{progress:?}"
    );
    assert!(
        progress.iter().any(|&p| 0 < p && p < 100),
        "the probes move the percent: {progress:?}"
    );
    assert_eq!(sched.scans().state(lib.id), None);
    if have_ffmpeg() {
        // The empty files fail to probe, and each failure is stored before the scan ends.
        for file in &files {
            assert!(
                failure_of(&fx.pool, file).await.is_some(),
                "{} was not probed before the scan ended",
                file.display()
            );
        }
    } else {
        println!("ffmpeg not found; skipping the probe check");
    }
}

// B18: a library's scans are runs. A scan queued while a run of the library waits joins that run,
// and one of the run's tickets claims it, so that only one caller scans; a scan queued while the
// run is going makes a new run. The registry and its updates follow the library's oldest run.

#[tokio::test]
async fn b18_one_ticket_claims_a_run() {
    let scans = ScanRegistry::new();
    let mut rx = scans.subscribe();
    let lib = Uuid::new_v4();
    let first = scans.queue(lib);
    let second = scans.queue(lib);
    assert!(first.claim(), "the first claim of a run wins");
    assert!(first.claim(), "the claimer keeps its claim");
    assert!(
        first.clone().claim(),
        "a clone of the claimer holds the run too"
    );
    assert!(!second.claim(), "the run has been claimed");
    assert!(!second.clone().claim(), "a clone of a waiter waits too");
    assert!(
        ScanTicket::untracked().claim(),
        "an untracked ticket always scans"
    );

    assert_eq!(scans.waiting(lib), 0);
    let waiter = tokio::spawn(second.wait_end());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiter.is_finished(), "the claimer still holds the run");
    assert_eq!(scans.waiting(lib), 1, "one wait_end waits for the run");
    drop(first);
    assert_eq!(
        scans.state(lib),
        None,
        "the run ends with its claimer's tickets"
    );
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("a waiter's wait ends with the run")
        .unwrap();
    use ScanStatus::{Idle, Queued};
    assert_eq!(b17_sent(&mut rx), [(lib, 0, Queued), (lib, 100, Idle)]);
}

#[tokio::test]
async fn b18_an_unclaimed_run_ends_when_its_last_ticket_drops() {
    let scans = ScanRegistry::new();
    let lib = Uuid::new_v4();
    let first = scans.queue(lib);
    let second = scans.queue(lib);
    drop(first);
    assert_eq!(
        scans.state(lib),
        b17_state(ScanStatus::Queued, None),
        "the second ticket keeps the run"
    );
    drop(second);
    assert_eq!(scans.state(lib), None);
}

/// B18 fix F3: the scheduler claims its runs with `claim`, and sends no Started or Completed. A
/// caller that sends them claims with `claim_announced`. A share of a run
/// that a silent caller claimed announces it, so that its caller sends them: one share only, and
/// none for a run that an announcing caller claimed, that nobody claimed, or that has ended.
#[tokio::test]
async fn b18_a_silently_claimed_run_is_announced_once() {
    let scans = ScanRegistry::new();
    let lib = Uuid::new_v4();
    let waiting = scans.queue(lib);
    assert!(!waiting.announce(), "nobody has claimed the run");
    let silent = scans.queue(lib);
    assert!(silent.claim());
    let first = scans.queue(lib);
    assert!(
        !first.claim_announced(),
        "the silent caller has claimed the run"
    );
    assert!(first.announce(), "the first share announces the run");
    let second = scans.queue(lib);
    assert!(!second.announce(), "the run has been announced");
    drop((waiting, silent, first));
    assert_eq!(scans.state(lib), None, "the run ends with its holder");
    assert!(!second.announce(), "the run has ended");
    assert!(!second.claim_announced(), "the run has ended");

    let manual = scans.queue(lib);
    assert!(manual.claim_announced());
    let joined = scans.queue(lib);
    assert!(!joined.claim_announced());
    assert!(
        !joined.announce(),
        "the caller that claimed the run announces it"
    );
    assert!(ScanTicket::untracked().claim_announced());
    assert!(
        !ScanTicket::untracked().announce(),
        "an untracked ticket has no run"
    );
}

#[tokio::test]
async fn b18_a_scan_queued_during_a_run_makes_a_new_run() {
    use ScanStatus::{Active, Idle, Queued};
    let scans = ScanRegistry::new();
    let mut rx = scans.subscribe();
    let lib = Uuid::new_v4();
    let running = scans.queue(lib);
    assert!(running.claim());
    running.start();
    running.add_total(2);
    running.advance(1);

    let next = scans.queue(lib);
    let joined = scans.queue(lib);
    assert!(next.claim(), "a run that has started is not joined");
    assert!(!joined.claim(), "a waiting run is joined");
    assert_eq!(
        scans.state(lib),
        b17_state(Active, Some(50)),
        "the oldest run is listed"
    );
    assert_eq!(
        scans.snapshot(),
        [(
            lib,
            ScanState {
                status: Active,
                progress: Some(50)
            }
        )]
    );

    let next_end = tokio::spawn(next.clone().wait_end());
    let running_end = tokio::spawn(running.clone().wait_end());
    drop(running);
    tokio::time::timeout(Duration::from_secs(5), running_end)
        .await
        .expect("the first run has ended")
        .unwrap();
    assert!(!next_end.is_finished(), "the second run has not");
    assert_eq!(
        scans.state(lib),
        b17_state(Queued, None),
        "the second run is listed once the first ends"
    );

    next.start();
    next.add_total(4);
    next.advance(1);
    drop(joined);
    assert_eq!(
        scans.state(lib),
        b17_state(Active, Some(25)),
        "a waiter does not hold a claimed run"
    );
    drop(next);
    tokio::time::timeout(Duration::from_secs(5), next_end)
        .await
        .expect("the second run has ended")
        .unwrap();
    assert_eq!(scans.state(lib), None);
    assert_eq!(
        b17_sent(&mut rx),
        [
            (lib, 0, Queued),
            (lib, 0, Active),
            (lib, 50, Active),
            (lib, 0, Queued),
            (lib, 0, Active),
            (lib, 25, Active),
            (lib, 100, Idle)
        ],
        "the updates follow the first run, then the second"
    );
}

#[tokio::test]
async fn b18_a_stale_ticket_leaves_a_later_run_alone() {
    let scans = ScanRegistry::new();
    let lib = Uuid::new_v4();
    let holder = scans.queue(lib);
    let stale = scans.queue(lib);
    assert!(holder.claim());
    holder.start();
    drop(holder);
    assert_eq!(scans.state(lib), None, "the claimed run has ended");

    let later = scans.queue(lib);
    assert!(!stale.claim(), "the stale ticket's run has ended");
    drop(stale);
    assert_eq!(
        scans.state(lib),
        b17_state(ScanStatus::Queued, None),
        "dropping the stale ticket leaves the later run"
    );
    assert!(later.claim(), "the later run is still unclaimed");
}

#[tokio::test]
async fn b18_scheduler_skips_a_library_another_caller_claimed() {
    let fx = Fixture::new().await;
    let lib = fx.library("B18 Claimed", LibraryContentType::Movies).await;
    touch(&root_of(&lib), "B18 Movie (2001)/B18 Movie (2001).mkv");
    let sched = TaskScheduler::new(fx.pool.clone(), Arc::new(fx.dir.clone()));
    let web = sched.scans().queue(lib.id);
    assert!(web.claim());

    assert!(sched.start_task(TASK_LIBRARY_SCAN).await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let task = sched.get_task(TASK_LIBRARY_SCAN).await.unwrap();
        if task.last_execution_result.is_some() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduled scan did not finish within 60 s"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        count(&fx.pool, "SELECT COUNT(*) FROM media_items").await,
        0,
        "the scheduler left the claimed library to its claimer"
    );
    assert_eq!(
        sched.scans().state(lib.id),
        b17_state(ScanStatus::Queued, None)
    );
    drop(web);
    assert_eq!(sched.scans().state(lib.id), None);
}

// B18: a music scan counts the files of every root before it walks them. With a metadata provider
// configured, each new artist, album and track is one more unit, which its refresh advances, so the
// percent reaches 99 only at the end. A rescan reuses the artist rows of earlier scans.

/// A music library whose roots are `<temp dir>/<root>` for each of `roots`, in that order.
async fn b18_music_library(fx: &Fixture, roots: &[&str]) -> Library {
    let paths = roots
        .iter()
        .map(|root| {
            let root = fx.path(root);
            std::fs::create_dir_all(&root).unwrap();
            root.to_str().unwrap().to_string()
        })
        .collect();
    let lib = Library {
        id: Uuid::new_v4(),
        name: roots[0].to_string(),
        content_type: LibraryContentType::Music,
        paths,
    };
    rustyfin_db::repo_library::create_library(&fx.pool, &lib)
        .await
        .unwrap();
    lib
}

/// Scans `lib` with `omdb_key` as its only provider key, and returns the frames its run sent.
/// OMDb looks up only movies and series, so a key makes the refresh count without a request.
async fn b18_music_frames(
    fx: &Fixture,
    lib: &Library,
    omdb_key: Option<&str>,
) -> Vec<(u8, ScanStatus)> {
    let scans = ScanRegistry::new();
    let mut rx = scans.subscribe();
    let ticket = scans.queue(lib.id);
    let client = reqwest::Client::new();
    scanner::scan_library_with_metadata(&fx.pool, lib, &client, None, omdb_key, None, &ticket)
        .await
        .unwrap();
    drop(ticket);
    b17_sent(&mut rx)
        .into_iter()
        .map(|(id, progress, status)| {
            assert_eq!(id, lib.id);
            (progress, status)
        })
        .collect()
}

/// The library's `MusicAlbum` rows (artists and albums) as (path, parent's path), relative to its
/// first root, sorted.
async fn b18_music_albums(pool: &SqlitePool, lib: &Library) -> Vec<(String, Option<String>)> {
    let root = root_of(lib);
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT a.path, p.path FROM media_items a LEFT JOIN media_items p ON p.id = a.parent_id
         WHERE a.library_id = ?1 AND a.media_type = 'MusicAlbum'",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|(path, parent)| (rel(&root, path), parent.as_deref().map(|p| rel(&root, p))))
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn b18_music_progress_counts_every_root_before_the_walk() {
    let fx = Fixture::new().await;
    let lib = b18_music_library(&fx, &["B18 Music A", "B18 Music B"]).await;
    let (a, b) = (PathBuf::from(&lib.paths[0]), PathBuf::from(&lib.paths[1]));
    touch(&a, "Artist A/Album 1/01 Song 1.mp3");
    touch(&a, "Artist A/Album 1/02 Song 2.mp3");
    touch(&a, "Artist A/Album 1/03 Song 3.mp3");
    touch(&b, "Artist B/Album 2/01 Song 4.mp3");

    let frames = b18_music_frames(&fx, &lib, None).await;
    use ScanStatus::{Active, Idle, Queued};
    // Root A's 3 files and root B's 1 are 4 units, known before the first file is read.
    assert_eq!(
        frames,
        [
            (0, Queued),
            (0, Active),
            (25, Active),
            (50, Active),
            (75, Active),
            (99, Active),
            (100, Idle)
        ]
    );
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM media_items WHERE media_type = 'Audio'"
        )
        .await,
        4
    );
}

#[tokio::test]
async fn b18_music_progress_counts_new_items_when_a_provider_is_set() {
    let fx = Fixture::new().await;
    let lib = b18_music_library(&fx, &["B18 Music C", "B18 Music D"]).await;
    let (c, d) = (PathBuf::from(&lib.paths[0]), PathBuf::from(&lib.paths[1]));
    touch(&c, "Artist C/Album 3/01 Song 1.mp3");
    touch(&c, "Artist C/Album 3/02 Song 2.mp3");
    touch(&d, "Artist D/Album 4/01 Song 3.mp3");

    let frames = b18_music_frames(&fx, &lib, Some("b18-test")).await;
    use ScanStatus::{Active, Idle, Queued};
    assert_eq!(frames[..2], [(0, Queued), (0, Active)], "{frames:?}");
    assert_eq!(frames.last(), Some(&(100, Idle)), "{frames:?}");
    let active: Vec<u8> = frames
        .iter()
        .filter(|(_, status)| *status == Active)
        .map(|(progress, _)| *progress)
        .collect();
    assert!(
        active.windows(2).all(|w| w[0] < w[1]),
        "the percent only goes up: {frames:?}"
    );
    // 3 files and 7 new items (2 artists, 2 albums, 3 tracks) are 10 units. The walk does the 3
    // files, and the refresh of the new items of both roots does the other 7, one tenth each.
    assert!(
        active.ends_with(&[50, 60, 70, 80, 90, 99]),
        "each refreshed item moves the percent by a tenth: {frames:?}"
    );
    assert_eq!(
        b18_music_albums(&fx.pool, &lib).await.len(),
        4,
        "2 artists and 2 albums"
    );
    assert_eq!(
        count(
            &fx.pool,
            "SELECT COUNT(*) FROM media_items WHERE media_type = 'Audio'"
        )
        .await,
        3
    );
}

#[tokio::test]
async fn b18_music_rescan_reuses_the_artist_of_an_earlier_scan() {
    let fx = Fixture::new().await;
    let lib = fx.library("B18 Music E", LibraryContentType::Music).await;
    let root = root_of(&lib);
    touch(&root, "Artist E/Album 5/01 Song 1.mp3");
    scan(&fx.pool, &lib).await;
    touch(&root, "Artist E/Album 6/01 Song 2.mp3");
    scan(&fx.pool, &lib).await;

    let artist = Some("Artist E".to_string());
    assert_eq!(
        b18_music_albums(&fx.pool, &lib).await,
        [
            ("Artist E".to_string(), None),
            ("Artist E/Album 5".to_string(), artist.clone()),
            ("Artist E/Album 6".to_string(), artist),
        ],
        "one artist row for the artist folder, the parent of both albums"
    );
}

// B18: every scan probes a known file again when its size or modification time differs from the
// stamp stored with its source. It removes the items whose files are gone, unless their library
// path is missing, empty or lists only ignored entries, as an unmounted share does, and then the
// containers left without children: seasons and series, albums and artists.

/// A source as (path relative to the library's first root, size, modified).
type Stamp = (String, Option<i64>, Option<i64>);

/// The stamps of the library's sources, sorted.
async fn b18_stamps(pool: &SqlitePool, lib: &Library) -> Vec<Stamp> {
    let root = root_of(lib);
    let rows: Vec<Stamp> = sqlx::query_as(
        "SELECT s.path, s.size, s.modified FROM media_sources AS s
         JOIN media_items AS i ON i.id = s.item_id
         WHERE i.library_id = ?1",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| panic!("the stamps of the sources: {e}"));
    let mut out: Vec<Stamp> = rows
        .into_iter()
        .map(|(path, size, modified)| (rel(&root, &path), size, modified))
        .collect();
    out.sort();
    out
}

/// What `b18_stamps` returns when the sources of `files`, relative to the library's root, hold the
/// size and modification time that the files have now.
fn b18_file_stamps(lib: &Library, files: &[&str]) -> Vec<Stamp> {
    let root = root_of(lib);
    let mut out: Vec<Stamp> = files
        .iter()
        .map(|file| {
            let (size, modified) = stat(&root.join(file));
            (file.to_string(), Some(size), Some(modified))
        })
        .collect();
    out.sort();
    out
}

/// Every source of the library as (path relative to its root, source id), sorted.
async fn b18_source_ids(pool: &SqlitePool, lib: &Library) -> Vec<(String, String)> {
    let root = root_of(lib);
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.path, s.id FROM media_sources AS s JOIN media_items AS i ON i.id = s.item_id
         WHERE i.library_id = ?1",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<(String, String)> = rows
        .into_iter()
        .map(|(path, id)| (rel(&root, &path), id))
        .collect();
    out.sort();
    out
}

/// The paths of the sources in `after` that are not in `before`: the files that a scan probed
/// again, since a probe replaces the source rows of its item.
fn b18_reprobed(before: &[(String, String)], after: &[(String, String)]) -> Vec<String> {
    after
        .iter()
        .filter(|source| !before.contains(source))
        .map(|(path, _)| path.clone())
        .collect()
}

async fn b18_runtime(pool: &SqlitePool, item: Uuid) -> Option<i64> {
    sqlx::query_scalar("SELECT runtime_ticks FROM media_items WHERE id = ?1")
        .bind(item.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Sets the modification time of `path` to `secs` seconds after the Unix epoch.
fn b18_set_modified(path: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(UNIX_EPOCH + Duration::from_secs(secs)))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// Writes a FLAC tone of `secs` seconds to `<root>/<rel>`, with its parent folders.
fn b18_tone(root: &Path, rel: &str, secs: u32) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tone = format!("sine=duration={secs}");
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", &tone, "-c:a", "flac"])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not write {}",
        path.display()
    );
    path
}

/// Waits up to 30 s for the probe of a music track, which runs after its scan has returned: until
/// the source at `path` holds the file's size and modification time and is not one of `old`.
/// Returns the source's id.
async fn b18_track_probed(pool: &SqlitePool, path: &Path, old: &[String]) -> String {
    let (size, modified) = stat(path);
    for _ in 0..300 {
        let row: Option<(String, Option<i64>, Option<i64>)> =
            sqlx::query_as("SELECT id, size, modified FROM media_sources WHERE path = ?1")
                .bind(path.to_str().unwrap())
                .fetch_optional(pool)
                .await
                .unwrap_or_else(|e| panic!("the source of {}: {e}", path.display()));
        if let Some((id, s, m)) = row
            && (s, m) == (Some(size), Some(modified))
            && !old.contains(&id)
        {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "{}: no new source with its stamp after 30 s",
        path.display()
    );
}

/// ffmpeg. B18 criterion 4: every probe stores its file's size and modification time with the
/// source. A rescan probes a movie again when one of its files has another size or modification
/// time, all of its versions in order, on the same item. Files that did not change are not probed.
#[tokio::test]
async fn b18_rescan_rereads_changed_movies() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    write_films(&fx, &root);
    let argo_uhd = "Argo (2012)/Argo (2012) Bluray-2160p.mkv";
    let argo = "Argo (2012)/Argo.mkv";
    let heat = "Heat (1995)/Heat (1995).mkv";
    let room = "The Room.mkv";
    let files = [argo_uhd, argo, heat, room];
    scan_idle(pool, &lib).await;
    assert_eq!(
        b18_stamps(pool, &lib).await,
        b18_file_stamps(&lib, &files),
        "a probe did not store its file's stamp"
    );

    // Heat becomes the 3 s clip, which has another size.
    let heat_id = item_at(pool, &lib, heat).await;
    let before = b18_source_ids(pool, &lib).await;
    clip(&fx, &root, heat, 3);
    scan_idle(pool, &lib).await;
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [heat]);
    assert_eq!(item_at(pool, &lib, heat).await, heat_id);
    assert_eq!(
        b18_runtime(pool, heat_id).await,
        b18_runtime(pool, item_at(pool, &lib, room).await).await,
        "Heat has the runtime of the 3 s clip, as The Room has"
    );

    // Only the modification time of Argo's second version changes. Both versions are probed.
    let before = after;
    b18_set_modified(&root.join(argo), 1_000_000_000);
    scan_idle(pool, &lib).await;
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [argo_uhd, argo]);
    assert_groups(pool, &lib, FILM_GROUPS).await;
    assert_eq!(b18_stamps(pool, &lib).await, b18_file_stamps(&lib, &files));

    scan_idle(pool, &lib).await;
    assert_eq!(
        b18_source_ids(pool, &lib).await,
        after,
        "a rescan probed a file that did not change"
    );
}

/// ffmpeg. B18 criterion 4: an episode replaced by a longer clip is probed again, on the same item.
#[tokio::test]
async fn b18_tv_rescan_rereads_a_changed_episode() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let e1 = "Show/Season 1/Show S01E01.mkv";
    let e2 = "Show/Season 1/Show S01E02.mkv";
    clip(&fx, &root, e1, 2);
    clip(&fx, &root, e2, 3);
    scan_idle(pool, &lib).await;
    assert_eq!(
        b18_stamps(pool, &lib).await,
        b18_file_stamps(&lib, &[e1, e2])
    );
    let item = item_at(pool, &lib, e1).await;
    let before = b18_source_ids(pool, &lib).await;

    clip(&fx, &root, e1, 3);
    scan_idle(pool, &lib).await;

    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [e1]);
    assert_eq!(item_at(pool, &lib, e1).await, item);
    assert_eq!(
        b18_runtime(pool, item).await,
        b18_runtime(pool, item_at(pool, &lib, e2).await).await,
        "E01 has the runtime of the 3 s clip, as E02 has"
    );
    assert_eq!(
        b18_stamps(pool, &lib).await,
        b18_file_stamps(&lib, &[e1, e2])
    );
}

/// ffmpeg. B18 criterion 4: a source without a modification time, as every source is after the
/// upgrade, counts as unchanged, whatever its size. The scan stores the stamp of its file and
/// probes nothing.
#[tokio::test]
async fn b18_unstamped_sources_are_stamped_without_a_probe() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let film = fx.library("film", LibraryContentType::Movies).await;
    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    write_films(&fx, &root_of(&film));
    let episode = "Show/Season 1/Show S01E01.mkv";
    clip(&fx, &root_of(&tv), episode, 2);
    scan_idle(pool, &film).await;
    scan_idle(pool, &tv).await;
    exec(
        pool,
        "UPDATE media_sources SET size = size + 1, modified = NULL",
        &[],
    )
    .await;
    let ids = (
        b18_source_ids(pool, &film).await,
        b18_source_ids(pool, &tv).await,
    );

    scan_idle(pool, &film).await;
    scan_idle(pool, &tv).await;

    assert_eq!(
        (
            b18_source_ids(pool, &film).await,
            b18_source_ids(pool, &tv).await
        ),
        ids,
        "a source without a stamp was probed"
    );
    let films = [
        "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
        "Argo (2012)/Argo.mkv",
        "Heat (1995)/Heat (1995).mkv",
        "The Room.mkv",
    ];
    assert_eq!(
        b18_stamps(pool, &film).await,
        b18_file_stamps(&film, &films)
    );
    assert_eq!(
        b18_stamps(pool, &tv).await,
        b18_file_stamps(&tv, &[episode])
    );
}

/// ffmpeg. B18 criterion 4: a music scan probes a known track again when its file changes, on the
/// same item, and keeps its name. A track whose source has no modification time gets the file's
/// stamp and no probe.
#[tokio::test]
async fn b18_music_rescan_rereads_a_changed_track() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let tone = "Artist M/Album M/01 Tone.flac";
    let track = b18_tone(&root, tone, 1);
    scan(pool, &lib).await;
    let first = b18_track_probed(pool, &track, &[]).await;
    let item = item_at(pool, &lib, tone).await;
    let short = b18_runtime(pool, item)
        .await
        .expect("the probe set a runtime");
    let id = item.to_string();
    exec(
        pool,
        "UPDATE media_items SET name = 'Kept Name' WHERE id = ?1",
        &[&id],
    )
    .await;

    // A longer tone in its place has another size.
    b18_tone(&root, tone, 2);
    scan(pool, &lib).await;
    let second = b18_track_probed(pool, &track, &[first]).await;
    assert_eq!(item_at(pool, &lib, tone).await, item);
    let long = b18_runtime(pool, item)
        .await
        .expect("the probe set a runtime");
    assert!(
        long > short,
        "the runtime is not the longer tone's: {long} <= {short}"
    );
    let name: String = sqlx::query_scalar("SELECT name FROM media_items WHERE id = ?1")
        .bind(&id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(name, "Kept Name", "the re-read named the track again");

    // A source without a modification time counts as unchanged, though the file's time changed.
    exec(pool, "UPDATE media_sources SET modified = NULL", &[]).await;
    b18_set_modified(&track, 1_000_000_000);
    scan(pool, &lib).await;
    assert_eq!(
        b18_track_probed(pool, &track, &[]).await,
        second,
        "a track without a stamp was probed"
    );
    // A probe would replace the source row within this second.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        b18_track_probed(pool, &track, &[]).await,
        second,
        "a track without a stamp was probed"
    );
}

/// B18 criterion 5: the item of a missing file stays while its library path is empty, lists only
/// ignored entries, or is missing, as when a share is not mounted. It goes once the path lists
/// something else.
#[tokio::test]
async fn b18_gone_files_stay_while_their_library_path_looks_unmounted() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let gone = "Gone (1999)/Gone (1999).mkv";
    touch(&root, gone);
    scan(pool, &lib).await;

    std::fs::remove_dir_all(root.join("Gone (1999)")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        [gone],
        "the library path is empty"
    );

    touch(&root, "#recycle/Junk (2001).mkv");
    touch(&root, "@eaDir/Gone (1999).mkv");
    touch(&root, ".DS_Store");
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        [gone],
        "the library path lists only ignored entries"
    );

    std::fs::remove_dir_all(&root).unwrap();
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        [gone],
        "the library path is missing"
    );

    touch(&root, "Here (2000)/Here (2000).mkv");
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        ["Here (2000)/Here (2000).mkv"]
    );
}

/// The ids of the library's items of `media_type`, by path relative to its root, sorted.
async fn b18_ids(pool: &SqlitePool, lib: &Library, media_type: &str) -> Vec<(String, String)> {
    let root = root_of(lib);
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT path, id FROM media_items WHERE library_id = ?1 AND media_type = ?2",
    )
    .bind(lib.id.to_string())
    .bind(media_type)
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out: Vec<(String, String)> = rows
        .into_iter()
        .map(|(path, id)| (rel(&root, &path), id))
        .collect();
    out.sort();
    out
}

/// Played, with a position, and a favorite.
fn b18_watched() -> Marks {
    Marks {
        position: 42,
        plays: 1,
        favorite: true,
        played: true,
        last_played: Some(EARLY.to_string()),
        ..Marks::default()
    }
}

/// B18 fix F1: a link whose target is gone is unreadable, not gone. Its item stays with its user
/// data through full scans and scoped rescans, whether the link is the file, a folder above it, or
/// a file directly in the library path, and the items come back unchanged with their targets. A
/// link that is itself removed is a missing file, so its item goes.
#[cfg(unix)]
#[tokio::test]
async fn b18_rescan_keeps_items_behind_dangling_links() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;
    let store = fx.path("store");
    let make_targets = || {
        touch(&store, "Linked.mkv");
        touch(&store, "Loose.mkv");
        touch(&store, "Folder (2007)/Folder (2007).mkv");
    };
    make_targets();
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    touch(&root, "Here (2000)/Here (2000).mkv");
    std::fs::create_dir_all(root.join("Linked (2006)")).unwrap();
    symlink(
        store.join("Linked.mkv"),
        root.join("Linked (2006)/Linked (2006).mkv"),
    )
    .unwrap();
    symlink(store.join("Folder (2007)"), root.join("Folder (2007)")).unwrap();
    symlink(store.join("Loose.mkv"), root.join("Loose (2003).mkv")).unwrap();
    scan(pool, &lib).await;
    let all = [
        "Folder (2007)/Folder (2007).mkv",
        "Here (2000)/Here (2000).mkv",
        "Linked (2006)/Linked (2006).mkv",
        "Loose (2003).mkv",
    ];
    assert_eq!(paths(pool, &lib, "Movie").await, all);
    let ids = b18_ids(pool, &lib, "Movie").await;
    let user = insert_user(pool).await;
    let linked = [
        "Folder (2007)/Folder (2007).mkv",
        "Linked (2006)/Linked (2006).mkv",
        "Loose (2003).mkv",
    ];
    for rel in linked {
        insert_marks(pool, &user, item_at(pool, &lib, rel).await, &b18_watched()).await;
    }

    // The store drops, as an rclone or debrid mount does.
    std::fs::remove_dir_all(&store).unwrap();
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        all,
        "a full scan keeps the items behind dangling links"
    );
    for scope in [
        ScanScope::Folder(root.join("Folder (2007)")),
        ScanScope::Folder(root.join("Linked (2006)")),
        ScanScope::RootFiles(root.clone()),
    ] {
        assert_eq!(
            b18_rescan(pool, &lib, &scope).await,
            (0, 0, 0),
            "a rescan of {scope:?} changes nothing"
        );
    }
    assert_eq!(b18_ids(pool, &lib, "Movie").await, ids);
    for rel in linked {
        assert_eq!(
            marks_of(pool, &user, item_at(pool, &lib, rel).await).await,
            Some(b18_watched()),
            "{rel} keeps its user data"
        );
    }

    // The store comes back: the same items, nothing new.
    make_targets();
    scan(pool, &lib).await;
    assert_eq!(b18_ids(pool, &lib, "Movie").await, ids);

    // A link that is removed is a missing file.
    std::fs::remove_file(root.join("Linked (2006)/Linked (2006).mkv")).unwrap();
    std::fs::remove_file(root.join("Folder (2007)")).unwrap();
    std::fs::remove_file(root.join("Loose (2003).mkv")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        ["Here (2000)/Here (2000).mkv"]
    );
}

/// B18 fix F1: a library path that lists only dangling links lists nothing, so a missing file under
/// it stays, as under an empty one.
#[cfg(unix)]
#[tokio::test]
async fn b18_gone_files_stay_while_their_library_path_lists_only_dangling_links() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;
    let store = fx.path("store");
    touch(&store, "Away (2001)/Away (2001).mkv");
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    touch(&root, "Gone (1999)/Gone (1999).mkv");
    symlink(store.join("Away (2001)"), root.join("Away (2001)")).unwrap();
    scan(pool, &lib).await;
    let both = ["Away (2001)/Away (2001).mkv", "Gone (1999)/Gone (1999).mkv"];
    assert_eq!(paths(pool, &lib, "Movie").await, both);

    std::fs::remove_dir_all(&store).unwrap();
    std::fs::remove_dir_all(root.join("Gone (1999)")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        both,
        "the library path lists only a dangling link"
    );
}

/// B18 fix F4: a file that the scan lists but cannot stat (its folder lacks search permission)
/// has not changed, so its sources stay and it is not probed again.
#[cfg(unix)]
#[tokio::test]
async fn b18_rescan_keeps_the_sources_of_a_file_it_cannot_stat() {
    use std::os::unix::fs::PermissionsExt;

    /// Sets a folder's permission bits on drop, so the temp dir can be removed.
    struct Mode(PathBuf, u32);
    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
        }
    }

    if !have_ffmpeg() {
        eprintln!("skip: no ffmpeg");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let file = clip(&fx, &root, "Argo (2012)/Argo (2012).mkv", 2);
    scan_idle(pool, &lib).await;
    let sources = b18_source_ids(pool, &lib).await;
    assert_eq!(sources.len(), 1, "{sources:?}");

    // Listing works without search permission; a stat of the file in it fails.
    let folder = root.join("Argo (2012)");
    let _restore = Mode(folder.clone(), 0o755);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o600)).unwrap();
    if std::fs::metadata(&file).is_ok() {
        eprintln!("skip: the file can still be stat'ed (running as root?)");
        return;
    }
    assert!(std::fs::read_dir(&folder).is_ok());
    scan_idle(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Movie").await,
        ["Argo (2012)/Argo (2012).mkv"]
    );
    assert_eq!(
        b18_source_ids(pool, &lib).await,
        sources,
        "a full scan keeps the sources"
    );
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(folder.clone())).await,
        (0, 0, 0),
        "a rescan of the folder re-reads nothing"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, sources);
}

/// B18 criterion 5: a TV rescan removes the episodes of a removed season folder, then the season,
/// and the episodes of a removed series folder, then its season and the series.
#[tokio::test]
async fn b18_tv_rescan_removes_gone_episodes_seasons_and_series() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let a1 = "Show A/Season 1/Show A S01E01.mkv";
    let b1 = "Show B/Season 1/Show B S01E01.mkv";
    touch(&root, a1);
    touch(&root, "Show A/Season 2/Show A S02E01.mkv");
    touch(&root, b1);
    scan(pool, &lib).await;
    assert_eq!(
        paths(pool, &lib, "Season").await,
        ["Show A/Season 1", "Show A/Season 2", "Show B/Season 1"]
    );

    std::fs::remove_dir_all(root.join("Show A/Season 2")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(paths(pool, &lib, "Episode").await, [a1, b1]);
    assert_eq!(
        paths(pool, &lib, "Season").await,
        ["Show A/Season 1", "Show B/Season 1"]
    );
    assert_eq!(paths(pool, &lib, "Series").await, ["Show A", "Show B"]);

    std::fs::remove_dir_all(root.join("Show B")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(paths(pool, &lib, "Episode").await, [a1]);
    assert_eq!(paths(pool, &lib, "Season").await, ["Show A/Season 1"]);
    assert_eq!(paths(pool, &lib, "Series").await, ["Show A"]);
}

/// B18 criterion 5: a music rescan removes the tracks of a removed album folder, then the album,
/// and the tracks of a removed artist folder, then its album and the artist. It also removes an
/// album row that has no tracks.
#[tokio::test]
async fn b18_music_rescan_removes_gone_tracks_albums_and_artists() {
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let song1 = "Artist A/Album 1/01 Song 1.mp3";
    let song3 = "Artist B/Album 3/01 Song 3.mp3";
    touch(&root, song1);
    touch(&root, "Artist A/Album 2/01 Song 2.mp3");
    touch(&root, song3);
    scan(pool, &lib).await;
    let artist_a = item_at(pool, &lib, "Artist A").await;
    insert_item(
        pool,
        &lib,
        "MusicAlbum",
        &root.join("Artist A/Album 9"),
        Some(artist_a),
    )
    .await;

    std::fs::remove_dir_all(root.join("Artist A/Album 2")).unwrap();
    scan(pool, &lib).await;
    let a = Some("Artist A".to_string());
    let b = Some("Artist B".to_string());
    assert_eq!(paths(pool, &lib, "Audio").await, [song1, song3]);
    assert_eq!(
        b18_music_albums(pool, &lib).await,
        [
            ("Artist A".to_string(), None),
            ("Artist A/Album 1".to_string(), a.clone()),
            ("Artist B".to_string(), None),
            ("Artist B/Album 3".to_string(), b),
        ]
    );

    std::fs::remove_dir_all(root.join("Artist B")).unwrap();
    scan(pool, &lib).await;
    assert_eq!(paths(pool, &lib, "Audio").await, [song1]);
    assert_eq!(
        b18_music_albums(pool, &lib).await,
        [
            ("Artist A".to_string(), None),
            ("Artist A/Album 1".to_string(), a),
        ]
    );
}

/// B18 criterion 5: the guard holds for each library path on its own. The tracks of an emptied
/// root stay, and a gone artist folder under a root that lists something else goes.
#[tokio::test]
async fn b18_music_gone_tracks_stay_while_their_root_is_empty() {
    let fx = Fixture::new().await;
    let lib = b18_music_library(&fx, &["B18 Music F", "B18 Music G"]).await;
    let pool = &fx.pool;
    let (f, g) = (PathBuf::from(&lib.paths[0]), PathBuf::from(&lib.paths[1]));
    touch(&f, "Artist F/Album 1/01 Song 1.mp3");
    touch(&f, "Artist F2/Album 2/01 Song 2.mp3");
    touch(&g, "Artist G/Album 3/01 Song 3.mp3");
    scan(pool, &lib).await;

    std::fs::remove_dir_all(f.join("Artist F2")).unwrap();
    std::fs::remove_dir_all(g.join("Artist G")).unwrap();
    scan(pool, &lib).await;

    let rows: Vec<String> =
        sqlx::query_scalar("SELECT path FROM media_items WHERE library_id = ?1")
            .bind(lib.id.to_string())
            .fetch_all(pool)
            .await
            .unwrap();
    let mut got: Vec<String> = rows.iter().map(|path| rel(&fx.dir, path)).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "B18 Music F/Artist F",
            "B18 Music F/Artist F/Album 1",
            "B18 Music F/Artist F/Album 1/01 Song 1.mp3",
            "B18 Music G/Artist G",
            "B18 Music G/Artist G/Album 3",
            "B18 Music G/Artist G/Album 3/01 Song 3.mp3",
        ]
    );
}

/// B18: a music scan keeps the tracks under a folder that it cannot list, as the other scans do.
#[cfg(unix)]
#[tokio::test]
async fn b18_music_rescan_keeps_tracks_in_unreadable_dirs() {
    use std::os::unix::fs::PermissionsExt;

    /// Sets a folder's permission bits on drop, so the temp dir can be removed.
    struct Mode(PathBuf, u32);
    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
        }
    }

    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let root = root_of(&lib);
    let songs = [
        "Artist A/Album 1/01 Song 1.mp3",
        "Artist B/Album 2/01 Song 2.mp3",
    ];
    for song in songs {
        touch(&root, song);
    }
    scan(&fx.pool, &lib).await;

    let locked = root.join("Artist B/Album 2");
    let _restore = Mode(locked.clone(), 0o755);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o100)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        eprintln!("skip: the folder is still readable (running as root?)");
        return;
    }
    scan(&fx.pool, &lib).await;

    assert_eq!(paths(&fx.pool, &lib, "Audio").await, songs);
}

// B18: a rescan of one top-level folder of a library, or of the files directly in one of its
// paths, walks and loads only that part, and leaves the database as a full scan of the files would.
// It counts the items that it added, removed and probed again, refreshes the metadata of its new
// items as a full scan does, and rebuilds the filter tables only when it changed something.

/// Rescans `scope` of `lib` with no metadata provider and waits for the probes it queued.
async fn b18_scan_scope(pool: &SqlitePool, lib: &Library, scope: &ScanScope) -> ScanChanges {
    let changes = scanner::scan_with_metadata(
        pool,
        lib,
        scope,
        &reqwest::Client::new(),
        None,
        None,
        None,
        &ScanTicket::untracked(),
    )
    .await
    .unwrap_or_else(|e| panic!("rescan of {scope:?}: {e}"));
    scanner::wait_for_idle().await;
    changes
}

/// `b18_scan_scope`, with its counts as (added, removed, reread).
async fn b18_rescan(pool: &SqlitePool, lib: &Library, scope: &ScanScope) -> (u32, u32, u32) {
    let changes = b18_scan_scope(pool, lib, scope).await;
    (changes.added, changes.removed, changes.reread)
}

/// Records a failed probe of each of `files`, with the file's size and modification time, unless
/// one is on record. With ffprobe, the probe of these empty files fails and records just that, so
/// later scans leave them alone until they change. Without ffprobe, as on CI's runner, the probe
/// cannot start and records nothing, so every later scan would read them again.
async fn b18_probes_failed(pool: &SqlitePool, files: &[&Path]) {
    for file in files {
        let (size, modified) = stat(file);
        exec(
            pool,
            "INSERT INTO probe_failures (path, size, modified, failed_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO NOTHING",
            &[
                file.to_str().unwrap(),
                &size.to_string(),
                &modified.to_string(),
                LONG_AGO,
            ],
        )
        .await;
    }
}

/// Waits up to 30 s until every track of `lib` has a stamped source: music probes run after their
/// scan has returned, and a probe stores the stamp last.
async fn b18_tracks_probed(pool: &SqlitePool, lib: &Library) {
    for _ in 0..300 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM media_items AS i
             WHERE i.library_id = ?1 AND i.media_type = 'Audio'
               AND NOT EXISTS (SELECT 1 FROM media_sources AS s
                               WHERE s.item_id = i.id AND s.modified IS NOT NULL)",
        )
        .bind(lib.id.to_string())
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{}: tracks without a probed source after 30 s", lib.name);
}

/// Asserts that `fx.pool` holds what a full scan of `lib`'s files, as they are now, gives in a new
/// database, `<temp dir>/<file>` (AC 2).
async fn b18_assert_like_full_scan(fx: &Fixture, lib: &Library, file: &str) {
    let fresh = fx.second_db(file, lib).await;
    scan_idle(&fresh, lib).await;
    b18_tracks_probed(&fx.pool, lib).await;
    b18_tracks_probed(&fresh, lib).await;
    assert_eq!(
        snapshot(&fx.pool, lib).await,
        snapshot(&fresh, lib).await,
        "{file}: the rescanned library (left) differs from a full scan of its files (right)"
    );
}

#[tokio::test]
async fn b18_scoped_movie_rescans_match_a_full_scan() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    write_films(&fx, &root);
    scan_idle(&fx.pool, &lib).await;
    let folder = |name: &str| ScanScope::Folder(root.join(name));
    let root_files = ScanScope::RootFiles(root.clone());

    clip(&fx, &root, "Ronin (1998)/Ronin (1998).mkv", 2);
    let changes = b18_rescan(&fx.pool, &lib, &folder("Ronin (1998)")).await;
    assert_eq!(changes, (1, 0, 0), "a new folder");
    b18_assert_like_full_scan(&fx, &lib, "new-folder.db").await;

    // The new version is the longer clip, so a runtime taken from the wrong file shows.
    clip(&fx, &root, "Heat (1995)/Heat (1995) Bluray-2160p.mkv", 3);
    let changes = b18_rescan(&fx.pool, &lib, &folder("Heat (1995)")).await;
    assert_eq!(changes, (0, 0, 1), "a new version in a known folder");
    b18_assert_like_full_scan(&fx, &lib, "new-version.db").await;

    // The same name, a longer clip, another size.
    clip(&fx, &root, "Ronin (1998)/Ronin (1998).mkv", 3);
    let changes = b18_rescan(&fx.pool, &lib, &folder("Ronin (1998)")).await;
    assert_eq!(changes, (0, 0, 1), "a file replaced under the same name");
    b18_assert_like_full_scan(&fx, &lib, "replaced.db").await;

    // Argo's primary goes, so its other version becomes the item's path.
    std::fs::remove_file(root.join("Argo (2012)/Argo (2012) Bluray-2160p.mkv")).unwrap();
    let changes = b18_rescan(&fx.pool, &lib, &folder("Argo (2012)")).await;
    assert_eq!(changes, (0, 0, 1), "a removed version");
    b18_assert_like_full_scan(&fx, &lib, "removed-version.db").await;

    std::fs::remove_dir_all(root.join("Heat (1995)")).unwrap();
    let changes = b18_rescan(&fx.pool, &lib, &folder("Heat (1995)")).await;
    assert_eq!(changes, (0, 1, 0), "a removed folder");
    b18_assert_like_full_scan(&fx, &lib, "removed-folder.db").await;

    clip(&fx, &root, "Alien (1979).mkv", 2);
    let changes = b18_rescan(&fx.pool, &lib, &root_files).await;
    assert_eq!(changes, (1, 0, 0), "a new file at the root");
    b18_assert_like_full_scan(&fx, &lib, "new-root-file.db").await;

    std::fs::remove_file(root.join("The Room.mkv")).unwrap();
    let changes = b18_rescan(&fx.pool, &lib, &root_files).await;
    assert_eq!(changes, (0, 1, 0), "a removed file at the root");
    b18_assert_like_full_scan(&fx, &lib, "removed-root-file.db").await;
}

#[tokio::test]
async fn b18_scoped_tv_rescans_match_a_full_scan() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("shows", LibraryContentType::TvShows).await;
    let root = root_of(&lib);
    clip(&fx, &root, "Show A/Season 1/Show A S01E01.mkv", 2);
    clip(&fx, &root, "Show B/Season 1/Show B S01E01.mkv", 2);
    scan_idle(&fx.pool, &lib).await;
    let show_a = ScanScope::Folder(root.join("Show A"));
    let root_files = ScanScope::RootFiles(root.clone());

    clip(&fx, &root, "Show A/Season 1/Show A S01E02.mkv", 3);
    let changes = b18_rescan(&fx.pool, &lib, &show_a).await;
    assert_eq!(changes, (1, 0, 0), "a new episode");
    b18_assert_like_full_scan(&fx, &lib, "new-episode.db").await;

    clip(&fx, &root, "Show A/Season 2/Show A S02E01.mkv", 2);
    let changes = b18_rescan(&fx.pool, &lib, &show_a).await;
    assert_eq!(changes, (2, 0, 0), "a new season and its episode");
    b18_assert_like_full_scan(&fx, &lib, "new-season.db").await;

    // The episode, its season and its series.
    std::fs::remove_dir_all(root.join("Show B")).unwrap();
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(root.join("Show B"))).await;
    assert_eq!(changes, (0, 3, 0), "a removed series");
    b18_assert_like_full_scan(&fx, &lib, "removed-series.db").await;

    // An episode directly in the library path goes to the series `<path>/Unknown Series` and its
    // `Season 1`. The second one reuses both.
    clip(&fx, &root, "Pilot S01E01.mkv", 2);
    let changes = b18_rescan(&fx.pool, &lib, &root_files).await;
    assert_eq!(changes, (3, 0, 0), "a first episode at the root");
    b18_assert_like_full_scan(&fx, &lib, "first-root-file.db").await;

    clip(&fx, &root, "Pilot S01E02.mkv", 2);
    let changes = b18_rescan(&fx.pool, &lib, &root_files).await;
    assert_eq!(changes, (1, 0, 0), "a second episode at the root");
    b18_assert_like_full_scan(&fx, &lib, "second-root-file.db").await;
}

#[tokio::test]
async fn b18_scoped_music_rescans_match_a_full_scan() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let root = root_of(&lib);
    b18_tone(&root, "Artist A/Album 1/01 One.flac", 1);
    b18_tone(&root, "Artist A/Album 1/02 Two.flac", 1);
    b18_tone(&root, "Loose 1.flac", 1);
    scan(&fx.pool, &lib).await;
    b18_tracks_probed(&fx.pool, &lib).await;

    b18_tone(&root, "Artist A/Album 2/01 Three.flac", 2);
    b18_tone(&root, "Artist A/Album 2/02 Four.flac", 1);
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(root.join("Artist A"))).await;
    assert_eq!(changes, (3, 0, 0), "a new album of a known artist");
    b18_assert_like_full_scan(&fx, &lib, "new-album.db").await;

    // A track directly in the library path belongs to the album whose path is the library path.
    b18_tone(&root, "Loose 2.flac", 2);
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::RootFiles(root.clone())).await;
    assert_eq!(changes, (1, 0, 0), "a new track at the root");
    b18_assert_like_full_scan(&fx, &lib, "new-root-track.db").await;
}

#[tokio::test]
async fn b18_scoped_rescans_stay_in_their_scope() {
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let heat = touch(&root, "Heat (1995)/Heat (1995).mkv");
    // Its folder's name starts with that of Heat's folder, so a prefix match takes it for Heat's.
    let extended = touch(&root, "Heat (1995) Extended/Heat (1995) Extended.mkv");
    let ronin = touch(&root, "Ronin (1998)/Ronin (1998).mkv");
    let loose = touch(&root, "Loose (2001).mkv");
    scan_idle(&fx.pool, &lib).await;
    b18_probes_failed(&fx.pool, &[&heat, &extended, &ronin, &loose]).await;
    assert_eq!(paths(&fx.pool, &lib, "Movie").await.len(), 4);

    std::fs::remove_file(&heat).unwrap();
    std::fs::remove_file(&extended).unwrap();
    std::fs::remove_file(&loose).unwrap();
    let new = touch(&root, "New (2010)/New (2010).mkv");

    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(root.join("Heat (1995)"))).await;
    assert_eq!(changes, (0, 1, 0), "Heat's folder");
    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        [
            "Heat (1995) Extended/Heat (1995) Extended.mkv",
            "Loose (2001).mkv",
            "Ronin (1998)/Ronin (1998).mkv",
        ],
        "a folder rescan removes only the missing files of its folder, and adds no file of another"
    );

    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::RootFiles(root.clone())).await;
    assert_eq!(changes, (0, 1, 0), "the files at the root");
    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        [
            "Heat (1995) Extended/Heat (1995) Extended.mkv",
            "Ronin (1998)/Ronin (1998).mkv",
        ],
        "a root-files rescan removes only the missing files directly in the root"
    );

    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(root.join("New (2010)"))).await;
    assert_eq!(changes, (1, 0, 0), "the new folder");
    b18_probes_failed(&fx.pool, &[&new]).await;

    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Library).await;
    assert_eq!(changes, (0, 1, 0), "the whole library");
    assert_eq!(
        paths(&fx.pool, &lib, "Movie").await,
        ["New (2010)/New (2010).mkv", "Ronin (1998)/Ronin (1998).mkv"]
    );
}

#[tokio::test]
async fn b18_scoped_rescans_outside_the_library_paths_change_nothing() {
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Heat (1995)/Heat (1995).mkv");
    touch(&root, "Ronin (1998)/Ronin (1998).mkv");
    scan_idle(&fx.pool, &lib).await;
    let before = paths(&fx.pool, &lib, "Movie").await;
    assert_eq!(before.len(), 2);

    // The folder that holds the library path walks no library path, so it must not remove the items
    // under it as files it did not collect.
    let above = root.parent().unwrap().to_path_buf();
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(above)).await;
    assert_eq!(changes, (0, 0, 0), "a folder above the library path");
    assert_eq!(paths(&fx.pool, &lib, "Movie").await, before);

    // Nor do the files directly in a folder that is not a library path.
    let inside = root.join("Heat (1995)");
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::RootFiles(inside)).await;
    assert_eq!(
        changes,
        (0, 0, 0),
        "the files of a folder inside the library path"
    );
    assert_eq!(paths(&fx.pool, &lib, "Movie").await, before);
}

#[tokio::test]
async fn b18_scoped_rescans_reread_only_their_scope() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    clip(&fx, &root, "Argo (2012)/Argo (2012).mkv", 2);
    clip(&fx, &root, "Heat (1995)/Heat (1995).mkv", 2);
    scan_idle(&fx.pool, &lib).await;
    let before = b18_source_ids(&fx.pool, &lib).await;

    // Both files are replaced by longer clips.
    clip(&fx, &root, "Argo (2012)/Argo (2012).mkv", 3);
    clip(&fx, &root, "Heat (1995)/Heat (1995).mkv", 3);
    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Folder(root.join("Argo (2012)"))).await;
    assert_eq!(changes, (0, 0, 1), "Argo's folder");
    let after = b18_source_ids(&fx.pool, &lib).await;
    assert_eq!(
        b18_reprobed(&before, &after),
        ["Argo (2012)/Argo (2012).mkv"]
    );

    let changes = b18_rescan(&fx.pool, &lib, &ScanScope::Library).await;
    assert_eq!(
        changes,
        (0, 0, 1),
        "the whole library, where only Heat is left to read"
    );
    let last = b18_source_ids(&fx.pool, &lib).await;
    assert_eq!(b18_reprobed(&after, &last), ["Heat (1995)/Heat (1995).mkv"]);
}

/// A stub TMDb on 127.0.0.1:0 that finds nothing, and the `query` of each movie search it got.
async fn b18_tmdb_stub() -> (TmdbApi, Arc<std::sync::Mutex<Vec<String>>>) {
    let searches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = searches.clone();
    let app = axum::Router::new().fallback(
        move |uri: axum::http::Uri,
              axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>| {
            let seen = seen.clone();
            async move {
                if uri.path() == "/3/search/movie" {
                    let name = query.get("query").cloned().unwrap_or_default();
                    seen.lock().unwrap().push(name);
                }
                axum::Json(serde_json::json!({ "results": [] }))
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let settings = TmdbSettings {
        api_base_url: format!("{origin}/3"),
        image_base_url: format!("{origin}/t/p/original"),
        env_key: TmdbKey::new("b18-fake-key"),
        min_interval: Duration::from_millis(1),
        initial_backoff: Duration::from_millis(5),
        ..TmdbSettings::default()
    };
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let api = TmdbClient::new(http, settings)
        .api(None)
        .expect("the env key is set");
    (api, searches)
}

#[tokio::test]
async fn b18_scoped_rescans_refresh_the_metadata_of_new_items() {
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Ronin (1998)/Ronin (1998).mkv");
    scan_idle(&fx.pool, &lib).await;
    touch(&root, "Heat (1995)/Heat (1995).mkv");
    let (tmdb, searches) = b18_tmdb_stub().await;

    for unit in ["Ronin (1998)", "Heat (1995)"] {
        scanner::scan_with_metadata(
            &fx.pool,
            &lib,
            &ScanScope::Folder(root.join(unit)),
            &reqwest::Client::new(),
            Some(&tmdb),
            None,
            None,
            &ScanTicket::untracked(),
        )
        .await
        .unwrap_or_else(|e| panic!("rescan of {unit}: {e}"));
        scanner::wait_for_idle().await;
    }

    let searches = searches.lock().unwrap().clone();
    assert!(
        searches.iter().any(|query| query.contains("Heat")),
        "no TMDb search for the new movie: {searches:?}"
    );
    assert!(
        !searches.iter().any(|query| query.contains("Ronin")),
        "a TMDb search for a known movie: {searches:?}"
    );
}

/// The rows of the year filter table, as (year, item count), by year.
async fn b18_filter_years(pool: &SqlitePool) -> Vec<(i64, i64)> {
    sqlx::query_as("SELECT year, item_count FROM filter_year_index ORDER BY year")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn b18_scoped_rescans_rebuild_the_filter_tables_only_after_a_change() {
    let fx = Fixture::new().await;
    let lib = fx.library("films", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    let ronin = touch(&root, "Ronin (1998)/Ronin (1998).mkv");
    scan_idle(&fx.pool, &lib).await;
    b18_probes_failed(&fx.pool, &[&ronin]).await;
    // No item has this year, so a rebuild removes the row.
    exec(
        &fx.pool,
        "INSERT INTO filter_year_index (year, item_count) VALUES (1, 99)",
        &[],
    )
    .await;

    let ronin = ScanScope::Folder(root.join("Ronin (1998)"));
    let changes = b18_scan_scope(&fx.pool, &lib, &ronin).await;
    assert!(!changes.changed(), "{changes:?}");
    assert_eq!(
        b18_filter_years(&fx.pool).await,
        [(1, 99), (1998, 1)],
        "a rescan that changed nothing rebuilt the filter tables"
    );

    touch(&root, "Heat (1995)/Heat (1995).mkv");
    let changes =
        b18_scan_scope(&fx.pool, &lib, &ScanScope::Folder(root.join("Heat (1995)"))).await;
    assert!(changes.changed(), "{changes:?}");
    assert_eq!(
        b18_filter_years(&fx.pool).await,
        [(1995, 1), (1998, 1)],
        "a rescan that added a movie did not rebuild the filter tables"
    );
}

/// B18 fix F2: a movie whose file gives way to a file with another name in the same folder, as a
/// Radarr upgrade does, keeps its item, with its id and its users' data, through a full scan and
/// through rescans of its folder and of the files in the library path. The item takes the new path
/// and is probed again.
#[tokio::test]
async fn b18_rescans_move_a_renamed_movie_to_its_new_file() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let old = [
        "Argo (2012)/Argo (2012) WEBDL-1080p.mkv",
        "Heat (1995)/Heat (1995) WEBDL-720p.mkv",
        "Loose (2003) DVD.mkv",
    ];
    let new = [
        "Argo (2012)/Argo (2012) Bluray-2160p.mkv",
        "Heat (1995)/Heat (1995) Remux-2160p.mkv",
        "Loose (2003) Bluray-1080p.mkv",
    ];
    for rel in old {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    let user = insert_user(pool).await;
    let mut ids = Vec::new();
    for rel in old {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, &b18_watched()).await;
        ids.push(item);
    }
    let upgrade = |at: usize| {
        std::fs::remove_file(root.join(old[at])).unwrap();
        touch(&root, new[at]);
    };

    upgrade(0);
    scan(pool, &lib).await;
    upgrade(1);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Heat (1995)"))).await,
        (0, 0, 1),
        "the folder rescan re-reads Heat's item, and adds and removes nothing"
    );
    upgrade(2);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::RootFiles(root.clone())).await,
        (0, 0, 1),
        "the rescan of the root files re-reads Loose's item, and adds and removes nothing"
    );

    let check = async || {
        assert_eq!(paths(pool, &lib, "Movie").await, new);
        for (rel, &item) in new.iter().zip(&ids) {
            assert_eq!(
                item_at(pool, &lib, rel).await,
                item,
                "{rel} keeps the item of the file it replaced"
            );
            assert_eq!(
                marks_of(pool, &user, item).await,
                Some(b18_watched()),
                "{rel} keeps its user data"
            );
        }
    };
    check().await;
    scan(pool, &lib).await;
    check().await;
}

/// B18 fix F2: a new file takes over only the item of a missing file of the same movie in the same
/// folder, with the same title and year. A version added next to a movie's file joins its item.
/// The missing file of another movie, or of the same title in another year, goes with its item.
#[tokio::test]
async fn b18_rescans_give_a_new_file_no_item_of_another_movie() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    for rel in [
        "Alpha (2001).mkv",
        "Argo (2012)/Argo (2012) - 1080p.mkv",
        "Collection/Alien (1979).mkv",
        "Collection/Aliens (1986).mkv",
        "Dune (1984).mkv",
    ] {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    let before = b18_ids(pool, &lib, "Movie").await;

    touch(&root, "Argo (2012)/Argo (2012) - 2160p.mkv");
    for (gone, added) in [
        ("Alpha (2001).mkv", "Beta (2002).mkv"),
        (
            "Collection/Alien (1979).mkv",
            "Collection/Alien 3 (1992).mkv",
        ),
        ("Dune (1984).mkv", "Dune (2021).mkv"),
    ] {
        std::fs::remove_file(root.join(gone)).unwrap();
        touch(&root, added);
    }
    scan(pool, &lib).await;

    let after = b18_ids(pool, &lib, "Movie").await;
    let id_of = |ids: &[(String, String)], rel: &str| {
        ids.iter()
            .find(|(path, _)| path == rel)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("no item at {rel}"))
    };
    assert_eq!(
        after
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "Argo (2012)/Argo (2012) - 2160p.mkv",
            "Beta (2002).mkv",
            "Collection/Alien 3 (1992).mkv",
            "Collection/Aliens (1986).mkv",
            "Dune (2021).mkv",
        ]
    );
    assert_eq!(
        id_of(&after, "Argo (2012)/Argo (2012) - 2160p.mkv"),
        id_of(&before, "Argo (2012)/Argo (2012) - 1080p.mkv"),
        "the new version joins Argo's item"
    );
    assert_eq!(
        id_of(&after, "Collection/Aliens (1986).mkv"),
        id_of(&before, "Collection/Aliens (1986).mkv")
    );
    let old_ids: Vec<String> = before.into_iter().map(|(_, id)| id).collect();
    for rel in [
        "Beta (2002).mkv",
        "Collection/Alien 3 (1992).mkv",
        "Dune (2021).mkv",
    ] {
        assert!(
            !old_ids.contains(&id_of(&after, rel)),
            "{rel} took over the item of another movie"
        );
    }
}

/// B18 fix F2: an item behind a dangling link is not missing, so a new file next to the link does
/// not take it over: the link keeps its item, and the new file gets one of its own.
#[cfg(unix)]
#[tokio::test]
async fn b18_rescans_give_a_new_file_no_item_behind_a_dangling_link() {
    use std::os::unix::fs::symlink;
    let fx = Fixture::new().await;
    let store = fx.path("store");
    touch(&store, "Linked.mkv");
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let link = "Linked (2006)/Linked (2006).mkv";
    std::fs::create_dir_all(root.join("Linked (2006)")).unwrap();
    symlink(store.join("Linked.mkv"), root.join(link)).unwrap();
    scan(pool, &lib).await;
    let linked = item_at(pool, &lib, link).await;

    std::fs::remove_dir_all(&store).unwrap();
    let added = "Linked (2006)/Linked (2006) Bluray-1080p.mkv";
    touch(&root, added);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Linked (2006)"))).await,
        (1, 0, 0)
    );
    assert_eq!(item_at(pool, &lib, link).await, linked);
    assert_ne!(item_at(pool, &lib, added).await, linked);
}

/// B18 fix F2: an episode whose file gives way to a file with another name in its show's folder, as
/// a Sonarr upgrade or rename does, keeps its item, with its id and its users' data, through a full
/// scan and a rescan of the show's folder. The file of another episode gets an item of its own.
#[tokio::test]
async fn b18_tv_rescans_move_a_renamed_episode_to_its_new_file() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let e1 = "Show/Season 1/Show - S01E01 - Pilot WEBDL-720p.mkv";
    let e2 = "Show/Season 1/Show - S01E02 - Second WEBDL-720p.mkv";
    let e3 = "Show/Season 1/Show - S01E03 - Third WEBDL-720p.mkv";
    for rel in [e1, e2, e3] {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    b18_probes_failed(pool, &[&root.join(e1), &root.join(e2), &root.join(e3)]).await;
    let user = insert_user(pool).await;
    let mut ids = Vec::new();
    for rel in [e1, e2, e3] {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, &b18_watched()).await;
        ids.push(item);
    }

    // A full scan after an upgrade of E01.
    let n1 = "Show/Season 1/Show - S01E01 - Pilot Bluray-1080p.mkv";
    std::fs::remove_file(root.join(e1)).unwrap();
    touch(&root, n1);
    scan(pool, &lib).await;
    assert_eq!(item_at(pool, &lib, n1).await, ids[0]);
    b18_probes_failed(pool, &[&root.join(n1)]).await;

    // A rescan of the show after a rename of E02, while E03 gives way to E04.
    let n2 = "Show/Season 1/Show - S01E02 - The Second WEBDL-720p.mkv";
    let e4 = "Show/Season 1/Show - S01E04 - Fourth WEBDL-720p.mkv";
    std::fs::rename(root.join(e2), root.join(n2)).unwrap();
    std::fs::remove_file(root.join(e3)).unwrap();
    touch(&root, e4);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Show"))).await,
        (1, 1, 1),
        "the show's rescan adds E04, removes E03 and re-reads E02"
    );
    assert_eq!(item_at(pool, &lib, n2).await, ids[1]);
    assert!(
        !ids.contains(&item_at(pool, &lib, e4).await),
        "E04 took over the item of another episode"
    );
    assert_eq!(paths(pool, &lib, "Episode").await, [n1, n2, e4]);
    for item in &ids[..2] {
        assert_eq!(
            marks_of(pool, &user, *item).await,
            Some(b18_watched()),
            "a renamed episode keeps its user data"
        );
    }
}

/// Writes a FLAC tone of 1 s to `<root>/<rel>`, with its parent folders, tagged with the track
/// number `track` and the title `title`.
fn b18_tagged_tone(root: &Path, rel: &str, track: u32, title: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
        .args(["-metadata", &format!("track={track}")])
        .args(["-metadata", &format!("title={title}")])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not write {}",
        path.display()
    );
    path
}

/// ffmpeg. B18 fix F2: a track whose file gives way to a file with another name in its album's
/// folder, with the same track number, as a Lidarr upgrade or rename does, keeps its item, with its
/// id and its users' data, through a full scan and a rescan of its artist's folder, and its new file
/// is probed. A file with another track number gets an item of its own.
#[tokio::test]
async fn b18_music_rescans_move_a_renamed_track_to_its_new_file() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let a1 = "Artist A/Album A/01 - Song.flac";
    let a2 = "Artist A/Album A/02 - Other.flac";
    let b1 = "Artist B/Album B/01 - Tune.flac";
    b18_tagged_tone(&root, a1, 1, "Song");
    b18_tagged_tone(&root, a2, 2, "Other");
    b18_tagged_tone(&root, b1, 1, "Tune");
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    let user = insert_user(pool).await;
    let mut ids = Vec::new();
    for rel in [a1, a2, b1] {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, &b18_watched()).await;
        ids.push(item);
    }

    // A full scan after a rename of Artist A's first track.
    let n1 = "Artist A/Album A/01 - Song (Remastered).flac";
    std::fs::rename(root.join(a1), root.join(n1)).unwrap();
    scan(pool, &lib).await;
    b18_track_probed(pool, &root.join(n1), &[]).await;
    assert_eq!(item_at(pool, &lib, n1).await, ids[0]);

    // A rescan of Artist B after a rename of its track.
    let nb1 = "Artist B/Album B/01 - Tune [Remaster].flac";
    std::fs::rename(root.join(b1), root.join(nb1)).unwrap();
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Artist B"))).await,
        (0, 0, 1),
        "the rescan of Artist B re-reads its track, and adds and removes nothing"
    );
    b18_track_probed(pool, &root.join(nb1), &[]).await;
    assert_eq!(item_at(pool, &lib, nb1).await, ids[2]);

    // In Album A, track 2 gives way to track 3.
    let a3 = "Artist A/Album A/03 - Third.flac";
    std::fs::remove_file(root.join(a2)).unwrap();
    b18_tagged_tone(&root, a3, 3, "Third");
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Artist A"))).await,
        (1, 1, 0),
        "the rescan of Artist A adds track 3 and removes track 2"
    );
    assert!(
        !ids.contains(&item_at(pool, &lib, a3).await),
        "track 3 took over the item of another track"
    );

    assert_eq!(paths(pool, &lib, "Audio").await, [n1, a3, nb1]);
    for item in [ids[0], ids[2]] {
        assert_eq!(
            marks_of(pool, &user, item).await,
            Some(b18_watched()),
            "a renamed track keeps its user data"
        );
    }
}

/// ffmpeg. B18 fix F2: the tracks of one album folder can share a track number, as an album's discs
/// do. A new file with that number takes over the gone track with its title, and none of them when
/// none has its title. A track taken over keeps its name and year, as one whose file is replaced in
/// place does.
#[tokio::test]
async fn b18_music_rescans_give_a_new_file_no_track_of_another_disc() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let intro = "Artist/Album/1-01 - Intro.flac";
    let song = "Artist/Album/1-02 - Song.flac";
    let outro = "Artist/Album/2-01 - Outro.flac";
    b18_tagged_tone(&root, intro, 1, "Intro");
    b18_tagged_tone(&root, song, 2, "Song");
    b18_tagged_tone(&root, outro, 1, "Outro");
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    let mut ids = Vec::new();
    for rel in [intro, song, outro] {
        ids.push(item_at(pool, &lib, rel).await);
    }
    // An edited name with the song's title: a track whose name has another title takes over nothing.
    exec(
        pool,
        "UPDATE media_items SET name = 'SONG!', production_year = 1999 WHERE id = ?1",
        &[&ids[1].to_string()],
    )
    .await;

    // Every track gives way to a remastered file.
    let renamed = [
        "Artist/Album/1-01 - Intro (Remastered).flac",
        "Artist/Album/1-02 - Song (Remastered).flac",
        "Artist/Album/2-01 - Outro (Remastered).flac",
    ];
    for (old, new) in [intro, song, outro].iter().zip(renamed) {
        std::fs::rename(root.join(old), root.join(new)).unwrap();
    }
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Artist"))).await,
        (0, 0, 3),
        "the rescan re-reads the three tracks, and adds and removes nothing"
    );
    for (rel, id) in renamed.iter().zip(&ids) {
        assert_eq!(
            item_at(pool, &lib, rel).await,
            *id,
            "{rel} took another item"
        );
    }
    let (name, year): (String, Option<i32>) =
        sqlx::query_as("SELECT name, production_year FROM media_items WHERE id = ?1")
            .bind(ids[1].to_string())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        (name.as_str(), year),
        ("SONG!", Some(1999)),
        "a track taken over keeps its name and year"
    );

    // Both tracks 1 go, and a track 1 with another title comes.
    for rel in [renamed[0], renamed[2]] {
        std::fs::remove_file(root.join(rel)).unwrap();
    }
    let prelude = "Artist/Album/1-01 - Prelude.flac";
    b18_tagged_tone(&root, prelude, 1, "Prelude");
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Artist"))).await,
        (1, 2, 0),
        "the rescan adds the new track 1 and removes both old ones"
    );
    assert!(
        !ids.contains(&item_at(pool, &lib, prelude).await),
        "the new track 1 took over the item of another track"
    );
}

/// ffmpeg. B18 fix round 2, item 3: another edition of an album, put in the album's folder in place
/// of the old one, keeps a track's item only where the track number and the title both match. The
/// other new tracks get items of their own.
#[tokio::test]
async fn b18_music_rescans_give_a_new_file_no_track_with_another_title() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let old = [
        ("Artist/Album/01 - Song.flac", 1, "Song"),
        ("Artist/Album/02 - Other.flac", 2, "Other"),
        ("Artist/Album/03 - Third.flac", 3, "Third"),
    ];
    for (rel, track, title) in old {
        b18_tagged_tone(&root, rel, track, title);
    }
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    let user = insert_user(pool).await;
    let mut ids = Vec::new();
    for (rel, ..) in old {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, &b18_watched()).await;
        ids.push(item);
    }

    // The deluxe edition: track 1 is the same song, track 2 a bonus, and track 3 the old track 2.
    for (rel, ..) in old {
        std::fs::remove_file(root.join(rel)).unwrap();
    }
    let new = [
        ("Artist/Album/01 - Song (2011 Remaster).flac", 1, "Song"),
        ("Artist/Album/02 - Bonus.flac", 2, "Bonus"),
        ("Artist/Album/03 - Other.flac", 3, "Other"),
    ];
    for (rel, track, title) in new {
        b18_tagged_tone(&root, rel, track, title);
    }
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Artist"))).await,
        (2, 2, 1),
        "the rescan re-reads track 1, adds tracks 2 and 3, and removes the old tracks 2 and 3"
    );
    b18_track_probed(pool, &root.join(new[0].0), &[]).await;
    assert_eq!(item_at(pool, &lib, new[0].0).await, ids[0]);
    assert_eq!(marks_of(pool, &user, ids[0]).await, Some(b18_watched()));
    for (rel, ..) in &new[1..] {
        let item = item_at(pool, &lib, rel).await;
        assert!(
            !ids.contains(&item),
            "{rel} took over the item of another song"
        );
        assert_eq!(marks_of(pool, &user, item).await, None);
    }
}

/// The id of the season item, as text, that holds the episode `item`.
async fn b18_season_of(pool: &SqlitePool, item: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT parent_id FROM media_items WHERE id = ?1")
        .bind(item.to_string())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// B18 fix round 2, item 1: when Sonarr's season folder format changes, as `Season 1` to
/// `Season 01`, Rename Files moves every file of a season to the new folder. Each episode keeps its
/// item, with its id and its users' data, through a rescan of its show or a full scan, and the old
/// season goes. The gone episode of another show is no episode of a new show with its numbers.
#[tokio::test]
async fn b18_tv_rescans_move_episodes_to_a_renamed_season_folder() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let old = [
        "Next/Season 2/Next - S02E01 - Return WEBDL-720p.mkv",
        "Show/Season 1/Show - S01E01 - Pilot WEBDL-720p.mkv",
        "Show/Season 1/Show - S01E02 - Second WEBDL-720p.mkv",
    ];
    let new = [
        "Next/Season 02/Next (2019) - S02E01 - Return [WEBDL-720p].mkv",
        "Show/Season 01/Show (2020) - S01E01 - Pilot [WEBDL-720p].mkv",
        "Show/Season 01/Show (2020) - S01E02 - Second [WEBDL-720p].mkv",
    ];
    let other = "Other/Season 1/Other - S01E01 - Start WEBDL-720p.mkv";
    for rel in old.iter().chain([&other]) {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    let user = insert_user(pool).await;
    let mut ids = Vec::new();
    for rel in old {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, &b18_watched()).await;
        ids.push(item);
    }
    let other_id = item_at(pool, &lib, other).await;
    let rename = |season: &str, renamed: &str, at: &[usize]| {
        std::fs::create_dir_all(root.join(renamed)).unwrap();
        for &i in at {
            std::fs::rename(root.join(old[i]), root.join(new[i])).unwrap();
        }
        std::fs::remove_dir(root.join(season)).unwrap();
    };

    // A rescan of Show after its season folder is renamed, and a full scan after Next's is.
    rename("Show/Season 1", "Show/Season 01", &[1, 2]);
    b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Show"))).await;
    rename("Next/Season 2", "Next/Season 02", &[0]);
    scan(pool, &lib).await;

    let check = async || {
        assert_eq!(
            paths(pool, &lib, "Episode").await,
            [new[0], other, new[1], new[2]]
        );
        assert_eq!(
            paths(pool, &lib, "Season").await,
            ["Next/Season 02", "Other/Season 1", "Show/Season 01"]
        );
        for (rel, &item) in new.iter().zip(&ids) {
            assert_eq!(
                item_at(pool, &lib, rel).await,
                item,
                "{rel} keeps the item of the file it replaced"
            );
            assert_eq!(
                marks_of(pool, &user, item).await,
                Some(b18_watched()),
                "{rel} keeps its user data"
            );
            let season = Path::new(rel).parent().unwrap().to_str().unwrap();
            assert_eq!(
                b18_season_of(pool, item).await,
                Some(item_at(pool, &lib, season).await.to_string()),
                "{rel} moves to its new season"
            );
        }
    };
    check().await;
    scan(pool, &lib).await;
    check().await;

    // Other's episode gives way to the same numbers in another show.
    let third = "Third/Season 1/Third - S01E01 - Begin WEBDL-720p.mkv";
    std::fs::remove_dir_all(root.join("Other")).unwrap();
    touch(&root, third);
    scan(pool, &lib).await;
    assert_ne!(
        item_at(pool, &lib, third).await,
        other_id,
        "an episode of another show took the gone episode over"
    );
}

/// B18 fix round 2, item 2: a new file that more than one gone item matches takes over the oldest
/// of them, and the others merge into it with their users' data, as the items of one file do. Here
/// two copies of one episode, in `Season 1` and `Season 01`, give way to one file.
#[tokio::test]
async fn b18_rescans_merge_every_gone_item_that_a_new_file_takes_over() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let first = "Show/Season 1/Show - S01E01 - Pilot WEBDL-720p.mkv";
    let second = "Show/Season 01/Show - S01E01 - Pilot HDTV-720p.mkv";
    touch(&root, first);
    scan(pool, &lib).await;
    touch(&root, second);
    scan(pool, &lib).await;
    let older = item_at(pool, &lib, first).await;
    let newer = item_at(pool, &lib, second).await;
    assert_ne!(older, newer, "each season folder holds an item of its own");
    for (item, date) in [
        (older, "2024-01-01T00:00:00+00:00"),
        (newer, "2024-02-01T00:00:00+00:00"),
    ] {
        exec(
            pool,
            "UPDATE media_items SET date_created = ?1 WHERE id = ?2",
            &[date, &item.to_string()],
        )
        .await;
    }
    let user = insert_user(pool).await;
    let older_marks = Marks {
        position: 42,
        plays: 2,
        played: true,
        last_played: Some(EARLY.to_string()),
        ..Marks::default()
    };
    let newer_marks = Marks {
        position: 7,
        plays: 1,
        favorite: true,
        last_played: Some(LATE.to_string()),
        rating: Some(8),
        ..Marks::default()
    };
    insert_marks(pool, &user, older, &older_marks).await;
    insert_marks(pool, &user, newer, &newer_marks).await;

    std::fs::remove_dir_all(root.join("Show/Season 1")).unwrap();
    std::fs::remove_file(root.join(second)).unwrap();
    let added = "Show/Season 01/Show - S01E01 - Pilot Bluray-1080p.mkv";
    touch(&root, added);
    b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Show"))).await;

    assert_eq!(
        b18_ids(pool, &lib, "Episode").await,
        [(added.to_string(), older.to_string())],
        "the new file takes over the older item, and the newer one merges into it"
    );
    assert_eq!(paths(pool, &lib, "Season").await, ["Show/Season 01"]);
    assert_eq!(
        marks_of(pool, &user, older).await,
        Some(Marks {
            position: 7,
            plays: 3,
            favorite: true,
            played: true,
            last_played: Some(LATE.to_string()),
            rating: Some(8),
            likes: None,
        }),
        "the merged item keeps the user data of both"
    );
    assert_eq!(marks_of(pool, &user, newer).await, None);
}

/// B18 fix round 2, item 4: a trailer or a sample, by Jellyfin's names for them, takes over no gone
/// item, even as the only new file in the folder of a movie or the season of an episode whose file
/// just went. It gets an item of its own. A movie's new file that comes with a trailer takes the
/// movie over.
#[tokio::test]
async fn b18_rescans_give_no_gone_item_to_a_trailer_or_sample() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;
    let user = insert_user(pool).await;
    let movie = "Heat (1995)/Heat (1995) WEBDL-720p.mkv";
    let mut took = Vec::new();
    for (n, extra) in [
        "Heat (1995)-trailer.mkv",
        "Heat (1995).Trailer.mkv",
        "Heat (1995) - trailer2.mkv",
        "Heat (1995)_sample.mkv",
        "Heat (1995)-SAMPLE.mkv",
        "trailer.mkv",
    ]
    .into_iter()
    .enumerate()
    {
        let lib = fx
            .library(&format!("film{n}"), LibraryContentType::Movies)
            .await;
        let root = root_of(&lib);
        touch(&root, movie);
        scan(pool, &lib).await;
        let gone = item_at(pool, &lib, movie).await;
        insert_marks(pool, &user, gone, &b18_watched()).await;
        std::fs::remove_file(root.join(movie)).unwrap();
        let rel = format!("Heat (1995)/{extra}");
        touch(&root, &rel);
        b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Heat (1995)"))).await;
        let item = item_at(pool, &lib, &rel).await;
        if item == gone || marks_of(pool, &user, item).await.is_some() {
            took.push(rel);
        }
    }

    let tv = fx.library("tv", LibraryContentType::TvShows).await;
    let root = root_of(&tv);
    let pilot = "Show/Season 1/Show - S01E01 - Pilot WEBDL-720p.mkv";
    touch(&root, pilot);
    scan(pool, &tv).await;
    let gone = item_at(pool, &tv, pilot).await;
    insert_marks(pool, &user, gone, &b18_watched()).await;
    std::fs::remove_file(root.join(pilot)).unwrap();
    let trailer = "Show/Season 1/Show - S01E01 - Pilot-trailer.mkv";
    touch(&root, trailer);
    b18_rescan(pool, &tv, &ScanScope::Folder(root.join("Show"))).await;
    let item = item_at(pool, &tv, trailer).await;
    if item == gone || marks_of(pool, &user, item).await.is_some() {
        took.push(trailer.to_string());
    }
    assert!(took.is_empty(), "these took over a gone item: {took:?}");

    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, movie);
    scan(pool, &lib).await;
    let gone = item_at(pool, &lib, movie).await;
    std::fs::remove_file(root.join(movie)).unwrap();
    touch(&root, "Heat (1995)/Heat (1995) Bluray-1080p.mkv");
    touch(&root, "Heat (1995)/Heat (1995)-trailer.mkv");
    b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Heat (1995)"))).await;
    assert_eq!(
        b18_ids(pool, &lib, "Movie")
            .await
            .into_iter()
            .map(|(_, id)| id)
            .collect::<Vec<_>>(),
        [gone.to_string()],
        "the new file and its trailer take the movie over"
    );
}

/// What [`b18_ids`] gives when the item at each of `rels`, in path order, is the one at the same
/// place in `ids`.
fn b18_ids_at(rels: &[String], ids: &[Uuid]) -> Vec<(String, String)> {
    rels.iter()
        .zip(ids)
        .map(|(rel, id)| (rel.clone(), id.to_string()))
        .collect()
}

/// B18 fix round 3, scenario 1: Jellyfin allows specials in a season folder. Such a special has its
/// season in its file name, as `S00E01`, and the season's own first episode has the same episode
/// number. Each keeps its item and its users' data when the season folder is renamed, and when both
/// files are renamed in one scan.
#[tokio::test]
async fn b18_tv_rescans_keep_a_special_and_an_episode_of_one_season_folder_apart() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let at = |season: &str, names: [&str; 2]| names.map(|name| format!("Show/{season}/{name}"));
    let names = [
        "Show (2020) - S00E01 - Special WEBDL-720p.mkv",
        "Show (2020) - S01E01 - Pilot WEBDL-720p.mkv",
    ];
    let renamed = [
        "Show (2020) - S00E01 - Special [Bluray-1080p].mkv",
        "Show (2020) - S01E01 - Pilot [Bluray-1080p].mkv",
    ];
    let first = at("Season 1", names);
    for rel in &first {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    let user = insert_user(pool).await;
    let marks = [
        Marks {
            position: 7,
            plays: 2,
            last_played: Some(LATE.to_string()),
            rating: Some(8),
            ..Marks::default()
        },
        b18_watched(),
    ];
    let mut ids = Vec::new();
    for (rel, marks) in first.iter().zip(&marks) {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, marks).await;
        ids.push(item);
    }
    let check = async |rels: [String; 2]| {
        assert_eq!(
            b18_ids(pool, &lib, "Episode").await,
            b18_ids_at(&rels, &ids),
            "the special and the episode keep their own items"
        );
        for (item, marks) in ids.iter().zip(&marks) {
            assert_eq!(
                marks_of(pool, &user, *item).await.as_ref(),
                Some(marks),
                "each keeps its own user data"
            );
        }
    };

    // A rescan of the show after its season folder is renamed, then a full scan after both files are.
    std::fs::rename(root.join("Show/Season 1"), root.join("Show/Season 01")).unwrap();
    b18_rescan(pool, &lib, &ScanScope::Folder(root.join("Show"))).await;
    check(at("Season 01", names)).await;
    for (name, new) in names.iter().zip(renamed) {
        std::fs::rename(
            root.join(format!("Show/Season 01/{name}")),
            root.join(format!("Show/Season 01/{new}")),
        )
        .unwrap();
    }
    scan(pool, &lib).await;
    check(at("Season 01", renamed)).await;
}

/// B18 fix round 3, scenario 2: the files in the root of a TV library all go to one series and one
/// season. Two shows' episodes there with one episode number, in different seasons by their file
/// names, keep their own items and their users' data when both files are renamed in one scan.
#[tokio::test]
async fn b18_tv_rescans_keep_loose_episodes_of_two_shows_apart() {
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let old = ["Alpha (2020) S02E03.mkv", "Beta (2021) S01E03.mkv"];
    let new = [
        "Alpha (2020) - S02E03 - Third [WEBDL-720p].mkv".to_string(),
        "Beta (2021) - S01E03 - Three [WEBDL-720p].mkv".to_string(),
    ];
    for rel in old {
        touch(&root, rel);
    }
    scan(pool, &lib).await;
    let user = insert_user(pool).await;
    let marks = [
        b18_watched(),
        Marks {
            position: 7,
            plays: 2,
            last_played: Some(LATE.to_string()),
            rating: Some(8),
            ..Marks::default()
        },
    ];
    let mut ids = Vec::new();
    for (rel, marks) in old.iter().zip(&marks) {
        let item = item_at(pool, &lib, rel).await;
        insert_marks(pool, &user, item, marks).await;
        ids.push(item);
    }

    for (old, new) in old.iter().zip(&new) {
        std::fs::rename(root.join(old), root.join(new)).unwrap();
    }
    scan(pool, &lib).await;

    assert_eq!(
        b18_ids(pool, &lib, "Episode").await,
        b18_ids_at(&new, &ids),
        "each show's episode keeps its own item"
    );
    for (item, marks) in ids.iter().zip(&marks) {
        assert_eq!(
            marks_of(pool, &user, *item).await.as_ref(),
            Some(marks),
            "each keeps its own user data"
        );
    }
}

// B26: re-read old streams. Each source records the version of the probe code that wrote it. A
// scan reads again, once, every file whose source an older probe wrote, the way it reads a changed
// file: videos through their version groups, music tracks and Mixed-library audio on their own.

/// Copies a clip to `<root>/<rel>` whose audio and subtitle streams have a language and a title.
/// ffmpeg writes it under `<temp dir>/clips` on first use.
fn b26_clip(fx: &Fixture, root: &Path, rel: &str) -> PathBuf {
    let template = fx.path("clips/b26-tagged.mkv");
    if !template.exists() {
        let srt = fx.path("clips/b26.srt");
        std::fs::create_dir_all(srt.parent().unwrap()).unwrap();
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,500\nHello\n").unwrap();
        let status = Command::new("ffmpeg")
            .args(["-nostdin", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc=size=64x48:rate=5:duration=2"])
            .args(["-f", "lavfi", "-i", "sine=duration=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=880:duration=2"])
            .arg("-i")
            .arg(&srt)
            .args(["-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:s"])
            .args(["-c:v", "mpeg4", "-c:a", "aac", "-c:s", "srt"])
            .args([
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:0",
                "title=Main",
            ])
            .args(["-metadata:s:a:1", "language=spa"])
            .args(["-metadata:s:a:1", "title=Commentary"])
            .args([
                "-metadata:s:s:0",
                "language=ita",
                "-metadata:s:s:0",
                "title=Signs",
            ])
            .arg(&template)
            .status()
            .expect("run ffmpeg");
        assert!(
            status.success(),
            "ffmpeg could not write {}",
            template.display()
        );
    }
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(&template, &path).unwrap();
    path
}

/// Writes a one second Ogg FLAC tone to `<root>/<rel>`, with its parent folders. Its stream has
/// the title "Song" and the language "ita".
fn b26_track(root: &Path, rel: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "sine=duration=1", "-c:a", "flac"])
        .args([
            "-metadata:s:a:0",
            "title=Song",
            "-metadata:s:a:0",
            "language=ita",
        ])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not write {}",
        path.display()
    );
    path
}

/// A stream as (index, codec type, language, title).
type B26Stream = (i64, String, Option<String>, Option<String>);

/// The audio and subtitle streams of the source at `<root>/<rel>`, by index.
async fn b26_streams(pool: &SqlitePool, lib: &Library, rel: &str) -> Vec<B26Stream> {
    let path = root_of(lib).join(rel);
    sqlx::query_as(
        "SELECT st.stream_index, st.codec_type, st.language, st.title
         FROM media_streams AS st JOIN media_sources AS s ON s.id = st.source_id
         WHERE s.path = ?1 AND st.codec_type IN ('audio', 'subtitle')
         ORDER BY st.stream_index",
    )
    .bind(path.to_str().unwrap())
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| panic!("the streams of {rel}: {e}"))
}

/// The streams of `labels`, as (index, codec type, language, title), with or without their labels.
fn b26_labels(labels: &[(i64, &str, &str, &str)], labelled: bool) -> Vec<B26Stream> {
    let label = |text: &str| labelled.then(|| text.to_string());
    labels
        .iter()
        .map(|&(index, kind, language, title)| {
            (index, kind.to_string(), label(language), label(title))
        })
        .collect()
}

/// The audio and subtitle streams of `b26_clip`, with or without their labels.
fn b26_clip_streams(labelled: bool) -> Vec<B26Stream> {
    b26_labels(
        &[
            (1, "audio", "eng", "Main"),
            (2, "audio", "spa", "Commentary"),
            (3, "subtitle", "ita", "Signs"),
        ],
        labelled,
    )
}

/// The stream of `b26_track`, with or without its labels.
fn b26_track_streams(labelled: bool) -> Vec<B26Stream> {
    b26_labels(&[(0, "audio", "ita", "Song")], labelled)
}

/// Makes the sources of `rels`, relative to the library's root, look as the probe before 06fda0d
/// left them: written by probe version 0, with no stream language or title.
async fn b26_make_old(pool: &SqlitePool, lib: &Library, rels: &[&str]) {
    let root = root_of(lib);
    for rel in rels {
        let path = root.join(rel);
        let path = path.to_str().unwrap();
        exec(
            pool,
            "UPDATE media_sources SET probe_version = 0 WHERE path = ?1",
            &[path],
        )
        .await;
        exec(
            pool,
            "UPDATE media_streams SET language = NULL, title = NULL
             WHERE source_id IN (SELECT id FROM media_sources WHERE path = ?1)",
            &[path],
        )
        .await;
    }
}

/// The probe version of every source of the library, as (path relative to its root, version),
/// sorted.
async fn b26_versions(pool: &SqlitePool, lib: &Library) -> Vec<(String, i64)> {
    let root = root_of(lib);
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT s.path, s.probe_version FROM media_sources AS s
         JOIN media_items AS i ON i.id = s.item_id
         WHERE i.library_id = ?1",
    )
    .bind(lib.id.to_string())
    .fetch_all(pool)
    .await
    .unwrap_or_else(|e| panic!("the probe versions of the sources: {e}"));
    let mut out: Vec<(String, i64)> = rows
        .into_iter()
        .map(|(path, version)| (rel(&root, &path), version))
        .collect();
    out.sort();
    out
}

/// `b26_versions` after the first scan of `files`: one source each, written by a current probe.
async fn b26_fresh_versions(
    pool: &SqlitePool,
    lib: &Library,
    files: &[&str],
) -> Vec<(String, i64)> {
    let fresh = b26_versions(pool, lib).await;
    let mut paths: Vec<&str> = fresh.iter().map(|(path, _)| path.as_str()).collect();
    let mut want = files.to_vec();
    paths.sort_unstable();
    want.sort_unstable();
    assert_eq!(paths, want, "the first scan did not probe each file once");
    assert!(
        fresh.iter().all(|(_, version)| *version > 0),
        "a new probe stored version 0: {fresh:?}"
    );
    fresh
}

/// The ids of the item's sources, sorted.
async fn b26_item_sources(pool: &SqlitePool, item: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM media_sources WHERE item_id = ?1 ORDER BY id")
        .bind(item.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Makes the file at `path` unreadable to ffprobe without changing its stamp: zeros of its size,
/// with its modification time. Only the version of its source then tells a scan to read it again.
fn b26_break(path: &Path) {
    let (size, modified) = stat(path);
    std::fs::write(path, vec![0u8; usize::try_from(size).unwrap()]).unwrap();
    b18_set_modified(path, u64::try_from(modified).unwrap());
    assert_eq!(stat(path), (size, modified));
}

/// ffmpeg. B26 criteria 3 and 4: a scan counts a movie whose source an older probe wrote as read
/// again, before any probe runs, and leaves alone one whose source a probe at least as new as this
/// one wrote. A version far above any real one stands for the current one, so the test holds when
/// the version goes up. Without ffprobe a scan reads nothing again, which
/// `b26_a_scan_without_ffprobe_reads_nothing_again` tests.
#[tokio::test]
async fn b26_a_scan_counts_old_sources_as_rereads() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    for (rel, version) in [
        ("Old (2001)/Old (2001).mkv", "0"),
        ("New (2002)/New (2002).mkv", "1000000"),
    ] {
        let path = touch(&root, rel);
        let item = insert_movie(pool, &lib, &path, Movie::default()).await;
        let (size, modified) = stat(&path);
        exec(
            pool,
            "INSERT INTO media_sources (id, item_id, uid, path, size, modified, probe_version)
             VALUES (?1, ?2, ?1, ?3, ?4, ?5, ?6)",
            &[
                &Uuid::new_v4().to_string(),
                &item.to_string(),
                path.to_str().unwrap(),
                &size.to_string(),
                &modified.to_string(),
                version,
            ],
        )
        .await;
    }
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan did not read the old source again, or not only it"
    );
}

/// ffmpeg. B26 criteria 3, 4 and 10: a scan reads again, once, each movie whose sources an older
/// probe wrote, every version of it, and its streams get their languages and titles back. The
/// items keep their ids and user data. The next scan reads nothing again.
#[tokio::test]
async fn b26_old_sources_are_read_again_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let alpha = "Alpha (2001)/Alpha (2001).mkv";
    let beta_uhd = "Beta (2002)/Beta (2002) Bluray-2160p.mkv";
    let beta_hd = "Beta (2002)/Beta (2002) Bluray-1080p.mkv";
    let files = [alpha, beta_uhd, beta_hd];
    for file in files {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    for file in files {
        assert_eq!(
            b26_streams(pool, &lib, file).await,
            b26_clip_streams(true),
            "{file}: a new probe stored no labels"
        );
    }

    b26_make_old(pool, &lib, &files).await;
    assert!(
        b26_versions(pool, &lib).await.iter().all(|(_, v)| *v == 0),
        "the sources are not old"
    );
    let user = insert_user(pool).await;
    let alpha_id = item_at(pool, &lib, alpha).await;
    let beta_id = item_at(pool, &lib, beta_uhd).await;
    insert_marks(pool, &user, alpha_id, &b18_watched()).await;
    insert_marks(pool, &user, beta_id, &at(1234, Some(LATE))).await;
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the scan did not read both movies again"
    );
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [alpha, beta_hd, beta_uhd]);
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    for file in files {
        assert_eq!(
            b26_streams(pool, &lib, file).await,
            b26_clip_streams(true),
            "{file}: the re-read stored no labels"
        );
    }
    assert_eq!(item_at(pool, &lib, alpha).await, alpha_id);
    assert_eq!(item_at(pool, &lib, beta_uhd).await, beta_id);
    assert_groups(
        pool,
        &lib,
        &[(alpha, &[alpha]), (beta_uhd, &[beta_uhd, beta_hd])],
    )
    .await;
    assert_eq!(marks_of(pool, &user, alpha_id).await, Some(b18_watched()));
    assert_eq!(
        marks_of(pool, &user, beta_id).await,
        Some(at(1234, Some(LATE)))
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a second scan read a file again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 criteria 3 and 4: a TV scan reads old episodes again, once.
#[tokio::test]
async fn b26_old_episodes_are_read_again_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("tv", LibraryContentType::TvShows).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let first = "Severance/Season 1/Severance S01E01.mkv";
    let second = "Severance/Season 1/Severance S01E02.mkv";
    let files = [first, second];
    for file in files {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    let ids = [
        item_at(pool, &lib, first).await,
        item_at(pool, &lib, second).await,
    ];

    b26_make_old(pool, &lib, &files).await;
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the scan did not read both episodes again"
    );
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    for file in files {
        assert_eq!(
            b26_streams(pool, &lib, file).await,
            b26_clip_streams(true),
            "{file}: the re-read stored no labels"
        );
    }
    assert_eq!(
        [
            item_at(pool, &lib, first).await,
            item_at(pool, &lib, second).await
        ],
        ids
    );

    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a second scan read an episode again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 criteria 3 and 4: a rescan of one folder, as the library monitor runs, reads the old
/// sources in it again and leaves the rest of the library to a later scan.
#[tokio::test]
async fn b26_a_folder_rescan_reads_its_old_sources_again() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let alpha = "Alpha (2001)/Alpha (2001).mkv";
    let beta = "Beta (2002)/Beta (2002).mkv";
    let files = [alpha, beta];
    for file in files {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    b26_make_old(pool, &lib, &files).await;

    let folder = ScanScope::Folder(root.join("Alpha (2001)"));
    assert_eq!(
        b18_rescan(pool, &lib, &folder).await,
        (0, 0, 1),
        "the folder rescan did not read its movie again"
    );
    assert_eq!(b26_streams(pool, &lib, alpha).await, b26_clip_streams(true));
    assert_eq!(
        b26_streams(pool, &lib, beta).await,
        b26_clip_streams(false),
        "the folder rescan read a movie outside its folder"
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the full scan did not read the rest again, or read Alpha again"
    );
    assert_eq!(b26_streams(pool, &lib, beta).await, b26_clip_streams(true));
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read a movie again"
    );
}

/// ffmpeg. B26 criterion 7: a scan that stopped part way had read one old movie again, had not
/// reached another, and stopped during the probe of a third after its sources were gone, as the
/// probe before B26 left them. The next scan reads the last two, and only them.
#[tokio::test]
async fn b26_a_stopped_reread_carries_on() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let done = "Done (2001)/Done (2001).mkv";
    let left = "Left (2002)/Left (2002).mkv";
    let cut = "Cut (2003)/Cut (2003).mkv";
    let files = [done, left, cut];
    for file in files {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;

    b26_make_old(pool, &lib, &[left, cut]).await;
    let cut_path = root.join(cut);
    exec(
        pool,
        "DELETE FROM media_sources WHERE path = ?1",
        &[cut_path.to_str().unwrap()],
    )
    .await;
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the scan did not carry on with the two movies left"
    );
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [cut, left]);
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    for file in files {
        assert_eq!(
            b26_streams(pool, &lib, file).await,
            b26_clip_streams(true),
            "{file}"
        );
    }
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read a movie again"
    );
}

/// ffmpeg. B26 criterion 5: an old file that also changed is read again once, and its stamp is
/// stored. After that it is read again only when it changes, as any file is.
#[tokio::test]
async fn b26_changed_old_files_are_read_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let alpha = "Alpha (2001)/Alpha (2001).mkv";
    let beta = "Beta (2002)/Beta (2002).mkv";
    let files = [alpha, beta];
    for file in files {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    b26_make_old(pool, &lib, &files).await;

    b18_set_modified(&root.join(alpha), 1_000_000_000);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the changed old movie was not read once, or the other old movie not at all"
    );
    assert_eq!(b18_stamps(pool, &lib).await, b18_file_stamps(&lib, &files));
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    assert_eq!(b26_streams(pool, &lib, alpha).await, b26_clip_streams(true));
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read a movie again"
    );

    b18_set_modified(&root.join(beta), 1_000_000_000);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "a changed movie was not read again"
    );
}

/// ffmpeg. B26 criterion 6: a re-read skips a file whose probe failed before, as every scan does.
/// An old file that no longer probes keeps its source and gets a failure record, and later scans
/// leave it alone (review 1, finding 1).
#[tokio::test]
async fn b26_failed_probes_are_not_read_again() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let alpha = "Alpha (2001)/Alpha (2001).mkv";
    let gamma = "Gamma (2003)/Gamma (2003).mkv";
    let broken = touch(&root, "Broken (2010)/Broken (2010).mkv");
    for file in [alpha, gamma] {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    assert!(
        failure_of(pool, &broken).await.is_some(),
        "the probe of the empty file did not fail"
    );
    b26_fresh_versions(pool, &lib, &[alpha, gamma]).await;
    b26_make_old(pool, &lib, &[alpha, gamma]).await;
    exec(
        pool,
        "UPDATE probe_failures SET failed_at = ?1",
        &[LONG_AGO],
    )
    .await;

    // Gamma no longer probes, and only its version tells the scan to read it again.
    let gamma_path = root.join(gamma);
    b26_break(&gamma_path);
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the scan did not read the two old movies again"
    );
    assert_eq!(
        failure_of(pool, &broken).await,
        failure(&broken, LONG_AGO),
        "the scan probed a known failure again"
    );
    let (_, _, failed_at) = failure_of(pool, &gamma_path)
        .await
        .expect("the failed re-read is not recorded");
    assert_eq!(
        failure_of(pool, &gamma_path).await,
        failure(&gamma_path, &failed_at)
    );
    assert_eq!(b26_streams(pool, &lib, alpha).await, b26_clip_streams(true));
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [alpha]);
    let kept = before
        .iter()
        .find(|(path, _)| path == gamma)
        .expect("Gamma has no source");
    assert!(
        after.contains(kept),
        "the failed re-read did not keep Gamma's source: {after:?}"
    );
    assert_eq!(
        b26_streams(pool, &lib, gamma).await,
        b26_clip_streams(false)
    );

    exec(
        pool,
        "UPDATE probe_failures SET failed_at = ?1",
        &[LONG_AGO],
    )
    .await;
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read a movie again"
    );
    for path in [&broken, &gamma_path] {
        assert_eq!(
            failure_of(pool, path).await,
            failure(path, LONG_AGO),
            "{}: a later scan probed it again",
            path.display()
        );
    }
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 criterion 9: a music scan reads again, once, a track whose source an older probe
/// wrote, on the same item, and leaves a current track alone.
#[tokio::test]
async fn b26_old_tracks_are_read_again_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let song = "Artist B/Album B/01 Song.ogg";
    let other = "Artist B/Album B/02 Other.ogg";
    let song_path = b26_track(&root, song);
    let other_path = b26_track(&root, other);
    scan(pool, &lib).await;
    let first = b18_track_probed(pool, &song_path, &[]).await;
    let other_id = b18_track_probed(pool, &other_path, &[]).await;
    let fresh = b26_fresh_versions(pool, &lib, &[song, other]).await;
    assert_eq!(b26_streams(pool, &lib, song).await, b26_track_streams(true));
    let item = item_at(pool, &lib, song).await;

    b26_make_old(pool, &lib, &[song]).await;
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan did not read the old track again, or not only it"
    );
    let second = b18_track_probed(pool, &song_path, &[first]).await;
    assert_eq!(item_at(pool, &lib, song).await, item);
    assert_eq!(b26_streams(pool, &lib, song).await, b26_track_streams(true));
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    assert_eq!(b18_track_probed(pool, &other_path, &[]).await, other_id);

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a second scan read a track again"
    );
    // A music scan's re-reads run under the scan lock, which `b18_rescan` waits for, so a re-read
    // of either track would have replaced its source by now.
    assert_eq!(b18_track_probed(pool, &song_path, &[]).await, second);
    assert_eq!(b18_track_probed(pool, &other_path, &[]).await, other_id);
}

/// ffmpeg. B26 criteria 3 and 9: in a Mixed library, an old video and an old audio file are each
/// read again, once.
#[tokio::test]
async fn b26_old_files_of_a_mixed_library_are_read_again_once() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("mixed", LibraryContentType::Mixed).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let movie = "Alpha (2001)/Alpha (2001).mkv";
    let song = "Songs/01 Song.ogg";
    b26_clip(&fx, &root, movie);
    b26_track(&root, song);
    scan_idle(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &[movie, song]).await;

    b26_make_old(pool, &lib, &[movie, song]).await;
    let before = b18_source_ids(pool, &lib).await;
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 2),
        "the scan did not read the old video and the old audio file again"
    );
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [movie, song]);
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    assert_eq!(b26_streams(pool, &lib, movie).await, b26_clip_streams(true));
    assert_eq!(b26_streams(pool, &lib, song).await, b26_track_streams(true));

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a second scan read a file again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 criterion 8: `probe_and_save` replaces an item's sources only once its probe has
/// ended, so a scan stopped during the probe of a track leaves the track's old source, which the
/// next scan reads again. A FIFO holds the probe open until the test writes the track into it.
#[cfg(unix)]
#[tokio::test]
async fn b26_a_probe_keeps_the_old_sources_until_it_ends() {
    use std::io::Write as _;

    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let song = "Artist B/Album B/01 Song.ogg";
    let track = b26_track(&root, song);
    scan(pool, &lib).await;
    let old = b18_track_probed(pool, &track, &[]).await;
    let item = item_at(pool, &lib, song).await;

    let fifo = fx.path("fifo/01 Song.ogg");
    let copy = fx.path("fifo/copy.ogg");
    std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    std::fs::copy(&track, &copy).unwrap();
    let made = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo could not make {}", fifo.display());

    // The writer's open of the FIFO ends once ffprobe has opened it to read the streams. The
    // writer then puts the copy at the FIFO's path, which the probe's read of the chapters opens,
    // and writes the track into the FIFO. It is not joined: a probe that never opens the FIFO
    // fails the test at the timeout instead of hanging it.
    let (opened, ffprobe_opened) = tokio::sync::oneshot::channel();
    let (go, wait_for_go) = std::sync::mpsc::channel::<()>();
    let bytes = std::fs::read(&track).unwrap();
    let writer_end = fifo.clone();
    std::thread::spawn(move || {
        let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(&writer_end) else {
            return;
        };
        let _ = opened.send(());
        if wait_for_go.recv().is_ok() {
            let _ = std::fs::rename(&copy, &writer_end);
            let _ = file.write_all(&bytes);
        }
    });
    let probe = tokio::spawn({
        let pool = pool.clone();
        let fifo = fifo.to_str().unwrap().to_string();
        async move { probe::probe_and_save(&pool, &item, &fifo).await }
    });
    tokio::time::timeout(Duration::from_secs(30), ffprobe_opened)
        .await
        .expect("ffprobe did not open the FIFO within 30 s")
        .expect("the writer ended");
    let during = b26_item_sources(pool, item).await;
    go.send(()).unwrap();
    let probed = tokio::time::timeout(Duration::from_secs(30), probe)
        .await
        .expect("the probe did not end within 30 s")
        .expect("the probe task panicked");
    assert_eq!(
        during,
        std::slice::from_ref(&old),
        "the item lost its source while its probe ran"
    );
    probed.unwrap_or_else(|e| panic!("probe of the FIFO: {e}"));
    let after = b26_item_sources(pool, item).await;
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after, [old], "the probe did not replace the source");
}

/// Scans all of `lib` with no metadata provider, and returns without waiting for the probes that
/// the scan queued.
async fn b26_scan_now(pool: &SqlitePool, lib: &Library) -> ScanChanges {
    scanner::scan_with_metadata(
        pool,
        lib,
        &ScanScope::Library,
        &reqwest::Client::new(),
        None,
        None,
        None,
        &ScanTicket::untracked(),
    )
    .await
    .unwrap_or_else(|e| panic!("scan of {}: {e}", lib.name))
}

/// Runs the test `name` again in a child process with `envs` added to its environment, and fails
/// unless the child passed.
fn b26_rerun(name: &str, envs: &[(&str, &std::ffi::OsStr)]) {
    let child = Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--test-threads=1", "--nocapture"])
        .envs(envs.iter().copied())
        .output()
        .expect("run the test binary again");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(
        child.status.success() && out.contains("1 passed"),
        "{name} failed in its child process:\n{out}"
    );
}

/// Runs the test `name` again in a child process whose ffprobe waits 3 s before each run once the
/// child has created the file that `B26_SLOW_MARKER` names. A scan that does not wait for its
/// re-reads then ends long before them.
async fn b26_with_slow_ffprobe(name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new().await;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let ffprobe = std::env::split_paths(&path)
        .map(|dir| dir.join("ffprobe"))
        .find(|file| file.is_file())
        .expect("ffprobe on PATH");
    let dir = fx.path("slow-ffprobe");
    std::fs::create_dir_all(&dir).unwrap();
    let wrapper = dir.join("ffprobe");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\n[ -e \"$B26_SLOW_MARKER\" ] && sleep 3\nexec \"$B26_FFPROBE\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path =
        std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path))).unwrap();
    let marker = fx.path("slow");
    b26_rerun(
        name,
        &[
            ("PATH", path.as_os_str()),
            ("B26_FFPROBE", ffprobe.as_os_str()),
            ("B26_SLOW_MARKER", marker.as_os_str()),
        ],
    );
}

/// ffmpeg. B26 criteria 9 and 13: a rescan of one folder of a music library, as the library monitor
/// runs, reads the old tracks in it again before it ends, and leaves the rest of the library to a
/// later scan. The test runs itself again with an ffprobe that takes 3 s longer for the re-reads.
#[tokio::test]
async fn b26_a_music_folder_rescan_reads_its_old_tracks_again() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let Some(marker) = std::env::var_os("B26_SLOW_MARKER") else {
        b26_with_slow_ffprobe("b26_a_music_folder_rescan_reads_its_old_tracks_again").await;
        return;
    };
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let inside = "Artist A/Album A/01 Song.ogg";
    let outside = "Artist B/Album B/01 Song.ogg";
    let files = [inside, outside];
    for file in files {
        b26_track(&root, file);
    }
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    b26_make_old(pool, &lib, &files).await;
    std::fs::write(&marker, "").unwrap();

    let folder = ScanScope::Folder(root.join("Artist A"));
    assert_eq!(
        b18_rescan(pool, &lib, &folder).await,
        (0, 0, 1),
        "the folder rescan did not read its track again"
    );
    assert_eq!(
        b26_streams(pool, &lib, inside).await,
        b26_track_streams(true),
        "the folder rescan ended before its re-read"
    );
    assert_eq!(
        b26_streams(pool, &lib, outside).await,
        b26_track_streams(false),
        "the folder rescan read a track outside its folder"
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the full scan did not read the rest again, or read the first track again"
    );
    assert_eq!(
        b26_streams(pool, &lib, outside).await,
        b26_track_streams(true),
        "the full scan ended before its re-read"
    );
    assert_eq!(b26_versions(pool, &lib).await, fresh);
    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read a track again"
    );
}

/// ffmpeg. B26 criterion 9: a scan that starts while another scan reads old tracks again waits for
/// those re-reads, so it reads none of the tracks a second time. The test runs itself again with an
/// ffprobe that takes 3 s longer for the re-reads.
#[tokio::test]
async fn b26_a_scan_during_music_rereads_waits_for_them() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let Some(marker) = std::env::var_os("B26_SLOW_MARKER") else {
        b26_with_slow_ffprobe("b26_a_scan_during_music_rereads_waits_for_them").await;
        return;
    };
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let files: Vec<String> = (1..=6)
        .map(|n| format!("Artist C/Album C/0{n} Song.ogg"))
        .collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    for file in &files {
        b26_track(&root, file);
    }
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    let fresh = b26_fresh_versions(pool, &lib, &files).await;
    b26_make_old(pool, &lib, &files).await;
    std::fs::write(&marker, "").unwrap();

    let first = b26_scan_now(pool, &lib).await;
    let second = b26_scan_now(pool, &lib).await;
    scanner::wait_for_idle().await;
    assert_eq!(
        (first.reread, second.reread),
        (6, 0),
        "the second scan did not wait for the first one's re-reads"
    );
    assert_eq!(b26_versions(pool, &lib).await, fresh);
}

/// ffmpeg. B26 review 1, finding 1: an alternate version whose re-read fails keeps its source, in
/// its place after the primary's, while the primary is read again. Its failure is recorded, so
/// later scans leave the movie alone.
#[tokio::test]
async fn b26_a_failed_alternate_keeps_its_source() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let uhd = "Beta (2002)/Beta (2002) Bluray-2160p.mkv";
    let hd = "Beta (2002)/Beta (2002) Bluray-1080p.mkv";
    for file in [uhd, hd] {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    b26_fresh_versions(pool, &lib, &[uhd, hd]).await;
    assert_groups(pool, &lib, &[(uhd, &[uhd, hd])]).await;
    b26_make_old(pool, &lib, &[uhd, hd]).await;
    let hd_path = root.join(hd);
    b26_break(&hd_path);
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan did not read the movie again"
    );
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [uhd]);
    assert_groups(pool, &lib, &[(uhd, &[uhd, hd])]).await;
    assert_eq!(b26_streams(pool, &lib, uhd).await, b26_clip_streams(true));
    assert_eq!(b26_streams(pool, &lib, hd).await, b26_clip_streams(false));
    let (_, _, failed_at) = failure_of(pool, &hd_path)
        .await
        .expect("the failed re-read is not recorded");
    assert_eq!(
        failure_of(pool, &hd_path).await,
        failure(&hd_path, &failed_at)
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read the movie again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 review 1, finding 1: a primary version whose re-read fails keeps its source and its
/// place. The alternate is read again but does not take the item over. Later scans leave the
/// movie alone.
#[tokio::test]
async fn b26_a_failed_primary_keeps_its_place() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let uhd = "Beta (2002)/Beta (2002) Bluray-2160p.mkv";
    let hd = "Beta (2002)/Beta (2002) Bluray-1080p.mkv";
    for file in [uhd, hd] {
        b26_clip(&fx, &root, file);
    }
    scan_idle(pool, &lib).await;
    b26_fresh_versions(pool, &lib, &[uhd, hd]).await;
    let item = item_at(pool, &lib, uhd).await;
    b26_make_old(pool, &lib, &[uhd, hd]).await;
    let uhd_path = root.join(uhd);
    b26_break(&uhd_path);
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan did not read the movie again"
    );
    let after = b18_source_ids(pool, &lib).await;
    assert_eq!(b18_reprobed(&before, &after), [hd]);
    assert_groups(pool, &lib, &[(uhd, &[uhd, hd])]).await;
    assert_eq!(item_at(pool, &lib, uhd).await, item);
    assert_eq!(b26_streams(pool, &lib, uhd).await, b26_clip_streams(false));
    assert_eq!(b26_streams(pool, &lib, hd).await, b26_clip_streams(true));
    assert!(
        failure_of(pool, &uhd_path).await.is_some(),
        "the failed re-read is not recorded"
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read the movie again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, after);
}

/// ffmpeg. B26 review 1, finding 1: a track whose re-read fails keeps its source, and its failure
/// is recorded with the file's stamp, so later scans leave it alone.
#[tokio::test]
async fn b26_a_failed_track_reread_keeps_its_source() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("music", LibraryContentType::Music).await;
    let pool = &fx.pool;
    let root = root_of(&lib);
    let song = "Artist B/Album B/01 Song.ogg";
    let other = "Artist B/Album B/02 Other.ogg";
    let song_path = b26_track(&root, song);
    b26_track(&root, other);
    scan(pool, &lib).await;
    b18_tracks_probed(pool, &lib).await;
    b26_fresh_versions(pool, &lib, &[song, other]).await;
    b26_make_old(pool, &lib, &[song]).await;
    b26_break(&song_path);
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan did not read the old track again, or not only it"
    );
    let (_, _, failed_at) = failure_of(pool, &song_path)
        .await
        .expect("the failed re-read is not recorded");
    assert_eq!(
        failure_of(pool, &song_path).await,
        failure(&song_path, &failed_at)
    );
    assert_eq!(
        b18_source_ids(pool, &lib).await,
        before,
        "the failed re-read did not keep the track's source"
    );
    assert_eq!(
        b26_streams(pool, &lib, song).await,
        b26_track_streams(false)
    );

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "a later scan read the track again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, before);
}

/// Gives `item` a source at `path` that probe version 0 wrote. The source stores the file's size,
/// and a modification time `age` seconds before the file's: with an `age` of 0, the file has not
/// changed since that probe. Returns the source's id.
async fn b26_old_source(pool: &SqlitePool, item: Uuid, path: &Path, age: i64) -> String {
    let (size, modified) = stat(path);
    let source = Uuid::new_v4().to_string();
    exec(
        pool,
        "INSERT INTO media_sources (id, item_id, uid, path, size, modified, probe_version)
         VALUES (?1, ?2, ?1, ?3, ?4, ?5, 0)",
        &[
            &source,
            &item.to_string(),
            path.to_str().unwrap(),
            &size.to_string(),
            &(modified - age).to_string(),
        ],
    )
    .await;
    source
}

/// B26 review 1, finding 1: before a scan reads the first file of an older probe again, it checks
/// that ffprobe runs. When ffprobe does not, the scan reads no file again for its version, so every
/// old source stays: a movie's, a music track's and a Mixed library's audio file's. A changed file
/// is still probed, and loses its source when the probe fails, as before. The test runs itself
/// again in a child process whose PATH holds no ffprobe.
#[tokio::test]
async fn b26_a_scan_without_ffprobe_reads_nothing_again() {
    if std::env::var_os("B26_NO_FFPROBE").is_some() {
        b26_scan_without_ffprobe().await;
        return;
    }
    let fx = Fixture::new().await;
    let empty = fx.path("no-ffprobe");
    std::fs::create_dir_all(&empty).unwrap();
    b26_rerun(
        "b26_a_scan_without_ffprobe_reads_nothing_again",
        &[
            ("B26_NO_FFPROBE", std::ffi::OsStr::new("1")),
            ("PATH", empty.as_os_str()),
        ],
    );
}

/// The child process of `b26_a_scan_without_ffprobe_reads_nothing_again`, with no ffprobe.
async fn b26_scan_without_ffprobe() {
    let fx = Fixture::new().await;
    let pool = &fx.pool;

    let film = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&film);
    let old = "Old (2001)/Old (2001).mkv";
    let changed = "Changed (2002)/Changed (2002).mkv";
    let old_path = touch(&root, old);
    let old_item = insert_movie(pool, &film, &old_path, Movie::default()).await;
    let old_source = b26_old_source(pool, old_item, &old_path, 0).await;
    let changed_path = touch(&root, changed);
    let changed_item = insert_movie(pool, &film, &changed_path, Movie::default()).await;
    b26_old_source(pool, changed_item, &changed_path, 60).await;
    assert_eq!(
        b18_rescan(pool, &film, &ScanScope::Library).await,
        (0, 0, 1),
        "the scan read the unchanged old movie again, or did not probe the changed one"
    );
    assert_eq!(
        b26_item_sources(pool, old_item).await,
        [old_source],
        "the unchanged old movie lost its source"
    );
    assert_eq!(
        b26_versions(pool, &film).await,
        [(old.to_string(), 0)],
        "the changed movie kept a source its probe could not read, or the old one was replaced"
    );

    let music = fx.library("music", LibraryContentType::Music).await;
    let track = touch(&root_of(&music), "Artist/Album/01 Song.ogg");
    let track_item = insert_item(pool, &music, "Audio", &track, None).await;
    let track_source = b26_old_source(pool, track_item, &track, 0).await;
    assert_eq!(
        b18_rescan(pool, &music, &ScanScope::Library).await,
        (0, 0, 0),
        "the music scan read the old track again"
    );
    assert_eq!(b26_item_sources(pool, track_item).await, [track_source]);

    let mixed = fx.library("mixed", LibraryContentType::Mixed).await;
    let song = touch(&root_of(&mixed), "Song.ogg");
    let song_item = insert_item(pool, &mixed, "Audio", &song, None).await;
    let song_source = b26_old_source(pool, song_item, &song, 0).await;
    assert_eq!(
        b18_rescan(pool, &mixed, &ScanScope::Library).await,
        (0, 0, 0),
        "the Mixed-library scan read the old audio file again"
    );
    assert_eq!(b26_item_sources(pool, song_item).await, [song_source]);
}

/// ffmpeg. B26 review 1, findings 11 and 12: a scan does not read a photo again for its version,
/// as the doc of `PROBE_VERSION` says. An image stream has no language or title to read.
#[tokio::test]
async fn b26_photos_are_not_read_again() {
    if !have_ffmpeg() {
        eprintln!("skipped: needs ffmpeg and ffprobe");
        return;
    }
    let fx = Fixture::new().await;
    let lib = fx.library("photos", LibraryContentType::Photos).await;
    let pool = &fx.pool;
    let photo = "Beach (2012).jpg";
    let path = root_of(&lib).join(photo);
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=64x48", "-frames:v", "1"])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(
        status.success(),
        "ffmpeg could not write {}",
        path.display()
    );
    scan_idle(pool, &lib).await;
    b26_fresh_versions(pool, &lib, &[photo]).await;
    b26_make_old(pool, &lib, &[photo]).await;
    let before = b18_source_ids(pool, &lib).await;

    assert_eq!(
        b18_rescan(pool, &lib, &ScanScope::Library).await,
        (0, 0, 0),
        "the scan read the photo again"
    );
    assert_eq!(b18_source_ids(pool, &lib).await, before);
    assert_eq!(b26_versions(pool, &lib).await, [(photo.to_string(), 0)]);
}
