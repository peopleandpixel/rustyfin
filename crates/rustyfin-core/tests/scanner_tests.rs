//! Library scans of real files in a temp dir: ignored folders and files (R1-12), symlink
//! loops (M-19), removal of items that a scan no longer collects, episode numbers (R1-13),
//! movie titles (R1-27), and the versions of a movie or an episode as one item (R1-20), which
//! keeps the user data of every copy and does not start on a version that cannot be probed
//! (G9b).
//!
//! The tests marked "ffmpeg" write real clips and need ffmpeg and ffprobe; without them they
//! print a line and pass.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[tokio::test]
async fn rescan_keeps_items_with_missing_files() {
    let fx = Fixture::new().await;
    let lib = fx.library("film", LibraryContentType::Movies).await;
    let root = root_of(&lib);
    touch(&root, "Gone (1999)/Gone (1999).mkv");
    touch(&root, "Here (2000)/Here (2000).mkv");
    // An item outside the library's roots, whose file exists.
    let elsewhere = touch(&fx.path("elsewhere"), "Old (1990).mkv");
    insert_item(&fx.pool, &lib, "Movie", &elsewhere, None).await;
    scan(&fx.pool, &lib).await;

    std::fs::remove_dir_all(root.join("Gone (1999)")).unwrap();
    scan(&fx.pool, &lib).await;

    let mut expected = vec![
        "Gone (1999)/Gone (1999).mkv".to_string(),
        "Here (2000)/Here (2000).mkv".to_string(),
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
