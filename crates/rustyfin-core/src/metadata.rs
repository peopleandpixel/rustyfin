use rustyfin_model::image::{ImageType, ItemImage};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use std::path::Path;
use uuid::Uuid;

use crate::local_images;
use crate::nfo_parser;
use crate::plugin_manager::{MetadataProviderHandle, PluginManager};
use rustyfin_model::metadata::{LookupFailed, SeasonMetadata};

/// How a metadata refresh may ask the providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshOptions {
    /// Search providers by name for an item they can't look up by id. Off for the "Refresh
    /// metadata" task, which never searches or renames.
    pub search: bool,
    /// Look a season or episode up through its season. Off for the task: a series' refresh fills
    /// the seasons and episodes that need it.
    pub season_lookups: bool,
    /// Ask again about a season or episode that a provider already answered for.
    pub force: bool,
    /// Jellyfin's ReplaceAllMetadata: clear what the sources fill (except locked fields) first, so
    /// that they fill it again instead of only the missing fields.
    pub replace_metadata: bool,
    /// Jellyfin's ReplaceAllImages: drop the providers' and the admin's images first, so that
    /// the providers' current ones replace them. Images next to the media are found again.
    pub replace_images: bool,
}

impl RefreshOptions {
    /// A refresh asked for through the API: everything, again.
    pub const EXPLICIT: RefreshOptions = RefreshOptions {
        search: true,
        season_lookups: true,
        force: true,
        replace_metadata: false,
        replace_images: false,
    };
    /// A scan's refresh of the items it added.
    pub const SCAN: RefreshOptions = RefreshOptions {
        search: true,
        season_lookups: true,
        force: false,
        replace_metadata: false,
        replace_images: false,
    };
    /// The "Refresh metadata" task: by id only.
    pub const TASK: RefreshOptions = RefreshOptions {
        search: false,
        season_lookups: false,
        force: false,
        replace_metadata: false,
        replace_images: false,
    };
}

/// Refresh metadata for a single item: local images, NFO, then optionally remote providers
pub async fn refresh_item_metadata(
    pool: &SqlitePool,
    client: &reqwest::Client,
    item: &MediaItem,
    plugin_manager: &PluginManager,
) -> Result<(), String> {
    refresh_item_metadata_with(pool, client, item, plugin_manager, RefreshOptions::EXPLICIT)
        .await
        .map(|_| ())
}

/// `refresh_item_metadata` with the given options.
pub async fn refresh_item_metadata_with(
    pool: &SqlitePool,
    client: &reqwest::Client,
    item: &MediaItem,
    plugin_manager: &PluginManager,
    options: RefreshOptions,
) -> Result<RefreshOutcome, String> {
    let media_path = Path::new(&item.path);

    // A "replace all" clears only once the provider has answered for the item: a failed lookup,
    // a missing key or no match leaves the item as it was. The answer is cached, so the
    // refresh below asks nothing again.
    let replacing = (options.replace_metadata || options.replace_images)
        && replacement_answered(client, item, &plugin_manager.metadata_providers().await).await;
    if (options.replace_metadata || options.replace_images) && !replacing {
        tracing::warn!(
            "no provider answered for '{}', so its metadata and images are kept, not replaced",
            item.name
        );
    }
    if options.replace_images && replacing {
        rustyfin_db::repo_image::delete_replaceable_images(pool, item.id)
            .await
            .map_err(|e| e.to_string())?;
    }
    // Each source fills this copy, so a later source keeps what an earlier one filled
    let mut working = item.clone();
    if options.replace_metadata && replacing {
        let locks = rustyfin_db::repo_media::item_locks(pool, item.id)
            .await
            .map_err(|e| e.to_string())?;
        clear_unlocked_metadata(&mut working, &locks);
        rustyfin_db::repo_media::save_refresh_changes(pool, item, &working)
            .await
            .map_err(|e| e.to_string())?;
        // Jellyfin's replace takes the sources' ForcedSortName too, none unless an NFO has one
        if !locks.forced_sort_name_locked() {
            rustyfin_db::repo_media::set_forced_sort_name(pool, item.id, None)
                .await
                .map_err(|e| e.to_string())?;
        }
        rustyfin_db::repo_media::clear_replaceable_links(pool, item.id, &locks)
            .await
            .map_err(|e| e.to_string())?;
    }

    // 1. Scan for local images and compute blurhash for each. An image whose file is unchanged
    // since its row was stored (same path and stamp) keeps the stored hash, so it isn't read again.
    let mut local_images = local_images::find_local_images(media_path, item.id);
    let stored = if local_images.is_empty() {
        Vec::new()
    } else {
        rustyfin_db::repo_image::get_images_for_item(pool, item.id)
            .await
            .unwrap_or_default()
    };
    for img in &mut local_images {
        if img.blurhash.is_none()
            && img.stamp.is_some()
            && let Some(known) = stored
                .iter()
                .find(|s| s.path == img.path && s.stamp == img.stamp && s.blurhash.is_some())
        {
            img.blurhash = known.blurhash.clone();
        }
        if img.blurhash.is_none()
            && let Some(bh) = crate::image_proxy::compute_blurhash(&img.path, Some(client)).await
        {
            img.blurhash = Some(bh);
        }
    }
    if !local_images.is_empty() {
        rustyfin_db::repo_image::batch_upsert_images(pool, &local_images)
            .await
            .map_err(|e| e.to_string())?;
    }
    tracing::debug!(
        "found {} local images for '{}'",
        local_images.len(),
        item.name
    );

    // 2. Parse NFO file if present. Its title is the user's, so a provider never replaces it.
    let name_locked = rustyfin_db::repo_media::is_field_locked(pool, item.id, "Name")
        .await
        .unwrap_or(false);
    let mut nfo_named = false;
    let nfo_path = match item.media_type {
        MediaType::Series => nfo_parser::find_series_nfo(media_path),
        MediaType::MusicAlbum => nfo_parser::find_folder_nfo(media_path, "album.nfo"),
        MediaType::MusicArtist => nfo_parser::find_folder_nfo(media_path, "artist.nfo"),
        _ => nfo_parser::find_nfo_for_media(media_path),
    };
    if let Some(nfo_path) = nfo_path
        && let Some(meta) = nfo_parser::parse_nfo(&nfo_path).await
    {
        nfo_named = meta.name.as_deref().is_some_and(|n| !n.trim().is_empty());
        // Update media item metadata from NFO
        apply_metadata_to_item(pool, &mut working, &meta, false).await?;

        // Batch-store image URLs from NFO, skipping types that already exist
        store_images_batch(pool, item.id, &item.path, &meta.image_urls, "nfo").await?;
    }
    let replace_name = !name_locked && !nfo_named;

    // 3. Use registered metadata providers (TMDb, OMDb, Fanart, etc.)
    // Only active/enabled providers are returned; if none are enabled, this is a fast scan
    // that relies solely on local images and NFO files.
    let providers = plugin_manager.metadata_providers().await;
    if providers.is_empty() {
        tracing::debug!(
            "no active metadata providers for '{}' (fast scan mode)",
            item.name
        );
        return Ok(RefreshOutcome::Done);
    }

    let mut outcome = RefreshOutcome::Done;
    match item.media_type {
        MediaType::Movie | MediaType::Series => {
            let asked = refresh_from_providers(
                pool,
                client,
                &mut working,
                &providers,
                replace_name,
                options.search,
                item.production_year,
            )
            .await?;
            // A series counts as answered only once its seasons have answered too, since the
            // task reaches them only through it. After its own lookup failed, its seasons
            // aren't asked for in this run either.
            let mut children = ChildrenAnswer {
                all_answered: true,
                unauthorized: false,
            };
            fill_from_imdb(pool, &mut working, &providers).await?;
            if item.media_type == MediaType::Series && working.tmdb_id.is_some() && !asked.failed {
                children =
                    refresh_series_children(pool, &working, &providers, options.force).await?;
            }
            if asked.unauthorized || children.unauthorized {
                outcome = RefreshOutcome::Unauthorized;
            }
            // Only a final answer counts; after a failure the next run asks again
            if asked.answered && !asked.failed && children.all_answered {
                rustyfin_db::repo_media::mark_provider_refreshed(pool, &[item.id])
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        MediaType::Season | MediaType::Episode if options.season_lookups => {
            let answered = !options.force
                && rustyfin_db::repo_media::provider_refreshed_ids(pool, &[item.id])
                    .await
                    .map_err(|e| e.to_string())?
                    .contains(&item.id);
            if !answered {
                refresh_from_season(pool, &mut working, &providers, replace_name).await?;
            }
        }
        _ => {}
    }

    Ok(outcome)
}

/// Clears the fields that a "replace all metadata" refresh fills again, except the locked ones.
/// The name, the provider ids, the runtime (the file's) and the tags (the admin's) stay.
fn clear_unlocked_metadata(item: &mut MediaItem, locks: &rustyfin_db::repo_media::ItemLocks) {
    if locks.lock_data {
        return;
    }
    if !locks.locked("Overview") {
        item.overview = None;
        item.taglines.clear();
    }
    if !locks.locked("Genres") {
        item.genres.clear();
    }
    if !locks.locked("OfficialRating") {
        item.official_rating = None;
    }
    if !locks.locked("ProductionLocations") {
        item.production_locations.clear();
    }
    item.community_rating = None;
    item.premiere_date = None;
    item.production_year = None;
    item.end_date = None;
    item.status = None;
    item.original_language = None;
    item.budget = None;
    item.revenue = None;
    item.homepage = None;
    item.vote_count = None;
    item.popularity = None;
}

/// The names a provider search tries, in order: the parsed name, then the file name without
/// release tags.
fn search_candidates(item: &MediaItem) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let parsed = item.name.trim();
    if !parsed.is_empty() {
        candidates.push(parsed.to_string());
    }
    let cleaned = clean_filename_for_search(&item.path);
    if !cleaned.is_empty() && !candidates.contains(&cleaned) {
        candidates.push(cleaned);
    }
    candidates
}

/// Whether the first provider with lookups (TMDb) answers for a movie or series: by its id, or
/// else by a search for its names, as `refresh_from_providers` asks. Other types, and an item no
/// such provider answers for (a failure, no key, no match), are never replaced.
async fn replacement_answered(
    client: &reqwest::Client,
    item: &MediaItem,
    providers: &[MetadataProviderHandle],
) -> bool {
    let Some(provider) = providers.iter().find(|p| p.lookups.is_some()) else {
        return false;
    };
    let Some(lookups) = &provider.lookups else {
        return false;
    };
    let series = match item.media_type {
        MediaType::Movie => false,
        MediaType::Series => true,
        _ => return false,
    };
    if let Some(id) = item.tmdb_id {
        let found = if series {
            lookups.series_by_id(id).await
        } else {
            lookups.movie_by_id(id).await
        };
        return matches!(found, Ok(Some(_)));
    }
    for candidate in search_candidates(item) {
        let found = if series {
            matches!(
                lookups
                    .search_series(&candidate, item.production_year)
                    .await,
                Ok(Some(_))
            )
        } else {
            (provider.search_movie)(client, &candidate, item.production_year)
                .await
                .is_some()
        };
        if found {
            return true;
        }
    }
    false
}

/// How a refresh ended for the caller that runs many of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Done, whatever the providers answered
    Done,
    /// A provider refused its API key (or has none): every other lookup in this run would fail
    /// the same way, so a run over many items can stop
    Unauthorized,
}

/// What `refresh_from_providers` got from the providers with lookups.
struct ProvidersAnswer {
    /// One gave a final answer (found, or a 404 for the id)
    answered: bool,
    /// One failed
    failed: bool,
    /// One failed for its API key
    unauthorized: bool,
}

/// What `refresh_series_children` got.
struct ChildrenAnswer {
    /// Every season asked for gave a final answer
    all_answered: bool,
    /// A season lookup failed for the provider's API key
    unauthorized: bool,
}

/// Identify a movie or series with each provider in turn and apply what it finds. A provider
/// with lookups (TMDb) fetches an item it already knows by id and never searches for it, even
/// when the id is gone from TMDb (the item stays as it is, as in Jellyfin); a series search sends
/// `year` (the folder's, from before a "replace all" cleared it). Without `search`, no provider is
/// searched by name. Returns what the providers with lookups answered.
async fn refresh_from_providers(
    pool: &SqlitePool,
    client: &reqwest::Client,
    working: &mut MediaItem,
    providers: &[MetadataProviderHandle],
    replace_name: bool,
    search: bool,
    year: Option<i32>,
) -> Result<ProvidersAnswer, String> {
    let mut answered = false;
    let mut failed = false;
    let mut unauthorized = false;
    // Candidate names in search order. Jellyfin searches the parsed name first; release
    // tags left in the file name (e.g. "Bluray-1080p") make TMDb find nothing.
    let candidates = search_candidates(working);
    let media_type = working.media_type;

    for provider in providers {
        // A provider with IMDb lookups (OMDb) fills after the others, in `fill_from_imdb`
        if provider.imdb_lookups.is_some() {
            continue;
        }
        let mut meta = None;
        let by_id = provider.lookups.is_some() && working.tmdb_id.is_some();
        if let (Some(lookups), Some(id)) = (&provider.lookups, working.tmdb_id) {
            let result = match media_type {
                MediaType::Series => lookups.series_by_id(id).await,
                _ => lookups.movie_by_id(id).await,
            };
            match result {
                Ok(found) => {
                    answered = true;
                    meta = found;
                }
                Err(e) => {
                    failed = true;
                    unauthorized |= e == LookupFailed::Unauthorized;
                }
            }
            tracing::debug!(
                "metadata lookup of id {} via {}: {}",
                id,
                provider.provider_name,
                if meta.is_some() { "found" } else { "no match" }
            );
        }
        // Search for metadata, stopping at the first candidate name that matches
        if meta.is_none() && search && !by_id {
            for candidate in &candidates {
                let found = match (media_type, &provider.lookups) {
                    (MediaType::Series, Some(lookups)) => {
                        match lookups.search_series(candidate, year).await {
                            Ok(found) => found,
                            Err(e) => {
                                failed = true;
                                unauthorized |= e == LookupFailed::Unauthorized;
                                None
                            }
                        }
                    }
                    (MediaType::Series, None) => (provider.search_tv)(client, candidate).await,
                    _ => (provider.search_movie)(client, candidate, year).await,
                };
                tracing::debug!(
                    "metadata search '{}' via {}: {}",
                    candidate,
                    provider.provider_name,
                    if found.is_some() { "found" } else { "no match" }
                );
                if found.is_some() {
                    // A search's match is an answer from a provider with lookups
                    answered |= provider.lookups.is_some();
                    meta = found;
                    break;
                }
            }
        }

        // A provider with lookups gives its images with the details it found. When it found
        // nothing (a failure, no key, a 404, no match), asking for images would only repeat
        // that lookup.
        let images_from_lookup = provider.lookups.is_some();
        let found = meta.is_some();
        if let Some(meta) = meta {
            apply_metadata_to_item(pool, working, &meta, replace_name).await?;
            store_images_batch(
                pool,
                working.id,
                &working.path,
                &meta.image_urls,
                &provider.provider_name,
            )
            .await?;
        }

        if images_from_lookup && !found {
            continue;
        }
        // Fetch additional images from this provider
        let images =
            (provider.get_images)(client, working.tmdb_id, working.tvdb_id, media_type).await;
        if !images.is_empty() {
            store_images_batch(
                pool,
                working.id,
                &working.path,
                &images,
                &provider.provider_name,
            )
            .await?;
        }
    }
    Ok(ProvidersAnswer {
        answered,
        failed,
        unauthorized,
    })
}

/// Forgets the providers' cached answers about a movie or series (a series' seasons included),
/// by its TMDb id and its IMDb id, or the searches for its names when it has no TMDb id, so that
/// its next refresh asks them again: for an explicit
/// "replace all metadata" refresh, as Jellyfin's bypasses its cache. Nothing else forgets.
pub async fn forget_cached_answers(
    pool: &SqlitePool,
    item: &MediaItem,
    plugin_manager: &PluginManager,
) -> Result<(), String> {
    let providers = plugin_manager.metadata_providers().await;
    let series = match item.media_type {
        MediaType::Movie => false,
        MediaType::Series => true,
        _ => return Ok(()),
    };
    let imdb_id = if providers.iter().any(|p| p.imdb_lookups.is_some()) {
        stored_imdb_id(pool, item.id).await?
    } else {
        None
    };
    for provider in &providers {
        if let Some(lookups) = &provider.lookups {
            match item.tmdb_id {
                Some(id) => lookups.forget(series, id).await,
                // Not identified yet: its searches ask again
                None => {
                    for title in search_candidates(item) {
                        lookups.forget_search(series, &title).await;
                    }
                }
            }
        }
        if let (Some(lookups), Some(imdb_id)) = (&provider.imdb_lookups, &imdb_id) {
            lookups.forget(imdb_id).await;
        }
    }
    Ok(())
}

/// The IMDb id stored for an item (by a provider, an NFO or an import).
async fn stored_imdb_id(pool: &SqlitePool, id: Uuid) -> Result<Option<String>, String> {
    let ids = rustyfin_db::repo_media::get_imdb_ids_for_items(pool, &[id])
        .await
        .map_err(|e| e.to_string())?;
    Ok(ids.get(&id.to_string()).cloned())
}

/// Fill what is still missing on a movie or series from the providers with IMDb lookups
/// (Jellyfin's OMDb provider, after TMDb): e.g. a movie's rating when TMDb has no US
/// certification. Without an IMDb id, or when the lookup fails, nothing changes.
async fn fill_from_imdb(
    pool: &SqlitePool,
    working: &mut MediaItem,
    providers: &[MetadataProviderHandle],
) -> Result<(), String> {
    if !providers.iter().any(|p| p.imdb_lookups.is_some()) {
        return Ok(());
    }
    let Some(imdb_id) = stored_imdb_id(pool, working.id).await? else {
        return Ok(());
    };
    for lookups in providers.iter().filter_map(|p| p.imdb_lookups.as_ref()) {
        match lookups.by_imdb_id(&imdb_id).await {
            Ok(Some(meta)) => apply_metadata_to_item(pool, working, &meta, false).await?,
            Ok(None) => {}
            Err(e) => tracing::debug!("IMDb lookup of {imdb_id} failed: {e:?}"),
        }
    }
    Ok(())
}

/// Fill what is still missing on an episode from the providers with IMDb lookups: its series'
/// season list (one cached request per season), matched by episode number. This is where
/// Jellyfin's episodes get their genres and rating (OMDb's), which TMDb's episodes lack.
async fn fill_episode_from_imdb(
    pool: &SqlitePool,
    episode: Uuid,
    series_imdb_id: &str,
    season_number: i32,
    providers: &[MetadataProviderHandle],
) -> Result<(), String> {
    for lookups in providers.iter().filter_map(|p| p.imdb_lookups.as_ref()) {
        let season = match lookups.season(series_imdb_id, season_number).await {
            Ok(Some(season)) => season,
            Ok(None) => continue,
            Err(e) => {
                tracing::debug!("IMDb season lookup of {series_imdb_id} failed: {e:?}");
                continue;
            }
        };
        // The stored episode, as the earlier providers left it
        let Some(mut working) = rustyfin_db::repo_media::get_item_by_id(pool, episode)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        let Some(number) = working.index_number else {
            return Ok(());
        };
        if let Some((_, meta)) = season.episodes.iter().find(|(n, _)| *n == number) {
            apply_metadata_to_item(pool, &mut working, meta, false).await?;
        }
    }
    Ok(())
}

/// The series that a season or episode belongs to, and the season number: an episode's parent is
/// its season, or the series itself when it has no season folder.
async fn series_and_season(
    pool: &SqlitePool,
    item: &MediaItem,
) -> Result<Option<(MediaItem, i32)>, String> {
    let get = |id: Uuid| async move {
        rustyfin_db::repo_media::get_item_by_id(pool, id)
            .await
            .map_err(|e| e.to_string())
    };
    let Some(parent) = (match item.parent_id {
        Some(id) => get(id).await?,
        None => None,
    }) else {
        return Ok(None);
    };
    let (series, season_number) = match (item.media_type, parent.media_type) {
        (MediaType::Season, MediaType::Series) => (parent, item.index_number),
        (MediaType::Episode, MediaType::Series) => (parent, item.parent_index_number),
        (MediaType::Episode, MediaType::Season) => {
            let number = item.parent_index_number.or(parent.index_number);
            match parent.parent_id {
                Some(id) => match get(id).await? {
                    Some(series) if series.media_type == MediaType::Series => (series, number),
                    _ => return Ok(None),
                },
                None => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    Ok(season_number.map(|n| (series, n)))
}

/// Fill a season or an episode from its series' season lookup (one provider call per season,
/// shared by all its episodes through the provider's cache). Nothing happens until the series is
/// identified; its own refresh then fills its seasons and episodes.
async fn refresh_from_season(
    pool: &SqlitePool,
    working: &mut MediaItem,
    providers: &[MetadataProviderHandle],
    replace_name: bool,
) -> Result<(), String> {
    let Some((series, season_number)) = series_and_season(pool, working).await? else {
        return Ok(());
    };
    let Some(series_id) = series.tmdb_id else {
        return Ok(());
    };
    // Marked only on final answers (found, or a 404): after a failure the next refresh asks again
    let mut answered = false;
    let mut failed = false;
    for provider in providers {
        let Some(lookups) = &provider.lookups else {
            continue;
        };
        let season = match lookups.season(series_id, season_number).await {
            Ok(found) => {
                answered = true;
                found
            }
            Err(_) => {
                failed = true;
                None
            }
        };
        let Some(season) = season else {
            continue;
        };
        apply_season(
            pool,
            working,
            &season,
            &provider.provider_name,
            replace_name,
        )
        .await?;
    }
    if working.media_type == MediaType::Episode
        && providers.iter().any(|p| p.imdb_lookups.is_some())
        && let Some(imdb_id) = stored_imdb_id(pool, series.id).await?
    {
        fill_episode_from_imdb(pool, working.id, &imdb_id, season_number, providers).await?;
    }
    if answered && !failed {
        rustyfin_db::repo_media::mark_provider_refreshed(pool, &[working.id])
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether the NFO next to a media file gives a title, which a provider's title doesn't replace.
async fn nfo_has_title(path: &str) -> bool {
    match nfo_parser::find_nfo_for_media(Path::new(path)) {
        Some(nfo) => nfo_parser::parse_nfo(&nfo)
            .await
            .and_then(|meta| meta.name)
            .is_some_and(|name| !name.trim().is_empty()),
        None => false,
    }
}

/// Apply a season lookup to a season item, or to the episode it lists under the item's number.
async fn apply_season(
    pool: &SqlitePool,
    working: &mut MediaItem,
    season: &SeasonMetadata,
    provider: &str,
    replace_name: bool,
) -> Result<(), String> {
    let meta = match working.media_type {
        // A season keeps its own name ("Season 1"), as in Jellyfin
        MediaType::Season => season.meta.clone(),
        MediaType::Episode => {
            let Some(number) = working.index_number else {
                return Ok(());
            };
            let Some((_, first)) = season.episodes.iter().find(|(n, _)| *n == number) else {
                return Ok(());
            };
            let mut meta = first.clone();
            // A file holding several episodes is named after all of them, as Jellyfin names it
            // ("The Shadow That Devours the Sun / Mushi that Inhale Rain")
            if let Some(end) = working.index_number_end.filter(|end| *end > number) {
                let names: Vec<&str> = (number..=end)
                    .filter_map(|n| season.episodes.iter().find(|(e, _)| *e == n))
                    .filter_map(|(_, m)| m.name.as_deref())
                    .collect();
                if names.len() > 1 {
                    meta.name = Some(names.join(" / "));
                }
            }
            meta
        }
        _ => return Ok(()),
    };
    let meta = &meta;
    apply_metadata_to_item(pool, working, meta, replace_name).await?;
    store_images_batch(pool, working.id, &working.path, &meta.image_urls, provider).await
}

/// After a series is identified, fill its seasons and episodes: one season lookup per season.
async fn refresh_series_children(
    pool: &SqlitePool,
    series: &MediaItem,
    providers: &[MetadataProviderHandle],
    force: bool,
) -> Result<ChildrenAnswer, String> {
    let Some(series_id) = series.tmdb_id else {
        return Ok(ChildrenAnswer {
            all_answered: true,
            unauthorized: false,
        });
    };
    let children = rustyfin_db::repo_media::get_children(pool, series.id)
        .await
        .map_err(|e| e.to_string())?;
    let mut targets: Vec<(MediaItem, i32)> = Vec::new();
    for child in children {
        match child.media_type {
            MediaType::Season => {
                let Some(number) = child.index_number else {
                    continue;
                };
                let episodes = rustyfin_db::repo_media::get_children(pool, child.id)
                    .await
                    .map_err(|e| e.to_string())?;
                for episode in episodes {
                    if episode.media_type == MediaType::Episode {
                        let n = episode.parent_index_number.unwrap_or(number);
                        targets.push((episode, n));
                    }
                }
                targets.push((child, number));
            }
            MediaType::Episode => {
                if let Some(n) = child.parent_index_number {
                    targets.push((child, n));
                }
            }
            _ => {}
        }
    }
    // Only the seasons and episodes no provider answered for yet, unless forced (an explicit
    // refresh refills them all): a later refresh of the series asks TMDb for nothing more
    if !force {
        let ids: Vec<Uuid> = targets.iter().map(|(t, _)| t.id).collect();
        let answered = rustyfin_db::repo_media::provider_refreshed_ids(pool, &ids)
            .await
            .map_err(|e| e.to_string())?;
        targets.retain(|(t, _)| !answered.contains(&t.id));
    }
    if targets.is_empty() {
        return Ok(ChildrenAnswer {
            all_answered: true,
            unauthorized: false,
        });
    }
    // Season numbers with a final answer from every provider with lookups, and any failed
    let mut answered: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut failed: std::collections::HashSet<i32> = std::collections::HashSet::new();
    let mut unauthorized = false;
    for provider in providers {
        let Some(lookups) = &provider.lookups else {
            continue;
        };
        let mut seasons: std::collections::HashMap<i32, Option<SeasonMetadata>> =
            std::collections::HashMap::new();
        for (target, number) in &targets {
            if !seasons.contains_key(number) {
                let found = match lookups.season(series_id, *number).await {
                    Ok(found) => {
                        answered.insert(*number);
                        found
                    }
                    Err(e) => {
                        unauthorized |= e == LookupFailed::Unauthorized;
                        failed.insert(*number);
                        None
                    }
                };
                seasons.insert(*number, found);
            }
            let Some(Some(season)) = seasons.get(number) else {
                continue;
            };
            let mut working = target.clone();
            // As in the item's own refresh: a locked name or an NFO's title stays
            let keep_name = rustyfin_db::repo_media::is_field_locked(pool, target.id, "Name")
                .await
                .unwrap_or(false)
                || nfo_has_title(&target.path).await;
            apply_season(
                pool,
                &mut working,
                season,
                &provider.provider_name,
                !keep_name,
            )
            .await?;
        }
    }
    if providers.iter().any(|p| p.imdb_lookups.is_some())
        && let Some(imdb_id) = stored_imdb_id(pool, series.id).await?
    {
        for (target, number) in &targets {
            if target.media_type == MediaType::Episode {
                fill_episode_from_imdb(pool, target.id, &imdb_id, *number, providers).await?;
            }
        }
    }
    let ids: Vec<Uuid> = targets
        .iter()
        .filter(|(_, n)| answered.contains(n) && !failed.contains(n))
        .map(|(t, _)| t.id)
        .collect();
    if !ids.is_empty() {
        rustyfin_db::repo_media::mark_provider_refreshed(pool, &ids)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(ChildrenAnswer {
        all_answered: failed.is_empty(),
        unauthorized,
    })
}

async fn apply_metadata_to_item(
    pool: &SqlitePool,
    updated: &mut MediaItem,
    meta: &rustyfin_model::metadata::ExternalMetadata,
    replace_name: bool,
) -> Result<(), String> {
    // Update fields from external metadata. A locked field (or a locked item) keeps its value,
    // as in Jellyfin, whatever the source.
    let locks = rustyfin_db::repo_media::item_locks(pool, updated.id)
        .await
        .map_err(|e| e.to_string())?;
    let open = |field: &str| !locks.locked(field);
    let before = updated.clone();
    let mut changed = false;

    // A provider's title replaces the name parsed from the path, as in Jellyfin ("Mission:
    // Impossible - Dead Reckoning Part One", not the folder's "Mission Impossible - ..."), unless
    // the name is locked or an NFO has a title. An NFO's title (`replace_name` false) replaces
    // only a name that still looks like a file name. Seasons keep "Season N".
    if (replace_name || looks_like_filename(&updated.name))
        && open("Name")
        && updated.media_type != MediaType::Season
        && let Some(new_name) = meta.name.as_deref().map(str::trim)
        && !new_name.is_empty()
        && updated.name != new_name
    {
        updated.name = new_name.to_string();
        changed = true;
        tracing::debug!("updated name '{}' to '{}'", before.name, new_name);
    }
    // The sort name, as Jellyfin's: made from the forced sort name, or else from the name. An
    // NFO's <sorttitle> becomes the forced sort name when there is none (a "replace all" clears
    // it first) and no lock (whole item, Name or SortName) keeps it. A season with none keeps
    // its own.
    let mut forced = rustyfin_db::repo_media::forced_sort_name(pool, updated.id)
        .await
        .map_err(|e| e.to_string())?;
    if forced.is_none()
        && !locks.forced_sort_name_locked()
        && let Some(sort_title) = meta.sort_title.as_deref().map(str::trim)
        && !sort_title.is_empty()
    {
        rustyfin_db::repo_media::set_forced_sort_name(pool, updated.id, Some(sort_title))
            .await
            .map_err(|e| e.to_string())?;
        forced = Some(sort_title.to_string());
    }
    if forced.is_some() || updated.media_type != MediaType::Season {
        let sort_name = match forced.as_deref() {
            Some(forced) => rustyfin_model::sort_name::forced_sort_name(forced),
            None => rustyfin_model::sort_name::item_sort_name(
                updated.media_type,
                &updated.name,
                updated.parent_index_number,
                updated.index_number,
                &rustyfin_model::sort_name::current_rules(),
            ),
        };
        if updated.sort_name.as_deref() != Some(sort_name.as_str()) {
            updated.sort_name = Some(sort_name);
            changed = true;
        }
    }
    if updated.premiere_date.is_none()
        && open("PremiereDate")
        && let Some(date) = meta.premiere_date.as_deref()
        && let Ok(day) =
            chrono::NaiveDate::parse_from_str(date.get(..10).unwrap_or(date), "%Y-%m-%d")
    {
        updated.premiere_date = Some(day.and_time(chrono::NaiveTime::MIN).and_utc());
        changed = true;
    }

    if updated.overview.is_none()
        && open("Overview")
        && let Some(ref v) = meta.overview
    {
        updated.overview = Some(v.clone());
        changed = true;
    }
    if updated.production_year.is_none()
        && open("ProductionYear")
        && let Some(v) = meta.production_year
    {
        updated.production_year = Some(v);
        changed = true;
    }
    if updated.community_rating.is_none()
        && open("CommunityRating")
        && let Some(v) = meta.community_rating
    {
        updated.community_rating = Some(v);
        changed = true;
    }
    // An empty rating (an NFO's empty <mpaa/>) counts as none
    if updated
        .official_rating
        .as_deref()
        .is_none_or(|r| r.trim().is_empty())
        && open("OfficialRating")
        && let Some(ref v) = meta.official_rating
        && !v.trim().is_empty()
    {
        updated.official_rating = Some(v.clone());
        changed = true;
    }
    if updated.genres.is_empty()
        && open("Genres")
        && let Some(ref v) = meta.genres
        && !v.is_empty()
    {
        updated.genres = v.clone();
        changed = true;
    }
    if updated.runtime_ticks.is_none()
        && open("Runtime")
        && let Some(v) = meta.runtime_ticks
    {
        updated.runtime_ticks = Some(v);
        changed = true;
    }
    if updated.tmdb_id.is_none()
        && let Some(v) = meta.tmdb_id
    {
        updated.tmdb_id = Some(v);
        changed = true;
    }
    if updated.tvdb_id.is_none()
        && let Some(v) = meta.tvdb_id
    {
        updated.tvdb_id = Some(v);
        changed = true;
    }
    if updated.status.is_none()
        && open("Status")
        && let Some(ref v) = meta.status
    {
        updated.status = Some(v.clone());
        changed = true;
    }
    if updated.original_language.is_none()
        && open("OriginalLanguage")
        && let Some(ref v) = meta.original_language
    {
        updated.original_language = Some(v.clone());
        changed = true;
    }
    if updated.taglines.is_empty()
        && open("Overview")
        && let Some(ref v) = meta.tagline
        && !v.is_empty()
    {
        updated.taglines = vec![v.clone()];
        changed = true;
    }
    if updated.production_locations.is_empty()
        && open("ProductionLocations")
        && let Some(ref v) = meta.production_countries
        && !v.is_empty()
    {
        updated.production_locations = v.clone();
        changed = true;
    }
    if updated.end_date.is_none()
        && open("EndDate")
        && let Some(ref v) = meta.end_date
    {
        // Parse end_date string to DateTime<Utc>
        if let Ok(dt) = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d")
            && let dt_utc = dt.and_time(chrono::NaiveTime::MIN).and_utc()
        {
            updated.end_date = Some(dt_utc);
            changed = true;
        }
    }
    if updated.budget.is_none()
        && !locks.lock_data
        && let Some(v) = meta.budget
    {
        updated.budget = Some(v);
        changed = true;
    }
    if updated.revenue.is_none()
        && !locks.lock_data
        && let Some(v) = meta.revenue
    {
        updated.revenue = Some(v);
        changed = true;
    }
    if updated.homepage.is_none()
        && !locks.lock_data
        && let Some(ref v) = meta.homepage
        && !v.is_empty()
    {
        updated.homepage = Some(v.clone());
        changed = true;
    }
    if updated.vote_count.is_none()
        && !locks.lock_data
        && let Some(v) = meta.vote_count
    {
        updated.vote_count = Some(v);
        changed = true;
    }
    if updated.popularity.is_none()
        && !locks.lock_data
        && let Some(v) = meta.popularity
    {
        updated.popularity = Some(v);
        changed = true;
    }

    if changed {
        let written = rustyfin_db::repo_media::save_refresh_changes(pool, &before, updated)
            .await
            .map_err(|e| e.to_string())?;
        tracing::debug!("metadata refresh wrote columns {:?}", written);
    }

    // Store studios from metadata
    if let Some(ref studios) = meta.studios
        && !studios.is_empty()
        && open("Studios")
    {
        store_studios(pool, updated.id, studios).await?;
    }
    if let Some(imdb) = meta.imdb_id.as_deref().filter(|v| v.starts_with("tt")) {
        rustyfin_db::repo_media::set_imdb_id_if_missing(pool, updated.id, imdb)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(people) = meta.people.as_deref()
        && open("Cast")
    {
        rustyfin_db::repo_person::set_item_people_if_none(pool, updated.id, people)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Whether a name still looks like a file name (dots, underscores, release tags), so that any
/// source's title may replace it.
fn looks_like_filename(name: &str) -> bool {
    const TAGS: [&str; 10] = [
        "-ENCOUNTERS",
        "-NIMA4K",
        "-eisbaer",
        "-WATCHABLE",
        "1080p",
        "2160p",
        "BluRay",
        "WEB-DL",
        "HDTV",
        "_",
    ];
    name.contains('.') || TAGS.iter().any(|tag| name.contains(tag)) || name.len() > 80
}

/// Clean a filename to make it suitable for TMDB search.
/// Removes file extensions, release tags, quality indicators, and normalizes separators.
/// Example: "Zombieland.2009.German.DL.1080p.BluRay.x264-SCENE.mkv" -> "Zombieland 2009"
pub fn clean_filename_for_search(path: &str) -> String {
    use std::path::Path;

    let p = Path::new(path);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    // Replace common separators with spaces
    let mut cleaned = stem
        .replace(['.', '_', '-'], " ")
        .replace("  ", " ")
        .trim()
        .to_string();

    // Remove common release groups and quality tags
    let tags_to_remove = [
        "GERMAN",
        "DL",
        "DC",
        "Dubbed",
        "PROPER",
        "1080p",
        "720p",
        "2160p",
        "4K",
        "UHD",
        "HDR",
        "Dolby",
        "BluRay",
        "BLURAY",
        "WEB-DL",
        "WEBDL",
        "HDTV",
        "BDRip",
        "WEBRip",
        "x264",
        "x265",
        "HEVC",
        "H264",
        "AVC",
        "MKV",
        "MP4",
        "AVI",
        "M4V",
        "EAC3",
        "DD51",
        "DDP",
        "DTSHD",
        "TrueHD",
        "GERMAN.DL",
        "ENGLISH",
    ];

    for tag in &tags_to_remove {
        let pattern = format!(" {} ", tag);
        if cleaned.contains(&pattern) {
            cleaned = cleaned.replace(&pattern, " ");
        }
        // Also check at start/end
        let start_pattern = format!("{} ", tag);
        if let Some(stripped) = cleaned.strip_prefix(&start_pattern) {
            cleaned = stripped.to_string();
        }
        let end_pattern = format!(" {}", tag);
        if let Some(stripped) = cleaned.strip_suffix(&end_pattern) {
            cleaned = stripped.to_string();
        }
    }

    // Remove encoder groups (e.g., "-SCENE", "-NIMA4K", "-eisbaer", "-WATCHABLE")
    if let Some(pos) = cleaned.rfind('-')
        && pos > 10
    {
        cleaned = cleaned[..pos].to_string();
    }

    // Clean up extra spaces
    cleaned = cleaned.split_whitespace().collect::<Vec<&str>>().join(" ");

    // Extract year if present (4-digit year at the end or in parentheses)
    // Keep the year for better search results
    cleaned = cleaned.trim().to_string();

    cleaned
}

/// How a provider image is stored: a web URL as it is; a local path (absolute, or relative to the
/// item's folder) only when it resolves inside that folder, as the resolved path. An NFO's
/// `<thumb>` can name any file, and the image routes would read it; a path outside the item's
/// folder, a missing file or any other scheme is dropped.
fn storable_image_path(url: &str, item_path: &Path) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(url.to_string());
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    if path.is_empty() || lower.contains("://") {
        return None;
    }
    // The folder an NFO sits in: the item's own folder, or a series' folder
    let folder = if item_path.is_dir() {
        item_path
    } else {
        item_path.parent()?
    };
    let folder = folder.canonicalize().ok()?;
    let resolved = folder.join(path).canonicalize().ok()?;
    (resolved.starts_with(&folder) && resolved.is_file())
        .then(|| resolved.to_string_lossy().into_owned())
}

/// Batch-store provider images, merging all sources.
/// Each provider's images get their own image_index slot so multiple providers
/// can contribute images of the same type (e.g. multiple backdrops from tmdb, fanart, etc.).
async fn store_images_batch(
    pool: &SqlitePool,
    item_id: Uuid,
    item_path: &str,
    images: &[rustyfin_model::metadata::MetadataImage],
    provider: &str,
) -> Result<(), String> {
    // A local picture (an NFO's <thumb>) is kept only inside the item's folder, by its resolved
    // path; a web URL as it is
    let images: Vec<rustyfin_model::metadata::MetadataImage> = images
        .iter()
        .filter_map(|image| {
            let url = storable_image_path(&image.url, Path::new(item_path))?;
            Some(rustyfin_model::metadata::MetadataImage {
                url,
                ..image.clone()
            })
        })
        .collect();
    let images = images.as_slice();
    // Group images by base type to assign proper indices
    // Parse image_type strings like "Primary", "Primary_1", "Backdrop", "Backdrop_2"
    let mut type_counts: std::collections::HashMap<
        String,
        Vec<(usize, &rustyfin_model::metadata::MetadataImage)>,
    > = std::collections::HashMap::new();
    for img_meta in images {
        // Extract base type: "Primary_2" -> "Primary", "Backdrop" -> "Backdrop"
        let base_type = img_meta
            .image_type
            .split('_')
            .next()
            .unwrap_or(&img_meta.image_type);
        let entry = type_counts.entry(base_type.to_string()).or_default();
        entry.push((entry.len(), img_meta));
    }

    // Fetch existing images to determine next available index
    let existing_images = rustyfin_db::repo_image::get_images_for_item(pool, item_id)
        .await
        .map_err(|e| e.to_string())?;

    // Build map of base_type -> max_index for THIS provider only
    // For the first provider image of each type, always use index 0
    // so the client can find it (client defaults to index 0)
    let mut provider_max_indices: std::collections::HashMap<String, i32> =
        std::collections::HashMap::new();
    // A type the admin uploaded or chose an image for keeps it until a refresh replaces all images
    let chosen: std::collections::HashSet<&str> = existing_images
        .iter()
        .filter(|img| img.provider == "upload" || img.provider == "user")
        .map(|img| img.image_type.as_str())
        .collect();
    for img in &existing_images {
        if img.provider == provider {
            let base = img.image_type.as_str().to_string();
            let current = provider_max_indices.entry(base).or_insert(-1);
            if img.image_index > *current {
                *current = img.image_index;
            }
        }
    }

    // Build list of images to insert
    let mut to_insert = Vec::new();
    for (base_type, img_group) in type_counts {
        if let Some(image_type_base) = ImageType::from_str(&base_type) {
            // A type this provider stored before keeps its images, so a refresh that asks the
            // provider again doesn't add the same pictures at new indices
            if chosen.contains(image_type_base.as_str())
                || provider_max_indices
                    .get(image_type_base.as_str())
                    .is_some_and(|&max_idx| max_idx >= 0)
            {
                continue;
            }
            let start_index = 0;

            for (idx, img_meta) in img_group {
                let item_image_type = if idx == 0 {
                    image_type_base
                } else {
                    // Parse the specific variant (e.g. "Primary_1" -> Primary with index)
                    ImageType::from_str(&img_meta.image_type).unwrap_or(image_type_base)
                };
                to_insert.push(ItemImage {
                    item_id,
                    image_type: item_image_type,
                    image_index: start_index + idx as i32,
                    path: img_meta.url.clone(),
                    provider: provider.to_string(),
                    width: None,
                    height: None,
                    blurhash: None,
                    stamp: None,
                });
            }
        }
    }

    // Batch-insert all images in a single transaction
    if !to_insert.is_empty() {
        rustyfin_db::repo_image::batch_upsert_images(pool, &to_insert)
            .await
            .map_err(|e| e.to_string())?;
        tracing::debug!(
            "stored {} images from '{}' for item {}",
            to_insert.len(),
            provider,
            item_id
        );
    }
    Ok(())
}

/// Store studios from TMDb into the studios and item_studios tables
async fn store_studios(
    pool: &SqlitePool,
    item_id: Uuid,
    studio_names: &[String],
) -> Result<(), String> {
    let item_id_str = item_id.to_string();
    for studio_name in studio_names {
        // Generate a stable ID from the studio name
        let studio_id = format!("studio-{}", studio_name.to_lowercase().replace(' ', "-"));
        // Upsert studio
        sqlx::query(
            "INSERT INTO studios (id, name)
             VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        )
        .bind(&studio_id)
        .bind(studio_name)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        // Link studio to item
        sqlx::query(
            "INSERT OR IGNORE INTO item_studios (item_id, studio_id)
             VALUES (?1, ?2)",
        )
        .bind(&item_id_str)
        .bind(&studio_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }
    tracing::debug!("stored {} studios for item {}", studio_names.len(), item_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3playback_nfo_thumbs_stay_in_the_items_folder() {
        let root = std::env::temp_dir().join(format!("rustyfin-r3thumb-{}", Uuid::new_v4()));
        let film = root.join("lib").join("Film");
        let outside = root.join("secret");
        std::fs::create_dir_all(&film).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(film.join("Film.mkv"), b"x").unwrap();
        std::fs::write(film.join("poster.jpg"), b"x").unwrap();
        std::fs::write(outside.join("key.pem"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.join("key.pem"), film.join("linked.jpg")).unwrap();
        let media = film.join("Film.mkv");
        let poster = film.join("poster.jpg").canonicalize().unwrap();
        let poster = poster.to_str().unwrap();

        // Inside the folder: relative or absolute, stored resolved; web URLs as they are
        assert_eq!(
            storable_image_path("poster.jpg", &media).as_deref(),
            Some(poster)
        );
        assert_eq!(storable_image_path(poster, &media).as_deref(), Some(poster));
        let url = "https://image.tmdb.org/t/p/original/x.jpg";
        assert_eq!(storable_image_path(url, &media).as_deref(), Some(url));
        // A series folder is its own folder
        assert_eq!(
            storable_image_path("poster.jpg", &film).as_deref(),
            Some(poster)
        );
        // Outside, through `..`, through a link, missing, or another scheme: ignored
        for thumb in [
            outside.join("key.pem").to_str().unwrap().to_string(),
            "../../secret/key.pem".to_string(),
            "linked.jpg".to_string(),
            "missing.jpg".to_string(),
            "smb://nas/poster.jpg".to_string(),
            String::new(),
        ] {
            assert_eq!(storable_image_path(&thumb, &media), None, "{thumb}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── clean_filename_for_search ──

    #[test]
    fn test_clean_filename_basic() {
        let result = clean_filename_for_search("/path/to/Movie.2023.mkv");
        assert_eq!(result, "Movie 2023");
    }

    #[test]
    fn test_clean_filename_complex() {
        let result = clean_filename_for_search(
            "/path/to/Zombieland.2009.German.DL.1080p.BluRay.x264-SCENE.mkv",
        );
        assert!(result.contains("Zombieland"));
        assert!(result.contains("2009"));
        assert!(!result.contains("1080p"));
        assert!(!result.contains("BluRay"));
    }

    #[test]
    fn test_clean_filename_underscores() {
        let result = clean_filename_for_search("/path/to/The.Movie.2020.mkv");
        assert_eq!(result, "The Movie 2020");
    }

    #[test]
    fn test_clean_filename_hyphens() {
        let result = clean_filename_for_search("/path/to/The-Movie-2020.mkv");
        // Hyphens are replaced with spaces, but trailing encoder groups after - are removed
        assert!(result.contains("The"));
        assert!(result.contains("Movie"));
    }

    #[test]
    fn test_clean_filename_720p() {
        let result = clean_filename_for_search("/path/to/Movie.2020.720p.WEB-DL.x264.mkv");
        assert!(!result.contains("720p"));
        assert!(!result.contains("WEB-DL"));
        assert!(!result.contains("x264"));
    }

    #[test]
    fn test_clean_filename_4k() {
        let result = clean_filename_for_search("/path/to/Movie.2020.2160p.4K.UHD.mkv");
        assert!(!result.contains("2160p"));
        assert!(!result.contains("4K"));
        assert!(!result.contains("UHD"));
    }

    #[test]
    fn test_clean_filename_encoder_group() {
        let result = clean_filename_for_search("/path/to/Movie.2020.x264-SCENE.mkv");
        // After replacing separators with spaces, "x264-SCENE" becomes "x264 SCENE"
        // x264 tag is removed, leaving "Movie 2020 SCENE"
        // The encoder group removal (rfind('-')) won't find '-' since it was replaced
        assert!(result.contains("Movie"));
        assert!(result.contains("2020"));
        assert!(!result.contains("x264"));
    }

    #[test]
    fn test_clean_filename_no_extension() {
        let result = clean_filename_for_search("Movie");
        assert_eq!(result, "Movie");
    }

    #[test]
    fn test_clean_filename_dolby() {
        let result = clean_filename_for_search("/path/to/Movie.2020.Dolby.Vision.mkv");
        assert!(!result.contains("Dolby"));
    }

    #[test]
    fn test_clean_filename_hevc() {
        let result = clean_filename_for_search("/path/to/Movie.2020.HEVC.x265.mkv");
        assert!(!result.contains("HEVC"));
        assert!(!result.contains("x265"));
    }

    #[test]
    fn test_clean_filename_proper() {
        let result = clean_filename_for_search("/path/to/Movie.2020.PROPER.mkv");
        assert!(!result.contains("PROPER"));
    }

    #[test]
    fn test_clean_filename_hdtv() {
        let result = clean_filename_for_search("/path/to/Show.S01E01.HDTV.x264.mkv");
        assert!(!result.contains("HDTV"));
        assert!(!result.contains("x264"));
    }
}
