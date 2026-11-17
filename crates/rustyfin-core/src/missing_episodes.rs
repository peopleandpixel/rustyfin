//! Jellyfin 12.4's missing and upcoming episodes: the "Refresh upcoming and missing episodes
//! (TheMovieDb)" task (`TmdbUpcomingEpisodesTask.cs`) and the provider it drives
//! (`TmdbMissingEpisodeProvider.cs`). For each series with a TMDb id it creates virtual episodes
//! (and the virtual seasons that hold them) for the episodes TMDb lists that the library has no
//! file for, and prunes the ones no longer wanted, as the TMDb plugin's settings say.
//!
//! API use: the series' season list is asked of TMDb once a run, for the series that have not
//! ended (the cached answer is replaced); a season's episode list comes from the cache unless the
//! new season list gives the season another episode count than the cached one has.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, Utc};
use rustyfin_db::repo_virtual::{self, EpisodeRow, SeriesRow};
use rustyfin_model::media::{LocationType, MediaItem, MediaType};
use rustyfin_model::metadata::{MetadataLookups, SeasonMetadata, SeasonSummary};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Jellyfin's task key, which this task also uses as its id.
pub const TASK_KEY: &str = "TmdbRefreshUpcomingEpisodes";
pub const TASK_NAME: &str = "Refresh upcoming and missing episodes (TheMovieDb)";
pub const TASK_DESCRIPTION: &str = "Checks TheMovieDb for newly announced episodes and creates virtual entries for unaired and missing episodes, according to the TMDb plugin settings. When both options are disabled, removes any virtual entries previously created.";
/// The TMDb plugin's id, whose configuration holds the settings.
pub const TMDB_PLUGIN_ID: &str = "tmdb";
/// Jellyfin's default `MissingEpisodeRefreshIntervalDays` and `UpcomingEpisodeGracePeriodDays`.
pub const DEFAULT_INTERVAL_DAYS: i64 = 7;
pub const DEFAULT_GRACE_PERIOD_DAYS: i64 = 7;

/// The TMDb plugin's settings for virtual episodes (Jellyfin 12.4 `PluginConfiguration.cs`), under
/// Jellyfin's names in the plugin's configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// `ImportUnairedEpisodes`
    pub import_unaired: bool,
    /// `ImportMissingEpisodes`
    pub import_missing: bool,
    /// `ImportSpecials`
    pub import_specials: bool,
    /// `EnabledMissingEpisodeLibraries`: library ids; none turns the feature off
    pub enabled_libraries: Vec<String>,
    /// `MissingEpisodeRefreshIntervalDays`
    pub refresh_interval_days: i64,
    /// `UpcomingEpisodeGracePeriodDays`
    pub grace_period_days: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            import_unaired: false,
            import_missing: false,
            import_specials: false,
            enabled_libraries: Vec::new(),
            refresh_interval_days: DEFAULT_INTERVAL_DAYS,
            grace_period_days: DEFAULT_GRACE_PERIOD_DAYS,
        }
    }
}

impl Settings {
    /// The settings in the TMDb plugin's configuration; a missing or unreadable value is the default.
    pub fn from_config(values: Option<&serde_json::Value>) -> Settings {
        let mut settings = Settings::default();
        let Some(values) = values else {
            return settings;
        };
        let flag = |name: &str| values.get(name).and_then(serde_json::Value::as_bool);
        let days = |name: &str| {
            values.get(name).and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            })
        };
        settings.import_unaired = flag("ImportUnairedEpisodes").unwrap_or(false);
        settings.import_missing = flag("ImportMissingEpisodes").unwrap_or(false);
        settings.import_specials = flag("ImportSpecials").unwrap_or(false);
        settings.enabled_libraries = values
            .get("EnabledMissingEpisodeLibraries")
            .and_then(serde_json::Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(d) = days("MissingEpisodeRefreshIntervalDays") {
            settings.refresh_interval_days = d;
        }
        if let Some(d) = days("UpcomingEpisodeGracePeriodDays") {
            settings.grace_period_days = d;
        }
        settings
    }

    /// Whether the task makes virtual episodes at all: an import is on and a library is opted in.
    pub fn active(&self) -> bool {
        (self.import_unaired || self.import_missing) && !self.enabled_libraries.is_empty()
    }

    /// Whether the library is opted in. Ids compare in any GUID format ("N" or with dashes).
    pub fn library_enabled(&self, library_id: Uuid) -> bool {
        let wanted = library_id.simple().to_string();
        self.enabled_libraries
            .iter()
            .any(|id| id.replace('-', "").eq_ignore_ascii_case(&wanted))
    }

    /// The task's default interval: `MissingEpisodeRefreshIntervalDays`, or 7 when it is not positive.
    pub fn interval_days(&self) -> i64 {
        if self.refresh_interval_days <= 0 {
            DEFAULT_INTERVAL_DAYS
        } else {
            self.refresh_interval_days
        }
    }
}

/// Whether TMDb's status means the series has ended (Jellyfin's `SeriesStatus.Ended`: TMDb's
/// "Ended" and "Canceled").
pub fn is_ended(status: Option<&str>) -> bool {
    matches!(
        status.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("ended" | "canceled" | "cancelled")
    )
}

/// Whether a TMDb episode becomes a virtual episode (12.4 `ShouldImportEpisode`): never when it has
/// no air date, a special only with `import_specials`, an unaired one (airing today or later) with
/// `import_unaired`, an aired one with `import_missing`.
pub fn should_import_episode(
    premiere: Option<NaiveDate>,
    today: NaiveDate,
    import_unaired: bool,
    import_missing: bool,
    is_special: bool,
    import_specials: bool,
) -> bool {
    let Some(premiere) = premiere else {
        return false;
    };
    if is_special && !import_specials {
        return false;
    }
    if premiere >= today {
        import_unaired
    } else {
        import_missing
    }
}

/// Whether a stored virtual episode with a TMDb id is pruned (12.4 `ShouldPrune`): a special when
/// specials are off; otherwise, with `prune_aged_out` (missing episodes off), one that aired more
/// than `grace_days` ago.
pub fn should_prune(
    episode: &EpisodeRow,
    prune_aged_out: bool,
    today: NaiveDate,
    grace_days: i64,
    import_specials: bool,
) -> bool {
    if !episode.is_virtual || episode.tmdb_id.is_none() {
        return false;
    }
    if episode.season_number == Some(0) && !import_specials {
        return true;
    }
    prune_aged_out
        && episode.premiere_date.is_some_and(|premiere| {
            premiere.date_naive() < today - chrono::Duration::days(grace_days.max(0))
        })
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Outcome {
    pub series: usize,
    pub created: usize,
    pub updated: usize,
    pub pruned: usize,
    pub seasons_created: usize,
    pub seasons_removed: usize,
    /// Series whose TMDb lookups failed; they are tried again on the next run
    pub failed: usize,
}

impl Outcome {
    fn add(&mut self, other: Outcome) {
        self.series += other.series;
        self.created += other.created;
        self.updated += other.updated;
        self.pruned += other.pruned;
        self.seasons_created += other.seasons_created;
        self.seasons_removed += other.seasons_removed;
        self.failed += other.failed;
    }
}

/// The TMDb lookups a run uses, with the provider name its images are filed under.
pub struct Source<'a> {
    pub lookups: &'a dyn MetadataLookups,
    pub provider_name: &'a str,
}

/// Runs the task (12.4 `TmdbUpcomingEpisodesTask.ExecuteAsync`). With the feature off (both
/// imports off, or no library opted in) it removes every virtual episode it made, and the virtual
/// seasons left empty. Otherwise it refreshes each series with a TMDb id that has not ended, and
/// each series in a library not opted in (to prune what it made there). `progress` gets the share done.
pub async fn run(
    pool: &SqlitePool,
    source: Option<Source<'_>>,
    settings: &Settings,
    today: NaiveDate,
    progress: &(dyn Fn(f64) + Sync),
) -> Result<Outcome, String> {
    let mut outcome = Outcome::default();
    if !settings.active() {
        outcome.add(remove_all(pool).await.map_err(|e| e.to_string())?);
        progress(100.0);
        return Ok(outcome);
    }
    let series: Vec<SeriesRow> = repo_virtual::series_with_tmdb_id(pool)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| !is_ended(s.status.as_deref()) || !settings.library_enabled(s.library_id))
        .collect();
    let total = series.len();
    for (done, row) in series.iter().enumerate() {
        match refresh_series(pool, source.as_ref(), row, settings, today).await {
            Ok(one) => outcome.add(one),
            Err(e) => {
                tracing::error!(
                    "error refreshing upcoming episodes for series {}: {e}",
                    row.name
                );
                outcome.failed += 1;
            }
        }
        progress(100.0 * (done + 1) as f64 / total.max(1) as f64);
    }
    progress(100.0);
    Ok(outcome)
}

/// Removes every virtual episode with a TMDb id, then the virtual seasons left without episodes.
pub async fn remove_all(pool: &SqlitePool) -> Result<Outcome, sqlx::Error> {
    let _scan = crate::scanner::SCAN_LOCK.lock().await;
    let mut outcome = Outcome::default();
    for id in repo_virtual::all_virtual_episodes(pool).await? {
        if repo_virtual::delete_virtual(pool, id).await? {
            outcome.pruned += 1;
        }
    }
    for id in repo_virtual::empty_virtual_seasons(pool, None).await? {
        if repo_virtual::delete_virtual(pool, id).await? {
            outcome.seasons_removed += 1;
        }
    }
    if outcome.pruned > 0 {
        tracing::info!(
            "removed {} virtual episodes: the TMDb missing episode provider is disabled",
            outcome.pruned
        );
    }
    Ok(outcome)
}

/// The TMDb answers for one series: its season list and each wanted season's episodes.
struct Fetched {
    status: Option<String>,
    seasons: Vec<(SeasonSummary, SeasonMetadata)>,
}

/// Asks TMDb for a series' season list (fresh), then each season's episode list: from the cache,
/// unless the season list gives the season another episode count. A season of specials is not
/// asked for when specials are off. `Ok(None)` is no answer (a 404); `Err` a failed lookup.
async fn fetch(
    source: &Source<'_>,
    tmdb_id: i64,
    settings: &Settings,
) -> Result<Option<Fetched>, String> {
    let list = source
        .lookups
        .series_seasons_fresh(tmdb_id)
        .await
        .map_err(|e| format!("season list: {e:?}"))?;
    let Some(list) = list else {
        return Ok(None);
    };
    let mut seasons = Vec::new();
    for summary in list.seasons {
        if summary.number == 0 && !settings.import_specials {
            continue;
        }
        let cached = source
            .lookups
            .season(tmdb_id, summary.number)
            .await
            .map_err(|e| format!("season {}: {e:?}", summary.number))?;
        let stale = match (&cached, summary.episode_count) {
            (Some(season), Some(count)) => season.episodes.len() != count.max(0) as usize,
            (None, Some(count)) => count > 0,
            (_, None) => false,
        };
        let season = if stale {
            source
                .lookups
                .season_fresh(tmdb_id, summary.number)
                .await
                .map_err(|e| format!("season {}: {e:?}", summary.number))?
        } else {
            cached
        };
        if let Some(season) = season {
            seasons.push((summary, season));
        }
    }
    Ok(Some(Fetched {
        status: list.status,
        seasons,
    }))
}

fn air_date(meta: &rustyfin_model::metadata::ExternalMetadata) -> Option<NaiveDate> {
    let text = meta.premiere_date.as_deref()?;
    NaiveDate::parse_from_str(text.get(..10)?, "%Y-%m-%d").ok()
}

fn midnight(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is valid")
        .and_utc()
}

/// Refreshes one series (12.4 `TmdbMissingEpisodeProvider.FetchAsync`).
pub async fn refresh_series(
    pool: &SqlitePool,
    source: Option<&Source<'_>>,
    series: &SeriesRow,
    settings: &Settings,
    today: NaiveDate,
) -> Result<Outcome, String> {
    let mut outcome = Outcome {
        series: 1,
        ..Outcome::default()
    };
    let db = |e: sqlx::Error| e.to_string();

    // Inactive for this series: remove every virtual episode it made here.
    if !settings.active() || !settings.library_enabled(series.library_id) {
        let _scan = crate::scanner::SCAN_LOCK.lock().await;
        for episode in repo_virtual::series_episodes(pool, series.id)
            .await
            .map_err(db)?
        {
            if episode.is_virtual
                && episode.tmdb_id.is_some()
                && repo_virtual::delete_virtual(pool, episode.id)
                    .await
                    .map_err(db)?
            {
                outcome.pruned += 1;
            }
        }
        outcome.seasons_removed += remove_empty_seasons(pool, series.id).await.map_err(db)?;
        return Ok(outcome);
    }
    let Some(source) = source else {
        return Ok(outcome);
    };
    let Some(fetched) = fetch(source, series.tmdb_id, settings).await? else {
        return Ok(outcome);
    };

    // The writes run under the scan lock, so that a scan never sees half of them.
    let _scan = crate::scanner::SCAN_LOCK.lock().await;
    if let Some(status) = fetched.status.as_deref()
        && series.status.as_deref() != Some(status)
        && !rustyfin_db::repo_media::item_locks(pool, series.id)
            .await
            .map_err(db)?
            .locked("Status")
    {
        repo_virtual::set_series_status(pool, series.id, status)
            .await
            .map_err(db)?;
    }

    // Existing episodes: prune, then collect the slots and TMDb ids that are taken.
    let episodes = repo_virtual::series_episodes(pool, series.id)
        .await
        .map_err(db)?;
    let mut taken: HashSet<(i32, i32)> = HashSet::new();
    let mut physical_slots: HashSet<(i32, i32)> = HashSet::new();
    let mut physical_tmdb_ids: HashSet<i64> = HashSet::new();
    let mut ours: Vec<((i32, i32), EpisodeRow)> = Vec::new();
    for episode in episodes {
        if should_prune(
            &episode,
            !settings.import_missing,
            today,
            settings.grace_period_days,
            settings.import_specials,
        ) {
            if repo_virtual::delete_virtual(pool, episode.id)
                .await
                .map_err(db)?
            {
                outcome.pruned += 1;
            }
            continue;
        }
        let (Some(season), Some(index)) = (episode.season_number, episode.index_number) else {
            if !episode.is_virtual
                && let Some(id) = episode.tmdb_id
            {
                physical_tmdb_ids.insert(id);
            }
            continue;
        };
        if episode.is_virtual {
            if episode.tmdb_id.is_some() {
                taken.insert((season, index));
                ours.push(((season, index), episode));
            }
            continue;
        }
        if let Some(id) = episode.tmdb_id {
            physical_tmdb_ids.insert(id);
        }
        // A file holding several episodes takes each of their slots
        let end = episode
            .index_number_end
            .filter(|e| *e > index)
            .unwrap_or(index);
        for n in index..=end {
            taken.insert((season, n));
            physical_slots.insert((season, n));
        }
    }
    // A virtual episode whose slot or TMDb episode now has a file is superseded
    let mut updatable: HashMap<(i32, i32), Uuid> = HashMap::new();
    for (key, episode) in ours {
        let superseded = physical_slots.contains(&key)
            || episode
                .tmdb_id
                .is_some_and(|id| physical_tmdb_ids.contains(&id));
        if superseded {
            if repo_virtual::delete_virtual(pool, episode.id)
                .await
                .map_err(db)?
            {
                outcome.pruned += 1;
            }
        } else {
            updatable.insert(key, episode.id);
        }
    }

    let mut seasons_by_number: HashMap<i32, Uuid> = repo_virtual::series_seasons(pool, series.id)
        .await
        .map_err(db)?
        .into_iter()
        .filter_map(|s| Some((s.index_number?, s.id)))
        .collect();

    for (summary, season) in &fetched.seasons {
        let number = summary.number;
        for (episode_number, meta) in &season.episodes {
            let premiere = air_date(meta);
            if !should_import_episode(
                premiere,
                today,
                settings.import_unaired,
                settings.import_missing,
                number == 0,
                settings.import_specials,
            ) {
                continue;
            }
            let key = (number, *episode_number);
            if let Some(&id) = updatable.get(&key) {
                if update_virtual_episode(pool, id, season, source.provider_name).await? {
                    outcome.updated += 1;
                }
                continue;
            }
            if !taken.insert(key) {
                continue;
            }
            if meta
                .tmdb_id
                .is_some_and(|id| physical_tmdb_ids.contains(&id))
            {
                continue;
            }
            let season_id = match seasons_by_number.get(&number) {
                Some(id) => *id,
                None => {
                    let id = create_season(pool, series, summary).await.map_err(db)?;
                    outcome.seasons_created += 1;
                    seasons_by_number.insert(number, id);
                    id
                }
            };
            create_episode(
                pool,
                series,
                season_id,
                number,
                *episode_number,
                premiere,
                season,
                source.provider_name,
            )
            .await?;
            outcome.created += 1;
        }
    }

    repo_virtual::settle_virtual_seasons(pool, series.id)
        .await
        .map_err(db)?;
    outcome.seasons_removed += remove_empty_seasons(pool, series.id).await.map_err(db)?;
    if outcome.created + outcome.pruned > 0 {
        tracing::info!(
            "series {}: created {} and removed {} virtual episodes",
            series.name,
            outcome.created,
            outcome.pruned
        );
    }
    Ok(outcome)
}

async fn remove_empty_seasons(pool: &SqlitePool, series_id: Uuid) -> Result<usize, sqlx::Error> {
    let mut removed = 0;
    for id in repo_virtual::empty_virtual_seasons(pool, Some(series_id)).await? {
        if repo_virtual::delete_virtual(pool, id).await? {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Creates a virtual season for a season the library has nothing of, named as TMDb names it.
async fn create_season(
    pool: &SqlitePool,
    series: &SeriesRow,
    summary: &SeasonSummary,
) -> Result<Uuid, sqlx::Error> {
    let number = summary.number;
    let name = summary.name.clone().unwrap_or_else(|| {
        if number == 0 {
            "Specials".to_string()
        } else {
            format!("Season {number}")
        }
    });
    let item = MediaItem {
        id: Uuid::new_v4(),
        name,
        sort_name: Some(rustyfin_model::sort_name::season_sort_name(Some(number))),
        media_type: MediaType::Season,
        path: String::new(),
        parent_id: Some(series.id),
        library_id: series.library_id,
        date_created: Utc::now(),
        index_number: Some(number),
        location_type: LocationType::Virtual,
        ..MediaItem::default_dto_fields()
    };
    rustyfin_db::repo_media::upsert_media_item(pool, &item).await?;
    repo_virtual::mark_virtual(pool, &[item.id]).await?;
    tracing::info!(
        "creating virtual season {number} for series {}",
        series.name
    );
    Ok(item.id)
}

/// Creates a virtual episode (no path, `LocationType` Virtual) from TMDb's listing, with its name,
/// overview, air date, still and people.
#[allow(clippy::too_many_arguments)]
async fn create_episode(
    pool: &SqlitePool,
    series: &SeriesRow,
    season_id: Uuid,
    season_number: i32,
    episode_number: i32,
    premiere: Option<NaiveDate>,
    season: &SeasonMetadata,
    provider: &str,
) -> Result<Uuid, String> {
    let meta = season
        .episodes
        .iter()
        .find(|(n, _)| *n == episode_number)
        .map(|(_, m)| m);
    let name = meta
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| format!("Episode {episode_number}"));
    let mut item = MediaItem {
        id: Uuid::new_v4(),
        sort_name: crate::scanner::auto_sort_name(
            MediaType::Episode,
            &name,
            Some(season_number),
            Some(episode_number),
        ),
        name,
        media_type: MediaType::Episode,
        path: String::new(),
        parent_id: Some(season_id),
        library_id: series.library_id,
        date_created: Utc::now(),
        premiere_date: premiere.map(midnight),
        production_year: premiere.map(|d| chrono::Datelike::year(&d)),
        index_number: Some(episode_number),
        parent_index_number: Some(season_number),
        tmdb_id: meta.and_then(|m| m.tmdb_id),
        location_type: LocationType::Virtual,
        ..MediaItem::default_dto_fields()
    };
    rustyfin_db::repo_media::upsert_media_item(pool, &item)
        .await
        .map_err(|e| e.to_string())?;
    repo_virtual::mark_virtual(pool, &[item.id])
        .await
        .map_err(|e| e.to_string())?;
    crate::metadata::apply_virtual_episode(pool, &mut item, season, provider).await?;
    tracing::info!(
        "creating virtual episode S{season_number}E{episode_number} for series {}",
        series.name
    );
    Ok(item.id)
}

/// Brings a virtual episode in line with TMDb's listing. Returns whether it changed.
async fn update_virtual_episode(
    pool: &SqlitePool,
    id: Uuid,
    season: &SeasonMetadata,
    provider: &str,
) -> Result<bool, String> {
    let Some(mut item) = rustyfin_db::repo_media::get_item_by_id(pool, id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(false);
    };
    let before = (item.name.clone(), item.overview.clone(), item.premiere_date);
    crate::metadata::apply_virtual_episode(pool, &mut item, season, provider).await?;
    Ok(before != (item.name.clone(), item.overview.clone(), item.premiere_date))
}

#[cfg(test)]
mod tests;
