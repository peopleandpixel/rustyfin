use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};
use sqlx::SqlitePool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// MediaSource DB row
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct MediaSourceRow {
    id: String,
    #[allow(dead_code)]
    item_id: String,
    uid: String,
    container: Option<String>,
    protocol: String,
    path: String,
    r#type: String,
    run_time_ticks: Option<i64>,
    size: Option<i64>,
    bitrate: Option<u32>,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    aspect_ratio: Option<String>,
    is_remote: bool,
    media_state: Option<String>,
}

impl From<MediaSourceRow> for MediaSourceResponse {
    fn from(r: MediaSourceRow) -> Self {
        Self {
            id: r.id,
            uid: r.uid,
            container: r.container,
            protocol: r.protocol,
            path: r.path,
            r#type: r.r#type,
            run_time_ticks: r.run_time_ticks,
            size: r.size,
            bitrate: r.bitrate,
            video_bitrate: r.video_bitrate,
            audio_bitrate: r.audio_bitrate,
            width: r.width,
            height: r.height,
            aspect_ratio: r.aspect_ratio,
            is_remote: r.is_remote,
            media_state: r.media_state,
            media_streams: Vec::new(),
            supports_transcoding: true,
            supports_direct_stream: false,
            supports_direct_play: false,
        }
    }
}

// ---------------------------------------------------------------------------
// MediaStream DB row
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct MediaStreamRow {
    index: i32,
    codec: Option<String>,
    codec_type: String,
    language: Option<String>,
    time_base: Option<String>,
    codec_time_base: Option<String>,
    codec_tag: Option<String>,
    is_external: bool,
    path: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    average_frame_rate: Option<String>,
    real_frame_rate: Option<String>,
    bit_rate: Option<u32>,
    bit_depth: Option<u32>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    sample_rate: Option<u32>,
    profile: Option<String>,
    r#type: Option<String>,
    aspect_ratio_text: Option<String>,
    aspect_ratio: Option<f64>,
    title: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    localized_uri: Option<String>,
    is_default: bool,
    is_forced: bool,
    is_interlaced: bool,
    embedded_bitmaps: bool,
}

impl From<MediaStreamRow> for MediaStream {
    fn from(r: MediaStreamRow) -> Self {
        Self {
            index: r.index,
            codec: r.codec,
            codec_type: r.codec_type,
            language: r.language,
            time_base: r.time_base,
            codec_time_base: r.codec_time_base,
            codec_tag: r.codec_tag,
            is_external: r.is_external,
            path: r.path,
            width: r.width,
            height: r.height,
            average_frame_rate: r.average_frame_rate,
            real_frame_rate: r.real_frame_rate,
            bit_rate: r.bit_rate,
            bit_depth: r.bit_depth,
            channels: r.channels,
            channel_layout: r.channel_layout,
            sample_rate: r.sample_rate,
            profile: r.profile,
            r#type: r.r#type,
            aspect_ratio_text: r.aspect_ratio_text,
            aspect_ratio: r.aspect_ratio,
            title: r.title,
            color_space: r.color_space,
            color_transfer: r.color_transfer,
            color_primaries: r.color_primaries,
            color_range: r.color_range,
            localized_uri: r.localized_uri,
            is_default: r.is_default,
            is_forced: r.is_forced,
            is_interlaced: r.is_interlaced,
            embedded_bitmaps: r.embedded_bitmaps,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Insert a new media source and return its ID.
pub async fn insert_media_source(
    pool: &SqlitePool,
    item_id: &Uuid,
    source: &MediaSourceResponse,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let uid = source.uid.clone();

    sqlx::query(
        "INSERT INTO media_sources \
         (id, item_id, uid, container, protocol, path, type, run_time_ticks, size, \
          bitrate, video_bitrate, audio_bitrate, width, height, aspect_ratio, is_remote, media_state) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(item_id.to_string())
    .bind(&uid)
    .bind(&source.container)
    .bind(&source.protocol)
    .bind(&source.path)
    .bind(&source.r#type)
    .bind(source.run_time_ticks)
    .bind(source.size)
    .bind(source.bitrate)
    .bind(source.video_bitrate)
    .bind(source.audio_bitrate)
    .bind(source.width)
    .bind(source.height)
    .bind(&source.aspect_ratio)
    .bind(source.is_remote)
    .bind(&source.media_state)
    .execute(pool)
    .await?;

    Ok(Uuid::parse_str(&id).unwrap())
}

/// Insert a media stream for a given source.
pub async fn insert_media_stream(
    pool: &SqlitePool,
    source_id: &Uuid,
    stream: &MediaStream,
) -> Result<(), sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let source_id_str = source_id.to_string();

    sqlx::query(
        "INSERT INTO media_streams \
         (id, source_id, codec, codec_type, language, time_base, codec_time_base, codec_tag, \
          is_external, path, width, height, average_frame_rate, real_frame_rate, bit_rate, \
          bit_depth, channels, channel_layout, sample_rate, profile, type, aspect_ratio_text, \
          aspect_ratio, title, color_space, color_transfer, color_primaries, color_range, \
          localized_uri, is_default, is_forced, is_interlaced, embedded_bitmaps, stream_index) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&source_id_str)
    .bind(&stream.codec)
    .bind(&stream.codec_type)
    .bind(&stream.language)
    .bind(&stream.time_base)
    .bind(&stream.codec_time_base)
    .bind(&stream.codec_tag)
    .bind(stream.is_external)
    .bind(&stream.path)
    .bind(stream.width)
    .bind(stream.height)
    .bind(&stream.average_frame_rate)
    .bind(&stream.real_frame_rate)
    .bind(stream.bit_rate)
    .bind(stream.bit_depth)
    .bind(stream.channels)
    .bind(&stream.channel_layout)
    .bind(stream.sample_rate)
    .bind(&stream.profile)
    .bind(&stream.r#type)
    .bind(&stream.aspect_ratio_text)
    .bind(stream.aspect_ratio)
    .bind(&stream.title)
    .bind(&stream.color_space)
    .bind(&stream.color_transfer)
    .bind(&stream.color_primaries)
    .bind(&stream.color_range)
    .bind(&stream.localized_uri)
    .bind(stream.is_default)
    .bind(stream.is_forced)
    .bind(stream.is_interlaced)
    .bind(stream.embedded_bitmaps)
    .bind(stream.index)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get all media sources for an item, with their streams.
pub async fn get_media_sources(
    pool: &SqlitePool,
    item_id: &Uuid,
) -> Result<Vec<MediaSourceResponse>, sqlx::Error> {
    let sources: Vec<MediaSourceRow> = sqlx::query_as(
        "SELECT id, item_id, uid, container, protocol, path, type, run_time_ticks, size, \
         bitrate, video_bitrate, audio_bitrate, width, height, aspect_ratio, is_remote, media_state \
         FROM media_sources WHERE item_id = ?",
    )
    .bind(item_id.to_string())
    .fetch_all(pool)
    .await?;

    let mut result: Vec<MediaSourceResponse> = sources.into_iter().map(Into::into).collect();

    // Fetch streams for each source
    for source in result.iter_mut() {
        let source_id = Uuid::parse_str(&source.id).ok();
        if let Some(sid) = source_id {
            let streams: Vec<MediaStreamRow> = sqlx::query_as(
                "SELECT stream_index as [index], codec, codec_type, language, time_base, codec_time_base, codec_tag, \
                 is_external, path, width, height, average_frame_rate, real_frame_rate, bit_rate, \
                 bit_depth, channels, channel_layout, sample_rate, profile, type, aspect_ratio_text, \
                 aspect_ratio, title, color_space, color_transfer, color_primaries, color_range, \
                 localized_uri, is_default, is_forced, is_interlaced, embedded_bitmaps \
                 FROM media_streams WHERE source_id = ? ORDER BY stream_index",
            )
            .bind(sid.to_string())
            .fetch_all(pool)
            .await?;

            source.media_streams = streams.into_iter().map(Into::into).collect();
        }
    }

    Ok(result)
}

/// Verify that a media item exists and fetch its media sources with streams in a single round-trip.
/// Returns None if the item does not exist.
/// Eliminates the N+1 pattern in streaming endpoints where get_item_by_id and get_media_sources
/// were called separately.
pub async fn get_media_sources_with_item_check(
    pool: &SqlitePool,
    item_id: &Uuid,
) -> Option<Vec<MediaSourceResponse>> {
    // Single query: JOIN media_sources with media_items to verify existence
    let sources: Vec<MediaSourceRow> = sqlx::query_as(
        "SELECT ms.id, ms.item_id, ms.uid, ms.container, ms.protocol, ms.path, ms.type, \
         ms.run_time_ticks, ms.size, ms.bitrate, ms.video_bitrate, ms.audio_bitrate, \
         ms.width, ms.height, ms.aspect_ratio, ms.is_remote, ms.media_state \
         FROM media_sources ms \
         INNER JOIN media_items mi ON mi.id = ms.item_id \
         WHERE ms.item_id = ?",
    )
    .bind(item_id.to_string())
    .fetch_all(pool)
    .await
    .ok()?;

    // If no sources found, item might exist without sources — check existence
    if sources.is_empty() {
        let exists: Option<(i32,)> =
            sqlx::query_as::<_, (i32,)>("SELECT EXISTS(SELECT 1 FROM media_items WHERE id = ?)")
                .bind(item_id.to_string())
                .fetch_optional(pool)
                .await
                .ok()?;
        if exists.map(|r| r.0).unwrap_or(0) == 0 {
            return None;
        }
        return Some(Vec::new());
    }

    let mut result: Vec<MediaSourceResponse> = sources.into_iter().map(Into::into).collect();

    // Batch-fetch all streams in one query instead of N separate queries
    let source_ids: Vec<String> = result.iter().map(|s| s.id.clone()).collect();
    if !source_ids.is_empty() {
        let placeholders: Vec<String> = (0..source_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let in_clause = placeholders.join(",");
        let query_str = format!(
            "SELECT stream_index as [index], codec, codec_type, language, time_base, codec_time_base, codec_tag, \
             is_external, path, width, height, average_frame_rate, real_frame_rate, bit_rate, \
             bit_depth, channels, channel_layout, sample_rate, profile, type, aspect_ratio_text, \
             aspect_ratio, title, color_space, color_transfer, color_primaries, color_range, \
             localized_uri, is_default, is_forced, is_interlaced, embedded_bitmaps, source_id \
             FROM media_streams WHERE source_id IN ({}) ORDER BY source_id, stream_index",
            in_clause
        );

        let mut builder =
            sqlx::query_as::<_, MediaStreamRowWithSource>(sqlx::AssertSqlSafe(query_str));
        for sid in &source_ids {
            builder = builder.bind(sid);
        }
        if let Ok(streams) = builder.fetch_all(pool).await {
            // Group streams by source_id
            let mut stream_map: std::collections::HashMap<String, Vec<MediaStream>> =
                std::collections::HashMap::new();
            for row in streams {
                let stream: MediaStream = MediaStreamRow {
                    index: row.index,
                    codec: row.codec,
                    codec_type: row.codec_type,
                    language: row.language,
                    time_base: row.time_base,
                    codec_time_base: row.codec_time_base,
                    codec_tag: row.codec_tag,
                    is_external: row.is_external,
                    path: row.path,
                    width: row.width,
                    height: row.height,
                    average_frame_rate: row.average_frame_rate,
                    real_frame_rate: row.real_frame_rate,
                    bit_rate: row.bit_rate,
                    bit_depth: row.bit_depth,
                    channels: row.channels,
                    channel_layout: row.channel_layout,
                    sample_rate: row.sample_rate,
                    profile: row.profile,
                    r#type: row.r#type,
                    aspect_ratio_text: row.aspect_ratio_text,
                    aspect_ratio: row.aspect_ratio,
                    title: row.title,
                    color_space: row.color_space,
                    color_transfer: row.color_transfer,
                    color_primaries: row.color_primaries,
                    color_range: row.color_range,
                    localized_uri: row.localized_uri,
                    is_default: row.is_default,
                    is_forced: row.is_forced,
                    is_interlaced: row.is_interlaced,
                    embedded_bitmaps: row.embedded_bitmaps,
                }
                .into();
                stream_map.entry(row.source_id).or_default().push(stream);
            }
            for source in result.iter_mut() {
                source.media_streams = stream_map.remove(&source.id).unwrap_or_default();
            }
        }
    }

    Some(result)
}

/// MediaStream row extended with source_id for batch fetching
#[derive(sqlx::FromRow)]
struct MediaStreamRowWithSource {
    source_id: String,
    index: i32,
    codec: Option<String>,
    codec_type: String,
    language: Option<String>,
    time_base: Option<String>,
    codec_time_base: Option<String>,
    codec_tag: Option<String>,
    is_external: bool,
    path: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    average_frame_rate: Option<String>,
    real_frame_rate: Option<String>,
    bit_rate: Option<u32>,
    bit_depth: Option<u32>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    sample_rate: Option<u32>,
    profile: Option<String>,
    r#type: Option<String>,
    aspect_ratio_text: Option<String>,
    aspect_ratio: Option<f64>,
    title: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    localized_uri: Option<String>,
    is_default: bool,
    is_forced: bool,
    is_interlaced: bool,
    embedded_bitmaps: bool,
}

/// Delete all media sources and streams for an item.
pub async fn delete_media_sources(pool: &SqlitePool, item_id: &Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM media_sources WHERE item_id = ?")
        .bind(item_id.to_string())
        .execute(pool)
        .await?;

    Ok(())
}

/// Delete a single media source and its streams.
pub async fn delete_media_source(pool: &SqlitePool, source_id: &Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM media_sources WHERE id = ?")
        .bind(source_id.to_string())
        .execute(pool)
        .await?;

    Ok(())
}

/// Insert a media source and all its streams in a single transaction.
/// Returns the source ID.
pub async fn insert_media_source_with_streams(
    pool: &SqlitePool,
    item_id: &Uuid,
    source: &MediaSourceResponse,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let uid = source.uid.clone();
    let item_id_str = item_id.to_string();

    let mut tx = pool.begin().await?;

    // Insert the source
    sqlx::query(
        "INSERT INTO media_sources \
         (id, item_id, uid, container, protocol, path, type, run_time_ticks, size, \
          bitrate, video_bitrate, audio_bitrate, width, height, aspect_ratio, is_remote, media_state) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&item_id_str)
    .bind(&uid)
    .bind(&source.container)
    .bind(&source.protocol)
    .bind(&source.path)
    .bind(&source.r#type)
    .bind(source.run_time_ticks)
    .bind(source.size)
    .bind(source.bitrate)
    .bind(source.video_bitrate)
    .bind(source.audio_bitrate)
    .bind(source.width)
    .bind(source.height)
    .bind(&source.aspect_ratio)
    .bind(source.is_remote)
    .bind(&source.media_state)
    .execute(&mut *tx)
    .await?;

    // Insert all streams
    for stream in &source.media_streams {
        let stream_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO media_streams \
             (id, source_id, codec, codec_type, language, time_base, codec_time_base, codec_tag, \
              is_external, path, width, height, average_frame_rate, real_frame_rate, bit_rate, \
              bit_depth, channels, channel_layout, sample_rate, profile, type, aspect_ratio_text, \
              aspect_ratio, title, color_space, color_transfer, color_primaries, color_range, \
              localized_uri, is_default, is_forced, is_interlaced, embedded_bitmaps, stream_index) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&stream_id)
        .bind(&id)
        .bind(&stream.codec)
        .bind(&stream.codec_type)
        .bind(&stream.language)
        .bind(&stream.time_base)
        .bind(&stream.codec_time_base)
        .bind(&stream.codec_tag)
        .bind(stream.is_external)
        .bind(&stream.path)
        .bind(stream.width)
        .bind(stream.height)
        .bind(&stream.average_frame_rate)
        .bind(&stream.real_frame_rate)
        .bind(stream.bit_rate)
        .bind(stream.bit_depth)
        .bind(stream.channels)
        .bind(&stream.channel_layout)
        .bind(stream.sample_rate)
        .bind(&stream.profile)
        .bind(&stream.r#type)
        .bind(&stream.aspect_ratio_text)
        .bind(stream.aspect_ratio)
        .bind(&stream.title)
        .bind(&stream.color_space)
        .bind(&stream.color_transfer)
        .bind(&stream.color_primaries)
        .bind(&stream.color_range)
        .bind(&stream.localized_uri)
        .bind(stream.is_default)
        .bind(stream.is_forced)
        .bind(stream.is_interlaced)
        .bind(stream.embedded_bitmaps)
        .bind(stream.index)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(Uuid::parse_str(&id).unwrap())
}

/// Batch fetch all media sources and streams for multiple items in a single query.
/// Returns a map from item_id -> Vec<MediaSourceResponse> with streams populated.
pub async fn get_media_sources_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
) -> Result<std::collections::HashMap<String, Vec<MediaSourceResponse>>, sqlx::Error> {
    if item_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut result: std::collections::HashMap<String, Vec<MediaSourceResponse>> =
        std::collections::HashMap::new();

    // Chunk into batches of 200 items to stay well below SQLite host parameter limits
    for chunk in item_ids.chunks(200) {
        let placeholders: Vec<String> = (0..chunk.len()).map(|i| format!("?{}", i + 1)).collect();
        let in_clause = placeholders.join(",");

        let query = format!(
            "SELECT ms.id as source_id, ms.item_id, ms.uid, ms.container, ms.protocol, ms.path, \
             ms.type as source_type, ms.run_time_ticks, ms.size, ms.bitrate, ms.video_bitrate, \
             ms.audio_bitrate, ms.width, ms.height, ms.aspect_ratio, ms.is_remote, ms.media_state, \
             mst.id as stream_id, mst.codec, mst.codec_type, mst.language, mst.time_base, \
             mst.codec_time_base, mst.codec_tag, mst.is_external as stream_is_external, \
             mst.path as stream_path, mst.width as stream_width, mst.height as stream_height, \
             mst.average_frame_rate, mst.real_frame_rate, mst.bit_rate, mst.bit_depth, \
             mst.channels, mst.channel_layout, mst.sample_rate, mst.profile, \
             mst.type as stream_type, mst.aspect_ratio_text, mst.aspect_ratio as stream_aspect_ratio, \
             mst.title, mst.color_space, mst.color_transfer, mst.color_primaries, mst.color_range, \
             mst.localized_uri, mst.is_default, mst.is_forced, mst.is_interlaced, mst.embedded_bitmaps, \
             mst.stream_index as stream_index_num \
             FROM media_sources ms \
             LEFT JOIN media_streams mst ON mst.source_id = ms.id \
             WHERE ms.item_id IN ({}) \
             ORDER BY ms.item_id, ms.id, mst.stream_index",
            in_clause
        );

        let mut builder = sqlx::query_as::<_, BatchSourceStreamRow>(sqlx::AssertSqlSafe(query));
        for id in chunk.iter() {
            builder = builder.bind(id.to_string());
        }
        let rows: Vec<BatchSourceStreamRow> = builder.fetch_all(pool).await?;

        let mut seen_sources: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for row in rows {
            let item_id = row.item_id.clone();
            let source_id = row.source_id.clone();

            let sources = result.entry(item_id).or_default();

            let source_idx = *seen_sources.entry(source_id.clone()).or_insert_with(|| {
                let idx = sources.len();
                let source = MediaSourceResponse {
                    id: source_id.clone(),
                    uid: row.uid.clone(),
                    container: row.container.clone(),
                    protocol: row.protocol.clone(),
                    path: row.path.clone(),
                    r#type: row.source_type.clone(),
                    run_time_ticks: row.run_time_ticks,
                    size: row.size,
                    bitrate: row.bitrate,
                    video_bitrate: row.video_bitrate,
                    audio_bitrate: row.audio_bitrate,
                    width: row.width,
                    height: row.height,
                    aspect_ratio: row.aspect_ratio.clone(),
                    is_remote: row.is_remote,
                    media_state: row.media_state.clone(),
                    media_streams: Vec::new(),
                    supports_transcoding: true,
                    supports_direct_stream: true,
                    supports_direct_play: true,
                };
                sources.push(source);
                idx
            });

            if row.stream_id.is_some()
                && let Some(source) = sources.get_mut(source_idx)
            {
                let stream = MediaStream {
                    index: row.stream_index_num.unwrap_or(0),
                    codec: row.codec.clone(),
                    codec_type: row
                        .codec_type
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string()),
                    language: row.language.clone(),
                    time_base: row.time_base.clone(),
                    codec_time_base: row.codec_time_base.clone(),
                    codec_tag: row.codec_tag.clone(),
                    is_external: row.stream_is_external.unwrap_or(false),
                    path: row.stream_path.clone(),
                    width: row.stream_width,
                    height: row.stream_height,
                    average_frame_rate: row.average_frame_rate.clone(),
                    real_frame_rate: row.real_frame_rate.clone(),
                    bit_rate: row.bit_rate,
                    bit_depth: row.bit_depth,
                    channels: row.channels,
                    channel_layout: row.channel_layout.clone(),
                    sample_rate: row.sample_rate,
                    profile: row.profile.clone(),
                    r#type: row.stream_type.clone(),
                    aspect_ratio_text: row.aspect_ratio_text.clone(),
                    aspect_ratio: row.stream_aspect_ratio,
                    title: row.title.clone(),
                    color_space: row.color_space.clone(),
                    color_transfer: row.color_transfer.clone(),
                    color_primaries: row.color_primaries.clone(),
                    color_range: row.color_range.clone(),
                    localized_uri: row.localized_uri.clone(),
                    is_default: row.is_default.unwrap_or(false),
                    is_forced: row.is_forced.unwrap_or(false),
                    is_interlaced: row.is_interlaced.unwrap_or(false),
                    embedded_bitmaps: row.embedded_bitmaps.unwrap_or(false),
                };
                source.media_streams.push(stream);
            }
        }
    }

    Ok(result)
}

/// Row type for the batch query combining sources and streams
#[derive(sqlx::FromRow)]
struct BatchSourceStreamRow {
    source_id: String,
    item_id: String,
    uid: String,
    container: Option<String>,
    protocol: String,
    path: String,
    source_type: String,
    run_time_ticks: Option<i64>,
    size: Option<i64>,
    bitrate: Option<u32>,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    aspect_ratio: Option<String>,
    is_remote: bool,
    media_state: Option<String>,
    // Stream fields (may be NULL for sources without streams)
    stream_id: Option<String>,
    codec: Option<String>,
    codec_type: Option<String>,
    language: Option<String>,
    time_base: Option<String>,
    codec_time_base: Option<String>,
    codec_tag: Option<String>,
    stream_is_external: Option<bool>,
    stream_path: Option<String>,
    stream_width: Option<u32>,
    stream_height: Option<u32>,
    average_frame_rate: Option<String>,
    real_frame_rate: Option<String>,
    bit_rate: Option<u32>,
    bit_depth: Option<u32>,
    channels: Option<u32>,
    channel_layout: Option<String>,
    sample_rate: Option<u32>,
    profile: Option<String>,
    stream_type: Option<String>,
    aspect_ratio_text: Option<String>,
    stream_aspect_ratio: Option<f64>,
    title: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    localized_uri: Option<String>,
    is_default: Option<bool>,
    is_forced: Option<bool>,
    is_interlaced: Option<bool>,
    embedded_bitmaps: Option<bool>,
    stream_index_num: Option<i32>,
}
