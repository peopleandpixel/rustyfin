use crate::query::Query;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use rustyfin_core::tmdb_client::TmdbApi;
use rustyfin_model::auth::Claims;
use rustyfin_model::media::{FiltersResult, MediaItem, QueryResult};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::routes::collections::Container;
use crate::routes::playback::{path_user, query_user};
use crate::state::AppState;

/// Populate `media_sources` on each item using a single batched query.
async fn populate_media_sources(
    pool: &SqlitePool,
    items: &mut [MediaItem],
) -> Result<(), ApiError> {
    if items.is_empty() {
        return Ok(());
    }

    // Collect all item IDs for a single batched query
    let item_ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();

    // Fetch all sources and streams in one round-trip
    let batch_map =
        rustyfin_db::repo_media_source::get_media_sources_batch(pool, &item_ids).await?;

    // Populate each item from the batch results
    for item in items.iter_mut() {
        let sources = batch_map
            .get(&item.id.to_string())
            .cloned()
            .unwrap_or_default();
        item.media_sources = sources;
    }
    // The subtitle files beside each source, each folder read once
    let mut sources: Vec<_> = items
        .iter_mut()
        .flat_map(|item| item.media_sources.iter_mut())
        .collect();
    crate::routes::subtitle::add_external_streams(&mut sources).await;
    Ok(())
}

/// Populate `media_sources` on a single item.
async fn populate_media_source_single(
    pool: &SqlitePool,
    item: &mut MediaItem,
) -> Result<(), ApiError> {
    let sources = rustyfin_db::repo_media_source::get_media_sources(pool, &item.id).await?;
    item.media_sources = sources;
    let mut sources: Vec<_> = item.media_sources.iter_mut().collect();
    crate::routes::subtitle::add_external_streams(&mut sources).await;
    Ok(())
}

/// Sets a user's watch data on an item, with its CompletionPercentage if the runtime is known.
fn apply_user_data(item: &mut MediaItem, mut user_data: rustyfin_model::playback::UserItemDataDto) {
    if let Some(runtime) = item.runtime_ticks
        && runtime > 0
    {
        let pos = user_data.playback_position_ticks.max(0) as f64;
        let run = runtime as f64;
        if user_data.played {
            item.completion_percentage = Some(100.0);
        } else {
            item.completion_percentage = Some((pos / run * 100.0).min(100.0));
        }
    }
    user_data.played_percentage = played_percentage(&user_data, item.runtime_ticks);
    item.user_data = Some(user_data.for_item(item.id));
}

/// Jellyfin's PlayedPercentage: 100 * position / runtime, only with a position and a known runtime.
fn played_percentage(
    user_data: &rustyfin_model::playback::UserItemDataDto,
    runtime_ticks: Option<i64>,
) -> Option<f64> {
    let runtime = runtime_ticks.filter(|r| *r > 0)?;
    (user_data.playback_position_ticks > 0)
        .then(|| 100.0 * user_data.playback_position_ticks as f64 / runtime as f64)
}

/// Whether an item's watch data counts its episodes (Series and Season).
fn is_episode_folder(item: &MediaItem) -> bool {
    matches!(
        item.media_type,
        rustyfin_model::media::MediaType::Series | rustyfin_model::media::MediaType::Season
    )
}

/// Sets a Series or Season's UnplayedItemCount from its `(episodes, unplayed)` counts, and its
/// Played from the same counts (overriding the stored flag). Call after `apply_user_data`.
fn apply_unplayed_counts(item: &mut MediaItem, counts: (i64, i64)) {
    let data = item.user_data.get_or_insert_with(Default::default);
    data.unplayed_item_count = Some(counts.1 as i32);
    data.played = rustyfin_db::repo_playback::folder_played(counts);
}

/// Sets a folder's ChildCount and its per-kind counts from its children's `(type, count)` rows; a
/// count of zero is left out.
fn set_folder_counts(item: &mut MediaItem, counts: Vec<(String, i64)>) {
    let of = |kinds: &[&str]| -> Option<i32> {
        let n: i64 = counts
            .iter()
            .filter(|(kind, _)| kinds.is_empty() || kinds.contains(&kind.as_str()))
            .map(|(_, n)| n)
            .sum();
        (n > 0).then_some(n as i32)
    };
    item.song_count = of(&["Audio"]);
    item.album_count = of(&["MusicAlbum"]);
    item.movie_count = of(&["Movie"]);
    item.series_count = of(&["Series"]);
    item.episode_count = of(&["Episode"]);
    item.child_count = of(&[]);
}

/// Set an item's image tags (the first image of each type), backdrop tags (every backdrop, in
/// order) and PrimaryImageAspectRatio from its image rows.
pub(crate) fn set_image_tags(item: &mut MediaItem, images: &[rustyfin_model::image::ItemImage]) {
    item.primary_image_aspect_ratio = images
        .iter()
        .find(|img| img.image_type == rustyfin_model::image::ImageType::Primary)
        .and_then(|img| primary_aspect_ratio(item.media_type, img));
    let mut tags: std::collections::HashMap<rustyfin_model::image::ImageType, String> =
        std::collections::HashMap::new();
    let mut backdrop_tags: Vec<String> = Vec::new();
    for img in images {
        let tag = rustyfin_core::image_proxy::image_tag(&img.path, img.stamp);
        if img.image_type == rustyfin_model::image::ImageType::Backdrop {
            backdrop_tags.push(tag.clone());
        }
        tags.entry(img.image_type).or_insert_with(|| tag);
    }
    item.image_tags = tags;
    item.backdrop_image_tags = backdrop_tags;
}

/// Jellyfin's PrimaryImageAspectRatio: the primary image's width over its height, or, when its
/// size isn't known, the kind's usual shape (GetDefaultPrimaryImageAspectRatio).
fn primary_aspect_ratio(
    kind: rustyfin_model::media::MediaType,
    img: &rustyfin_model::image::ItemImage,
) -> Option<f64> {
    use rustyfin_model::media::MediaType;
    match (img.width, img.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            Some(f64::from(width) / f64::from(height))
        }
        _ => match kind {
            MediaType::Movie | MediaType::Series | MediaType::Season | MediaType::BoxSet => {
                Some(2.0 / 3.0)
            }
            MediaType::Episode => Some(16.0 / 9.0),
            MediaType::Audio | MediaType::MusicAlbum | MediaType::MusicArtist => Some(1.0),
            _ => None,
        },
    }
}

/// Adds an image's BlurHash to an item, under its type and tag (Jellyfin's ImageBlurHashes).
fn add_blur_hash(item: &mut MediaItem, img: &rustyfin_model::image::ItemImage, hash: String) {
    item.image_blur_hashes
        .entry(img.image_type.as_str().to_string())
        .or_default()
        .insert(
            rustyfin_core::image_proxy::image_tag(&img.path, img.stamp),
            hash,
        );
}

/// The album artists of a music album: its `AlbumArtist` links `(person id, name)` in order,
/// else its parent item when that is a MusicArtist or MusicAlbum, else none.
fn album_artists_of(
    links: &[(String, String)],
    parent: Option<&MediaItem>,
) -> Vec<rustyfin_model::media::GenreItem> {
    if !links.is_empty() {
        return links
            .iter()
            .map(|(id, name)| rustyfin_model::media::GenreItem {
                name: name.clone(),
                id: id.clone(),
            })
            .collect();
    }
    match parent {
        Some(p)
            if matches!(
                p.media_type,
                rustyfin_model::media::MediaType::MusicArtist
                    | rustyfin_model::media::MediaType::MusicAlbum
            ) =>
        {
            vec![rustyfin_model::media::GenreItem {
                name: p.name.clone(),
                id: p.id.to_string(),
            }]
        }
        _ => Vec::new(),
    }
}

/// The `Artists` and `ArtistItems` of a music album: the distinct artists of its tracks
/// (`track_artists`, from `album_track_artists_batch`). Without any, the album artists by name and
/// no items.
fn album_credit(
    track_artists: &[(String, String)],
    album_artists: &[rustyfin_model::media::GenreItem],
) -> (Vec<String>, Vec<rustyfin_model::media::GenreItem>) {
    if track_artists.is_empty() {
        return (
            album_artists.iter().map(|a| a.name.clone()).collect(),
            Vec::new(),
        );
    }
    let items: Vec<rustyfin_model::media::GenreItem> = track_artists
        .iter()
        .map(|(id, name)| rustyfin_model::media::GenreItem {
            name: name.clone(),
            id: id.clone(),
        })
        .collect();
    (items.iter().map(|a| a.name.clone()).collect(), items)
}

/// Load the image tags of an item read with `get_parent_item` or `get_album_for_audio`, which
/// leave them empty.
async fn load_image_tags(pool: &SqlitePool, item: &mut MediaItem) {
    if let Ok(images) = rustyfin_db::repo_image::get_images_for_item(pool, item.id).await {
        set_image_tags(item, &images);
    }
}

/// Sets an item's `Chapters` and `ChapterCount`, as Jellyfin's `ChapterInfo`s. A chapter with an
/// image gets its `ImageTag` and `ImageDateModified`; one whose image failed counts as none.
fn set_chapters(item: &mut MediaItem, chapters: &[rustyfin_model::chapter::Chapter]) {
    use rustyfin_model::media::ChapterInfo;
    item.chapters = chapters
        .iter()
        .map(|c| {
            let image = c.image_path.as_deref().filter(|p| !p.is_empty());
            let date = image.and(c.image_date_modified.as_deref());
            ChapterInfo {
                start_position_ticks: c.start_position_ticks,
                name: c.name.clone(),
                image_date_modified: date.unwrap_or(ChapterInfo::NO_IMAGE_DATE).to_string(),
                image_tag: image.map(|path| chapter_image_tag(path, date)),
            }
        })
        .collect();
    item.chapter_count = item.chapters.len() as i32;
}

/// A chapter image's tag: it changes when the image is made again.
fn chapter_image_tag(path: &str, date: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    date.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Sets an item's `Trickplay`: its tile sets by width, under its own id as the media source
/// Jellyfin clients look them up by, and under its first source's id too when that differs.
fn set_trickplay(item: &mut MediaItem, sets: Vec<rustyfin_model::trickplay::TrickplayInfo>) {
    if sets.is_empty() {
        return;
    }
    let by_width: std::collections::BTreeMap<_, _> = sets
        .into_iter()
        .map(|info| (info.width.to_string(), info))
        .collect();
    let mut by_source = std::collections::BTreeMap::new();
    if let Some(source) = item.media_sources.first()
        && source.id != item.id.simple().to_string()
    {
        by_source.insert(source.id.clone(), by_width.clone());
    }
    by_source.insert(item.id.simple().to_string(), by_width);
    item.trickplay = Some(by_source);
}

/// Puts what a photo's file says into its item, as Jellyfin's DtoService does for a photo. The date
/// taken is already the item's premiere date and production year.
fn apply_photo_info(item: &mut MediaItem, info: &rustyfin_model::photo::PhotoInfo) {
    item.width = info.width;
    item.height = info.height;
    item.camera_make = info.camera_make.clone();
    item.camera_model = info.camera_model.clone();
    item.software = info.software.clone();
    item.image_orientation = info.orientation.clone();
    item.exposure_time = info.exposure_time;
    item.focal_length = info.focal_length;
    item.aperture = info.aperture;
    item.iso_speed_rating = info.iso_speed_rating;
    item.latitude = info.latitude;
    item.longitude = info.longitude;
    item.altitude = info.altitude;
}

/// TMDb's path segment for an item's TMDb id: a movie's or a series'. A season's or an episode's
/// TMDb id is not one TMDb's movie or tv routes take.
fn tmdb_kind(item: &MediaItem) -> Option<&'static str> {
    match item.media_type {
        rustyfin_model::media::MediaType::Movie => Some("movie"),
        rustyfin_model::media::MediaType::Series => Some("tv"),
        _ => None,
    }
}

/// Enrich a single MediaItem with all response fields: server_id, provider_ids, chapters,
/// trickplay screenshots, etag, completion_percentage, external_urls, type flags,
/// album/artist info, folder counts, inherited images, episode numbers, series branding,
/// original title, trailers (from TMDb API), blurhashes, and more.
async fn enrich_item(
    pool: &SqlitePool,
    server_id: &str,
    user_id: Option<Uuid>,
    item: &mut MediaItem,
    http_client: &reqwest::Client,
    tmdb_api: Option<&TmdbApi>,
    data_dir: &std::path::Path,
) {
    // ServerId
    item.server_id = Some(server_id.to_string());
    // Container: the first media source's, as Jellyfin sends it
    if item.container.is_none() {
        item.container = item.media_sources.first().and_then(|s| s.container.clone());
    }

    // Jellyfin sends HasLyrics on audio items only
    if item.media_type == rustyfin_model::media::MediaType::Audio {
        let with_lyrics = rustyfin_db::repo_lyrics::with_lyrics(pool, &[item.id])
            .await
            .unwrap_or_default();
        item.has_lyrics = Some(!with_lyrics.is_empty());
    }

    // Type flags
    item.is_movie = item.media_type == rustyfin_model::media::MediaType::Movie;
    item.is_series = item.media_type == rustyfin_model::media::MediaType::Series;
    item.is_kids = item.official_rating.as_deref() == Some("G")
        || item.official_rating.as_deref() == Some("PG");
    item.is_live = false;

    // SourceType & EnableMediaSourceDisplay
    item.source_type = Some("Default".to_string());
    item.enable_media_source_display = matches!(
        item.media_type,
        rustyfin_model::media::MediaType::Movie
            | rustyfin_model::media::MediaType::Episode
            | rustyfin_model::media::MediaType::Audio
    );

    // ProviderIds from tmdb_id/tvdb_id
    let mut provider_ids = rustyfin_model::media::ProviderIds::default();
    if let Some(tmdb) = item.tmdb_id {
        provider_ids.set("Tmdb".to_string(), tmdb.to_string());
    }
    if let Some(tvdb) = item.tvdb_id {
        provider_ids.set("Tvdb".to_string(), tvdb.to_string());
    }
    item.provider_ids = provider_ids;

    // Jellyfin import: metadata columns that live outside media_items
    if let Ok(Some(extras)) = rustyfin_db::repo_jellyfin_import::item_extras(pool, item.id).await {
        if let Some(title) = extras.original_title {
            item.original_title = Some(title);
        }
        if let Some(rating) = extras.critic_rating {
            item.critic_rating = Some(rating);
        }
        if let Some(imdb) = extras.imdb_id {
            item.provider_ids.set("Imdb".to_string(), imdb);
        }
        item.forced_sort_name = extras.forced_sort_name.filter(|s| !s.trim().is_empty());
        item.lock_data = extras.lock_data;
        item.locked_fields = extras
            .locked_fields
            .split('|')
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .collect();
    }

    // Photos: what the scan read from the file's header and EXIF block
    if item.media_type == rustyfin_model::media::MediaType::Photo
        && let Ok(Some(info)) = rustyfin_db::repo_photo::get_photo_info(pool, item.id).await
    {
        apply_photo_info(item, &info);
    }

    // ExternalUrls from provider_ids
    let mut external_urls = Vec::new();
    if let Some(tmdb) = item.tmdb_id
        && let Some(kind) = tmdb_kind(item)
    {
        external_urls.push(rustyfin_model::media::ExternalUrl {
            name: "TMDb".to_string(),
            url: format!("https://www.themoviedb.org/{}/{}", kind, tmdb),
        });
    }
    if let Some(tvdb) = item.tvdb_id {
        external_urls.push(rustyfin_model::media::ExternalUrl {
            name: "TheTVDB".to_string(),
            url: format!("https://www.thetvdb.com/?id={}", tvdb),
        });
    }
    item.external_urls = external_urls;

    // RemoteTrailers from TMDb API (real YouTube trailer URLs)
    if let Some(tmdb_id) = item.tmdb_id
        && let Some(api) = tmdb_api
        && let Some(kind) = tmdb_kind(item)
    {
        let trailers = rustyfin_core::tmdb_client::get_trailers(api, kind, tmdb_id).await;
        item.remote_trailers = trailers
            .into_iter()
            .map(|(name, url)| rustyfin_model::media::ExternalUrl { name, url })
            .collect();
        item.trailer_count = Some(item.remote_trailers.len() as i32);
    }
    // The local trailers and special features (extras).
    let (trailers, specials) = rustyfin_db::repo_extras::extra_counts(pool, &[item.id.to_string()])
        .await
        .ok()
        .and_then(|counts| counts.into_values().next())
        .unwrap_or_default();
    item.local_trailer_count = Some(trailers);
    item.special_feature_count = Some(specials);
    // A movie split into files: Jellyfin's PartCount, part 1 included.
    let parts = rustyfin_db::repo_extras::additional_part_count(pool, item.id)
        .await
        .unwrap_or(0);
    if parts > 0 {
        item.part_count = Some(parts as i32 + 1);
    }

    // Parallel queries: images, chapters, trickplay, user_data, people, studios
    // These are independent and can run concurrently instead of sequentially.
    let item_id = item.id;
    let pool_clone = pool.clone();

    let images_future = rustyfin_db::repo_image::get_images_for_item(&pool_clone, item_id);
    let chapters_future = rustyfin_db::repo_chapter::get_chapters(&pool_clone, item_id);
    let trickplay_future = rustyfin_db::repo_trickplay::get_trickplay(&pool_clone, item_id);
    let user_data_future = async {
        match user_id {
            Some(uid) => {
                rustyfin_db::repo_playback::get_user_item_data(&pool_clone, uid, item_id).await
            }
            None => Err(sqlx::Error::RowNotFound), // Skip if no user
        }
    };
    let people_future = rustyfin_db::repo_media::get_artists_for_audio(&pool_clone, item_id);
    let studios_future = rustyfin_db::repo_media::get_studios_for_item(&pool_clone, item_id);

    let (
        images_result,
        chapters_result,
        trickplay_result,
        user_data_result,
        people_result,
        studios_result,
    ) = tokio::join!(
        images_future,
        chapters_future,
        trickplay_future,
        user_data_future,
        people_future,
        studios_future
    );

    // ImageTags from images table
    if let Ok(images) = images_result {
        set_image_tags(item, &images);

        // ImageBlurHashes: prefer stored values, fall back to on-demand computation
        for img in images {
            // Use pre-computed blurhash from the database if available
            if let Some(ref hash) = img.blurhash {
                add_blur_hash(item, &img, hash.clone());
            } else if !img.path.starts_with("http://") && !img.path.starts_with("https://") {
                // Fall back to on-demand computation for images without stored blurhash
                let full_path = data_dir.join(&img.path);
                if let Ok(Some(hash)) = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    rustyfin_core::image_proxy::compute_blurhash(
                        full_path.to_str().unwrap_or(""),
                        Some(http_client),
                    ),
                )
                .await
                {
                    add_blur_hash(item, &img, hash);
                }
            }
        }
    }

    // OriginalTitle (same as name if not set)
    if item.original_title.is_none() {
        item.original_title = Some(item.name.clone());
    }

    // Chapters from chapters table
    if let Ok(chapters) = chapters_result {
        set_chapters(item, &chapters);
    }

    // Trickplay tile sets
    if let Ok(trickplay) = trickplay_result {
        set_trickplay(item, trickplay);
    }

    // Etag from date_created
    item.etag = Some(item.date_created.to_rfc3339());

    // CompletionPercentage from user_item_data
    if let Ok(user_data) = user_data_result {
        apply_user_data(item, user_data);
    }

    // Media source count and streams
    if !item.media_sources.is_empty() {
        item.media_source_count = item.media_sources.len() as i32;
        if let Some(source) = item.media_sources.first() {
            item.media_streams = source.media_streams.clone();
        }
    }

    // Display preferences ID
    item.display_preferences_id = Some(item.id.to_string());

    // Play access
    item.play_access = Some("Full".to_string());

    // --- Episode-specific fields ---
    if item.media_type == rustyfin_model::media::MediaType::Episode {
        // Number: "S01E03" format
        if let (Some(season), Some(ep)) = (item.parent_index_number, item.index_number) {
            item.number = Some(format!("{:02}x{:02}", season, ep));
        }
        // EpisodeTitle: "{Season N} - Episode M - Name"
        if let Some(season) = item.parent_index_number {
            item.episode_title = Some(format!(
                "Season {} - Episode {} - {}",
                season,
                item.index_number.unwrap_or(0),
                item.name
            ));
        }
        // SeriesName, SeriesId, SeriesPrimaryImageTag from grandparent (the Series)
        // Load Season once and reuse for both series info and parent image tags.
        // Season and Series are loaded in parallel to reduce latency.
        if let Some(parent_id) = item.parent_id
            && let Ok(Some(mut season_item)) =
                rustyfin_db::repo_media::get_parent_item(pool, parent_id).await
        {
            load_image_tags(pool, &mut season_item).await;
            item.season_name = Some(season_item.name.clone());
            item.season_id = Some(season_item.id);

            // Fill parent image tags from Season (reuses the already-loaded season_item)
            if let Some(tag) = season_item
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.parent_primary_image_tag = Some(tag.clone());
                item.parent_primary_image_item_id = Some(season_item.id);
            }
            if let Some(tag) = season_item
                .image_tags
                .get(&rustyfin_model::image::ImageType::Thumb)
            {
                item.parent_thumb_image_tag = Some(tag.clone());
                item.parent_thumb_item_id = Some(season_item.id);
            }
            if let Some(tag) = season_item
                .image_tags
                .get(&rustyfin_model::image::ImageType::Art)
            {
                item.parent_art_image_tag = Some(tag.clone());
                item.parent_art_item_id = Some(season_item.id);
            }
            if let Some(tag) = season_item
                .image_tags
                .get(&rustyfin_model::image::ImageType::Logo)
            {
                item.parent_logo_image_tag = Some(tag.clone());
                item.parent_logo_item_id = Some(season_item.id);
            }
            // Parent backdrop
            if !season_item.backdrop_image_tags.is_empty() {
                item.parent_backdrop_image_tags = season_item.backdrop_image_tags.clone();
                item.parent_backdrop_item_id = Some(season_item.id);
            }

            // SeriesPrimaryImageTag from the Series (grandparent)
            if let Some(series_id) = season_item.parent_id
                && let Ok(Some(mut series_item)) =
                    rustyfin_db::repo_media::get_parent_item(pool, series_id).await
            {
                load_image_tags(pool, &mut series_item).await;
                item.series_name = Some(series_item.name.clone());
                item.series_id = Some(series_item.id);
                if let Some(tag) = series_item
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Primary)
                {
                    item.series_primary_image_tag = Some(tag.clone());
                }
                if let Some(tag) = series_item
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Thumb)
                {
                    item.series_thumb_image_tag = Some(tag.clone());
                }
                // The series' Thumb replaces the season's, as in Jellyfin's AddInheritedImages.
                // The parent Backdrop falls back to the series when the season has none.
                if let Some(tag) = series_item
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Thumb)
                {
                    item.parent_thumb_image_tag = Some(tag.clone());
                    item.parent_thumb_item_id = Some(series_item.id);
                }
                if item.parent_backdrop_image_tags.is_empty()
                    && !series_item.backdrop_image_tags.is_empty()
                {
                    item.parent_backdrop_image_tags = series_item.backdrop_image_tags.clone();
                    item.parent_backdrop_item_id = Some(series_item.id);
                }
            } else if let Some(series_id) = season_item.parent_id {
                item.series_id = Some(series_id);
            }
        }
    }

    // --- Audio-specific fields ---
    if item.media_type == rustyfin_model::media::MediaType::Audio {
        // Album info from parent (MusicAlbum)
        if let Some(parent_id) = item.parent_id
            && let Ok(Some(mut album)) =
                rustyfin_db::repo_media::get_album_for_audio(pool, parent_id).await
        {
            load_image_tags(pool, &mut album).await;
            item.album = Some(album.name.clone());
            item.album_id = Some(album.id);
            // Parent image tags from album
            if let Some(tag) = album
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.parent_primary_image_tag = Some(tag.clone());
                item.parent_primary_image_item_id = Some(album.id);
            }
            if let Some(tag) = album.image_tags.get(&rustyfin_model::image::ImageType::Art) {
                item.parent_art_image_tag = Some(tag.clone());
                item.parent_art_item_id = Some(album.id);
            }
            // Album artists: the album's AlbumArtist links, else its parent artist item.
            let links = rustyfin_db::repo_person::get_links_batch(pool, &[album.id], "AlbumArtist")
                .await
                .unwrap_or_default();
            let album_parent = match album.parent_id {
                Some(pid) => rustyfin_db::repo_media::get_parent_item(pool, pid)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let album_links = links
                .get(&album.id.to_string())
                .cloned()
                .unwrap_or_default();
            item.album_artists = album_artists_of(&album_links, album_parent.as_ref());
            item.album_artist = item.album_artists.first().map(|a| a.name.clone());
        }
        // Artists: the track's Artist links in credit order, already fetched in parallel.
        if let Ok(rows) = &people_result {
            let artists: Vec<&rustyfin_db::repo_media::PersonRow> =
                rows.iter().filter(|row| row.3 == "Artist").collect();
            item.artists = artists.iter().map(|row| row.1.clone()).collect();
            item.artist_items = artists
                .iter()
                .map(|row| rustyfin_model::media::GenreItem {
                    name: row.1.clone(),
                    id: row.0.clone(),
                })
                .collect();
            // Also populate People for audio items
            item.people = artists
                .iter()
                .map(
                    |(pid, name, role, _, primary_path)| rustyfin_model::media::PersonInfo {
                        id: pid.clone(),
                        name: name.clone(),
                        role: role.clone(),
                        person_type: Some("Artist".to_string()),
                        primary_image_tag: primary_path
                            .as_deref()
                            .map(|path| rustyfin_core::image_proxy::image_tag(path, None)),
                    },
                )
                .collect();
        }
    } else if item.media_type == rustyfin_model::media::MediaType::MusicAlbum {
        // Album artists from the album's own AlbumArtist links, else its parent artist.
        let links = rustyfin_db::repo_person::get_links_batch(pool, &[item.id], "AlbumArtist")
            .await
            .unwrap_or_default();
        let parent = match item.parent_id {
            Some(pid) => rustyfin_db::repo_media::get_parent_item(pool, pid)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let album_links = links.get(&item.id.to_string()).cloned().unwrap_or_default();
        item.album_artists = album_artists_of(&album_links, parent.as_ref());
        item.album_artist = item.album_artists.first().map(|a| a.name.clone());
        // Artists: the distinct artists of the album's tracks.
        let track_artists = rustyfin_db::repo_person::album_track_artists_batch(pool, &[item.id])
            .await
            .unwrap_or_default();
        let (artists, artist_items) = album_credit(
            track_artists
                .get(&item.id.to_string())
                .map(Vec::as_slice)
                .unwrap_or_default(),
            &item.album_artists,
        );
        item.artists = artists;
        item.artist_items = artist_items;
    }

    // --- People for all non-audio items (Movie, Episode, Series, etc.) ---
    if item.media_type != rustyfin_model::media::MediaType::Audio
        && item.people.is_empty()
        && let Ok(people_rows) = &people_result
    {
        item.people = people_rows
            .iter()
            .map(
                |(pid, name, role, kind, primary_path)| rustyfin_model::media::PersonInfo {
                    id: pid.clone(),
                    name: name.clone(),
                    role: role.clone(),
                    person_type: Some(kind.clone()),
                    primary_image_tag: primary_path
                        .as_deref()
                        .map(|path| rustyfin_core::image_proxy::image_tag(path, None)),
                },
            )
            .collect();
    }

    // --- Studios from item_studios table, with the ids that /Studios lists ---
    if item.studios.is_empty()
        && let Ok(studio_rows) = studios_result
    {
        let studio = rustyfin_db::repo_by_name::ByNameKind::Studio;
        item.studios = studio_rows
            .into_iter()
            .map(|(_, sname)| rustyfin_model::media::StudioInfo {
                id: rustyfin_db::repo_by_name::by_name_id(studio, &sname).to_string(),
                name: sname,
            })
            .collect();
    }

    // --- Folder counts (for Series, Season, MusicAlbum, Folder), of every child: `media_item`
    // caches this item for every user and counts again for a user with limits ---
    if item.media_type.is_folder() {
        if let Ok(counts) =
            rustyfin_db::repo_media::count_items_by_type(pool, item.id, None, false).await
        {
            set_folder_counts(item, counts);
        }
        // Display order for folders
        item.display_order = Some("SortName".to_string());
    }

    // --- Parent image tags for non-episode items ---
    if item.media_type != rustyfin_model::media::MediaType::Episode
        && let Some(parent_id) = item.parent_id
        && let Ok(Some(mut parent)) =
            rustyfin_db::repo_media::get_parent_item(pool, parent_id).await
    {
        load_image_tags(pool, &mut parent).await;
        if item.parent_primary_image_tag.is_none()
            && let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
        {
            item.parent_primary_image_tag = Some(tag.clone());
            item.parent_primary_image_item_id = Some(parent.id);
        }
        if item.parent_thumb_image_tag.is_none()
            && let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Thumb)
        {
            item.parent_thumb_image_tag = Some(tag.clone());
            item.parent_thumb_item_id = Some(parent.id);
        }
        if item.parent_art_image_tag.is_none()
            && let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Art)
        {
            item.parent_art_image_tag = Some(tag.clone());
            item.parent_art_item_id = Some(parent.id);
        }
        if item.parent_logo_image_tag.is_none()
            && let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Logo)
        {
            item.parent_logo_image_tag = Some(tag.clone());
            item.parent_logo_item_id = Some(parent.id);
        }
        if item.parent_backdrop_image_tags.is_empty() && !parent.backdrop_image_tags.is_empty() {
            item.parent_backdrop_image_tags = parent.backdrop_image_tags.clone();
            item.parent_backdrop_item_id = Some(parent.id);
        }
    }

    // --- Series branding for Seasons ---
    if item.media_type == rustyfin_model::media::MediaType::Season
        && let Some(series_id) = item.parent_id
        && let Ok(Some(mut series)) =
            rustyfin_db::repo_media::get_parent_item(pool, series_id).await
    {
        load_image_tags(pool, &mut series).await;
        item.series_name = Some(series.name.clone());
        item.series_id = Some(series.id);
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Primary)
        {
            item.series_primary_image_tag = Some(tag.clone());
        }
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Thumb)
        {
            item.series_thumb_image_tag = Some(tag.clone());
        }
        // Parent image tags from series
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Primary)
        {
            item.parent_primary_image_tag = Some(tag.clone());
            item.parent_primary_image_item_id = Some(series.id);
        }
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Thumb)
        {
            item.parent_thumb_image_tag = Some(tag.clone());
            item.parent_thumb_item_id = Some(series.id);
        }
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Art)
        {
            item.parent_art_image_tag = Some(tag.clone());
            item.parent_art_item_id = Some(series.id);
        }
        if let Some(tag) = series
            .image_tags
            .get(&rustyfin_model::image::ImageType::Logo)
        {
            item.parent_logo_image_tag = Some(tag.clone());
            item.parent_logo_item_id = Some(series.id);
        }
        if !series.backdrop_image_tags.is_empty() {
            item.parent_backdrop_image_tags = series.backdrop_image_tags.clone();
            item.parent_backdrop_item_id = Some(series.id);
        }
    }

    // --- GenreItems from genres, with the ids that /Genres or /MusicGenres lists ---
    if item.genre_items.is_empty() && !item.genres.is_empty() {
        let kind = rustyfin_db::repo_by_name::genre_kind_of(item.media_type);
        for genre in &item.genres {
            item.genre_items.push(rustyfin_model::media::GenreItem {
                name: genre.clone(),
                id: rustyfin_db::repo_by_name::by_name_id(kind, genre).to_string(),
            });
        }
    }

    // --- DateLastMediaAdded for folders ---
    if item.media_type.is_folder()
        && let Ok(Some(last_added)) =
            rustyfin_db::repo_media::get_last_media_added(pool, item.id).await
    {
        item.date_last_media_added = Some(last_added);
    }
    set_top_level_parent(item);
}

/// Jellyfin's ParentId of an item at the top of a library: the library (its CollectionFolder).
/// An extra keeps none, as it hangs off its owner.
fn set_top_level_parent(item: &mut MediaItem) {
    if item.parent_id.is_none() && item.extra_type.is_none() && !item.library_id.is_nil() {
        item.parent_id = Some(item.library_id);
    }
}

/// Batch-enrich multiple MediaItems using batched DB queries.
/// Fetches chapters, trickplay, user_data, parent items, folder counts, trailers (from TMDb),
/// and blurhashes in bulk to avoid N+1 query problems.
/// If `skip_blurhash` is true, blurhash computation is skipped (useful for list views).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enrich_items(
    pool: &SqlitePool,
    server_id: &str,
    user_id: Option<Uuid>,
    items: &mut [MediaItem],
    http_client: &reqwest::Client,
    tmdb_api: Option<&TmdbApi>,
    data_dir: &std::path::Path,
    skip_blurhash: bool,
) {
    enrich_items_with(
        pool,
        server_id,
        user_id,
        items,
        http_client,
        tmdb_api,
        data_dir,
        skip_blurhash,
        &ImageOptions::default(),
    )
    .await
}

/// Jellyfin's image DTO options: EnableImages=false sends no image, EnableImageTypes keeps only
/// the types it names (all when it names none it knows), ImageTypeLimit keeps that many images of
/// a type (backdrops are the type with several). The default keeps everything.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ImageOptions {
    pub types: Option<Vec<rustyfin_model::image::ImageType>>,
    pub limit: Option<usize>,
}

impl ImageOptions {
    fn narrows(&self) -> bool {
        self.types.is_some() || self.limit.is_some()
    }

    fn sends(&self, image_type: rustyfin_model::image::ImageType) -> bool {
        self.limit != Some(0) && self.types.as_ref().is_none_or(|t| t.contains(&image_type))
    }

    /// Leaves out the images of an enriched item these options don't send: its own, its parents'
    /// and its series', and the BlurHashes of the images left out.
    fn apply(&self, item: &mut MediaItem) {
        use rustyfin_model::image::ImageType as T;
        if !self.narrows() {
            return;
        }
        item.image_tags.retain(|t, _| self.sends(*t));
        if !self.sends(T::Backdrop) {
            item.backdrop_image_tags.clear();
            item.parent_backdrop_image_tags.clear();
        }
        if let Some(n) = self.limit {
            item.backdrop_image_tags.truncate(n);
            item.parent_backdrop_image_tags.truncate(n);
        }
        if !self.sends(T::Primary) {
            item.series_primary_image_tag = None;
            item.parent_primary_image_tag = None;
        }
        if !self.sends(T::Thumb) {
            item.series_thumb_image_tag = None;
            item.parent_thumb_image_tag = None;
        }
        if !self.sends(T::Logo) {
            item.parent_logo_image_tag = None;
        }
        if !self.sends(T::Art) {
            item.parent_art_image_tag = None;
        }
        let kept: std::collections::HashSet<&String> = item
            .image_tags
            .values()
            .chain(&item.backdrop_image_tags)
            .chain(&item.parent_backdrop_image_tags)
            .chain(&item.series_primary_image_tag)
            .chain(&item.series_thumb_image_tag)
            .chain(&item.parent_primary_image_tag)
            .chain(&item.parent_thumb_image_tag)
            .chain(&item.parent_logo_image_tag)
            .chain(&item.parent_art_image_tag)
            .collect();
        let kept: std::collections::HashSet<String> = kept.into_iter().cloned().collect();
        item.image_blur_hashes.retain(|_, by_tag| {
            by_tag.retain(|tag, _| kept.contains(tag));
            !by_tag.is_empty()
        });
    }
}

/// `enrich_items` with Jellyfin's image options: the image rows read are only those `images`
/// sends, and the answer is narrowed to them (see `ImageOptions`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enrich_items_with(
    pool: &SqlitePool,
    server_id: &str,
    user_id: Option<Uuid>,
    items: &mut [MediaItem],
    http_client: &reqwest::Client,
    tmdb_api: Option<&TmdbApi>,
    data_dir: &std::path::Path,
    skip_blurhash: bool,
    images: &ImageOptions,
) {
    if items.is_empty() {
        return;
    }

    // Collect all item IDs for batch queries
    let item_ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();

    // Batch query 1: All chapters for all items
    let chapters_map = rustyfin_db::repo_chapter::get_chapters_batch(pool, &item_ids)
        .await
        .unwrap_or_default();

    // Batch query 2: All trickplay info for all items
    let mut trickplay_map = rustyfin_db::repo_trickplay::get_trickplay_batch(pool, &item_ids)
        .await
        .unwrap_or_default();

    // Batch query 3: All user item data for all items (if user_id provided)
    let user_data_map = if let Some(uid) = user_id {
        rustyfin_db::repo_playback::get_user_item_data_batch(pool, uid, &item_ids)
            .await
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    // Batch query 3a: what the scan read from the photos in the batch
    let photo_ids: Vec<Uuid> = items
        .iter()
        .filter(|item| item.media_type == rustyfin_model::media::MediaType::Photo)
        .map(|item| item.id)
        .collect();
    if !photo_ids.is_empty() {
        let photos = rustyfin_db::repo_photo::get_photo_infos(pool, &photo_ids)
            .await
            .unwrap_or_default();
        for item in items.iter_mut() {
            if let Some(info) = photos.get(&item.id) {
                apply_photo_info(item, info);
            }
        }
    }

    // Batch query 3a': which audio items have lyrics; Jellyfin sends HasLyrics on audio only
    let audio_ids: Vec<Uuid> = items
        .iter()
        .filter(|item| item.media_type == rustyfin_model::media::MediaType::Audio)
        .map(|item| item.id)
        .collect();
    if !audio_ids.is_empty() {
        let with_lyrics = rustyfin_db::repo_lyrics::with_lyrics(pool, &audio_ids)
            .await
            .unwrap_or_default();
        for item in items.iter_mut() {
            if item.media_type == rustyfin_model::media::MediaType::Audio {
                item.has_lyrics = Some(with_lyrics.contains(&item.id.to_string()));
            }
        }
    }

    // Batch query 3c: each item's Container, from its first media source, as Jellyfin sends it
    let containers = rustyfin_db::repo_media_source::first_containers(pool, &item_ids)
        .await
        .unwrap_or_default();
    for item in items.iter_mut() {
        if item.container.is_none() {
            item.container = containers.get(&item.id.to_string()).cloned();
        }
    }

    // What the user may see, for the counts of the folders in the batch; read only with folders.
    let visible = if items
        .iter()
        .any(|item| item.media_type.is_folder() || is_episode_folder(item))
    {
        crate::parental::user_filter(pool, user_id).await
    } else {
        None
    };

    // Batch query 3b: unplayed episode counts for the Series and Seasons in the batch
    let folder_ids: Vec<Uuid> = items
        .iter()
        .filter(|item| is_episode_folder(item))
        .map(|item| item.id)
        .collect();
    let unplayed_map = match user_id {
        Some(uid) if !folder_ids.is_empty() => rustyfin_db::repo_playback::get_unplayed_counts(
            pool,
            uid,
            &folder_ids,
            visible.as_deref(),
        )
        .await
        .unwrap_or_default(),
        _ => std::collections::HashMap::new(),
    };

    // Batch query 4: Collect unique parent IDs and fetch them
    let parent_ids: Vec<Uuid> = items
        .iter()
        .filter_map(|item| item.parent_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let mut parent_map = if parent_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        rustyfin_db::repo_media::get_parent_items_batch(pool, &parent_ids)
            .await
            .unwrap_or_default()
    };
    // Grandparents (an episode's series is its season's parent), so their images load too
    let grandparent_ids: Vec<Uuid> = parent_map
        .values()
        .filter_map(|parent| parent.parent_id)
        .filter(|id| !parent_map.contains_key(&id.to_string()))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    if !grandparent_ids.is_empty() {
        parent_map.extend(
            rustyfin_db::repo_media::get_parent_items_batch(pool, &grandparent_ids)
                .await
                .unwrap_or_default(),
        );
    }

    // Batch query 5: Folder counts for folder items (virtual items only with DisplayMissingEpisodes)
    let include_virtual = crate::routes::shows::display_missing_episodes(pool, user_id).await;
    let folder_ids: Vec<Uuid> = items
        .iter()
        .filter(|item| item.media_type.is_folder())
        .map(|item| item.id)
        .collect();
    let folder_counts_map = if folder_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        rustyfin_db::repo_media::get_folder_counts_batch(
            pool,
            &folder_ids,
            visible.as_deref(),
            include_virtual,
        )
        .await
        .unwrap_or_default()
    };

    // Batch query 6: All images for all items - populates image_tags. Only the image types and
    // counts the answer sends are read (one row an item for a Primary-only list).
    let images_map = rustyfin_db::repo_image::get_images_batch_of(
        pool,
        &item_ids,
        images.types.as_deref(),
        images.limit,
    )
    .await
    .unwrap_or_default();

    // Batch query 7: Images for parent items too (needed for inherited image tags)
    let parent_image_ids: Vec<Uuid> = parent_map.values().map(|p| p.id).collect();
    let parent_images_map = if parent_image_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        rustyfin_db::repo_image::get_images_batch_of(
            pool,
            &parent_image_ids,
            images.types.as_deref(),
            images.limit,
        )
        .await
        .unwrap_or_default()
    };

    // Batch query 7b: AlbumArtist links of the music albums that are listed or are parents of
    // listed items, and the distinct artists of their tracks.
    let album_link_ids: Vec<Uuid> = parent_map
        .values()
        .chain(items.iter())
        .filter(|item| item.media_type == rustyfin_model::media::MediaType::MusicAlbum)
        .map(|item| item.id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let album_links_map: std::collections::HashMap<String, Vec<(String, String)>> =
        rustyfin_db::repo_person::get_links_batch(pool, &album_link_ids, "AlbumArtist")
            .await
            .unwrap_or_default();
    let album_track_artists_map =
        rustyfin_db::repo_person::album_track_artists_batch(pool, &album_link_ids)
            .await
            .unwrap_or_default();

    // Batch query: studios and IMDb ids for all items
    let studios_map = rustyfin_db::repo_media::get_studios_for_items(pool, &item_ids)
        .await
        .unwrap_or_default();
    let imdb_map = rustyfin_db::repo_media::get_imdb_ids_for_items(pool, &item_ids)
        .await
        .unwrap_or_default();

    // Batch query 8: People for all items
    let people_map: std::collections::HashMap<String, Vec<rustyfin_db::repo_media::PersonRow>> = {
        let mut map: std::collections::HashMap<String, Vec<rustyfin_db::repo_media::PersonRow>> =
            std::collections::HashMap::new();
        for chunk_ids in item_ids.chunks(100) {
            let placeholders: Vec<String> =
                (1..=chunk_ids.len()).map(|i| format!("?{}", i)).collect();
            let query = format!(
                "SELECT ip.item_id, CASE WHEN ip.type IN ('Artist', 'AlbumArtist') THEN {} ELSE p.id END, p.name, ip.role, ip.type, (SELECT pi.path FROM person_images pi WHERE pi.person_id = p.id AND pi.image_type = 'Primary' AND pi.image_index = 0) FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id IN ({}) ORDER BY ip.item_id, ip.sort_order IS NULL, ip.sort_order, ip.rowid",
                rustyfin_db::repo_person::item_artist_id_sql(),
                placeholders.join(",")
            );
            let mut query_builder = sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    Option<String>,
                    String,
                    Option<String>,
                ),
            >(sqlx::AssertSqlSafe(query.clone()));
            for id in chunk_ids {
                query_builder = query_builder.bind(id.to_string());
            }
            if let Ok(rows) = query_builder.fetch_all(pool).await {
                for (item_id, pid, pname, role, kind, primary_path) in rows {
                    map.entry(item_id)
                        .or_default()
                        .push((pid, pname, role, kind, primary_path));
                }
            }
        }
        map
    };

    // Populate image_tags on parent items
    for parent_item in parent_map.values_mut() {
        if let Some(images) = parent_images_map.get(&parent_item.id.to_string()) {
            let mut tags: std::collections::HashMap<rustyfin_model::image::ImageType, String> =
                std::collections::HashMap::new();
            let mut backdrop_tags: Vec<String> = Vec::new();
            for img in images {
                let tag = rustyfin_core::image_proxy::image_tag(&img.path, img.stamp);
                if img.image_type == rustyfin_model::image::ImageType::Backdrop {
                    backdrop_tags.push(tag.clone());
                }
                tags.entry(img.image_type).or_insert_with(|| tag);
            }
            parent_item.image_tags = tags;
            parent_item.backdrop_image_tags = backdrop_tags;
        }
    }

    // Enrich each item from batch results
    for item in items.iter_mut() {
        // ServerId
        item.server_id = Some(server_id.to_string());

        // Type flags
        item.is_movie = item.media_type == rustyfin_model::media::MediaType::Movie;
        item.is_series = item.media_type == rustyfin_model::media::MediaType::Series;
        item.is_kids = item.official_rating.as_deref() == Some("G")
            || item.official_rating.as_deref() == Some("PG");
        item.is_live = false;

        // SourceType & EnableMediaSourceDisplay
        item.source_type = Some("Default".to_string());
        item.enable_media_source_display = matches!(
            item.media_type,
            rustyfin_model::media::MediaType::Movie
                | rustyfin_model::media::MediaType::Episode
                | rustyfin_model::media::MediaType::Audio
        );

        // ProviderIds from tmdb_id/tvdb_id
        let mut provider_ids = rustyfin_model::media::ProviderIds::default();
        if let Some(tmdb) = item.tmdb_id {
            provider_ids.set("Tmdb".to_string(), tmdb.to_string());
        }
        if let Some(tvdb) = item.tvdb_id {
            provider_ids.set("Tvdb".to_string(), tvdb.to_string());
        }
        if let Some(imdb) = imdb_map.get(&item.id.to_string()) {
            provider_ids.set("Imdb".to_string(), imdb.clone());
        }
        item.provider_ids = provider_ids;

        // ExternalUrls from provider_ids
        let mut external_urls = Vec::new();
        if let Some(tmdb) = item.tmdb_id
            && let Some(kind) = tmdb_kind(item)
        {
            external_urls.push(rustyfin_model::media::ExternalUrl {
                name: "TMDb".to_string(),
                url: format!("https://www.themoviedb.org/{}/{}", kind, tmdb),
            });
        }
        if let Some(tvdb) = item.tvdb_id {
            external_urls.push(rustyfin_model::media::ExternalUrl {
                name: "TheTVDB".to_string(),
                url: format!("https://www.thetvdb.com/?id={}", tvdb),
            });
        }
        item.external_urls = external_urls;

        // RemoteTrailers from TMDb API (fetched below in batch)
        // Placeholder - will be filled after the loop
        item.remote_trailers = Vec::new();
        item.trailer_count = Some(0);
        item.local_trailer_count = Some(0);

        // ImageTags from batch-loaded images
        if let Some(images) = images_map.get(&item.id.to_string()) {
            let mut tags: std::collections::HashMap<rustyfin_model::image::ImageType, String> =
                std::collections::HashMap::new();
            let mut backdrop_tags: Vec<String> = Vec::new();
            for img in images {
                let tag = rustyfin_core::image_proxy::image_tag(&img.path, img.stamp);
                if img.image_type == rustyfin_model::image::ImageType::Backdrop {
                    backdrop_tags.push(tag.clone());
                }
                tags.entry(img.image_type).or_insert_with(|| tag);
            }
            item.image_tags = tags;
            item.backdrop_image_tags = backdrop_tags;
            item.primary_image_aspect_ratio = images
                .iter()
                .find(|img| img.image_type == rustyfin_model::image::ImageType::Primary)
                .and_then(|img| primary_aspect_ratio(item.media_type, img));
        }

        // ImageBlurHashes: stored values always (lists paint them at once, as Jellyfin's do); an
        // image without one is computed on demand only when `skip_blurhash` is false. The image
        // warmer stores the missing ones in the background.
        {
            let images_for_blurhash = images_map
                .get(&item.id.to_string())
                .cloned()
                .unwrap_or_default();
            for img in images_for_blurhash {
                // Use pre-computed blurhash from the database if available
                if let Some(ref hash) = img.blurhash {
                    add_blur_hash(item, &img, hash.clone());
                } else if !skip_blurhash
                    && !img.path.starts_with("http://")
                    && !img.path.starts_with("https://")
                {
                    // Fall back to on-demand computation
                    let full_path = data_dir.join(&img.path);
                    if let Ok(Some(hash)) = tokio::time::timeout(
                        std::time::Duration::from_millis(500),
                        rustyfin_core::image_proxy::compute_blurhash(
                            full_path.to_str().unwrap_or(""),
                            Some(http_client),
                        ),
                    )
                    .await
                    {
                        add_blur_hash(item, &img, hash);
                    }
                }
            }
        }

        // OriginalTitle
        if item.original_title.is_none() {
            item.original_title = Some(item.name.clone());
        }

        // Chapters from batch map
        if let Some(chapters) = chapters_map.get(&item.id.to_string()) {
            set_chapters(item, chapters);
        }

        // Trickplay tile sets from batch map
        if let Some(trickplay) = trickplay_map.remove(&item.id.to_string()) {
            set_trickplay(item, trickplay);
        }

        // Etag from date_created
        item.etag = Some(item.date_created.to_rfc3339());

        // CompletionPercentage and PlayedPercentage from user_data batch map. Series, seasons and
        // episodes always have user data when the user is known (defaults without a row), and a
        // Series or Season counts its unplayed episodes.
        let stored = user_data_map.get(&item.id.to_string()).cloned();
        let folder = is_episode_folder(item);
        let in_show = folder || item.media_type == rustyfin_model::media::MediaType::Episode;
        let user_data = match stored {
            Some(data) => Some(data),
            None if in_show && user_id.is_some() => Some(Default::default()),
            None => None,
        };
        if let Some(user_data) = user_data {
            apply_user_data(item, user_data);
            if folder {
                let counts = unplayed_map
                    .get(&item.id.to_string())
                    .copied()
                    .unwrap_or((0, 0));
                apply_unplayed_counts(item, counts);
            }
        } else if user_id.is_some() {
            // Jellyfin sends every item's UserData for a user; one never played has the defaults.
            item.user_data =
                Some(rustyfin_model::playback::UserItemDataDto::default().for_item(item.id));
        }

        // Media source count and streams
        if !item.media_sources.is_empty() {
            item.media_source_count = item.media_sources.len() as i32;
            if let Some(source) = item.media_sources.first() {
                item.media_streams = source.media_streams.clone();
            }
        }

        // Display preferences ID
        item.display_preferences_id = Some(item.id.to_string());

        // Play access
        item.play_access = Some("Full".to_string());

        // --- Episode-specific fields ---
        if item.media_type == rustyfin_model::media::MediaType::Episode {
            if let (Some(season), Some(ep)) = (item.parent_index_number, item.index_number) {
                item.number = Some(format!("{:02}x{:02}", season, ep));
            }
            if let Some(season) = item.parent_index_number {
                item.episode_title = Some(format!(
                    "Season {} - Episode {} - {}",
                    season,
                    item.index_number.unwrap_or(0),
                    item.name
                ));
            }
            // Series info from grandparent
            if let Some(parent_id) = item.parent_id {
                if let Some(season_item) = parent_map.get(&parent_id.to_string()) {
                    item.season_name = Some(season_item.name.clone());
                    item.season_id = Some(season_item.id);
                    if let Some(series_id) = season_item.parent_id
                        && let Some(series_item) = parent_map.get(&series_id.to_string())
                    {
                        item.series_name = Some(series_item.name.clone());
                        item.series_id = Some(series_item.id);
                        if let Some(tag) = series_item
                            .image_tags
                            .get(&rustyfin_model::image::ImageType::Primary)
                        {
                            item.series_primary_image_tag = Some(tag.clone());
                        }
                        if let Some(tag) = series_item
                            .image_tags
                            .get(&rustyfin_model::image::ImageType::Thumb)
                        {
                            item.series_thumb_image_tag = Some(tag.clone());
                        }
                    } else if let Some(series_id) = season_item.parent_id {
                        item.series_id = Some(series_id);
                    }
                }
                // Parent image tags from Season
                if let Some(parent) = parent_map.get(&parent_id.to_string()) {
                    if let Some(tag) = parent
                        .image_tags
                        .get(&rustyfin_model::image::ImageType::Primary)
                    {
                        item.parent_primary_image_tag = Some(tag.clone());
                        item.parent_primary_image_item_id = Some(parent.id);
                    }
                    if let Some(tag) = parent
                        .image_tags
                        .get(&rustyfin_model::image::ImageType::Thumb)
                    {
                        item.parent_thumb_image_tag = Some(tag.clone());
                        item.parent_thumb_item_id = Some(parent.id);
                    }
                    if let Some(tag) = parent
                        .image_tags
                        .get(&rustyfin_model::image::ImageType::Art)
                    {
                        item.parent_art_image_tag = Some(tag.clone());
                        item.parent_art_item_id = Some(parent.id);
                    }
                    if let Some(tag) = parent
                        .image_tags
                        .get(&rustyfin_model::image::ImageType::Logo)
                    {
                        item.parent_logo_image_tag = Some(tag.clone());
                        item.parent_logo_item_id = Some(parent.id);
                    }
                    if !parent.backdrop_image_tags.is_empty() {
                        item.parent_backdrop_image_tags = parent.backdrop_image_tags.clone();
                        item.parent_backdrop_item_id = Some(parent.id);
                    }
                    // The series' Thumb replaces the season's, as in Jellyfin's AddInheritedImages.
                    // The parent Backdrop falls back to the series when the season has none.
                    if let Some(series) = parent
                        .parent_id
                        .and_then(|sid| parent_map.get(&sid.to_string()))
                    {
                        if let Some(tag) = series
                            .image_tags
                            .get(&rustyfin_model::image::ImageType::Thumb)
                        {
                            item.parent_thumb_image_tag = Some(tag.clone());
                            item.parent_thumb_item_id = Some(series.id);
                        }
                        if item.parent_backdrop_image_tags.is_empty()
                            && !series.backdrop_image_tags.is_empty()
                        {
                            item.parent_backdrop_image_tags = series.backdrop_image_tags.clone();
                            item.parent_backdrop_item_id = Some(series.id);
                        }
                    }
                }
            }
        }

        // --- Audio-specific fields ---
        if item.media_type == rustyfin_model::media::MediaType::Audio
            && let Some(parent_id) = item.parent_id
            && let Some(parent) = parent_map.get(&parent_id.to_string())
            && parent.media_type == rustyfin_model::media::MediaType::MusicAlbum
        {
            item.album = Some(parent.name.clone());
            item.album_id = Some(parent.id);
            if let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.parent_primary_image_tag = Some(tag.clone());
                item.parent_primary_image_item_id = Some(parent.id);
            }
            if let Some(tag) = parent
                .image_tags
                .get(&rustyfin_model::image::ImageType::Art)
            {
                item.parent_art_image_tag = Some(tag.clone());
                item.parent_art_item_id = Some(parent.id);
            }
        }

        // --- Folder counts ---
        if item.media_type.is_folder() {
            if let Some(counts) = folder_counts_map.get(&item.id.to_string()) {
                let mut song_count: i32 = 0;
                let mut album_count: i32 = 0;
                let mut movie_count: i32 = 0;
                let mut series_count: i32 = 0;
                let mut episode_count: i32 = 0;
                let mut child_count: i32 = 0;
                for (media_type, count) in counts {
                    let c = *count as i32;
                    match media_type.as_str() {
                        "Audio" => song_count += c,
                        "MusicAlbum" => album_count += c,
                        "Movie" => movie_count += c,
                        "Series" => series_count += c,
                        "Episode" => episode_count += c,
                        _ => {}
                    }
                    child_count += c;
                }
                item.song_count = if song_count > 0 {
                    Some(song_count)
                } else {
                    None
                };
                item.album_count = if album_count > 0 {
                    Some(album_count)
                } else {
                    None
                };
                item.movie_count = if movie_count > 0 {
                    Some(movie_count)
                } else {
                    None
                };
                item.series_count = if series_count > 0 {
                    Some(series_count)
                } else {
                    None
                };
                item.episode_count = if episode_count > 0 {
                    Some(episode_count)
                } else {
                    None
                };
                item.child_count = if child_count > 0 {
                    Some(child_count)
                } else {
                    None
                };
            }
            item.display_order = Some("SortName".to_string());
        }

        // --- Parent image tags for non-episode items ---
        if item.media_type != rustyfin_model::media::MediaType::Episode
            && let Some(parent_id) = item.parent_id
            && let Some(parent) = parent_map.get(&parent_id.to_string())
        {
            if item.parent_primary_image_tag.is_none()
                && let Some(tag) = parent
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.parent_primary_image_tag = Some(tag.clone());
                item.parent_primary_image_item_id = Some(parent.id);
            }
            if item.parent_thumb_image_tag.is_none()
                && let Some(tag) = parent
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Thumb)
            {
                item.parent_thumb_image_tag = Some(tag.clone());
                item.parent_thumb_item_id = Some(parent.id);
            }
            if item.parent_art_image_tag.is_none()
                && let Some(tag) = parent
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Art)
            {
                item.parent_art_image_tag = Some(tag.clone());
                item.parent_art_item_id = Some(parent.id);
            }
            if item.parent_logo_image_tag.is_none()
                && let Some(tag) = parent
                    .image_tags
                    .get(&rustyfin_model::image::ImageType::Logo)
            {
                item.parent_logo_image_tag = Some(tag.clone());
                item.parent_logo_item_id = Some(parent.id);
            }
            if item.parent_backdrop_image_tags.is_empty() && !parent.backdrop_image_tags.is_empty()
            {
                item.parent_backdrop_image_tags = parent.backdrop_image_tags.clone();
                item.parent_backdrop_item_id = Some(parent.id);
            }
        }

        // --- Series branding for Seasons ---
        if item.media_type == rustyfin_model::media::MediaType::Season
            && let Some(series_id) = item.parent_id
            && let Some(series) = parent_map.get(&series_id.to_string())
        {
            item.series_name = Some(series.name.clone());
            item.series_id = Some(series.id);
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.series_primary_image_tag = Some(tag.clone());
            }
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Thumb)
            {
                item.series_thumb_image_tag = Some(tag.clone());
            }
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Primary)
            {
                item.parent_primary_image_tag = Some(tag.clone());
                item.parent_primary_image_item_id = Some(series.id);
            }
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Thumb)
            {
                item.parent_thumb_image_tag = Some(tag.clone());
                item.parent_thumb_item_id = Some(series.id);
            }
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Art)
            {
                item.parent_art_image_tag = Some(tag.clone());
                item.parent_art_item_id = Some(series.id);
            }
            if let Some(tag) = series
                .image_tags
                .get(&rustyfin_model::image::ImageType::Logo)
            {
                item.parent_logo_image_tag = Some(tag.clone());
                item.parent_logo_item_id = Some(series.id);
            }
            if !series.backdrop_image_tags.is_empty() {
                item.parent_backdrop_image_tags = series.backdrop_image_tags.clone();
                item.parent_backdrop_item_id = Some(series.id);
            }
        }

        // --- BlurHashes of the parent and series pictures the item names, as Jellyfin sends
        // them, so a card that shows one of those can paint it at once ---
        let shown: std::collections::HashSet<String> = item
            .parent_thumb_image_tag
            .iter()
            .chain(&item.series_thumb_image_tag)
            .chain(&item.series_primary_image_tag)
            .chain(&item.parent_logo_image_tag)
            .chain(&item.parent_art_image_tag)
            .chain(&item.parent_backdrop_image_tags)
            .cloned()
            .collect();
        if !shown.is_empty() {
            let mut owners = [
                item.parent_id,
                item.series_id,
                item.parent_thumb_item_id,
                item.parent_backdrop_item_id,
            ];
            owners.sort();
            let mut last = None;
            for owner in owners.into_iter().flatten() {
                if last == Some(owner) {
                    continue;
                }
                last = Some(owner);
                for img in parent_images_map
                    .get(&owner.to_string())
                    .into_iter()
                    .flatten()
                {
                    if let Some(hash) = &img.blurhash
                        && shown
                            .contains(&rustyfin_core::image_proxy::image_tag(&img.path, img.stamp))
                    {
                        add_blur_hash(item, img, hash.clone());
                    }
                }
            }
        }

        // --- GenreItems from genres, with the ids that /Genres or /MusicGenres lists ---
        if item.genre_items.is_empty() && !item.genres.is_empty() {
            let kind = rustyfin_db::repo_by_name::genre_kind_of(item.media_type);
            for genre in &item.genres {
                item.genre_items.push(rustyfin_model::media::GenreItem {
                    name: genre.clone(),
                    id: rustyfin_db::repo_by_name::by_name_id(kind, genre).to_string(),
                });
            }
        }

        // --- People from batch map ---
        if item.people.is_empty()
            && let Some(people_rows) = people_map.get(&item.id.to_string())
        {
            item.people = people_rows
                .iter()
                .map(
                    |(pid, name, role, kind, primary_path)| rustyfin_model::media::PersonInfo {
                        id: pid.clone(),
                        name: name.clone(),
                        role: role.clone(),
                        person_type: Some(kind.clone()),
                        primary_image_tag: primary_path
                            .as_deref()
                            .map(|path| rustyfin_core::image_proxy::image_tag(path, None)),
                    },
                )
                .collect();
        }

        // --- Music artist fields from the batched links ---
        if item.media_type == rustyfin_model::media::MediaType::Audio {
            let own: Vec<(String, String)> = people_map
                .get(&item.id.to_string())
                .map(|rows| {
                    rows.iter()
                        .filter(|row| row.3 == "Artist")
                        .map(|row| (row.0.clone(), row.1.clone()))
                        .collect()
                })
                .unwrap_or_default();
            item.artists = own.iter().map(|(_, name)| name.clone()).collect();
            item.artist_items = own
                .iter()
                .map(|(id, name)| rustyfin_model::media::GenreItem {
                    name: name.clone(),
                    id: id.clone(),
                })
                .collect();
            let album = item
                .parent_id
                .and_then(|pid| parent_map.get(&pid.to_string()))
                .filter(|p| p.media_type == rustyfin_model::media::MediaType::MusicAlbum);
            let album_links = album
                .and_then(|a| album_links_map.get(&a.id.to_string()))
                .cloned()
                .unwrap_or_default();
            let album_parent = album
                .and_then(|a| a.parent_id)
                .and_then(|pid| parent_map.get(&pid.to_string()));
            item.album_artists = album_artists_of(&album_links, album_parent);
            item.album_artist = item.album_artists.first().map(|a| a.name.clone());
        } else if item.media_type == rustyfin_model::media::MediaType::MusicAlbum {
            let album_links = album_links_map
                .get(&item.id.to_string())
                .cloned()
                .unwrap_or_default();
            let parent = item
                .parent_id
                .and_then(|pid| parent_map.get(&pid.to_string()));
            item.album_artists = album_artists_of(&album_links, parent);
            item.album_artist = item.album_artists.first().map(|a| a.name.clone());
            let track_artists = album_track_artists_map
                .get(&item.id.to_string())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let (artists, artist_items) = album_credit(track_artists, &item.album_artists);
            item.artists = artists;
            item.artist_items = artist_items;
        }

        // --- Studios from batch map, with the ids that /Studios lists (as enrich_item) ---
        if item.studios.is_empty()
            && let Some(studio_rows) = studios_map.get(&item.id.to_string())
        {
            let studio = rustyfin_db::repo_by_name::ByNameKind::Studio;
            item.studios = studio_rows
                .iter()
                .map(|(_, sname)| rustyfin_model::media::StudioInfo {
                    id: rustyfin_db::repo_by_name::by_name_id(studio, sname).to_string(),
                    name: sname.clone(),
                })
                .collect();
        }
    }

    // Batch fetch trailers from TMDb API for items with tmdb_id
    if let Some(api) = tmdb_api {
        let trailer_items: Vec<(usize, i64, bool)> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| tmdb_kind(item).is_some())
            .filter_map(|(i, item)| item.tmdb_id.map(|tmdb| (i, tmdb, item.is_movie)))
            .collect();

        for (idx, tmdb_id, is_movie) in trailer_items {
            let kind = if is_movie { "movie" } else { "tv" };
            let trailers = rustyfin_core::tmdb_client::get_trailers(api, kind, tmdb_id).await;
            let item = &mut items[idx];
            item.remote_trailers = trailers
                .into_iter()
                .map(|(name, url)| rustyfin_model::media::ExternalUrl { name, url })
                .collect();
            item.trailer_count = Some(item.remote_trailers.len() as i32);
        }
    }

    // The local trailers and special features (extras), in one query.
    let ids: Vec<String> = items.iter().map(|item| item.id.to_string()).collect();
    let counts = rustyfin_db::repo_extras::extra_counts(pool, &ids)
        .await
        .unwrap_or_default();
    for item in items.iter_mut() {
        let (trailers, specials) = counts
            .get(&item.id.to_string())
            .copied()
            .unwrap_or_default();
        item.local_trailer_count = Some(trailers);
        item.special_feature_count = Some(specials);
    }

    for item in items.iter_mut() {
        images.apply(item);
        set_top_level_parent(item);
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Items", get(get_items_for_query_user))
        // DELETE checks the caller itself: a playlist's owner may delete it, as in Jellyfin.
        .route("/Items/{id}", get(get_item).delete(delete_item))
        .route("/Items/{id}/Children", get(get_children))
        .route("/Items/{id}/Similar", get(get_similar))
        .route("/Items/Latest", get(get_latest))
        .route("/Items/Resume", get(get_resume))
        .route("/UserItems/Resume", get(get_resume))
        .route("/Items/Filters", get(get_filters))
        .route("/Items/Filters2", get(get_filters2))
        .route("/Items/{id}/Intros", get(get_intros))
        .route("/Items/{id}/ThemeMedia", get(get_theme_media))
        .route("/Items/{id}/ThemeSongs", get(get_theme_songs))
        .route("/Items/{id}/ThemeVideos", get(get_theme_videos))
        .route("/Items/{id}/LocalTrailers", get(get_local_trailers))
        .route("/Items/{id}/SpecialFeatures", get(get_special_features))
        .route(
            "/Users/{userId}/Items/{itemId}/LocalTrailers",
            get(get_user_local_trailers),
        )
        .route(
            "/Users/{userId}/Items/{itemId}/SpecialFeatures",
            get(get_user_special_features),
        )
        .route("/Users/{userId}/Items/{itemId}", get(get_user_item))
        .route("/Users/{userId}/Items/{itemId}/Intros", get(get_intros))
        .route("/Users/{userId}/Items", get(get_user_items))
        .route("/Users/{userId}/Items/Latest", get(get_user_latest))
        .route("/Users/{userId}/Items/Resume", get(get_user_resume))
        // Additional Items endpoints
        .route("/Items/{id}/Ancestors", get(get_ancestors))
        .route("/Items/{id}/Collections", get(get_item_collections))
        .route("/Items/{id}/Attachments/{index}", get(get_attachments))
        // Channels
        .route("/Channels", get(get_channels))
        .route("/Channels/Items/Latest", get(get_channels_items_latest))
        .route("/Channels/{channelId}/Features", get(get_channel_features))
}

/// Administrator-only item removal routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Items", delete(delete_items))
        .route("/Items/{id}/Images/{image_type}", delete(delete_item_image))
        .route(
            "/Items/{id}/Images/{image_type}/{image_index}",
            delete(delete_item_image_index),
        )
        .route(
            "/Items/{id}/AlternateSources",
            delete(delete_alternate_sources),
        )
        .route("/Items/{id}/Subtitles/{index}", delete(delete_subtitles))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
struct ItemsQuery {
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
    library_id: Option<Uuid>,
    #[serde(rename = "libraryId")]
    library_id_lower: Option<Uuid>,
    include_item_types: Option<String>,
    #[serde(rename = "includeItemTypes")]
    include_item_types_lower: Option<String>,
    exclude_item_types: Option<String>,
    #[serde(rename = "excludeItemTypes")]
    exclude_item_types_lower: Option<String>,
    #[serde(alias = "genres")]
    genres: Option<String>,
    years: Option<String>,
    #[serde(rename = "years")]
    years_lower: Option<String>,
    search_term: Option<String>,
    #[serde(rename = "searchTerm")]
    search_term_lower: Option<String>,
    sort_by: Option<String>,
    #[serde(rename = "sortBy")]
    sort_by_lower: Option<String>,
    sort_order: Option<String>,
    #[serde(rename = "sortOrder")]
    sort_order_lower: Option<String>,
    start_index: Option<i64>,
    #[serde(rename = "startIndex")]
    start_index_lower: Option<i64>,
    #[serde(alias = "limit")]
    limit: Option<i64>,
    name_starts_with: Option<String>,
    #[serde(rename = "nameStartsWith")]
    name_starts_with_lower: Option<String>,
    name_starts_with_or_greater: Option<String>,
    #[serde(rename = "nameStartsWithOrGreater")]
    name_starts_with_or_greater_lower: Option<String>,
    // --- Items Query Parameters ---
    recursive: Option<bool>,
    #[serde(rename = "recursive")]
    recursive_lower: Option<bool>,
    fields: Option<String>,
    #[serde(rename = "fields")]
    fields_lower: Option<String>,
    image_types_limit: Option<i32>,
    #[serde(rename = "imageTypesLimit")]
    image_types_limit_lower: Option<i32>,
    /// A Rustyfin extension, not a Jellyfin option: `RustyfinCardIndex=true` answers with only
    /// what a poster card shows (see `card_index_result`). Jellyfin clients never send it, so their
    /// answers keep Jellyfin's DTO.
    rustyfin_card_index: Option<bool>,
    #[serde(rename = "rustyfinCardIndex")]
    rustyfin_card_index_lower: Option<bool>,
    // Jellyfin's DTO options: EnableUserData=false leaves out UserData only; EnableImages,
    // EnableImageTypes and ImageTypeLimit narrow the images (see `ImageOptions`)
    enable_user_data: Option<bool>,
    #[serde(rename = "enableUserData")]
    enable_user_data_lower: Option<bool>,
    enable_image_types: Option<String>,
    #[serde(rename = "enableImageTypes")]
    enable_image_types_lower: Option<String>,
    image_type_limit: Option<usize>,
    #[serde(rename = "imageTypeLimit")]
    image_type_limit_lower: Option<usize>,
    enable_images: Option<bool>,
    #[serde(rename = "enableImages")]
    enable_images_lower: Option<bool>,
    is_favorite: Option<bool>,
    #[serde(rename = "isFavorite")]
    is_favorite_lower: Option<bool>,
    is_unplayed: Option<bool>,
    #[serde(rename = "isUnplayed")]
    is_unplayed_lower: Option<bool>,
    is_played: Option<bool>,
    #[serde(rename = "isPlayed")]
    is_played_lower: Option<bool>,
    is_movie: Option<bool>,
    #[serde(rename = "isMovie")]
    is_movie_lower: Option<bool>,
    is_series: Option<bool>,
    #[serde(rename = "isSeries")]
    is_series_lower: Option<bool>,
    is_people: Option<bool>,
    #[serde(rename = "isPeople")]
    is_people_lower: Option<bool>,
    has_subtitles: Option<bool>,
    #[serde(rename = "hasSubtitles")]
    has_subtitles_lower: Option<bool>,
    media_types: Option<String>,
    #[serde(rename = "mediaTypes")]
    media_types_lower: Option<String>,
    genre_ids: Option<String>,
    #[serde(rename = "genreIds")]
    genre_ids_lower: Option<String>,
    studio_ids: Option<String>,
    #[serde(rename = "studioIds")]
    studio_ids_lower: Option<String>,
    artist_ids: Option<String>,
    #[serde(rename = "artistIds")]
    artist_ids_lower: Option<String>,
    album_ids: Option<String>,
    #[serde(rename = "albumIds")]
    album_ids_lower: Option<String>,
    #[serde(alias = "tags")]
    tags: Option<String>,
    #[serde(alias = "features")]
    features: Option<String>,
    official_ratings: Option<String>,
    #[serde(rename = "officialRatings")]
    official_ratings_lower: Option<String>,
    filters: Option<String>,
    #[serde(rename = "filters")]
    filters_lower: Option<String>,
    is_resumable: Option<bool>,
    #[serde(rename = "isResumable")]
    is_resumable_lower: Option<bool>,
    series_status: Option<String>,
    #[serde(rename = "seriesStatus")]
    series_status_lower: Option<String>,
    start_date: Option<String>,
    #[serde(rename = "startDate")]
    start_date_lower: Option<String>,
    end_date: Option<String>,
    #[serde(rename = "endDate")]
    end_date_lower: Option<String>,
    premiere_date: Option<String>,
    #[serde(rename = "premiereDate")]
    premiere_date_lower: Option<String>,
    date_created: Option<String>,
    #[serde(rename = "dateCreated")]
    date_created_lower: Option<String>,
    #[serde(alias = "runtime")]
    runtime: Option<String>,
    video_codecs: Option<String>,
    #[serde(rename = "videoCodecs")]
    video_codecs_lower: Option<String>,
    audio_codecs: Option<String>,
    #[serde(rename = "audioCodecs")]
    audio_codecs_lower: Option<String>,
    audio_bitrate: Option<String>,
    #[serde(rename = "audioBitrate")]
    audio_bitrate_lower: Option<String>,
    audio_bitrates: Option<String>,
    #[serde(rename = "audioBitrates")]
    audio_bitrates_lower: Option<String>,
    audio_channels: Option<String>,
    #[serde(rename = "audioChannels")]
    audio_channels_lower: Option<String>,
    has_theme_song: Option<bool>,
    #[serde(rename = "hasThemeSong")]
    has_theme_song_lower: Option<bool>,
    has_special_feature: Option<bool>,
    #[serde(rename = "hasSpecialFeature")]
    has_special_feature_lower: Option<bool>,
    has_trailer: Option<bool>,
    #[serde(rename = "hasTrailer")]
    has_trailer_lower: Option<bool>,
    has_theme_video: Option<bool>,
    #[serde(rename = "hasThemeVideo")]
    has_theme_video_lower: Option<bool>,
    subtitle_language: Option<String>,
    #[serde(rename = "subtitleLanguage")]
    subtitle_language_lower: Option<String>,
    /// Jellyfin's SubtitleLanguages: the items with a subtitle in one of these languages
    subtitle_languages: Option<String>,
    #[serde(rename = "subtitleLanguages")]
    subtitle_languages_lower: Option<String>,
    enable_auto_hide: Option<bool>,
    #[serde(rename = "enableAutoHide")]
    enable_auto_hide_lower: Option<bool>,
    collapse_box_set_items: Option<bool>,
    #[serde(rename = "collapseBoxSetItems")]
    collapse_box_set_items_lower: Option<bool>,
    group_projects: Option<bool>,
    #[serde(rename = "groupProjects")]
    group_projects_lower: Option<bool>,
    /// Jellyfin's Ids: only these items, in this order when no SortBy is given
    ids: Option<String>,
    #[serde(rename = "ids")]
    ids_lower: Option<String>,
    exclude_item_ids: Option<String>,
    #[serde(rename = "excludeItemIds")]
    exclude_item_ids_lower: Option<String>,
    // The other `InternalItemsQuery` conditions (`repo_item_filter::ItemFilter`).
    #[serde(alias = "personIds")]
    person_ids: Option<String>,
    #[serde(alias = "person")]
    person: Option<String>,
    /// The credit kinds `PersonIds` and `Person` keep (Actor, Director, …), as in Jellyfin 12.4.
    #[serde(alias = "personTypes")]
    person_types: Option<String>,
    #[serde(alias = "studios")]
    studios: Option<String>,
    #[serde(alias = "artists")]
    artists: Option<String>,
    #[serde(alias = "albumArtistIds")]
    album_artist_ids: Option<String>,
    #[serde(alias = "contributingArtistIds")]
    contributing_artist_ids: Option<String>,
    #[serde(alias = "excludeArtistIds")]
    exclude_artist_ids: Option<String>,
    #[serde(alias = "albums")]
    albums: Option<String>,
    #[serde(alias = "minCommunityRating")]
    min_community_rating: Option<f64>,
    /// Accepted and, as in Jellyfin 12.4's item query, without effect.
    #[serde(alias = "minCriticRating")]
    min_critic_rating: Option<f64>,
    #[serde(alias = "minPremiereDate")]
    min_premiere_date: Option<String>,
    #[serde(alias = "maxPremiereDate")]
    max_premiere_date: Option<String>,
    #[serde(alias = "isUnaired")]
    is_unaired: Option<bool>,
    #[serde(alias = "isMissing")]
    is_missing: Option<bool>,
    #[serde(alias = "hasImdbId")]
    has_imdb_id: Option<bool>,
    #[serde(alias = "hasTmdbId")]
    has_tmdb_id: Option<bool>,
    #[serde(alias = "hasTvdbId")]
    has_tvdb_id: Option<bool>,
    #[serde(rename = "IsHD", alias = "IsHd", alias = "isHd", alias = "isHD")]
    is_hd: Option<bool>,
    #[serde(rename = "Is4K", alias = "is4K", alias = "is4k")]
    is_4k: Option<bool>,
    #[serde(rename = "Is3D", alias = "is3D", alias = "is3d")]
    is_3d: Option<bool>,
    #[serde(alias = "minWidth")]
    min_width: Option<i64>,
    #[serde(alias = "minHeight")]
    min_height: Option<i64>,
    #[serde(alias = "maxWidth")]
    max_width: Option<i64>,
    #[serde(alias = "maxHeight")]
    max_height: Option<i64>,
    #[serde(alias = "hasOverview")]
    has_overview: Option<bool>,
    #[serde(alias = "hasOfficialRating")]
    has_official_rating: Option<bool>,
    #[serde(alias = "hasParentalRating")]
    has_parental_rating: Option<bool>,
    #[serde(alias = "minOfficialRating")]
    min_official_rating: Option<String>,
    #[serde(alias = "maxOfficialRating")]
    max_official_rating: Option<String>,
    #[serde(alias = "isLocked")]
    is_locked: Option<bool>,
    #[serde(alias = "isPlaceHolder")]
    is_place_holder: Option<bool>,
    #[serde(alias = "locationTypes")]
    location_types: Option<String>,
    #[serde(alias = "excludeLocationTypes")]
    exclude_location_types: Option<String>,
    #[serde(alias = "videoTypes")]
    video_types: Option<String>,
    #[serde(alias = "nameLessThan")]
    name_less_than: Option<String>,
    #[serde(alias = "indexNumber")]
    index_number: Option<i64>,
    #[serde(alias = "parentIndexNumber")]
    parent_index_number: Option<i64>,
    #[serde(alias = "imageTypes")]
    image_types: Option<String>,
    #[serde(alias = "adjacentTo")]
    adjacent_to: Option<Uuid>,
    #[serde(alias = "isKids")]
    is_kids: Option<bool>,
    #[serde(alias = "isNews")]
    is_news: Option<bool>,
    #[serde(alias = "isSports")]
    is_sports: Option<bool>,
    /// False: TotalRecordCount is the number of items returned, as in Jellyfin.
    #[serde(alias = "enableTotalRecordCount")]
    enable_total_record_count: Option<bool>,
    #[serde(alias = "minDateLastSaved")]
    min_date_last_saved: Option<String>,
    #[serde(alias = "minDateLastSavedForUser")]
    min_date_last_saved_for_user: Option<String>,
    /// Jellyfin 12.4's AudioLanguages: items with an audio stream in one of these languages
    #[serde(alias = "audioLanguages")]
    audio_languages: Option<String>,
}

#[allow(dead_code)]
impl ItemsQuery {
    fn parent_id(&self) -> Option<Uuid> {
        self.parent_id.or(self.parent_id_lower)
    }
    fn library_id(&self) -> Option<Uuid> {
        self.library_id.or(self.library_id_lower)
    }
    fn include_item_types(&self) -> Option<&str> {
        self.include_item_types
            .as_deref()
            .or(self.include_item_types_lower.as_deref())
    }
    fn years(&self) -> Option<&str> {
        self.years.as_deref().or(self.years_lower.as_deref())
    }
    fn exclude_item_types(&self) -> Option<&str> {
        self.exclude_item_types
            .as_deref()
            .or(self.exclude_item_types_lower.as_deref())
    }
    fn search_term(&self) -> Option<&str> {
        self.search_term
            .as_deref()
            .or(self.search_term_lower.as_deref())
    }
    fn sort_by(&self) -> Option<&str> {
        self.sort_by.as_deref().or(self.sort_by_lower.as_deref())
    }
    fn ids(&self) -> Option<&str> {
        self.ids.as_deref().or(self.ids_lower.as_deref())
    }
    fn exclude_item_ids(&self) -> Option<&str> {
        self.exclude_item_ids
            .as_deref()
            .or(self.exclude_item_ids_lower.as_deref())
    }
    fn sort_order(&self) -> Option<&str> {
        self.sort_order
            .as_deref()
            .or(self.sort_order_lower.as_deref())
    }
    fn start_index(&self) -> i64 {
        self.start_index
            .or(self.start_index_lower)
            .unwrap_or(0)
            .max(0)
    }
    /// The StartIndex as sent, before `start_index` clamps negatives to 0.
    fn start_index_requested(&self) -> i64 {
        self.start_index.or(self.start_index_lower).unwrap_or(0)
    }
    fn limit(&self) -> i64 {
        self.limit.filter(|&limit| limit >= 0).unwrap_or(10_000)
    }
    fn name_starts_with(&self) -> Option<&str> {
        self.name_starts_with
            .as_deref()
            .or(self.name_starts_with_lower.as_deref())
    }
    fn name_starts_with_or_greater(&self) -> Option<&str> {
        self.name_starts_with_or_greater
            .as_deref()
            .or(self.name_starts_with_or_greater_lower.as_deref())
    }
    fn recursive(&self) -> bool {
        self.recursive_given().unwrap_or(false)
    }
    fn recursive_given(&self) -> Option<bool> {
        self.recursive.or(self.recursive_lower)
    }
    fn fields(&self) -> Option<&str> {
        self.fields.as_deref().or(self.fields_lower.as_deref())
    }
    fn card_index(&self) -> bool {
        self.rustyfin_card_index
            .or(self.rustyfin_card_index_lower)
            .unwrap_or(false)
    }
    fn enable_user_data(&self) -> bool {
        self.enable_user_data
            .or(self.enable_user_data_lower)
            .unwrap_or(true)
    }
    /// The image options: EnableImages=false sends none; EnableImageTypes keeps the types it
    /// names, and naming none it knows (or none at all) keeps all of them, as Jellyfin applies
    /// the list only when it holds a type.
    fn image_options(&self) -> ImageOptions {
        let types: Option<Vec<rustyfin_model::image::ImageType>> = self
            .enable_image_types
            .as_deref()
            .or(self.enable_image_types_lower.as_deref())
            .map(|types| {
                types
                    .split(',')
                    .filter_map(|t| rustyfin_model::image::ImageType::from_str(t.trim()))
                    .collect::<Vec<_>>()
            })
            .filter(|types| !types.is_empty());
        let enabled = self
            .enable_images
            .or(self.enable_images_lower)
            .unwrap_or(true);
        ImageOptions {
            types,
            limit: if enabled {
                self.image_type_limit.or(self.image_type_limit_lower)
            } else {
                Some(0)
            },
        }
    }
    /// Whether Fields is given and names no field: Jellyfin's "no optional fields" (see
    /// `minimal_result`).
    fn fields_given_empty(&self) -> bool {
        self.fields()
            .is_some_and(|fields| fields.split(',').all(|f| f.trim().is_empty()))
    }
    fn image_types_limit(&self) -> Option<i32> {
        self.image_types_limit.or(self.image_types_limit_lower)
    }
    fn is_favorite(&self) -> Option<bool> {
        self.is_favorite.or(self.is_favorite_lower)
    }
    fn is_unplayed(&self) -> Option<bool> {
        self.is_unplayed.or(self.is_unplayed_lower)
    }
    fn is_played(&self) -> Option<bool> {
        self.is_played.or(self.is_played_lower)
    }
    /// Whether `Filters` lists a flag (`IsPlayed`, `IsFavorite`, ...), case-insensitively.
    fn has_filter(&self, flag: &str) -> bool {
        self.filters
            .as_deref()
            .or(self.filters_lower.as_deref())
            .is_some_and(|list| list.split(',').any(|f| f.trim().eq_ignore_ascii_case(flag)))
    }
    fn is_resumable(&self) -> Option<bool> {
        self.is_resumable.or(self.is_resumable_lower)
    }
    fn series_status(&self) -> Option<&str> {
        self.series_status
            .as_deref()
            .or(self.series_status_lower.as_deref())
    }
    fn is_movie(&self) -> Option<bool> {
        self.is_movie.or(self.is_movie_lower)
    }
    fn is_series(&self) -> Option<bool> {
        self.is_series.or(self.is_series_lower)
    }
    fn is_people(&self) -> Option<bool> {
        self.is_people.or(self.is_people_lower)
    }
    fn has_subtitles(&self) -> Option<bool> {
        self.has_subtitles.or(self.has_subtitles_lower)
    }
    fn media_types(&self) -> Option<&str> {
        self.media_types
            .as_deref()
            .or(self.media_types_lower.as_deref())
    }
    fn genre_ids(&self) -> Option<&str> {
        self.genre_ids
            .as_deref()
            .or(self.genre_ids_lower.as_deref())
    }
    fn studio_ids(&self) -> Option<&str> {
        self.studio_ids
            .as_deref()
            .or(self.studio_ids_lower.as_deref())
    }
    fn artist_ids(&self) -> Option<&str> {
        self.artist_ids
            .as_deref()
            .or(self.artist_ids_lower.as_deref())
    }
    fn album_ids(&self) -> Option<&str> {
        self.album_ids
            .as_deref()
            .or(self.album_ids_lower.as_deref())
    }
    fn tags(&self) -> Option<&str> {
        self.tags.as_deref()
    }
    fn features(&self) -> Option<&str> {
        self.features.as_deref()
    }
    fn official_ratings(&self) -> Option<&str> {
        self.official_ratings
            .as_deref()
            .or(self.official_ratings_lower.as_deref())
    }
    fn start_date(&self) -> Option<&str> {
        self.start_date
            .as_deref()
            .or(self.start_date_lower.as_deref())
    }
    fn end_date(&self) -> Option<&str> {
        self.end_date.as_deref().or(self.end_date_lower.as_deref())
    }
    fn premiere_date(&self) -> Option<&str> {
        self.premiere_date
            .as_deref()
            .or(self.premiere_date_lower.as_deref())
    }
    fn date_created(&self) -> Option<&str> {
        self.date_created
            .as_deref()
            .or(self.date_created_lower.as_deref())
    }
    fn runtime(&self) -> Option<&str> {
        self.runtime.as_deref()
    }
    fn video_codecs(&self) -> Option<&str> {
        self.video_codecs
            .as_deref()
            .or(self.video_codecs_lower.as_deref())
    }
    fn audio_codecs(&self) -> Option<&str> {
        self.audio_codecs
            .as_deref()
            .or(self.audio_codecs_lower.as_deref())
    }
    fn audio_bitrate(&self) -> Option<&str> {
        self.audio_bitrate
            .as_deref()
            .or(self.audio_bitrate_lower.as_deref())
    }
    fn audio_bitrates(&self) -> Option<&str> {
        self.audio_bitrates
            .as_deref()
            .or(self.audio_bitrates_lower.as_deref())
    }
    fn audio_channels(&self) -> Option<&str> {
        self.audio_channels
            .as_deref()
            .or(self.audio_channels_lower.as_deref())
    }
    fn has_theme_song(&self) -> Option<bool> {
        self.has_theme_song.or(self.has_theme_song_lower)
    }
    fn has_special_feature(&self) -> Option<bool> {
        self.has_special_feature.or(self.has_special_feature_lower)
    }
    fn has_trailer(&self) -> Option<bool> {
        self.has_trailer.or(self.has_trailer_lower)
    }
    fn has_theme_video(&self) -> Option<bool> {
        self.has_theme_video.or(self.has_theme_video_lower)
    }
    /// The subtitle languages to filter by: Jellyfin's comma list `SubtitleLanguages`, else the
    /// single `SubtitleLanguage` Rustyfin took before. Each code is matched in all its spellings,
    /// so `ger` and `deu` find the same items.
    fn subtitle_languages(&self) -> Option<Vec<String>> {
        let list = self
            .subtitle_languages
            .as_deref()
            .or(self.subtitle_languages_lower.as_deref())
            .or(self.subtitle_language.as_deref())
            .or(self.subtitle_language_lower.as_deref())?;
        let forms: Vec<String> = list
            .split(',')
            .filter(|code| !code.trim().is_empty())
            .flat_map(rustyfin_model::media_source::language_forms)
            .collect();
        (!forms.is_empty()).then_some(forms)
    }
    fn enable_auto_hide(&self) -> bool {
        self.enable_auto_hide
            .or(self.enable_auto_hide_lower)
            .unwrap_or(false)
    }
    fn collapse_box_set_items(&self) -> bool {
        self.collapse_box_set_items
            .or(self.collapse_box_set_items_lower)
            .unwrap_or(false)
    }
    fn group_projects(&self) -> bool {
        self.group_projects
            .or(self.group_projects_lower)
            .unwrap_or(false)
    }
}

/// The UUIDs in a comma-separated id list. A value that isn't a UUID is dropped, as Jellyfin's
/// comma-delimited binder drops it (`CommaDelimitedCollectionModelBinder.cs:65-73`).
fn parse_ids(value: Option<&str>) -> Vec<Uuid> {
    value
        .into_iter()
        .flat_map(|ids| ids.split(','))
        .filter_map(|id| Uuid::parse_str(id.trim()).ok())
        .collect()
}

/// Jellyfin's `Ids` and `ExcludeItemIds` on an item list.
struct IdsFilter {
    /// The ids asked for, in order; none filters nothing.
    requested: Vec<Uuid>,
    wanted: std::collections::HashSet<Uuid>,
    excluded: Vec<Uuid>,
    /// More than one id and no SortBy: the answer keeps the order of Ids, as Jellyfin's
    /// `Folder.SortItemsByRequest` does.
    in_request_order: bool,
}

impl IdsFilter {
    fn new(query: &ItemsQuery) -> Self {
        let requested = parse_ids(query.ids());
        Self {
            wanted: requested.iter().copied().collect(),
            excluded: parse_ids(query.exclude_item_ids()),
            in_request_order: requested.len() > 1
                && query.sort_by().is_none_or(|s| s.trim().is_empty()),
            requested,
        }
    }

    fn is_empty(&self) -> bool {
        self.requested.is_empty() && self.excluded.is_empty()
    }

    fn keeps(&self, id: Uuid) -> bool {
        (self.wanted.is_empty() || self.wanted.contains(&id)) && !self.excluded.contains(&id)
    }

    /// The items of a whole list that the filter keeps, in the order of Ids when it asks for
    /// that, then paged.
    fn page(&self, mut items: Vec<MediaItem>, start: i64, limit: i64) -> QueryResult {
        items.retain(|item| self.keeps(item.id));
        if self.in_request_order {
            let mut position = std::collections::HashMap::new();
            for (index, id) in self.requested.iter().enumerate() {
                position.entry(*id).or_insert(index);
            }
            items.sort_by_key(|item| position.get(&item.id).copied().unwrap_or(usize::MAX));
        }
        let total = items.len() as i64;
        let items = items
            .into_iter()
            .skip(start as usize)
            .take(limit as usize)
            .collect();
        QueryResult {
            items,
            total_record_count: total,
            start_index: start,
        }
    }
}

/// The collections and playlists among the requested ids that aren't media items (`found`), as
/// `/Items/{id}` answers them: only those the caller may see, and only when the list's types
/// (`include`, `exclude`) keep `BoxSet` or `Playlist`.
async fn requested_containers(
    state: &AppState,
    claims: &Claims,
    by_id: &IdsFilter,
    found: &[MediaItem],
    include: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<Vec<MediaItem>, ApiError> {
    let mut seen: std::collections::HashSet<Uuid> = found.iter().map(|item| item.id).collect();
    let mut containers = Vec::new();
    for &id in &by_id.requested {
        if !by_id.keeps(id) || !seen.insert(id) {
            continue;
        }
        let item = match super::collections::container_details(state, id, claims).await {
            Ok(Some(item)) => item,
            Ok(None) | Err(ApiError::NotFound(_)) => continue,
            Err(e) => return Err(e),
        };
        let kind = match item.media_type {
            rustyfin_model::media::MediaType::BoxSet => "BoxSet",
            _ => "Playlist",
        };
        let names = |types: Option<&[String]>| {
            types.is_some_and(|types| {
                types
                    .iter()
                    .any(|t| t.trim_matches('"').eq_ignore_ascii_case(kind))
            })
        };
        if include.is_some_and(|types| !types.is_empty()) && !names(include) || names(exclude) {
            continue;
        }
        containers.push(item);
    }
    Ok(containers)
}

/// The values of a Jellyfin pipe-delimited list (`PipeDelimitedCollectionModelBinder`), trimmed,
/// without empty ones.
fn pipe_list(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|v| v.split('|'))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// The values of a comma-delimited list, trimmed, without empty ones.
fn comma_list(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// A date query value (RFC 3339, or a date and time without a zone, read as UTC, or a date) as
/// the UTC instant SQLite's `julianday` reads; 400 when it is none of those, as in Jellyfin.
fn query_instant(name: &str, value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let instant = chrono::DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&chrono::Utc).naive_utc())
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f"))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap_or_default())
        })
        .map_err(|_| ApiError::BadRequest(format!("invalid {name}: {value}")))?;
    Ok(Some(instant.format("%Y-%m-%d %H:%M:%S").to_string()))
}

/// The item types whose Jellyfin `MediaType` (`MediaType::media_kind`) is one of `kinds`, or
/// `None` to filter nothing. As Jellyfin 12.4's comma-delimited binder drops a value that isn't a
/// `MediaType` (`Unknown`, `Video`, `Audio`, `Photo`, `Book`), only those count, and with none
/// left nothing is filtered; a kind no item has (`Book`) keeps a type no item has, so it lists
/// nothing.
fn item_types_of_media_kinds(kinds: &[String]) -> Option<Vec<String>> {
    use rustyfin_model::media::MediaType;
    const KINDS: [&str; 5] = ["Unknown", "Video", "Audio", "Photo", "Book"];
    let kinds: Vec<&String> = kinds
        .iter()
        .filter(|k| KINDS.iter().any(|known| known.eq_ignore_ascii_case(k)))
        .collect();
    if kinds.is_empty() {
        return None;
    }
    const TYPES: [MediaType; 12] = [
        MediaType::Movie,
        MediaType::Series,
        MediaType::Season,
        MediaType::Episode,
        MediaType::Audio,
        MediaType::MusicAlbum,
        MediaType::MusicArtist,
        MediaType::Photo,
        MediaType::Video,
        MediaType::Genre,
        MediaType::BoxSet,
        MediaType::Playlist,
    ];
    let mut types: Vec<String> = TYPES
        .iter()
        .filter(|t| kinds.iter().any(|k| k.eq_ignore_ascii_case(t.media_kind())))
        .map(|t| format!("{t:?}"))
        .collect();
    if types.is_empty() {
        types.push("Book".to_string());
    }
    Some(types)
}

/// The `InternalItemsQuery` conditions of a list request that `repo_item_filter` answers.
fn item_filter(query: &ItemsQuery) -> Result<rustyfin_db::repo_item_filter::ItemFilter, ApiError> {
    let ids = |value: Option<&str>| -> Vec<String> {
        parse_ids(value).iter().map(Uuid::to_string).collect()
    };
    let rating = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .and_then(rustyfin_model::parental::rating_score)
    };
    // IsKids, IsNews and IsSports are the tags of those names, as in Jellyfin.
    let mut tags = pipe_list(query.tags());
    let mut exclude_tags = Vec::new();
    for (flag, tag) in [
        (query.is_kids, "Kids"),
        (query.is_news, "News"),
        (query.is_sports, "Sports"),
    ] {
        match flag {
            Some(true) => tags.push(tag.to_string()),
            Some(false) => exclude_tags.push(tag.to_string()),
            None => {}
        }
    }
    // Jellyfin reads the location types as whether an item is virtual (`ItemsController`).
    let location = comma_list(query.location_types.as_deref());
    let excluded_locations = comma_list(query.exclude_location_types.as_deref());
    let virtual_ = |list: &[String]| list.iter().any(|t| t.eq_ignore_ascii_case("Virtual"));
    let is_virtual = if virtual_(&excluded_locations) {
        Some(false)
    } else if !location.is_empty() && location.len() < 4 {
        Some(virtual_(&location))
    } else {
        query.is_missing
    };
    let min_rating = rating(query.min_official_rating.as_deref());
    let max_rating = rating(query.max_official_rating.as_deref());
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    Ok(rustyfin_db::repo_item_filter::ItemFilter {
        person_ids: ids(query.person_ids.as_deref()),
        person_types: comma_list(query.person_types.as_deref()),
        official_ratings: query
            .official_ratings()
            .into_iter()
            .flat_map(|s| s.split(['|', ',']))
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string)
            .collect(),
        audio_languages: comma_list(query.audio_languages.as_deref())
            .iter()
            .flat_map(|code| rustyfin_model::media_source::language_forms(code))
            .collect(),
        // Jellyfin 12.4: languages make HasSubtitles=true redundant, and HasSubtitles=false
        // drops the languages.
        subtitle_languages: query
            .subtitle_languages()
            .filter(|_| query.has_subtitles() != Some(false))
            .unwrap_or_default(),
        person: query
            .person
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string),
        studios: pipe_list(query.studios.as_deref()),
        artist_ids: ids(query.artist_ids()),
        artist_names: pipe_list(query.artists.as_deref()),
        album_artist_ids: ids(query.album_artist_ids.as_deref()),
        contributing_artist_ids: ids(query.contributing_artist_ids.as_deref()),
        exclude_artist_ids: ids(query.exclude_artist_ids.as_deref()),
        album_ids: ids(query.album_ids()),
        album_names: pipe_list(query.albums.as_deref()),
        tags,
        exclude_tags,
        min_community_rating: query.min_community_rating,
        min_premiere_date: query_instant("MinPremiereDate", query.min_premiere_date.as_deref())?,
        max_premiere_date: query_instant("MaxPremiereDate", query.max_premiere_date.as_deref())?,
        // Both compare the item's last save in Jellyfin 12.4; the later bound wins.
        min_date_last_saved: query_instant(
            "MinDateLastSaved",
            query.min_date_last_saved.as_deref(),
        )?
        .into_iter()
        .chain(query_instant(
            "MinDateLastSavedForUser",
            query.min_date_last_saved_for_user.as_deref(),
        )?)
        .max(),
        unaired: query.is_unaired.map(|unaired| (unaired, now)),
        has_imdb_id: query.has_imdb_id,
        has_tmdb_id: query.has_tmdb_id,
        has_tvdb_id: query.has_tvdb_id,
        is_hd: query.is_hd,
        is_4k: query.is_4k,
        is_3d: query.is_3d,
        min_width: query.min_width,
        min_height: query.min_height,
        max_width: query.max_width,
        max_height: query.max_height,
        has_subtitles: query
            .has_subtitles()
            .filter(|&has| !(has && query.subtitle_languages().is_some())),
        has_overview: query.has_overview,
        has_official_rating: query.has_official_rating,
        no_parental_rating: query.has_parental_rating == Some(false)
            && min_rating.is_none()
            && max_rating.is_none(),
        min_rating,
        max_rating,
        is_locked: query.is_locked,
        is_placeholder: query.is_place_holder,
        is_virtual,
        // Set by the caller, which knows the user
        hide_virtual_seasons: false,
        video_types: comma_list(query.video_types.as_deref()),
        name_less_than: query
            .name_less_than
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .map(str::to_string),
        index_number: query.index_number,
        parent_index_number: query.parent_index_number,
        image_types: comma_list(query.image_types.as_deref()),
    })
}

/// Jellyfin 12.4's `AdjacentTo` (`ApplyAdjacencyFilter`): the item `id` of the ordered list and
/// the ones before and after it; nothing when the list doesn't hold it.
fn adjacent_items(items: Vec<MediaItem>, id: Uuid) -> Vec<MediaItem> {
    let index = items.iter().position(|item| item.id == id);
    let keep = |i: usize| index.is_some_and(|index| i + 1 == index || i == index || i == index + 1);
    items
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep(*i))
        .map(|(_, item)| item)
        .collect()
}

/// The ids of `kept` (all items when `None`) that are also in `ids`.
/// The hidden items' filter (`HiddenItems::json`: `{"h": [ids], "l": [libraries]}`) that also hides `ids`.
fn hide_also(hidden: Option<String>, ids: Vec<String>) -> Option<String> {
    if ids.is_empty() {
        return hidden;
    }
    let mut filter = hidden
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let mut all: Vec<Value> = filter["h"].as_array().cloned().unwrap_or_default();
    all.extend(ids.into_iter().map(Value::String));
    filter["h"] = Value::Array(all);
    Some(filter.to_string())
}

fn narrow_ids(kept: Option<Vec<String>>, ids: Vec<String>) -> Vec<String> {
    match kept {
        Some(kept) => {
            let ids: std::collections::HashSet<String> = ids.into_iter().collect();
            kept.into_iter().filter(|id| ids.contains(id)).collect()
        }
        None => ids,
    }
}

async fn get_items(
    State(state): State<AppState>,
    Query(query): Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    let media_types: Option<Vec<String>> = query
        .include_item_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let exclude_media_types: Option<Vec<String>> = query
        .exclude_item_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    // Genres and OfficialRatings are pipe-delimited, as in Jellyfin (`PipeDelimitedCollectionModelBinder`):
    // a genre name may hold a comma. Ratings never do, so a comma still separates them too.
    let genres: Option<Vec<String>> = query
        .genres
        .as_ref()
        .map(|s| s.split('|').map(|g| g.trim().to_string()).collect());
    let years: Option<Vec<i32>> = query
        .years()
        .map(|s| s.split(',').filter_map(|y| y.trim().parse().ok()).collect());
    // OfficialRatings, as SubtitleLanguages, goes to `item_filter`: Jellyfin 12.4 matches a
    // folder by its contents too.
    // `Filters=IsPlayed,IsUnplayed,IsResumable,IsFavorite` sets the same conditions as the flags.
    let flag = |set: Option<bool>, names: &[&str]| {
        set.or(names.iter().any(|n| query.has_filter(n)).then_some(true))
    };
    let is_favorite = flag(query.is_favorite(), &["IsFavorite", "IsFavoriteOrLikes"]);
    let is_played = flag(query.is_played(), &["IsPlayed"]);
    let is_unplayed = flag(query.is_unplayed(), &["IsUnplayed"]);
    let is_resumable = flag(query.is_resumable(), &["IsResumable"]);
    // Tags (with IsKids, IsNews, IsSports) go to `item_filter`: any of them, or the series'.
    let tags: Option<Vec<String>> = None;
    // MediaTypes names Jellyfin's media kinds (Video, Audio, …), kept as the item types of them.
    let media_types_filter: Option<Vec<String>> = query
        .media_types()
        .and_then(|s| item_types_of_media_kinds(&comma_list(Some(s))));
    let mut conditions = item_filter(&query)?;
    let runtime_parts: Option<(i32, i32)> = query.runtime().map(|s| {
        let parts: Vec<&str> = s.split('-').collect();
        let min = parts
            .first()
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(0);
        let max = parts
            .get(1)
            .and_then(|p| p.trim().parse().ok())
            .unwrap_or(i32::MAX);
        (min, max)
    });

    let start_index = query.start_index();
    let limit = query.limit();
    let by_id = IdsFilter::new(&query);
    // With Ids or ExcludeItemIds, a whole listing is fetched, narrowed, ordered, then paged.
    let (list_start, list_limit) = if by_id.is_empty() {
        (start_index as usize, limit as usize)
    } else {
        (0, usize::MAX)
    };
    let narrowed = |list: QueryResult| {
        if by_id.is_empty() {
            list
        } else {
            by_id.page(list.items, start_index, limit)
        }
    };

    // Collections and playlists live outside media_items: a list of only those kinds, or one of
    // them as ParentId, is answered from their own tables.
    let mut parent_id = query.parent_id();
    let mut collection_ids = None;
    let mut in_collection = false;
    let only_containers = media_types.as_ref().filter(|kinds| {
        !kinds.is_empty()
            && kinds.iter().all(|kind| {
                kind.eq_ignore_ascii_case("BoxSet") || kind.eq_ignore_ascii_case("Playlist")
            })
    });
    let wants = |name: &str| {
        only_containers.is_some_and(|kinds| kinds.iter().any(|k| k.eq_ignore_ascii_case(name)))
    };
    if let Some(id) = parent_id {
        match super::collections::container_of(&state, id, &claims).await? {
            Some(Container::Playlist) => {
                let entries = super::collections::playlist_entries(
                    &state,
                    id,
                    &claims,
                    list_start,
                    Some(list_limit),
                    query
                        .sort_by()
                        .is_some_and(rustyfin_db::repo_media::sorts_randomly),
                );
                return entries.await.map(|r| Json(narrowed(r)).into_response());
            }
            Some(Container::Collection(ids)) => {
                in_collection = true;
                parent_id = None;
                collection_ids = Some(ids);
            }
            // The collections or playlists under a library: those holding its items
            None if only_containers.is_some()
                && rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id)
                    .await?
                    .is_some() =>
            {
                let listing = super::collections::container_listing(
                    &state,
                    &claims,
                    wants("BoxSet"),
                    wants("Playlist"),
                    Some(id),
                    list_start,
                    list_limit,
                );
                return listing.await.map(|r| Json(narrowed(r)).into_response());
            }
            None => {}
        }
    } else if only_containers.is_some() {
        let listing = super::collections::container_listing(
            &state,
            &claims,
            wants("BoxSet"),
            wants("Playlist"),
            None,
            list_start,
            list_limit,
        );
        return listing.await.map(|r| Json(narrowed(r)).into_response());
    }

    // Whether IncludeItemTypes names any type, or only MusicAlbum (for the defaults below).
    let typed_list = media_types
        .as_ref()
        .is_some_and(|t| t.iter().any(|t| !t.is_empty()));
    let albums_only = media_types
        .as_ref()
        .is_some_and(|t| t.len() == 1 && t[0].eq_ignore_ascii_case("MusicAlbum"));
    // IsMovie and IsSeries are Live TV program flags in Jellyfin, and library items are never
    // programs (as on /Search/Hints): IsMovie=true keeps the list when the include types are
    // empty or name Movie or Trailer and empties it otherwise, IsSeries=true empties it, and
    // false filters nothing.
    let names_movies = media_types.as_ref().is_none_or(|kinds| {
        kinds.iter().all(|k| k.is_empty())
            || kinds
                .iter()
                .any(|k| k.eq_ignore_ascii_case("Movie") || k.eq_ignore_ascii_case("Trailer"))
    });
    if (query.is_movie() == Some(true) && !names_movies) || query.is_series() == Some(true) {
        return Ok(Json(empty_result()).into_response());
    }
    let constrained_media_types = media_types;

    // GenreIds and StudioIds keep the items with one of those genres and one of those studios.
    let mut item_ids = rustyfin_db::repo_by_name::item_ids_with(
        &state.db_ro,
        &parse_ids(query.genre_ids()),
        &parse_ids(query.studio_ids()),
    )
    .await?;
    // A collection as ParentId, IsResumable and SeriesStatus narrow the same id list.
    // IsResumable=false filters nothing.
    if let Some(ids) = collection_ids {
        let ids = ids.iter().map(Uuid::to_string).collect();
        item_ids = Some(narrow_ids(item_ids, ids));
    }
    if is_resumable == Some(true) {
        let ids = rustyfin_db::repo_media::resumable_item_ids(&state.db_ro, claims.sub).await?;
        item_ids = Some(narrow_ids(item_ids, ids));
    }
    if let Some(statuses) = query.series_status() {
        let statuses: Vec<&str> = statuses.split(',').map(str::trim).collect();
        let ids = rustyfin_db::repo_media::series_ids_with_status(&state.db_ro, &statuses).await?;
        item_ids = Some(narrow_ids(item_ids, ids));
    }
    // A parent's virtual seasons (a series' seasons with no file behind them) are left out unless
    // the user's DisplayMissingEpisodes, or an API key, asks for them (12.4 SetSeasonQueryOptions).
    conditions.hide_virtual_seasons = parent_id.is_some()
        && !(claims.sub.is_nil()
            || crate::routes::shows::display_missing_episodes(&state.db_ro, Some(claims.sub))
                .await);
    // The other conditions join the list's own SQL, beside the parental limits and the count.
    let extra = rustyfin_db::repo_item_filter::filter_sql(&state.db_ro, &conditions).await?;
    // Ids keeps only the listed items (Jellyfin's `InternalItemsQuery.ItemIds`); ids of no item
    // are skipped, and an empty list filters nothing, as in Jellyfin.
    if !by_id.requested.is_empty() {
        let ids = by_id.requested.iter().map(Uuid::to_string).collect();
        item_ids = Some(narrow_ids(item_ids, ids));
    }
    // HasTrailer, HasSpecialFeature, HasThemeSong and HasThemeVideo, as in Jellyfin 12.4: an item matches
    // when it or an item inside it has that kind of extra (`owners_with_extras`). A special feature is any
    // extra but a trailer, a theme or an `Unknown` one (`extras/`). True keeps the matches; false leaves
    // them out with the hidden items, so neither reads the ids of the whole library.
    let mut without: Vec<String> = Vec::new();
    for (has, types) in [
        (query.has_trailer(), &["Trailer"][..]),
        (query.has_special_feature(), &SPECIAL_FEATURE_TYPES[1..]),
        (query.has_theme_song(), &["ThemeSong"][..]),
        (query.has_theme_video(), &["ThemeVideo"][..]),
    ] {
        if let Some(has) = has {
            let ids = rustyfin_db::repo_extras::owners_with_extras(&state.db_ro, types).await?;
            if has {
                item_ids = Some(narrow_ids(item_ids, ids));
            } else {
                without.extend(ids);
            }
        }
    }
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    // ExcludeItemIds, and the items without the extras asked for, drop items as the caller's
    // hidden ones are dropped.
    let hidden_json = hide_also(hidden.json_excluding(&by_id.excluded), without);

    // With Ids every match is fetched, then merged with the requested collections and
    // playlists, ordered and paged here.
    // AdjacentTo: the SQL finds the item and its neighbours, but in the order of Ids it is
    // picked here from that short list.
    let adjacent_to = query.adjacent_to;
    let whole_list = !by_id.requested.is_empty() || adjacent_to.is_some();
    let sql_adjacent_to = adjacent_to.filter(|_| by_id.requested.is_empty());
    let (fetch_start, fetch_limit) = if !whole_list {
        (start_index, limit)
    } else if by_id.requested.is_empty() {
        (0, 3)
    } else {
        (0, by_id.requested.len() as i64)
    };

    // Jellyfin 12.4: with item types, a library as ParentId lists its contents recursively unless
    // the client chose; an artist's albums come newest first unless an order is asked for.
    let recursive = match query.recursive_given() {
        Some(recursive) => recursive,
        None => match parent_id {
            Some(id) if typed_list => {
                rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id)
                    .await?
                    .is_some()
            }
            _ => false,
        },
    };
    let (sort_by, sort_order) = if query.sort_by().is_none_or(|s| s.trim().is_empty())
        && (!conditions.artist_ids.is_empty() || !conditions.artist_names.is_empty())
        && albums_only
    {
        (
            Some("ProductionYear,SortName"),
            Some("Descending,Ascending"),
        )
    } else {
        (query.sort_by(), query.sort_order())
    };

    let (mut items, mut total) = rustyfin_db::repo_media::get_items_filtered(
        &state.db_ro,
        parent_id,
        query.library_id(),
        constrained_media_types.as_deref(),
        exclude_media_types.as_deref(),
        genres.as_deref(),
        years.as_deref(),
        query.search_term(),
        query.name_starts_with(),
        query.name_starts_with_or_greater(),
        sort_by,
        sort_order,
        fetch_start,
        fetch_limit,
        recursive,
        is_favorite,
        is_unplayed,
        is_played,
        None,
        tags.as_deref(),
        media_types_filter.as_deref(),
        query.start_date(),
        query.end_date(),
        query.premiere_date(),
        query.date_created(),
        runtime_parts,
        Some(claims.sub),
        item_ids.as_deref(),
        hidden_json.as_deref(),
        extra.as_ref(),
        sql_adjacent_to,
    )
    .await?;
    // Collections and playlists named in Ids are listed as `/Items/{id}` answers them; they are
    // under no library or collection, so a ParentId or LibraryId leaves them out.
    let mut containers = std::collections::HashSet::new();
    if whole_list {
        if !by_id.requested.is_empty()
            && parent_id.is_none()
            && !in_collection
            && query.library_id().is_none()
        {
            let found = requested_containers(
                &state,
                &claims,
                &by_id,
                &items,
                constrained_media_types.as_deref(),
                exclude_media_types.as_deref(),
            )
            .await?;
            containers.extend(found.iter().map(|item| item.id));
            items.extend(found);
        }
        let mut page = by_id.page(items, 0, i64::MAX);
        if let Some(id) = adjacent_to.filter(|_| !by_id.requested.is_empty()) {
            page.items = adjacent_items(page.items, id);
        }
        total = page.items.len() as i64;
        items = page
            .items
            .into_iter()
            .skip(start_index as usize)
            .take(limit as usize)
            .collect();
    }

    if query.enable_total_record_count == Some(false) {
        total = items.len() as i64;
    }

    if query.card_index() {
        let result =
            card_index_result(&state.db_ro, &items, total, query.start_index_requested()).await?;
        return Ok(Json(result).into_response());
    }

    // Only populate media_sources when explicitly requested via Fields parameter
    // This significantly speeds up listing views with many items
    let fields: Vec<&str> = query
        .fields()
        .map(|s| s.split(',').map(|f| f.trim()).collect::<Vec<&str>>())
        .unwrap_or_default();
    let need_media_sources = fields
        .iter()
        .any(|f| f.eq_ignore_ascii_case("MediaSources"));

    // The collections and playlists are sent as they are, and only the media items are
    // enriched; `is_container` keeps their places.
    let is_container: Vec<bool> = items.iter().map(|i| containers.contains(&i.id)).collect();
    let (held, items): (Vec<MediaItem>, Vec<MediaItem>) =
        items.into_iter().partition(|i| containers.contains(&i.id));

    let mut items = if need_media_sources {
        let mut items_copy = items;
        let _ = populate_media_sources(&state.db_ro, &mut items_copy).await;
        items_copy
    } else {
        items
    };

    // Enrich items with all response fields using batch queries. EnableUserData=false enriches
    // for no user, which leaves out UserData and the per-user counts, and nothing else. With
    // Fields given empty no optional field is sent, so the trailers aren't looked up.
    // Skip blurhash for list views to avoid per-item disk I/O
    let minimal = query.fields_given_empty();
    let tmdb = if minimal { None } else { state.tmdb.api(None) };
    enrich_items_with(
        &state.db_ro,
        &state.server_id,
        query.enable_user_data().then_some(claims.sub),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
        &query.image_options(),
    )
    .await;
    if !held.is_empty() {
        let (mut media, mut held) = (items.into_iter(), held.into_iter());
        items = is_container
            .iter()
            .filter_map(|&c| if c { held.next() } else { media.next() })
            .collect();
    }

    if minimal {
        return Ok(
            Json(minimal_result(items, total, query.start_index_requested())).into_response(),
        );
    }
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: query.start_index_requested(),
    })
    .into_response())
}

/// The answer to a list request with `RustyfinCardIndex=true` (a Rustyfin extension; the web
/// client's library index asks this way): per item only what a poster card shows before its page
/// arrives, Jellyfin's names and shapes, null fields left out: Id, Name, SortName (the letter
/// rail; only when it differs from the name beyond case), Type, ProductionYear, CommunityRating
/// (to the tenth), for episodes their numbers (IndexNumberEnd too) and series, and the Primary
/// picture's tag and BlurHash: about 300 bytes an item. The list's filters and order are the request's; the only
/// other query is one batch of Primary pictures, with no per-item work.
async fn card_index_result(
    pool: &sqlx::SqlitePool,
    items: &[MediaItem],
    total: i64,
    start_index: i64,
) -> Result<Value, ApiError> {
    use rustyfin_model::image::ImageType;
    let ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();
    let images = rustyfin_db::repo_image::get_images_batch_of(
        pool,
        &ids,
        Some(&[ImageType::Primary]),
        Some(1),
    )
    .await?;
    let cards: Vec<Value> = items
        .iter()
        .map(|item| {
            let mut card = serde_json::Map::new();
            card.insert("Id".into(), json!(item.id));
            card.insert("Name".into(), json!(item.name));
            // Only when it sorts otherwise than the name (the rail reads Name when it is missing)
            if let Some(sort_name) = &item.sort_name
                && sort_name.to_lowercase() != item.name.to_lowercase()
            {
                card.insert("SortName".into(), json!(sort_name));
            }
            card.insert("Type".into(), json!(item.media_type.item_kind()));
            if let Some(year) = item.production_year {
                card.insert("ProductionYear".into(), json!(year));
            }
            // To the tenth the card shows
            if let Some(rating) = item.community_rating {
                card.insert(
                    "CommunityRating".into(),
                    json!((rating * 10.0).round() / 10.0),
                );
            }
            if let Some(n) = item.index_number {
                card.insert("IndexNumber".into(), json!(n));
            }
            // A file holding several episodes: the card shows "E1-2"
            if let Some(n) = item.index_number_end {
                card.insert("IndexNumberEnd".into(), json!(n));
            }
            if let Some(n) = item.parent_index_number {
                card.insert("ParentIndexNumber".into(), json!(n));
            }
            if let Some(series) = &item.series_name {
                card.insert("SeriesName".into(), json!(series));
            }
            let primary = images
                .get(&item.id.to_string())
                .and_then(|list| list.iter().find(|img| img.image_type == ImageType::Primary));
            if let Some(img) = primary {
                let tag = rustyfin_core::image_proxy::image_tag(&img.path, img.stamp);
                if let Some(hash) = &img.blurhash {
                    card.insert(
                        "ImageBlurHashes".into(),
                        json!({ "Primary": { tag.clone(): hash } }),
                    );
                }
                card.insert("ImageTags".into(), json!({ "Primary": tag }));
            }
            Value::Object(card)
        })
        .collect();
    Ok(json!({ "Items": cards, "TotalRecordCount": total, "StartIndex": start_index }))
}

/// Jellyfin's optional item fields (ItemFields) that Rustyfin fills on every item, by JSON name.
/// An answer to a request that gives Fields empty leaves them out, as Jellyfin does.
const OPTIONAL_ITEM_FIELDS: &[&str] = &[
    "Overview",
    "Genres",
    "GenreItems",
    "Tags",
    "Studios",
    "People",
    "ProviderIds",
    "ExternalUrls",
    "Chapters",
    "MediaSources",
    "MediaStreams",
    "Path",
    "Taglines",
    "ProductionLocations",
    "RemoteTrailers",
    "OriginalTitle",
    "Etag",
    "ScreenshotImageTags",
    "PrimaryImageAspectRatio",
];

/// The answer to a list request that gives Fields empty (Jellyfin's "no optional fields"): each
/// item without the optional fields and without null fields, as Jellyfin's serializer leaves those
/// out (JsonIgnoreCondition.WhenWritingNull). The base fields, user data and images stay. The web
/// client's library index asks this way, so a whole library stays small.
fn minimal_result(items: Vec<MediaItem>, total: i64, start_index: i64) -> Value {
    let items: Vec<Value> = items
        .iter()
        .map(|item| match serde_json::to_value(item) {
            Ok(Value::Object(mut fields)) => {
                fields.retain(|k, v| !v.is_null() && !OPTIONAL_ITEM_FIELDS.contains(&k.as_str()));
                Value::Object(fields)
            }
            Ok(other) => other,
            Err(_) => Value::Null,
        })
        .collect();
    json!({ "Items": items, "TotalRecordCount": total, "StartIndex": start_index })
}

/// GET /Items/{id}: a media item, or a library as its `CollectionFolder` view.
async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    item_or_library(&state, id, claims.sub, &claims).await
}

/// The media item `id` with `user`'s watch data, the library `id` as its view, or the collection
/// or playlist `id` (a playlist only when `claims`' caller may see it).
async fn item_or_library(
    state: &AppState,
    id: Uuid,
    user: Uuid,
    claims: &Claims,
) -> Result<Response, ApiError> {
    match media_item(state, id, user).await {
        Err(ApiError::NotFound(message)) => {
            if let Some(lib) =
                rustyfin_db::repo_library::get_library_by_id(&state.db_ro, id).await?
            {
                return Ok(
                    Json(super::library::library_view(&lib, &state.server_id)).into_response()
                );
            }
            if let Some(item) = super::collections::container_details(state, id, claims).await? {
                return Ok(Json(item).into_response());
            }
            // A music artist without a folder item is listed under its person id.
            match rustyfin_db::repo_person::music_artist_by_id(&state.db_ro, id).await? {
                Some(artist) => Ok(Json(artist).into_response()),
                None => Err(ApiError::NotFound(message)),
            }
        }
        other => other.map(|item| Json(item).into_response()),
    }
}

/// GET /Users/{userId}/Items/{itemId}: Jellyfin's older form of `GET /Items/{id}`.
async fn get_user_item(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    let user = path_user(&state, &claims, user_id).await?;
    item_or_library(&state, item_id, user, &claims).await
}

/// The media item `id` with `user`'s watch data.
pub(crate) async fn media_item(
    state: &AppState,
    id: Uuid,
    user: Uuid,
) -> Result<MediaItem, ApiError> {
    let id_str = id.to_string();
    // The details every user sees; the requester's watch data is added below. A scan that reads
    // a file again replaces the item's sources, with new ids, so a cached copy is served only
    // while its source ids are still the stored ones.
    let cached = match state.cache.item_details.get(&id_str) {
        Some(cached) => {
            let mut cached_ids: Vec<String> =
                cached.media_sources.iter().map(|s| s.id.clone()).collect();
            cached_ids.sort();
            let stored =
                rustyfin_db::repo_media_source::get_media_source_ids(&state.db_ro, &id).await;
            stored.is_ok_and(|ids| ids == cached_ids).then_some(cached)
        }
        None => None,
    };
    let mut item = match cached {
        Some(cached) => cached,
        None => {
            let mut item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
                .await?
                .ok_or(ApiError::NotFound("item not found".into()))?;
            let _ = populate_media_source_single(&state.db_ro, &mut item).await;
            // Enrich with all response fields, without any user's watch data
            let tmdb = state.tmdb.api(None);
            enrich_item(
                &state.db_ro,
                &state.server_id,
                None,
                &mut item,
                &state.http_client,
                tmdb.as_ref(),
                &state.data_dir,
            )
            .await;
            // Movies, episodes and other videos are not cached: their sources, streams,
            // chapters and trickplay change on rescans, re-probes and extraction, and none of
            // those clear this cache.
            if !matches!(
                item.media_type,
                rustyfin_model::media::MediaType::Movie
                    | rustyfin_model::media::MediaType::Episode
                    | rustyfin_model::media::MediaType::Video
            ) {
                state.cache.item_details.insert(id_str, item.clone());
            }
            item
        }
    };
    // The cached counts are of every child; a user with limits gets those of what they may see.
    let visible = match item.media_type.is_folder() {
        true => crate::parental::user_filter(&state.db_ro, Some(user)).await,
        false => None,
    };
    let include_virtual =
        crate::routes::shows::display_missing_episodes(&state.db_ro, Some(user)).await;
    if (visible.is_some() || include_virtual)
        && let Ok(counts) = rustyfin_db::repo_media::count_items_by_type(
            &state.db_ro,
            id,
            visible.as_deref(),
            include_virtual,
        )
        .await
    {
        set_folder_counts(&mut item, counts);
    }
    let folder = is_episode_folder(&item);
    let stored = rustyfin_db::repo_playback::get_user_item_data(&state.db_ro, user, id)
        .await
        .ok();
    if let Some(user_data) = stored.or_else(|| folder.then(Default::default)) {
        apply_user_data(&mut item, user_data);
        if folder {
            let counts = rustyfin_db::repo_playback::get_unplayed_counts(
                &state.db_ro,
                user,
                &[id],
                visible.as_deref(),
            )
            .await
            .unwrap_or_default();
            let key = id.to_string();
            apply_unplayed_counts(&mut item, counts.get(&key).copied().unwrap_or((0, 0)));
        }
    }
    item.can_download = super::item_download::can_download(state, user, &item).await;
    Ok(item)
}

/// An empty Jellyfin `QueryResult`.
fn empty_result() -> Value {
    json!({ "Items": [], "TotalRecordCount": 0, "StartIndex": 0 })
}

/// GET /Items/{id}/Intros and /Users/{userId}/Items/{itemId}/Intros: Rustyfin plays no intros.
async fn get_intros() -> Json<Value> {
    Json(empty_result())
}

/// The extra types that Jellyfin lists as special features (`BaseItem.DisplayExtraTypes`). `Unknown` comes
/// first: the `HasSpecialFeature` filter takes the rest.
const SPECIAL_FEATURE_TYPES: &[&str] = &[
    "Unknown",
    "BehindTheScenes",
    "Clip",
    "DeletedScene",
    "Interview",
    "Sample",
    "Scene",
    "Featurette",
    "Short",
];

/// The extras of `owner` with one of `types`, as full items for `user_id`, each with its `ExtraType`.
pub(crate) async fn extra_items(
    state: &AppState,
    owner: Uuid,
    user_id: Uuid,
    types: &[&str],
) -> Result<Vec<MediaItem>, ApiError> {
    let extras = rustyfin_db::repo_extras::extras_of(&state.db_ro, owner, types).await?;
    let mut items = Vec::with_capacity(extras.len());
    for (id, extra_type) in extras {
        if let Some(mut item) = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id).await? {
            item.extra_type = Some(extra_type);
            items.push(item);
        }
    }
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(user_id),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
    )
    .await;
    for item in &mut items {
        item.media_sources =
            rustyfin_db::repo_media_source::get_media_sources(&state.db_ro, &item.id).await?;
    }
    Ok(items)
}

/// 404 unless `id` is an item.
async fn require_item(state: &AppState, id: Uuid) -> Result<MediaItem, ApiError> {
    rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("Item not found".into()))
}

/// GET /Items/{id}/LocalTrailers: the item's trailer files, a bare array as in Jellyfin.
async fn get_local_trailers(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    require_item(&state, id).await?;
    Ok(Json(
        extra_items(&state, id, claims.sub, &["Trailer"]).await?,
    ))
}

/// GET /Users/{userId}/Items/{itemId}/LocalTrailers.
async fn get_user_local_trailers(
    state: State<AppState>,
    Path((_user, id)): Path<(Uuid, Uuid)>,
    claims: Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    get_local_trailers(state, Path(id), claims).await
}

/// GET /Items/{id}/SpecialFeatures: the item's special features (behind the scenes, featurettes, deleted
/// scenes and the other extras that are not trailers or themes), a bare array as in Jellyfin.
async fn get_special_features(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    require_item(&state, id).await?;
    Ok(Json(
        extra_items(&state, id, claims.sub, SPECIAL_FEATURE_TYPES).await?,
    ))
}

/// GET /Users/{userId}/Items/{itemId}/SpecialFeatures.
async fn get_user_special_features(
    state: State<AppState>,
    Path((_user, id)): Path<(Uuid, Uuid)>,
    claims: Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    get_special_features(state, Path(id), claims).await
}

/// A `ThemeMediaResult`: the theme items of `owner`.
fn theme_result(owner: Uuid, items: Vec<MediaItem>) -> Value {
    json!({
        "TotalRecordCount": items.len(),
        "Items": items,
        "StartIndex": 0,
        "OwnerId": owner.simple().to_string(),
    })
}

/// The theme items (`ThemeSong` or `ThemeVideo`) of `id`, with their owner. With `inherit`, an item with none
/// takes those of the nearest item above it (an episode its season's or series', as Jellyfin's
/// `inheritFromParent`); the owner is `id` when none has any.
async fn theme_items(
    state: &AppState,
    id: Uuid,
    user_id: Uuid,
    kind: &str,
    inherit: bool,
) -> Result<(Uuid, Vec<MediaItem>), ApiError> {
    let item = require_item(state, id).await?;
    let mut owner = Some(item.id);
    // A parent chain is a few levels deep; the cap stops a cycle in bad data.
    for _ in 0..8 {
        let Some(current) = owner else { break };
        let items = extra_items(state, current, user_id, &[kind]).await?;
        if !items.is_empty() {
            return Ok((current, items));
        }
        if !inherit {
            break;
        }
        owner = rustyfin_db::repo_media::get_item_parent_id(&state.db_ro, current).await?;
    }
    Ok((id, Vec::new()))
}

/// Whether `inheritFromParent` is true.
fn inherits(params: &[(String, String)]) -> bool {
    crate::query::first_value(params, "inheritFromParent")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// GET /Items/{id}/ThemeMedia: the item's theme songs and videos. Rustyfin has no soundtrack songs.
async fn get_theme_media(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let inherit = inherits(&params);
    let (video_owner, videos) = theme_items(&state, id, claims.sub, "ThemeVideo", inherit).await?;
    let (song_owner, songs) = theme_items(&state, id, claims.sub, "ThemeSong", inherit).await?;
    Ok(Json(json!({
        "ThemeVideosResult": theme_result(video_owner, videos),
        "ThemeSongsResult": theme_result(song_owner, songs),
        "SoundtrackSongsResult": theme_result(id, Vec::new()),
    })))
}

/// GET /Items/{id}/ThemeSongs.
async fn get_theme_songs(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let (owner, items) =
        theme_items(&state, id, claims.sub, "ThemeSong", inherits(&params)).await?;
    Ok(Json(theme_result(owner, items)))
}

/// GET /Items/{id}/ThemeVideos.
async fn get_theme_videos(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, ApiError> {
    let (owner, items) =
        theme_items(&state, id, claims.sub, "ThemeVideo", inherits(&params)).await?;
    Ok(Json(theme_result(owner, items)))
}

async fn delete_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    if let Some(status) = super::collections::delete_container(&state, id, &claims).await? {
        return Ok(status.into_response());
    }
    // Jellyfin's EnableContentDeletion; left unset, administrators may and others not.
    let may_delete = claims.sub.is_nil()
        || crate::parental::caller_access(&state, &claims)
            .await?
            .enable_content_deletion
            .unwrap_or(claims.is_admin);
    if !may_delete {
        return Err(ApiError::Forbidden(
            "This user may not delete media.".to_string(),
        ));
    }
    // The uploaded lyric files of the item and the items under it go with them.
    let library = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .map(|item| item.library_id);
    let lyrics = match library {
        Some(library) => rustyfin_core::lyrics::uploads_in_library(&state.db, library).await,
        None => Vec::new(),
    };
    let affected = crate::cache::ApiCache::affected_items(&state.db, id).await;
    let deleted = rustyfin_db::repo_media::delete_item(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound("item not found".into()));
    }
    state.cache.items_changed(&state.image_cache, &affected);
    rustyfin_core::lyrics::remove_orphaned_uploads(&state.db, &lyrics).await;
    // The items under it lose theirs at the image tasks' next run.
    rustyfin_core::trickplay::remove_item_files(&state.data_dir, id).await;
    Ok(Json(json!({ "status": "ok" })).into_response())
}

async fn get_children(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let mut children = rustyfin_db::repo_media::get_children(&state.db_ro, id).await?;
    HiddenItems::for_caller(&state, &claims)
        .await?
        .retain(&mut children);
    let total = children.len() as i64;

    // Only populate media_sources when explicitly requested
    let fields: Vec<&str> = query
        .fields()
        .map(|s| s.split(',').map(|f| f.trim()).collect::<Vec<&str>>())
        .unwrap_or_default();
    let need_media_sources = fields
        .iter()
        .any(|f| f.eq_ignore_ascii_case("MediaSources"));

    let mut items = if need_media_sources {
        let mut children_copy = children;
        let _ = populate_media_sources(&state.db_ro, &mut children_copy).await;
        children_copy
    } else {
        children
    };

    // Enrich items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(claims.sub),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
    )
    .await;

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

async fn get_similar(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<LimitQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let hidden = HiddenItems::for_caller(&state, &claims).await?;
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let limit = query.limit.filter(|&limit| limit >= 0).unwrap_or(12);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);

    let tmdb = state.tmdb.api(None);
    let mut similar: Vec<MediaItem> = Vec::new();

    // Priority 1: TMDb Similar API (highest weight)
    if let (Some(tmdb_id), Some(api), Some(media_type)) =
        (item.tmdb_id, tmdb.as_ref(), tmdb_kind(&item))
    {
        let tmdb_similar = rustyfin_core::tmdb_client::get_similar(api, media_type, tmdb_id).await;

        // For each TMDb similar item, find the local item by tmdb_id
        for (similar_tmdb_id, _name) in tmdb_similar {
            if similar.len() >= limit {
                break;
            }
            // Find local item by tmdb_id
            let local_item: Option<MediaItem> =
                sqlx::query_as::<_, rustyfin_db::repo_media::MediaItemRow>(
                    "SELECT * FROM media_items WHERE tmdb_id = ? AND media_type = ? AND id != ?",
                )
                .bind(similar_tmdb_id)
                .bind(item.media_type.item_kind().to_string())
                .bind(item.id.to_string())
                .fetch_optional(&state.db_ro)
                .await
                .ok()
                .flatten()
                .map(|row| row.into());

            if let Some(local_item) = local_item.filter(|i| !hidden.hides(i.id, i.library_id)) {
                similar.push(local_item);
            }
        }
    }

    // Priority 2: Local algorithm fills remaining slots
    if similar.len() < limit {
        let mut remaining = i64::try_from(limit - similar.len()).unwrap_or(i64::MAX);
        if hidden.is_restricted() {
            // Some of the best matches may be hidden: ask for more to fill the list.
            remaining = remaining.saturating_mul(4);
        }
        let local_similar =
            rustyfin_db::repo_media::get_similar_items(&state.db_ro, &item, remaining).await?;

        // Filter out items already added from TMDb
        let existing_ids: std::collections::HashSet<String> =
            similar.iter().map(|i| i.id.to_string()).collect();
        for local_item in local_similar {
            if similar.len() >= limit {
                break;
            }
            if !existing_ids.contains(&local_item.id.to_string())
                && !hidden.hides(local_item.id, local_item.library_id)
            {
                similar.push(local_item);
            }
        }
    }

    let total = similar.len() as i64;

    // Enrich similar items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(claims.sub),
        &mut similar,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
    )
    .await;

    Ok(Json(QueryResult {
        items: similar,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LimitQuery {
    limit: Option<i64>,
}

async fn get_latest(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    let user = query_user(
        &state,
        &claims,
        crate::query::uuid_param(&params, "userId")?,
    )
    .await?
    .unwrap_or(claims.sub);
    latest_page(&state, user, &params).await
}

/// The "Latest" row (`/Items/Latest`, `/Users/{userId}/Items/Latest`) for `user`, as Jellyfin
/// 12.4 builds it (see `latest::latest_entries`).
async fn latest_page(
    state: &AppState,
    user: Uuid,
    params: &[(String, String)],
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    let (mut items, child_counts) =
        crate::routes::latest::latest_entries(state, user, params).await?;
    // Enrich latest items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(user),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
    )
    .await;
    // Jellyfin 12.4 sets ChildCount to the number of new items of a group shown as its
    // container; other entries keep their own.
    for (item, count) in items.iter_mut().zip(child_counts) {
        if count > 0 {
            item.child_count = Some(count);
        }
    }
    Ok(Json(items))
}

async fn get_resume(
    State(state): State<AppState>,
    Query(query): Query<ResumeQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user = query_user(&state, &claims, query.user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    resume_page(&state, &claims, user, &query).await
}

/// One page of the user's resume list (`/UserItems/Resume`, `/Items/Resume` and the legacy
/// `/Users/{userId}/Items/Resume`), with the count of all matching items, as Jellyfin answers it.
async fn resume_page(
    state: &AppState,
    claims: &Claims,
    user: Uuid,
    query: &ResumeQuery,
) -> Result<Json<QueryResult>, ApiError> {
    let start_index = query.start_index.unwrap_or(0).max(0);
    let exclude_active =
        ResumeQuery::flag("excludeActiveSessions", &query.exclude_active_sessions)?
            .unwrap_or(false);
    let count_total =
        ResumeQuery::flag("enableTotalRecordCount", &query.enable_total_record_count)?
            .unwrap_or(true);
    // Jellyfin's `excludeActiveSessions`: leave out what the user's sessions are playing.
    let exclude_ids = if exclude_active {
        crate::sessions::now_playing_items(state, user).await?
    } else {
        Vec::new()
    };
    let filter = rustyfin_db::repo_media::ResumeFilter {
        include_types: query.include_types(),
        exclude_types: csv(query.exclude_item_types.as_deref()),
        parent_id: query.parent_id,
        start_index,
        limit: query.limit.filter(|l| *l >= 0),
        hidden_ids_json: HiddenItems::for_caller(state, claims).await?.json(),
        exclude_ids,
    };
    let (mut items, total) =
        rustyfin_db::repo_media::get_user_resume_items(&state.db_ro, user, &filter).await?;
    // Enrich resume items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(user),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
    )
    .await;
    Ok(Json(QueryResult {
        items,
        total_record_count: if count_total { total } else { 0 },
        start_index,
    }))
}

/// The values of a comma-separated query parameter, trimmed, empty ones dropped.
fn csv(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// Jellyfin's resume query parameters. Its clients send them in camelCase or PascalCase.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeQuery {
    #[serde(default, alias = "Limit")]
    limit: Option<i64>,
    #[serde(default, alias = "StartIndex")]
    start_index: Option<i64>,
    #[serde(default, alias = "UserId")]
    user_id: Option<Uuid>,
    #[serde(default, alias = "ParentId")]
    parent_id: Option<Uuid>,
    #[serde(default, alias = "IncludeItemTypes")]
    include_item_types: Option<String>,
    #[serde(default, alias = "ExcludeItemTypes")]
    exclude_item_types: Option<String>,
    #[serde(default, alias = "MediaTypes")]
    media_types: Option<String>,
    #[serde(default, alias = "ExcludeActiveSessions")]
    exclude_active_sessions: Option<String>,
    #[serde(default, alias = "EnableTotalRecordCount")]
    enable_total_record_count: Option<String>,
}

impl ResumeQuery {
    /// A boolean parameter (`true`/`false` in any case; blank is unset).
    fn flag(name: &str, value: &Option<String>) -> Result<Option<bool>, ApiError> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| crate::query::parse_bool(name, v))
            .transpose()
    }

    /// The stored types to list: `IncludeItemTypes`, narrowed to those of `MediaTypes`. Empty means
    /// any type; when the two leave nothing, a type no item has, so nothing is listed.
    fn include_types(&self) -> Vec<String> {
        let kinds = csv(self.include_item_types.as_deref());
        let media = csv(self.media_types.as_deref());
        if media.is_empty() {
            return kinds;
        }
        let of_media: Vec<String> = media
            .iter()
            .flat_map(|m| match m.to_ascii_lowercase().as_str() {
                "video" => &["Movie", "Episode"][..],
                "audio" => &["Audio"][..],
                "photo" => &["Photo"][..],
                _ => &[][..],
            })
            .map(|k| k.to_string())
            .collect();
        let types: Vec<String> = if kinds.is_empty() {
            of_media
        } else {
            kinds.into_iter().filter(|k| of_media.contains(k)).collect()
        };
        if types.is_empty() {
            vec![String::new()]
        } else {
            types
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FiltersQuery {
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
    include_item_types: Option<String>,
    #[serde(rename = "includeItemTypes")]
    include_item_types_lower: Option<String>,
}

impl FiltersQuery {
    /// The `IncludeItemTypes` list, without empty entries.
    fn item_types(&self) -> Vec<String> {
        self.include_item_types
            .as_deref()
            .or(self.include_item_types_lower.as_deref())
            .into_iter()
            .flat_map(|types| types.split(','))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

async fn get_filters(
    State(state): State<AppState>,
    Query(query): Query<FiltersQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<FiltersResult>, ApiError> {
    // A library, an item or a collection; the database works out which.
    let parent_id = query.parent_id.or(query.parent_id_lower);
    let item_types = query.item_types();
    // A user with parental limits gets the values of the items they may see, never the cached
    // list every user shares.
    if let Some(hidden) = HiddenItems::for_caller(&state, &claims).await?.json() {
        let (genres, years, official_ratings) = rustyfin_db::repo_media::get_filters_hiding(
            &state.db_ro,
            parent_id,
            &item_types,
            &hidden,
        )
        .await?;
        return Ok(Json(FiltersResult {
            genres,
            years,
            official_ratings,
        }));
    }
    let cache_key = format!(
        "{}:{}",
        parent_id.map_or("all".to_string(), |id| id.to_string()),
        item_types.join(",")
    );
    // Check cache first
    if let Some(cached) = state.cache.filter_lists.get(&cache_key)
        && let Ok(filters) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(filters));
    }
    let (genres, years, official_ratings) =
        rustyfin_db::repo_media::get_filters(&state.db_ro, parent_id, &item_types).await?;
    let result = FiltersResult {
        genres,
        years,
        official_ratings,
    };
    // Cache the result
    if let Ok(json) = serde_json::to_value(&result) {
        state.cache.filter_lists.insert(cache_key, json);
    }
    Ok(Json(result))
}

/// GET /Items/Filters2: Jellyfin's `QueryFilters`, the genres (with their ids) and tags under the
/// parent. Rustyfin keeps no tag list for filters, so Tags is empty.
async fn get_filters2(
    State(state): State<AppState>,
    Query(query): Query<FiltersQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Value>, ApiError> {
    let library_id = query.parent_id.or(query.parent_id_lower);
    let item_types = query.item_types();
    // A user with parental limits gets the genres of the items they may see.
    let (genres, _, _) = match HiddenItems::for_caller(&state, &claims).await?.json() {
        Some(hidden) => {
            rustyfin_db::repo_media::get_filters_hiding(
                &state.db_ro,
                library_id,
                &item_types,
                &hidden,
            )
            .await?
        }
        None => rustyfin_db::repo_media::get_filters(&state.db_ro, library_id, &item_types).await?,
    };
    let genres: Vec<Value> = genres
        .into_iter()
        .map(|name| {
            let id = rustyfin_db::repo_by_name::by_name_id(
                rustyfin_db::repo_by_name::ByNameKind::Genre,
                &name,
            );
            json!({ "Name": name, "Id": id.simple().to_string() })
        })
        .collect();
    let (audio, subtitles) = filter_languages(&state, &claims, library_id, &item_types).await?;
    Ok(Json(json!({
        "Genres": genres,
        "Tags": [],
        "AudioLanguages": audio,
        "SubtitleLanguages": subtitles,
    })))
}

/// The AudioLanguages and SubtitleLanguages of `/Items/Filters2`, as Jellyfin 12.4's
/// `FilterController.GetQueryFilters` lists them: only for movies, series, seasons or episodes
/// (a series' or season's streams are its episodes'), each `{ Name, Value }` with the language's
/// display name and code (the code alone when it names no language), sorted by name.
async fn filter_languages(
    state: &AppState,
    claims: &Claims,
    library_id: Option<Uuid>,
    item_types: &[String],
) -> Result<(Vec<Value>, Vec<Value>), ApiError> {
    let video = ["Movie", "Series", "Season", "Episode"];
    let has = |kind: &str| item_types.iter().any(|t| t.eq_ignore_ascii_case(kind));
    if !video.iter().any(|kind| has(kind)) {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut types = item_types.to_vec();
    if (has("Series") || has("Season")) && !has("Episode") {
        types.push("Episode".to_string());
    }
    let hidden = HiddenItems::for_caller(state, claims).await?.json();
    let mut lists = Vec::new();
    for kind in ["audio", "subtitle"] {
        let languages = rustyfin_db::repo_item_filter::stream_languages(
            &state.db_ro,
            kind,
            library_id,
            &types,
            hidden.as_deref(),
        )
        .await?;
        let mut pairs: Vec<(String, String)> = languages
            .into_iter()
            .map(|language| {
                let name = match super::localization::language_display_name(&language) {
                    Some(display) => format!("{display} ({language})"),
                    None => language.clone(),
                };
                (name, language)
            })
            .collect();
        pairs.sort_by_key(|(name, _)| name.to_lowercase());
        lists.push(
            pairs
                .into_iter()
                .map(|(name, value)| json!({ "Name": name, "Value": value }))
                .collect::<Vec<_>>(),
        );
    }
    let subtitles = lists.pop().unwrap_or_default();
    let audio = lists.pop().unwrap_or_default();
    Ok((audio, subtitles))
}

/// The claims a list answers for: the user `user_id` names, under `path_user`'s rules, with that
/// user's administrator flag, so their play state, favourites, filters and access apply, as in
/// Jellyfin. No user, the nil id or the caller's own id leaves the claims as they are.
async fn claims_for_user(
    state: &AppState,
    claims: Claims,
    user_id: Option<Uuid>,
) -> Result<Claims, ApiError> {
    let Some(user_id) = user_id.filter(|id| !id.is_nil() && *id != claims.sub) else {
        return Ok(claims);
    };
    path_user(state, &claims, user_id).await?;
    let user = rustyfin_db::repo_user::get_user_by_id(&state.db_ro, user_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("user not found".into()))?;
    Ok(Claims {
        sub: user_id,
        name: user.name,
        is_admin: user.is_administrator,
        ..claims
    })
}

/// GET /Items, for the user a `userId` names.
async fn get_items_for_query_user(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    query: Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    let user_id = crate::query::uuid_param(&params, "userId")?;
    let claims = claims_for_user(&state, claims, user_id).await?;
    get_items(State(state), query, Extension(claims)).await
}

/// GET /Users/{userId}/Items, for that user.
async fn get_user_items(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    query: Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    let claims = claims_for_user(&state, claims, Some(user_id)).await?;
    get_items(State(state), query, Extension(claims)).await
}

async fn get_user_latest(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    let user = path_user(&state, &claims, user_id).await?;
    latest_page(&state, user, &params).await
}

async fn get_user_resume(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ResumeQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    resume_page(&state, &claims, user_id, &query).await
}

/// DELETE /Items - Delete multiple items
async fn delete_items(
    State(state): State<AppState>,
    Query(query): Query<ItemsQuery>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let ids: Vec<Uuid> = if let Some(parent_id) = query.parent_id.or(query.parent_id_lower) {
        // Delete all items under a parent
        let items = rustyfin_db::repo_media::get_items_by_parent(&state.db_ro, parent_id).await?;
        items.into_iter().map(|i| i.id).collect()
    } else {
        Vec::new()
    };
    let mut affected = Vec::new();
    for id in &ids {
        let under = crate::cache::ApiCache::affected_items(&state.db, *id).await;
        if let Ok(true) = rustyfin_db::repo_media::delete_item(&state.db, *id).await {
            affected.extend(under);
            rustyfin_core::trickplay::remove_item_files(&state.data_dir, *id).await;
        }
    }
    if let Some(parent) = query.parent_id.or(query.parent_id_lower) {
        affected.push(parent);
    }
    state.cache.items_changed(&state.image_cache, &affected);
    Ok((StatusCode::OK, Json(serde_json::json!({"Success": true}))))
}

/// GET /Items/{id}/Ancestors - Get parent chain
/// GET /Items/{id}/Ancestors: as in Jellyfin, an array of the item's parents, nearest first,
/// ending with its library's `CollectionFolder`.
async fn get_ancestors(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<Value>>, ApiError> {
    // `item_gate` has answered 404 already when the caller's limits hide the item; an item they
    // may see has only visible items above it
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let mut parents = Vec::new();
    let mut next = item.parent_id;
    // A parent chain is a few levels deep; the cap stops a cycle in bad data.
    while let Some(parent_id) = next.filter(|_| parents.len() < 16) {
        let Some(parent) = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, parent_id).await?
        else {
            break;
        };
        next = parent.parent_id;
        parents.push(parent);
    }
    let tmdb = state.tmdb.api(None);
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(claims.sub),
        &mut parents,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
    )
    .await;
    let mut ancestors: Vec<Value> = parents
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .collect();
    if let Some(lib) =
        rustyfin_db::repo_library::get_library_by_id(&state.db_ro, item.library_id).await?
    {
        ancestors.push(super::library::library_view(&lib, &state.server_id));
    }
    Ok(Json(ancestors))
}

/// GET /Items/{id}/Collections?userId=&startIndex=&limit=, as Jellyfin 12.4's
/// `LibraryController.GetItemCollections`: the collections holding the item that the caller (or
/// `userId`) may see, as `/Items/{id}` answers each, sorted by sort name then name without case,
/// then paged; the count is of every one. 404 for an item that doesn't exist (a hidden one is a
/// 404 already, by `parental`).
async fn get_item_collections(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Query<Vec<(String, String)>>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    use crate::query::{count_param, uuid_param};
    let user = query_user(&state, &claims, uuid_param(&params, "userId")?)
        .await?
        .unwrap_or(claims.sub);
    match media_item(&state, id, user).await {
        Ok(_) => {}
        Err(ApiError::NotFound(_))
            if super::collections::container_details(&state, id, &claims)
                .await?
                .is_some() => {}
        Err(e) => return Err(e),
    }
    let start = count_param(&params, "startIndex")?;
    let limit = count_param(&params, "limit")?;
    let mut collections = Vec::new();
    for collection in rustyfin_db::repo_media::get_item_collections(&state.db_ro, id).await? {
        match super::collections::container_details(&state, collection.id, &claims).await {
            Ok(Some(item)) => collections.push(item),
            Ok(None) | Err(ApiError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    let key = |item: &MediaItem| {
        (
            item.sort_name
                .as_deref()
                .unwrap_or(&item.name)
                .to_uppercase(),
            item.name.to_uppercase(),
        )
    };
    collections.sort_by_key(key);
    let total = collections.len() as i64;
    let items = collections
        .into_iter()
        .skip(start.unwrap_or(0))
        .take(limit.unwrap_or(usize::MAX))
        .collect();
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: start.unwrap_or(0) as i64,
    }))
}

/// DELETE /Items/{id}/Images/{imageType}?imageIndex=: the item's images of that type, or the one
/// at imageIndex. 204, or 404 for an unknown item, as in Jellyfin.
async fn delete_item_image(
    State(state): State<AppState>,
    Path((id, image_type)): Path<(Uuid, String)>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<StatusCode, ApiError> {
    match crate::query::first_value(&params, "imageIndex") {
        Some(index) => {
            let index = index
                .parse()
                .map_err(|_| ApiError::BadRequest(format!("invalid imageIndex: {index}")))?;
            remove_item_image(&state, id, &image_type, Some(index)).await
        }
        None => remove_item_image(&state, id, &image_type, None).await,
    }
}

/// DELETE /Items/{id}/Images/{imageType}/{imageIndex}
async fn delete_item_image_index(
    State(state): State<AppState>,
    Path((id, image_type, index)): Path<(Uuid, String, i32)>,
) -> Result<StatusCode, ApiError> {
    remove_item_image(&state, id, &image_type, Some(index)).await
}

async fn remove_item_image(
    state: &AppState,
    id: Uuid,
    image_type: &str,
    index: Option<i32>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound("item not found".into()))?;
    let image_type = rustyfin_model::image::ImageType::from_str(image_type)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown image type: {image_type}")))?;
    match index {
        Some(index) => {
            rustyfin_db::repo_media::delete_item_image_index(
                &state.db,
                id,
                image_type.as_str(),
                index,
            )
            .await?
        }
        None => {
            rustyfin_db::repo_media::delete_item_image(&state.db, id, image_type.as_str()).await?
        }
    }
    state
        .cache
        .item_changed(&state.db, &state.image_cache, id)
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /Items/{id}/AlternateSources
async fn delete_alternate_sources(
    State(_state): State<AppState>,
    Path(_id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}

/// DELETE /Items/{id}/Subtitles/{index}
async fn delete_subtitles(
    State(_state): State<AppState>,
    Path((_id, _index)): Path<(Uuid, i32)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}

/// GET /Items/{id}/Attachments/{index}
async fn get_attachments(
    State(_state): State<AppState>,
    Path((_id, _index)): Path<(Uuid, i32)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}

/// GET /Channels - Return empty channels list
async fn get_channels(State(_state): State<AppState>) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
        start_index: 0,
    }))
}

/// GET /Channels/Items/Latest
async fn get_channels_items_latest(
    State(_state): State<AppState>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(QueryResult {
        items: Vec::new(),
        total_record_count: 0,
        start_index: 0,
    }))
}

/// GET /Channels/{channelId}/Features
async fn get_channel_features(
    State(_state): State<AppState>,
    Path(_channel_id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}
