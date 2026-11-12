use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
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
    Ok(())
}

/// Populate `media_sources` on a single item.
async fn populate_media_source_single(
    pool: &SqlitePool,
    item: &mut MediaItem,
) -> Result<(), ApiError> {
    let sources = rustyfin_db::repo_media_source::get_media_sources(pool, &item.id).await?;
    item.media_sources = sources;
    Ok(())
}

/// Sets a user's watch data on an item, with its CompletionPercentage if the runtime is known.
fn apply_user_data(item: &mut MediaItem, user_data: rustyfin_model::playback::UserItemDataDto) {
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
    item.user_data = Some(user_data);
}

/// Set an item's image tags (the first image of each type) and backdrop tags (every backdrop, in
/// order) from its image rows.
fn set_image_tags(item: &mut MediaItem, images: &[rustyfin_model::image::ItemImage]) {
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

/// Load the image tags of an item read with `get_parent_item` or `get_album_for_audio`, which
/// leave them empty.
async fn load_image_tags(pool: &SqlitePool, item: &mut MediaItem) {
    if let Ok(images) = rustyfin_db::repo_image::get_images_for_item(pool, item.id).await {
        set_image_tags(item, &images);
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
        provider_ids.set("tmdb".to_string(), tmdb.to_string());
    }
    if let Some(tvdb) = item.tvdb_id {
        provider_ids.set("tvdb".to_string(), tvdb.to_string());
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
            item.provider_ids.set("imdb".to_string(), imdb);
        }
        item.lock_data = extras.lock_data;
        item.locked_fields = extras
            .locked_fields
            .split('|')
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .collect();
    }

    // ExternalUrls from provider_ids
    let mut external_urls = Vec::new();
    if let Some(tmdb) = item.tmdb_id {
        let kind = if item.is_movie { "movie" } else { "tv" };
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
    if let Some(tmdb) = item.tmdb_id
        && let Some(api) = tmdb_api
    {
        let kind = if item.is_movie { "movie" } else { "tv" };
        let trailers = rustyfin_core::provider_tmdb::get_trailers(api, kind, tmdb).await;
        item.remote_trailers = trailers
            .into_iter()
            .map(|(name, url)| rustyfin_model::media::ExternalUrl { name, url })
            .collect();
        item.trailer_count = Some(item.remote_trailers.len() as i32);
        item.local_trailer_count = Some(0);
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
                let key = format!("{}-{}", img.image_type.as_str(), img.image_index);
                item.image_blur_hashes.insert(key, hash.clone());
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
                    let key = format!("{}-{}", img.image_type.as_str(), img.image_index);
                    item.image_blur_hashes.insert(key, hash);
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
        item.chapters = chapters
            .iter()
            .map(|c| rustyfin_model::media::ChapterInfo {
                start_ticks: c.start_position_ticks,
                name: c.name.clone(),
                image: c.image_path.clone(),
                image_date_modified: None,
            })
            .collect();
        item.chapter_count = item.chapters.len() as i32;
    }

    // ScreenshotImageTags from trickplay table
    if let Ok(Some(_trickplay)) = trickplay_result {
        item.screenshot_image_tags = vec![format!("{}-{}", item.id, _trickplay.width)];
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
        }
        // AlbumArtist from album name
        if item.album_artist.is_none()
            && let Some(parent_id) = item.parent_id
            && let Ok(Some(album)) = rustyfin_db::repo_media::get_parent_item(pool, parent_id).await
        {
            item.album_artist = Some(album.name.clone());
        }
        // Artists from item_people table (already fetched in parallel)
        if let Ok(artists) = &people_result {
            item.artists = artists
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
            item.album_artists = item.artists.clone();
            // Also populate People for audio items
            item.people = item.artists.clone();
        }
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

    // --- Folder counts (for Series, Season, MusicAlbum, Folder) ---
    if item.media_type.is_folder() {
        if let Ok(counts) = rustyfin_db::repo_media::count_items_by_type(pool, item.id).await {
            let mut song_count: i32 = 0;
            let mut album_count: i32 = 0;
            let mut movie_count: i32 = 0;
            let mut series_count: i32 = 0;
            let mut episode_count: i32 = 0;
            let mut child_count: i32 = 0;

            for (media_type, count) in counts {
                let c = count as i32;
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
    let trickplay_map = rustyfin_db::repo_trickplay::get_trickplay_batch(pool, &item_ids)
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

    // Batch query 5: Folder counts for folder items
    let folder_ids: Vec<Uuid> = items
        .iter()
        .filter(|item| item.media_type.is_folder())
        .map(|item| item.id)
        .collect();
    let folder_counts_map = if folder_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        rustyfin_db::repo_media::get_folder_counts_batch(pool, &folder_ids)
            .await
            .unwrap_or_default()
    };

    // Batch query 6: All images for all items - populates image_tags
    let images_map = rustyfin_db::repo_image::get_images_batch(pool, &item_ids)
        .await
        .unwrap_or_default();

    // Batch query 7: Images for parent items too (needed for inherited image tags)
    let parent_image_ids: Vec<Uuid> = parent_map.values().map(|p| p.id).collect();
    let parent_images_map = if parent_image_ids.is_empty() {
        std::collections::HashMap::new()
    } else {
        rustyfin_db::repo_image::get_images_batch(pool, &parent_image_ids)
            .await
            .unwrap_or_default()
    };

    // Batch query 8: People for all items
    let people_map: std::collections::HashMap<String, Vec<rustyfin_db::repo_media::PersonRow>> = {
        let mut map: std::collections::HashMap<String, Vec<rustyfin_db::repo_media::PersonRow>> =
            std::collections::HashMap::new();
        for chunk_ids in item_ids.chunks(100) {
            let placeholders: Vec<String> =
                (1..=chunk_ids.len()).map(|i| format!("?{}", i)).collect();
            let query = format!(
                "SELECT ip.item_id, p.id, p.name, ip.role, ip.type, (SELECT pi.path FROM person_images pi WHERE pi.person_id = p.id AND pi.image_type = 'Primary' AND pi.image_index = 0) FROM item_people ip JOIN people p ON p.id = ip.person_id WHERE ip.item_id IN ({}) ORDER BY ip.item_id, ip.sort_order IS NULL, ip.sort_order, ip.rowid",
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
            provider_ids.set("tmdb".to_string(), tmdb.to_string());
        }
        if let Some(tvdb) = item.tvdb_id {
            provider_ids.set("tvdb".to_string(), tvdb.to_string());
        }
        item.provider_ids = provider_ids;

        // ExternalUrls from provider_ids
        let mut external_urls = Vec::new();
        if let Some(tmdb) = item.tmdb_id {
            let kind = if item.is_movie { "movie" } else { "tv" };
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
        }

        // ImageBlurHashes: prefer stored values, fall back to on-demand computation
        if !skip_blurhash {
            // Use images from the batch-loaded map if available
            let images_for_blurhash = images_map
                .get(&item.id.to_string())
                .cloned()
                .unwrap_or_default();
            for img in images_for_blurhash {
                // Use pre-computed blurhash from the database if available
                if let Some(ref hash) = img.blurhash {
                    let key = format!("{}-{}", img.image_type.as_str(), img.image_index);
                    item.image_blur_hashes.insert(key, hash.clone());
                } else if !img.path.starts_with("http://") && !img.path.starts_with("https://") {
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
                        let key = format!("{}-{}", img.image_type.as_str(), img.image_index);
                        item.image_blur_hashes.insert(key, hash);
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
            item.chapters = chapters
                .iter()
                .map(|c| rustyfin_model::media::ChapterInfo {
                    start_ticks: c.start_position_ticks,
                    name: c.name.clone(),
                    image: c.image_path.clone(),
                    image_date_modified: None,
                })
                .collect();
            item.chapter_count = item.chapters.len() as i32;
        }

        // ScreenshotImageTags from trickplay batch map
        if let Some(trickplay) = trickplay_map.get(&item.id.to_string()) {
            item.screenshot_image_tags = vec![format!("{}-{}", item.id, trickplay.width)];
        }

        // Etag from date_created
        item.etag = Some(item.date_created.to_rfc3339());

        // CompletionPercentage from user_data batch map
        if let Some(user_data) = user_data_map.get(&item.id.to_string()) {
            item.user_data = Some(user_data.clone());
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
            item.album_artist = Some(parent.name.clone());
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
            // For audio items, also populate artists
            if item.media_type == rustyfin_model::media::MediaType::Audio {
                item.artists = item.people.clone();
                item.album_artists = item.people.clone();
            }
        }
    }

    // Batch fetch trailers from TMDb API for items with tmdb_id
    if let Some(api) = tmdb_api {
        let trailer_items: Vec<(usize, i64, bool)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.tmdb_id.map(|tmdb| (i, tmdb, item.is_movie)))
            .collect();

        for (idx, tmdb_id, is_movie) in trailer_items {
            let kind = if is_movie { "movie" } else { "tv" };
            let trailers = rustyfin_core::provider_tmdb::get_trailers(api, kind, tmdb_id).await;
            let item = &mut items[idx];
            item.remote_trailers = trailers
                .into_iter()
                .map(|(name, url)| rustyfin_model::media::ExternalUrl { name, url })
                .collect();
            item.trailer_count = Some(item.remote_trailers.len() as i32);
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Items", get(get_items))
        .route("/Items/{id}", get(get_item))
        .route("/Items/{id}/Children", get(get_children))
        .route("/Items/{id}/Similar", get(get_similar))
        .route("/Items/Latest", get(get_latest))
        .route("/Items/Resume", get(get_resume))
        .route("/Items/Filters", get(get_filters))
        .route(
            "/Items/{id}/Trickplay/Preview/{width}/{seekTicks}",
            get(get_trickplay_preview),
        )
        .route("/Users/{userId}/Items", get(get_user_items))
        .route("/Users/{userId}/Items/Latest", get(get_user_latest))
        .route("/Users/{userId}/Items/Resume", get(get_user_resume))
        // Additional Items endpoints
        .route("/Items/{id}/Ancestors", get(get_ancestors))
        .route("/Items/{id}/Collections", get(get_item_collections))
        .route("/Items/{id}/Attachments/{index}", get(get_attachments))
        // User items with rating
        .route(
            "/Users/{userId}/Items/{itemId}/Rating",
            delete(delete_rating),
        )
        // Channels
        .route("/Channels", get(get_channels))
        .route("/Channels/Items/Latest", get(get_channels_items_latest))
        .route("/Channels/{channelId}/Features", get(get_channel_features))
}

/// Administrator-only item removal routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Items", delete(delete_items))
        .route("/Items/{id}", delete(delete_item))
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
    tags: Option<String>,
    features: Option<String>,
    official_ratings: Option<String>,
    #[serde(rename = "officialRatings")]
    official_ratings_lower: Option<String>,
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
    subtitle_language: Option<String>,
    #[serde(rename = "subtitleLanguage")]
    subtitle_language_lower: Option<String>,
    enable_auto_hide: Option<bool>,
    #[serde(rename = "enableAutoHide")]
    enable_auto_hide_lower: Option<bool>,
    collapse_box_set_items: Option<bool>,
    #[serde(rename = "collapseBoxSetItems")]
    collapse_box_set_items_lower: Option<bool>,
    group_projects: Option<bool>,
    #[serde(rename = "groupProjects")]
    group_projects_lower: Option<bool>,
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
        self.recursive.or(self.recursive_lower).unwrap_or(false)
    }
    fn fields(&self) -> Option<&str> {
        self.fields.as_deref().or(self.fields_lower.as_deref())
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
    fn subtitle_language(&self) -> Option<&str> {
        self.subtitle_language
            .as_deref()
            .or(self.subtitle_language_lower.as_deref())
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

async fn get_items(
    State(state): State<AppState>,
    Query(query): Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let media_types: Option<Vec<String>> = query
        .include_item_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let exclude_media_types: Option<Vec<String>> = query
        .exclude_item_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let genres: Option<Vec<String>> = query
        .genres
        .as_ref()
        .map(|s| s.split(',').map(|g| g.trim().to_string()).collect());
    let years: Option<Vec<i32>> = query
        .years()
        .map(|s| s.split(',').filter_map(|y| y.trim().parse().ok()).collect());
    let official_ratings: Option<Vec<String>> = query
        .official_ratings()
        .map(|s| s.split(',').map(|r| r.trim().to_string()).collect());
    let tags: Option<Vec<String>> = query
        .tags()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
    let media_types_filter: Option<Vec<String>> = query
        .media_types()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
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

    // If IsMovie/IsSeries is set, constrain media_type
    let constrained_media_types: Option<Vec<String>> = if let Some(true) = query.is_movie() {
        Some(vec!["\"Movie\"".to_string()])
    } else if let Some(true) = query.is_series() {
        Some(vec!["\"Series\"".to_string()])
    } else {
        media_types
    };

    // GenreIds and StudioIds keep the items with one of those genres and one of those studios.
    let item_ids = rustyfin_db::repo_by_name::item_ids_with(
        &state.db_ro,
        &parse_ids(query.genre_ids()),
        &parse_ids(query.studio_ids()),
    )
    .await?;

    let (items, total) = rustyfin_db::repo_media::get_items_filtered(
        &state.db_ro,
        query.parent_id(),
        query.library_id(),
        constrained_media_types.as_deref(),
        exclude_media_types.as_deref(),
        genres.as_deref(),
        years.as_deref(),
        query.search_term(),
        query.name_starts_with(),
        query.name_starts_with_or_greater(),
        query.sort_by(),
        query.sort_order(),
        start_index,
        limit,
        query.recursive(),
        query.is_favorite(),
        query.is_unplayed(),
        query.is_played(),
        official_ratings.as_deref(),
        tags.as_deref(),
        media_types_filter.as_deref(),
        query.start_date(),
        query.end_date(),
        query.premiere_date(),
        query.date_created(),
        runtime_parts,
        Some(claims.sub),
        item_ids.as_deref(),
    )
    .await?;

    // Only populate media_sources when explicitly requested via Fields parameter
    // This significantly speeds up listing views with many items
    let fields: Vec<&str> = query
        .fields()
        .map(|s| s.split(',').map(|f| f.trim()).collect::<Vec<&str>>())
        .unwrap_or_default();
    let need_media_sources = fields.contains(&"MediaSources");

    let mut items = if need_media_sources {
        let mut items_copy = items;
        let _ = populate_media_sources(&state.db_ro, &mut items_copy).await;
        items_copy
    } else {
        items
    };

    // Enrich items with all response fields using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
    drop(config);

    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: query.start_index_requested(),
    }))
}

async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<MediaItem>, ApiError> {
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
            let config = state.config.read().await;
            let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
            drop(config);
            // Movies and episodes are not cached: their sources, streams, chapters and
            // trickplay change on rescans, re-probes and extraction, and none of those
            // clear this cache.
            if !matches!(
                item.media_type,
                rustyfin_model::media::MediaType::Movie | rustyfin_model::media::MediaType::Episode
            ) {
                state.cache.item_details.insert(id_str, item.clone());
            }
            item
        }
    };
    if let Ok(user_data) =
        rustyfin_db::repo_playback::get_user_item_data(&state.db_ro, claims.sub, id).await
    {
        apply_user_data(&mut item, user_data);
    }
    Ok(Json(item))
}

async fn delete_item(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let deleted = rustyfin_db::repo_media::delete_item(&state.db, id).await?;
    if !deleted {
        return Err(ApiError::NotFound("item not found".into()));
    }
    Ok(Json(json!({ "status": "ok" })))
}

async fn get_children(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let children = rustyfin_db::repo_media::get_children(&state.db_ro, id).await?;
    let total = children.len() as i64;

    // Only populate media_sources when explicitly requested
    let fields: Vec<&str> = query
        .fields()
        .map(|s| s.split(',').map(|f| f.trim()).collect::<Vec<&str>>())
        .unwrap_or_default();
    let need_media_sources = fields.contains(&"MediaSources");

    let mut items = if need_media_sources {
        let mut children_copy = children;
        let _ = populate_media_sources(&state.db_ro, &mut children_copy).await;
        children_copy
    } else {
        children
    };

    // Enrich items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
    drop(config);

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
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    let limit = query.limit.filter(|&limit| limit >= 0).unwrap_or(12);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);

    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
    let mut similar: Vec<MediaItem> = Vec::new();

    // Priority 1: TMDb Similar API (highest weight)
    if let (Some(tmdb_id), Some(api)) = (item.tmdb_id, tmdb.as_ref()) {
        let media_type = if item.is_movie { "movie" } else { "tv" };
        let tmdb_similar =
            rustyfin_core::provider_tmdb::get_similar(api, media_type, tmdb_id).await;

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

            if let Some(local_item) = local_item {
                similar.push(local_item);
            }
        }
    }

    // Priority 2: Local algorithm fills remaining slots
    if similar.len() < limit {
        let remaining = i64::try_from(limit - similar.len()).unwrap_or(i64::MAX);
        let local_similar =
            rustyfin_db::repo_media::get_similar_items(&state.db_ro, &item, remaining).await?;

        // Filter out items already added from TMDb
        let existing_ids: std::collections::HashSet<String> =
            similar.iter().map(|i| i.id.to_string()).collect();
        for local_item in local_similar {
            if similar.len() >= limit {
                break;
            }
            if !existing_ids.contains(&local_item.id.to_string()) {
                similar.push(local_item);
            }
        }
    }

    drop(config);
    let total = similar.len() as i64;

    // Enrich similar items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
    drop(config);

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

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LatestQuery {
    limit: Option<i64>,
    include_item_types: Option<String>,
    #[serde(rename = "includeItemTypes")]
    include_item_types_lower: Option<String>,
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
}

async fn get_latest(
    State(state): State<AppState>,
    Query(query): Query<LatestQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    let media_type = query
        .include_item_types
        .as_deref()
        .or(query.include_item_types_lower.as_deref())
        .map(|s| format!("\"{}\"", s.split(',').next().unwrap_or("").trim()));
    let library_id = query.parent_id.or(query.parent_id_lower);
    let mut items = rustyfin_db::repo_media::get_latest_items(
        &state.db_ro,
        limit,
        media_type.as_deref(),
        library_id,
    )
    .await?;
    // Enrich latest items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
    drop(config);
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
    let limit = query.limit.unwrap_or(10);
    let mut items =
        rustyfin_db::repo_media::get_user_resume_items(&state.db_ro, user, limit).await?;
    let total = items.len() as i64;
    // Enrich resume items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
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
    drop(config);
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ResumeQuery {
    limit: Option<i64>,
    #[serde(alias = "userId")]
    user_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct FiltersQuery {
    parent_id: Option<Uuid>,
    #[serde(rename = "parentId")]
    parent_id_lower: Option<Uuid>,
}

async fn get_filters(
    State(state): State<AppState>,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<FiltersResult>, ApiError> {
    let library_id = query.parent_id.or(query.parent_id_lower);
    let cache_key = library_id.map_or("all".to_string(), |id| id.to_string());
    // Check cache first
    if let Some(cached) = state.cache.filter_lists.get(&cache_key)
        && let Ok(filters) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(filters));
    }
    let (genres, years, official_ratings) =
        rustyfin_db::repo_media::get_filters(&state.db_ro, library_id).await?;
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

// User-specific item endpoints (same as global for now; user-specific status comes in Phase 2)
async fn get_user_items(
    State(state): State<AppState>,
    Path(_user_id): Path<Uuid>,
    Query(query): Query<ItemsQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    // Delegate to get_items with the user's claims
    get_items(State(state), Query(query), Extension(claims)).await
}

async fn get_user_latest(
    State(state): State<AppState>,
    Path(_user_id): Path<Uuid>,
    Query(query): Query<LatestQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<Vec<MediaItem>>, ApiError> {
    get_latest(State(state), Query(query), Extension(claims)).await
}

async fn get_user_resume(
    State(state): State<AppState>,
    Path(user_id): Path<Uuid>,
    Query(query): Query<ResumeQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    let limit = query.limit.unwrap_or(10);
    let mut items =
        rustyfin_db::repo_media::get_user_resume_items(&state.db_ro, user_id, limit).await?;
    let total = items.len() as i64;
    // Enrich user resume items using batch queries
    // Skip blurhash for list views to avoid per-item disk I/O
    let config = state.config.read().await;
    let tmdb = state.tmdb.api(config.tmdb_api_key.as_deref());
    enrich_items(
        &state.db_ro,
        &state.server_id,
        Some(user_id),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true, // skip_blurhash for list views
    )
    .await;
    drop(config);
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

/// `GET /Items/{id}/Trickplay/Preview/{width}/{seekTicks}` — Returns a trickplay preview thumbnail.
async fn get_trickplay_preview(
    State(state): State<AppState>,
    Path((id, width, seek_ticks)): Path<(Uuid, u32, i64)>,
) -> Result<Response, ApiError> {
    let result =
        rustyfin_core::trickplay::get_trickplay_thumbnail(&state.db_ro, id, width, seek_ticks)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

    match result {
        Some((file_path, content_type)) => {
            let bytes = tokio::fs::read(&file_path)
                .await
                .map_err(|e| ApiError::NotFound(format!("Thumbnail not found: {}", e)))?;

            Ok((
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type)],
                bytes,
            )
                .into_response())
        }
        None => Err(ApiError::NotFound("Trickplay not available".to_string())),
    }
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
    for id in &ids {
        rustyfin_db::repo_media::delete_item(&state.db, *id)
            .await
            .ok();
    }
    Ok((StatusCode::OK, Json(serde_json::json!({"Success": true}))))
}

/// GET /Items/{id}/Ancestors - Get parent chain
async fn get_ancestors(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let mut ancestors = Vec::new();
    let mut current_id = id;
    loop {
        let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, current_id).await?;
        let Some(item) = item else { break };
        match item.parent_id {
            Some(parent_id) => {
                ancestors.push(item);
                current_id = parent_id;
            }
            None => break,
        }
    }
    ancestors.reverse();
    let total = ancestors.len() as i64;
    Ok(Json(QueryResult {
        items: ancestors,
        total_record_count: total,
        start_index: 0,
    }))
}

/// GET /Items/{id}/Collections - Get collections containing this item
async fn get_item_collections(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<QueryResult>, ApiError> {
    let collections = rustyfin_db::repo_media::get_item_collections(&state.db_ro, id).await?;
    let total = collections.len() as i64;
    Ok(Json(QueryResult {
        items: collections,
        total_record_count: total,
        start_index: 0,
    }))
}

/// DELETE /Items/{id}/Images/{imageType}
async fn delete_item_image(
    State(state): State<AppState>,
    Path((id, _image_type)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    rustyfin_db::repo_media::delete_item_image(&state.db, id, &_image_type)
        .await
        .ok();
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}

/// DELETE /Items/{id}/Images/{imageType}/{imageIndex}
async fn delete_item_image_index(
    State(state): State<AppState>,
    Path((id, _image_type, _index)): Path<(Uuid, String, i32)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    rustyfin_db::repo_media::delete_item_image_index(&state.db, id, &_image_type, _index)
        .await
        .ok();
    Ok((StatusCode::OK, Json(serde_json::json!({}))))
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

/// DELETE /Users/{userId}/Items/{itemId}/Rating
async fn delete_rating(
    State(state): State<AppState>,
    Path((user_id, item_id)): Path<(Uuid, Uuid)>,
    Extension(claims): Extension<Claims>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let user_id = path_user(&state, &claims, user_id).await?;
    rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await?
        .ok_or(ApiError::NotFound("item not found".into()))?;
    rustyfin_db::repo_playback::clear_user_rating(&state.db, user_id, item_id)
        .await
        .ok();
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
