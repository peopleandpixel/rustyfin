//! Unit tests for the missing-episode task, against a mock TMDb.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::NaiveDate;
use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{LocationType, MediaItem, MediaType};
use rustyfin_model::metadata::{
    ExternalMetadata, LookupResult, MetadataImage, MetadataLookups, SeasonMetadata, SeasonSummary,
    SeriesSeasons,
};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::*;

type Answer<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = LookupResult<T>> + Send + 'a>>;

/// A TMDb that answers from maps and records each call.
#[derive(Default)]
struct MockTmdb {
    lists: HashMap<i64, SeriesSeasons>,
    /// What the cache holds
    cached: HashMap<(i64, i32), SeasonMetadata>,
    /// What TMDb answers now
    current: HashMap<(i64, i32), SeasonMetadata>,
    calls: Mutex<Vec<String>>,
}

impl MockTmdb {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn log(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

impl MetadataLookups for MockTmdb {
    fn movie_by_id(&self, _id: i64) -> Answer<'_, ExternalMetadata> {
        Box::pin(async { Ok(None) })
    }
    fn series_by_id(&self, _id: i64) -> Answer<'_, ExternalMetadata> {
        Box::pin(async { Ok(None) })
    }
    fn search_series(&self, _name: &str, _year: Option<i32>) -> Answer<'_, ExternalMetadata> {
        Box::pin(async { Ok(None) })
    }
    fn season(&self, series_id: i64, n: i32) -> Answer<'_, SeasonMetadata> {
        self.log(format!("season {series_id}/{n}"));
        let answer = self
            .cached
            .get(&(series_id, n))
            .or_else(|| self.current.get(&(series_id, n)))
            .cloned();
        Box::pin(async move { Ok(answer) })
    }
    fn season_fresh(&self, series_id: i64, n: i32) -> Answer<'_, SeasonMetadata> {
        self.log(format!("season_fresh {series_id}/{n}"));
        let answer = self.current.get(&(series_id, n)).cloned();
        Box::pin(async move { Ok(answer) })
    }
    fn series_seasons_fresh(&self, id: i64) -> Answer<'_, SeriesSeasons> {
        self.log(format!("seasons_fresh {id}"));
        let answer = self.lists.get(&id).cloned();
        Box::pin(async move { Ok(answer) })
    }
}

const TMDB_SERIES: i64 = 1001;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2030, 6, 15).unwrap()
}

fn day(offset: i64) -> String {
    (today() + chrono::Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// A TMDb episode `n` airing `offset` days from today (None: no air date).
fn tmdb_episode(season: i32, n: i32, offset: Option<i64>) -> (i32, ExternalMetadata) {
    (
        n,
        ExternalMetadata {
            name: Some(format!("Episode Name {season}x{n}")),
            overview: Some(format!("Overview {season}x{n}")),
            premiere_date: offset.map(day),
            tmdb_id: Some(50_000 + i64::from(season) * 100 + i64::from(n)),
            image_urls: vec![MetadataImage {
                image_type: "Primary".to_string(),
                url: format!("https://image.example/still-{season}-{n}.jpg"),
            }],
            ..Default::default()
        },
    )
}

fn season_of(episodes: Vec<(i32, ExternalMetadata)>) -> SeasonMetadata {
    SeasonMetadata {
        meta: ExternalMetadata::default(),
        episodes,
    }
}

fn summary(number: i32, count: i32) -> SeasonSummary {
    SeasonSummary {
        number,
        name: Some(if number == 0 {
            "Specials".to_string()
        } else {
            format!("Season {number}")
        }),
        episode_count: Some(count),
    }
}

/// Season 1: E1 (aired 30 days ago, the library has its file), E2 (aired 20 days ago), E3 (airs
/// in 5 days), E4 (no air date). Season 2: E1 airs in 40 days. Specials: E1 aired 100 days ago.
fn mock() -> MockTmdb {
    let s1 = season_of(vec![
        tmdb_episode(1, 1, Some(-30)),
        tmdb_episode(1, 2, Some(-20)),
        tmdb_episode(1, 3, Some(5)),
        tmdb_episode(1, 4, None),
    ]);
    let s2 = season_of(vec![tmdb_episode(2, 1, Some(40))]);
    let s0 = season_of(vec![tmdb_episode(0, 1, Some(-100))]);
    let mut current = HashMap::new();
    current.insert((TMDB_SERIES, 1), s1);
    current.insert((TMDB_SERIES, 2), s2);
    current.insert((TMDB_SERIES, 0), s0);
    let mut lists = HashMap::new();
    lists.insert(
        TMDB_SERIES,
        SeriesSeasons {
            status: Some("Returning Series".to_string()),
            seasons: vec![summary(0, 1), summary(1, 4), summary(2, 1)],
        },
    );
    MockTmdb {
        lists,
        cached: current.clone(),
        current,
        ..Default::default()
    }
}

/// A temp dir, removed when the test ends.
struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    _dir: TempDir,
    pool: SqlitePool,
    library: Uuid,
    series: Uuid,
    season1: Uuid,
    episode1: Uuid,
}

async fn fixture(status: &str) -> Fixture {
    let dir = TempDir(std::env::temp_dir().join(format!("rf-missing-{}", Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let pool = rustyfin_db::init_db(dir.0.join("test.db").to_str().unwrap())
        .await
        .unwrap();
    let library = Library {
        id: Uuid::new_v4(),
        name: "TV".to_string(),
        content_type: LibraryContentType::TvShows,
        paths: vec!["/tv".to_string()],
    };
    rustyfin_db::repo_library::create_library(&pool, &library)
        .await
        .unwrap();
    let series = MediaItem {
        id: Uuid::new_v4(),
        name: "Show A".to_string(),
        media_type: MediaType::Series,
        path: "/tv/Show A".to_string(),
        library_id: library.id,
        tmdb_id: Some(TMDB_SERIES),
        status: Some(status.to_string()),
        ..MediaItem::default_dto_fields()
    };
    let season1 = MediaItem {
        id: Uuid::new_v4(),
        name: "Season 1".to_string(),
        media_type: MediaType::Season,
        path: "/tv/Show A/Season 1".to_string(),
        parent_id: Some(series.id),
        library_id: library.id,
        index_number: Some(1),
        ..MediaItem::default_dto_fields()
    };
    let episode1 = MediaItem {
        id: Uuid::new_v4(),
        name: "Show A S01E01".to_string(),
        media_type: MediaType::Episode,
        path: "/tv/Show A/Season 1/Show A S01E01.mkv".to_string(),
        parent_id: Some(season1.id),
        library_id: library.id,
        index_number: Some(1),
        parent_index_number: Some(1),
        ..MediaItem::default_dto_fields()
    };
    for item in [&series, &season1, &episode1] {
        rustyfin_db::repo_media::upsert_media_item(&pool, item)
            .await
            .unwrap();
    }
    Fixture {
        _dir: dir,
        pool,
        library: library.id,
        series: series.id,
        season1: season1.id,
        episode1: episode1.id,
    }
}

fn settings(fx: &Fixture, unaired: bool, missing: bool, specials: bool) -> Settings {
    Settings {
        import_unaired: unaired,
        import_missing: missing,
        import_specials: specials,
        enabled_libraries: vec![fx.library.simple().to_string()],
        ..Settings::default()
    }
}

async fn run_with(fx: &Fixture, tmdb: &MockTmdb, settings: &Settings) -> Outcome {
    let source = Source {
        lookups: tmdb,
        provider_name: "TMDb",
    };
    run(&fx.pool, Some(source), settings, today(), &|_| {})
        .await
        .unwrap()
}

/// (season, episode, is_virtual) of the series' episodes, sorted.
async fn slots(fx: &Fixture) -> Vec<(i32, i32, bool)> {
    let mut out: Vec<(i32, i32, bool)> = repo_virtual::series_episodes(&fx.pool, fx.series)
        .await
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                e.season_number.unwrap(),
                e.index_number.unwrap(),
                e.is_virtual,
            )
        })
        .collect();
    out.sort();
    out
}

async fn virtual_id(fx: &Fixture, season: i32, episode: i32) -> Uuid {
    repo_virtual::series_episodes(&fx.pool, fx.series)
        .await
        .unwrap()
        .into_iter()
        .find(|e| {
            e.is_virtual && e.season_number == Some(season) && e.index_number == Some(episode)
        })
        .map(|e| e.id)
        .expect("virtual episode")
}

#[test]
fn missing_episodes_settings_read_jellyfin_names_and_defaults() {
    let defaults = Settings::from_config(None);
    assert_eq!(defaults, Settings::default());
    assert!(!defaults.active());
    assert_eq!(defaults.interval_days(), 7);
    assert_eq!(defaults.grace_period_days, 7);

    let library = Uuid::new_v4();
    let config = serde_json::json!({
        "api_key": "x",
        "ImportUnairedEpisodes": true,
        "ImportMissingEpisodes": false,
        "ImportSpecials": true,
        "EnabledMissingEpisodeLibraries": [library.simple().to_string().to_uppercase()],
        "MissingEpisodeRefreshIntervalDays": 0,
        "UpcomingEpisodeGracePeriodDays": "3",
    });
    let s = Settings::from_config(Some(&config));
    assert!(s.import_unaired && !s.import_missing && s.import_specials);
    assert!(s.active());
    assert!(s.library_enabled(library), "N format, any case");
    assert!(!s.library_enabled(Uuid::new_v4()));
    assert_eq!(
        s.interval_days(),
        7,
        "a non-positive interval is the default"
    );
    assert_eq!(s.grace_period_days, 3);

    let dashed = serde_json::json!({
        "ImportMissingEpisodes": true,
        "EnabledMissingEpisodeLibraries": [library.to_string()],
    });
    assert!(Settings::from_config(Some(&dashed)).library_enabled(library));
    let no_library = serde_json::json!({ "ImportMissingEpisodes": true });
    assert!(!Settings::from_config(Some(&no_library)).active());
}

#[test]
fn missing_episodes_should_import_follows_air_date_and_options() {
    let t = today();
    let past = Some(t - chrono::Duration::days(1));
    let future = Some(t + chrono::Duration::days(1));
    // No air date: never
    assert!(!should_import_episode(None, t, true, true, false, true));
    // Unaired (today counts as unaired) needs ImportUnairedEpisodes
    assert!(should_import_episode(Some(t), t, true, false, false, false));
    assert!(!should_import_episode(
        Some(t),
        t,
        false,
        true,
        false,
        false
    ));
    assert!(should_import_episode(future, t, true, false, false, false));
    // Aired needs ImportMissingEpisodes
    assert!(should_import_episode(past, t, false, true, false, false));
    assert!(!should_import_episode(past, t, true, false, false, false));
    // Specials need ImportSpecials
    assert!(!should_import_episode(future, t, true, true, true, false));
    assert!(should_import_episode(future, t, true, true, true, true));
}

#[test]
fn missing_episodes_should_prune_specials_and_after_the_grace_period() {
    let t = today();
    let episode = |season: i32, offset: i64, is_virtual: bool, tmdb: Option<i64>| EpisodeRow {
        id: Uuid::new_v4(),
        season_id: None,
        season_number: Some(season),
        index_number: Some(1),
        index_number_end: None,
        tmdb_id: tmdb,
        premiere_date: Some(midnight(t + chrono::Duration::days(offset))),
        is_virtual,
    };
    // A real episode, or a virtual one without a TMDb id, is never pruned
    assert!(!should_prune(
        &episode(1, -100, false, Some(1)),
        true,
        t,
        7,
        false
    ));
    assert!(!should_prune(
        &episode(1, -100, true, None),
        true,
        t,
        7,
        false
    ));
    // Specials go when specials are off, whatever their date
    assert!(should_prune(
        &episode(0, 10, true, Some(1)),
        false,
        t,
        7,
        false
    ));
    assert!(!should_prune(
        &episode(0, 10, true, Some(1)),
        false,
        t,
        7,
        true
    ));
    // With missing episodes off, an episode aired more than the grace period ago goes
    assert!(!should_prune(
        &episode(1, -7, true, Some(1)),
        true,
        t,
        7,
        false
    ));
    assert!(should_prune(
        &episode(1, -8, true, Some(1)),
        true,
        t,
        7,
        false
    ));
    assert!(should_prune(
        &episode(1, -1, true, Some(1)),
        true,
        t,
        0,
        false
    ));
    assert!(!should_prune(
        &episode(1, 0, true, Some(1)),
        true,
        t,
        0,
        false
    ));
    // With missing episodes on, nothing ages out
    assert!(!should_prune(
        &episode(1, -100, true, Some(1)),
        false,
        t,
        7,
        false
    ));
}

#[test]
fn missing_episodes_ended_statuses() {
    assert!(is_ended(Some("Ended")));
    assert!(is_ended(Some("Canceled")));
    assert!(!is_ended(Some("Returning Series")));
    assert!(!is_ended(None));
}

#[tokio::test]
async fn missing_episodes_unaired_only_creates_upcoming_episodes_and_their_season() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    let outcome = run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    // S1E3 and S2E1 are unaired; S1E2 aired (missing import off); S1E4 has no date; S1E1 has a file
    assert_eq!(
        slots(&fx).await,
        vec![(1, 1, false), (1, 3, true), (2, 1, true)]
    );
    assert_eq!(outcome.created, 2);
    assert_eq!(outcome.seasons_created, 1);

    let id = virtual_id(&fx, 1, 3).await;
    let item = rustyfin_db::repo_media::get_item_by_id(&fx.pool, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.location_type, LocationType::Virtual);
    assert_eq!(item.path, "");
    assert_eq!(item.parent_id, Some(fx.season1), "into the existing season");
    assert_eq!(item.name, "Episode Name 1x3");
    assert_eq!(item.overview.as_deref(), Some("Overview 1x3"));
    assert_eq!(item.tmdb_id, Some(50_103));
    assert_eq!(
        item.premiere_date.map(|d| d.date_naive()),
        Some(today() + chrono::Duration::days(5))
    );
    let images = rustyfin_db::repo_image::get_images_for_item(&fx.pool, id)
        .await
        .unwrap();
    assert!(
        images.iter().any(|i| i.path.contains("still-1-3")),
        "the still is stored: {images:?}"
    );

    // The new season is virtual, named as TMDb names it; the real one is not virtual
    let seasons = repo_virtual::series_seasons(&fx.pool, fx.series)
        .await
        .unwrap();
    let s2 = seasons.iter().find(|s| s.index_number == Some(2)).unwrap();
    assert!(s2.is_virtual);
    assert!(
        !seasons
            .iter()
            .find(|s| s.id == fx.season1)
            .unwrap()
            .is_virtual
    );
    let s2_item = rustyfin_db::repo_media::get_item_by_id(&fx.pool, s2.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s2_item.name, "Season 2");
    assert_eq!(s2_item.location_type, LocationType::Virtual);

    // A second run creates nothing more
    let again = run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    assert_eq!(again.created, 0);
    assert_eq!(slots(&fx).await.len(), 3);
}

#[tokio::test]
async fn missing_episodes_missing_import_adds_aired_episodes_without_files() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert_eq!(
        slots(&fx).await,
        vec![(1, 1, false), (1, 2, true), (1, 3, true), (2, 1, true)]
    );
    // Missing only: the unaired ones are not made
    let fx = fixture("Returning Series").await;
    run_with(&fx, &tmdb, &settings(&fx, false, true, false)).await;
    assert_eq!(slots(&fx).await, vec![(1, 1, false), (1, 2, true)]);
}

#[tokio::test]
async fn missing_episodes_grace_period_keeps_recently_aired_placeholders() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    // Made while unaired: S1E3 airs in 5 days
    run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    let s1e3 = virtual_id(&fx, 1, 3).await;

    // 3 days after it aired it is still within the 7-day grace period
    let source = Source {
        lookups: &tmdb,
        provider_name: "TMDb",
    };
    let s = settings(&fx, true, false, false);
    let later = today() + chrono::Duration::days(8);
    run(&fx.pool, Some(source), &s, later, &|_| {})
        .await
        .unwrap();
    assert!(repo_virtual::is_virtual(&fx.pool, s1e3).await.unwrap());

    // 13 days after: pruned (missing import is off)
    let source = Source {
        lookups: &tmdb,
        provider_name: "TMDb",
    };
    let much_later = today() + chrono::Duration::days(13);
    let outcome = run(&fx.pool, Some(source), &s, much_later, &|_| {})
        .await
        .unwrap();
    assert!(outcome.pruned >= 1);
    assert!(
        rustyfin_db::repo_media::get_item_by_id(&fx.pool, s1e3)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn missing_episodes_specials_only_when_enabled_and_pruned_when_turned_off() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    // Specials off: season 0 is not even asked for
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert!(!slots(&fx).await.iter().any(|s| s.0 == 0));
    assert!(!tmdb.calls().iter().any(|c| c.ends_with("/0")));

    // Specials on: the aired special is made, in a virtual "Specials" season
    run_with(&fx, &tmdb, &settings(&fx, true, true, true)).await;
    assert!(slots(&fx).await.contains(&(0, 1, true)));
    let seasons = repo_virtual::series_seasons(&fx.pool, fx.series)
        .await
        .unwrap();
    let specials = seasons.iter().find(|s| s.index_number == Some(0)).unwrap();
    assert!(specials.is_virtual);

    // Specials off again: the special and its now empty virtual season go
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert!(!slots(&fx).await.iter().any(|s| s.0 == 0));
    let seasons = repo_virtual::series_seasons(&fx.pool, fx.series)
        .await
        .unwrap();
    assert!(!seasons.iter().any(|s| s.index_number == Some(0)));
}

#[tokio::test]
async fn missing_episodes_a_file_for_the_slot_or_the_tmdb_episode_supersedes_the_placeholder() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    let s1e2 = virtual_id(&fx, 1, 2).await;
    let s1e3 = virtual_id(&fx, 1, 3).await;

    // A file for S1E2 (stored as a new row: no scan took the placeholder over)
    let file = MediaItem {
        id: Uuid::new_v4(),
        name: "Show A S01E02".to_string(),
        media_type: MediaType::Episode,
        path: "/tv/Show A/Season 1/Show A S01E02.mkv".to_string(),
        parent_id: Some(fx.season1),
        library_id: fx.library,
        index_number: Some(2),
        parent_index_number: Some(1),
        ..MediaItem::default_dto_fields()
    };
    rustyfin_db::repo_media::upsert_media_item(&fx.pool, &file)
        .await
        .unwrap();
    // The real S1E1 file turns out to hold TMDb's S1E3 (another episode order)
    let mut first = rustyfin_db::repo_media::get_item_by_id(&fx.pool, fx.episode1)
        .await
        .unwrap()
        .unwrap();
    first.tmdb_id = Some(50_103);
    rustyfin_db::repo_media::upsert_media_item(&fx.pool, &first)
        .await
        .unwrap();

    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    for id in [s1e2, s1e3] {
        assert!(
            rustyfin_db::repo_media::get_item_by_id(&fx.pool, id)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        slots(&fx).await,
        vec![(1, 1, false), (1, 2, false), (2, 1, true)]
    );
}

#[tokio::test]
async fn missing_episodes_a_multi_episode_file_takes_every_slot_it_holds() {
    let fx = fixture("Returning Series").await;
    let mut first = rustyfin_db::repo_media::get_item_by_id(&fx.pool, fx.episode1)
        .await
        .unwrap()
        .unwrap();
    first.index_number_end = Some(2);
    rustyfin_db::repo_media::upsert_media_item(&fx.pool, &first)
        .await
        .unwrap();
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, false, true, false)).await;
    assert_eq!(slots(&fx).await, vec![(1, 1, false)]);
}

#[tokio::test]
async fn missing_episodes_library_not_opted_in_or_feature_off_removes_placeholders() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert_eq!(slots(&fx).await.len(), 4);

    // Another library opted in: this series' placeholders go, with no TMDb request
    let calls_before = tmdb.calls().len();
    let mut other = settings(&fx, true, true, false);
    other.enabled_libraries = vec![Uuid::new_v4().simple().to_string()];
    run_with(&fx, &tmdb, &other).await;
    assert_eq!(slots(&fx).await, vec![(1, 1, false)]);
    assert_eq!(tmdb.calls().len(), calls_before);
    assert_eq!(
        repo_virtual::series_seasons(&fx.pool, fx.series)
            .await
            .unwrap()
            .len(),
        1,
        "the virtual season 2 is gone"
    );

    // Both imports off: everything virtual goes
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert_eq!(slots(&fx).await.len(), 4);
    let outcome = run_with(&fx, &tmdb, &settings(&fx, false, false, false)).await;
    assert_eq!(outcome.pruned, 3);
    assert_eq!(slots(&fx).await, vec![(1, 1, false)]);
}

#[tokio::test]
async fn missing_episodes_ask_tmdb_for_the_season_list_once_and_seasons_only_when_changed() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    let calls = tmdb.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("seasons_fresh"))
            .count(),
        1
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("season_fresh")),
        "the cached seasons match the episode counts: {calls:?}"
    );

    // TMDb now lists a fifth episode in season 1: that season alone is asked again
    let mut tmdb = mock();
    let mut s1 = tmdb.current[&(TMDB_SERIES, 1)].clone();
    s1.episodes.push(tmdb_episode(1, 5, Some(12)));
    tmdb.current.insert((TMDB_SERIES, 1), s1);
    tmdb.lists.get_mut(&TMDB_SERIES).unwrap().seasons[1].episode_count = Some(5);
    run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    let fresh: Vec<String> = tmdb
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("season_fresh"))
        .collect();
    assert_eq!(fresh, vec![format!("season_fresh {TMDB_SERIES}/1")]);
    assert!(slots(&fx).await.contains(&(1, 5, true)));
}

#[tokio::test]
async fn missing_episodes_ended_series_are_skipped_and_status_is_kept_current() {
    let fx = fixture("Ended").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, true, false)).await;
    assert!(
        tmdb.calls().is_empty(),
        "an ended series is not asked about"
    );
    assert_eq!(slots(&fx).await, vec![(1, 1, false)]);

    // A running series that TMDb now lists as ended gets the new status
    let fx = fixture("Returning Series").await;
    let mut tmdb = mock();
    tmdb.lists.get_mut(&TMDB_SERIES).unwrap().status = Some("Ended".to_string());
    run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    let series = rustyfin_db::repo_media::get_item_by_id(&fx.pool, fx.series)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(series.status.as_deref(), Some("Ended"));
}

#[tokio::test]
async fn missing_episodes_update_placeholder_metadata_from_tmdb() {
    let fx = fixture("Returning Series").await;
    let tmdb = mock();
    run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    let s1e3 = virtual_id(&fx, 1, 3).await;

    // TMDb renames S1E3 and moves it a day; the cached season is replaced because the count changed
    let mut tmdb = mock();
    let mut s1 = tmdb.current[&(TMDB_SERIES, 1)].clone();
    s1.episodes[2].1.name = Some("Renamed".to_string());
    s1.episodes[2].1.premiere_date = Some(day(6));
    s1.episodes.push(tmdb_episode(1, 5, None));
    tmdb.current.insert((TMDB_SERIES, 1), s1);
    tmdb.lists.get_mut(&TMDB_SERIES).unwrap().seasons[1].episode_count = Some(5);
    let outcome = run_with(&fx, &tmdb, &settings(&fx, true, false, false)).await;
    assert_eq!(outcome.updated, 1);
    let item = rustyfin_db::repo_media::get_item_by_id(&fx.pool, s1e3)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.name, "Renamed");
    assert_eq!(
        item.premiere_date.map(|d| d.date_naive()),
        Some(today() + chrono::Duration::days(6))
    );
}
