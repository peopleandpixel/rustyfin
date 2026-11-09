//! Rustyfin-side reads and writes for the Jellyfin import (spec 2.5, 2.10, 2.31): Rustyfin's users,
//! the items a Jellyfin item can match, and the watch data and track choices of the import.
//! Libraries come from `repo_library`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rustyfin_model::media::MediaType;
use rustyfin_model::media_source::StreamIdentity;
use sqlx::{SqliteConnection, SqlitePool};
use uuid::Uuid;

/// A Rustyfin item an imported Jellyfin item can match (spec 2.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    pub id: Uuid,
    pub media_type: MediaType,
    pub path: Option<String>,
    pub parent_id: Option<Uuid>,
    pub index_number: Option<i64>,
    pub parent_index_number: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
}

/// One user's watch data for one item (spec 2.31): a `user_item_data` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserDataRow {
    pub played: bool,
    pub play_count: i64,
    pub position_ticks: i64,
    pub last_played: Option<DateTime<Utc>>,
    pub is_favorite: bool,
    pub user_rating: Option<i64>,
    pub likes: Option<bool>,
    /// The remembered audio stream index, in Rustyfin's numbering.
    pub audio: Option<i64>,
    /// The key of the stream the audio index named (`StreamIdentity::key`); None when none.
    pub audio_key: Option<String>,
    /// The remembered subtitle stream index, in Rustyfin's numbering; -1 means subtitles off.
    pub subtitle: Option<i64>,
    /// The key of the stream the subtitle index named (`StreamIdentity::key`); None when none, and
    /// for subtitles off.
    pub subtitle_key: Option<String>,
}

/// A media stream of a Rustyfin media source (spec 2.31): a `media_streams` row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRow {
    pub index: i64,
    pub codec_type: String,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_external: bool,
    pub path: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
}

impl StreamRow {
    /// This stream's identity (see `StreamIdentity`).
    pub fn identity(&self) -> StreamIdentity<'_> {
        StreamIdentity {
            codec_type: &self.codec_type,
            codec: self.codec.as_deref(),
            language: self.language.as_deref(),
            title: self.title.as_deref(),
            is_external: self.is_external,
            path: self.path.as_deref(),
            is_default: self.is_default,
            is_forced: self.is_forced,
        }
    }
}

/// A media source of an item, with its streams in `stream_index` order (spec 2.31).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceStreams {
    pub path: String,
    pub streams: Vec<StreamRow>,
}

/// `user_item_data`: item_id, played, play_count, playback_position_ticks, last_played_date,
/// is_favorite, user_rating, likes, audio_stream_index, subtitle_stream_index, audio_stream_key,
/// subtitle_stream_key.
type DataDbRow = (
    String,
    bool,
    i64,
    i64,
    Option<String>,
    bool,
    Option<i64>,
    Option<bool>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// `media_streams`: stream_index, codec_type, codec, language, title, is_external, path, is_default,
/// is_forced.
type StreamDbRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    bool,
    bool,
);

/// The `media_items` columns `import_targets` reads, in select order.
type TargetRow = (
    String,         // id
    String,         // media_type
    Option<String>, // path
    Option<String>, // parent_id
    Option<i64>,    // index_number
    Option<i64>,    // parent_index_number
    Option<i64>,    // tmdb_id
    Option<i64>,    // tvdb_id
);

/// The Rustyfin items a Jellyfin item can match: movies, series, seasons, episodes and music.
pub async fn import_targets(pool: &SqlitePool) -> Result<Vec<ImportTarget>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TargetRow>(
        "SELECT id, media_type, path, parent_id, index_number, parent_index_number, tmdb_id, tvdb_id \
         FROM media_items \
         WHERE media_type IN ('Movie', 'Series', 'Season', 'Episode', 'Audio', 'MusicAlbum')",
    )
    .fetch_all(pool)
    .await?;
    let mut targets = Vec::with_capacity(rows.len());
    for (id, media_type, path, parent_id, index_number, parent_index_number, tmdb_id, tvdb_id) in
        rows
    {
        let Ok(media_type) = media_type.parse::<MediaType>() else {
            continue;
        };
        targets.push(ImportTarget {
            id: parse_id(&id)?,
            media_type,
            path,
            parent_id: parent_id.as_deref().map(parse_id).transpose()?,
            index_number,
            parent_index_number,
            tmdb_id,
            tvdb_id,
        });
    }
    Ok(targets)
}

/// The user's watch data by item (spec 2.31), read on `db`: a pool, or a transaction's connection. A
/// stored date that doesn't parse reads as none; a row whose item id doesn't parse is skipped.
pub async fn user_data_rows<'e>(
    db: impl sqlx::SqliteExecutor<'e>,
    user: Uuid,
) -> Result<HashMap<Uuid, UserDataRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DataDbRow>(
        "SELECT item_id, played, play_count, playback_position_ticks, last_played_date, \
         is_favorite, user_rating, likes, audio_stream_index, subtitle_stream_index, \
         audio_stream_key, subtitle_stream_key \
         FROM user_item_data WHERE user_id = ?",
    )
    .bind(user.to_string())
    .fetch_all(db)
    .await?;
    let mut data = HashMap::with_capacity(rows.len());
    for (
        item,
        played,
        play_count,
        position_ticks,
        last_played,
        is_favorite,
        user_rating,
        likes,
        audio,
        subtitle,
        audio_key,
        subtitle_key,
    ) in rows
    {
        let Ok(item) = Uuid::parse_str(&item) else {
            continue;
        };
        data.insert(
            item,
            UserDataRow {
                played,
                play_count,
                position_ticks,
                last_played: last_played.as_deref().and_then(parse_date),
                is_favorite,
                user_rating,
                likes,
                audio,
                audio_key,
                subtitle,
                subtitle_key,
            },
        );
    }
    Ok(data)
}

/// Writes the user's rows on `conn` (spec 2.31): each row is inserted, or every value column of its
/// existing row is replaced. No transaction of its own: the caller's makes the writes atomic.
pub async fn write_user_data(
    conn: &mut SqliteConnection,
    user: Uuid,
    rows: &[(Uuid, UserDataRow)],
) -> Result<(), sqlx::Error> {
    let user_id = user.to_string();
    for (item, row) in rows {
        sqlx::query(
            "INSERT INTO user_item_data (user_id, item_id, played, play_count, \
             playback_position_ticks, last_played_date, is_favorite, user_rating, likes, \
             audio_stream_index, subtitle_stream_index, audio_stream_key, subtitle_stream_key) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(user_id, item_id) DO UPDATE SET \
             played = excluded.played, \
             play_count = excluded.play_count, \
             playback_position_ticks = excluded.playback_position_ticks, \
             last_played_date = excluded.last_played_date, \
             is_favorite = excluded.is_favorite, \
             user_rating = excluded.user_rating, \
             likes = excluded.likes, \
             audio_stream_index = excluded.audio_stream_index, \
             subtitle_stream_index = excluded.subtitle_stream_index, \
             audio_stream_key = excluded.audio_stream_key, \
             subtitle_stream_key = excluded.subtitle_stream_key",
        )
        .bind(&user_id)
        .bind(item.to_string())
        .bind(row.played)
        .bind(row.play_count)
        .bind(row.position_ticks)
        .bind(row.last_played.map(|date| date.to_rfc3339()))
        .bind(row.is_favorite)
        .bind(row.user_rating)
        .bind(row.likes)
        .bind(row.audio)
        .bind(row.subtitle)
        .bind(row.audio_key.as_deref())
        .bind(row.subtitle_key.as_deref())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The item's media sources by `created_at` then `id`, each with its streams by `stream_index`
/// (spec 2.31).
pub async fn item_sources(
    pool: &SqlitePool,
    item: Uuid,
) -> Result<Vec<SourceStreams>, sqlx::Error> {
    let sources = sqlx::query_as::<_, (String, String)>(
        "SELECT id, path FROM media_sources WHERE item_id = ? ORDER BY created_at, id",
    )
    .bind(item.to_string())
    .fetch_all(pool)
    .await?;
    let mut result = Vec::with_capacity(sources.len());
    for (source_id, source_path) in sources {
        let rows = sqlx::query_as::<_, StreamDbRow>(
            "SELECT stream_index, codec_type, codec, language, title, is_external, path, \
             is_default, is_forced FROM media_streams WHERE source_id = ? ORDER BY stream_index",
        )
        .bind(&source_id)
        .fetch_all(pool)
        .await?;
        let streams = rows
            .into_iter()
            .map(
                |(
                    index,
                    codec_type,
                    codec,
                    language,
                    title,
                    is_external,
                    path,
                    is_default,
                    is_forced,
                )| {
                    StreamRow {
                        index,
                        codec_type,
                        codec,
                        language,
                        title,
                        is_external,
                        path,
                        is_default,
                        is_forced,
                    }
                },
            )
            .collect();
        result.push(SourceStreams {
            path: source_path,
            streams,
        });
    }
    Ok(result)
}

/// A Rustyfin item's metadata (spec 5.4, 5.5): its `media_items` columns and its extras columns
/// (NULL, and unlocked, when the item has no `item_metadata_extras` row).
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ItemMetadata {
    pub id: String,
    pub name: String,
    pub sort_name: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f64>,
    pub premiere_date: Option<String>,
    pub end_date: Option<String>,
    pub production_year: Option<i64>,
    pub genres: String,
    pub tags: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub original_title: Option<String>,
    pub critic_rating: Option<f64>,
    pub imdb_id: Option<String>,
    pub lock_data: bool,
    pub locked_fields: String,
}

/// An `item_people` row with its person's name (spec 5.7), in the order the item DTOs show them.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StoredLink {
    pub item_id: String,
    pub person_id: String,
    pub name: String,
    pub kind: String,
    pub role: Option<String>,
    pub sort_order: Option<i64>,
}

/// An `item_images` or `person_images` row (spec 5.8); `owner_id` is the item or person id.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StoredImage {
    pub owner_id: String,
    pub image_type: String,
    pub image_index: i64,
    pub path: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub blurhash: Option<String>,
}

/// Rustyfin's metadata rows, read whole; the import filters them by the matched items (spec 5.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetadataState {
    pub items: Vec<ItemMetadata>,
    /// `item_studios` as (item id, studio id).
    pub studios: Vec<(String, String)>,
    /// `people` as (id, name), lowest id first.
    pub people: Vec<(String, String)>,
    pub links: Vec<StoredLink>,
    pub item_images: Vec<StoredImage>,
    pub person_images: Vec<StoredImage>,
}

/// An item's extras (spec 5.5, 5.6): its `item_metadata_extras` row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemExtras {
    pub original_title: Option<String>,
    pub critic_rating: Option<f64>,
    pub imdb_id: Option<String>,
    pub lock_data: bool,
    pub locked_fields: String,
}

/// A media item's metadata columns after the import (spec 5.4): every value the UPDATE binds.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemWrite {
    pub id: Uuid,
    pub name: String,
    pub sort_name: Option<String>,
    pub overview: Option<String>,
    pub tagline: Option<String>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f64>,
    pub premiere_date: Option<String>,
    pub end_date: Option<String>,
    pub production_year: Option<i64>,
    pub genres: String,
    pub tags: String,
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
}

/// An item's extras columns the import sets (spec 5.4); the lock columns are never written.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtrasWrite {
    pub item_id: Uuid,
    pub original_title: Option<String>,
    pub critic_rating: Option<f64>,
    pub imdb_id: Option<String>,
}

/// An item's studios (spec 5.4): (studio id, name) pairs that replace its `item_studios` rows.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioWrite {
    pub item_id: Uuid,
    pub studios: Vec<(String, String)>,
}

/// A person the import creates (spec 5.7).
#[derive(Debug, Clone, PartialEq)]
pub struct NewPerson {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
}

/// One person of an item's links (spec 5.7).
#[derive(Debug, Clone, PartialEq)]
pub struct PersonLink {
    pub person_id: String,
    pub kind: String,
    pub role: Option<String>,
    pub sort_order: i64,
}

/// An item's links that replace its `item_people` rows (spec 5.7).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkWrite {
    pub item_id: Uuid,
    pub links: Vec<PersonLink>,
}

/// One image row to write (spec 5.9); the provider is `jellyfin`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRow {
    pub index: i64,
    pub path: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub blurhash: Option<String>,
}

/// The rows of one owner and image type, which replace that type's rows (spec 5.8, 5.9).
/// `is_person` picks `person_images` over `item_images`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageWrite {
    pub owner_id: Uuid,
    pub is_person: bool,
    pub image_type: String,
    pub rows: Vec<ImageRow>,
}

/// Everything one metadata run writes (spec 5.10), applied by `write_metadata` in one transaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetadataWrites {
    pub items: Vec<ItemWrite>,
    pub extras: Vec<ExtrasWrite>,
    pub studios: Vec<StudioWrite>,
    pub new_people: Vec<NewPerson>,
    pub links: Vec<LinkWrite>,
    pub images: Vec<ImageWrite>,
}

impl MetadataWrites {
    /// Whether the run has nothing to write.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self.extras.is_empty()
            && self.studios.is_empty()
            && self.new_people.is_empty()
            && self.links.is_empty()
            && self.images.is_empty()
    }
}

/// Rustyfin's metadata rows, read whole tables (spec 5.1 to 5.8). Links come in the order the item
/// DTOs show them; people lowest id first.
pub async fn metadata_state(pool: &SqlitePool) -> Result<MetadataState, sqlx::Error> {
    let items = sqlx::query_as::<_, ItemMetadata>(
        "SELECT m.id, m.name, m.sort_name, m.overview, m.tagline, m.official_rating, \
         m.community_rating, m.premiere_date, m.end_date, m.production_year, m.genres, m.tags, \
         m.tmdb_id, m.tvdb_id, e.original_title, e.critic_rating, e.imdb_id, \
         COALESCE(e.lock_data, 0) AS lock_data, COALESCE(e.locked_fields, '') AS locked_fields \
         FROM media_items m LEFT JOIN item_metadata_extras e ON e.item_id = m.id",
    )
    .fetch_all(pool)
    .await?;
    let studios =
        sqlx::query_as::<_, (String, String)>("SELECT item_id, studio_id FROM item_studios")
            .fetch_all(pool)
            .await?;
    let people = sqlx::query_as::<_, (String, String)>("SELECT id, name FROM people ORDER BY id")
        .fetch_all(pool)
        .await?;
    let links = sqlx::query_as::<_, StoredLink>(
        "SELECT ip.item_id AS item_id, ip.person_id AS person_id, p.name AS name, \
         ip.type AS kind, ip.role AS role, ip.sort_order AS sort_order \
         FROM item_people ip JOIN people p ON p.id = ip.person_id \
         ORDER BY ip.item_id, ip.sort_order IS NULL, ip.sort_order, ip.rowid",
    )
    .fetch_all(pool)
    .await?;
    let item_images = sqlx::query_as::<_, StoredImage>(
        "SELECT item_id AS owner_id, image_type, image_index, path, width, height, blurhash \
         FROM item_images",
    )
    .fetch_all(pool)
    .await?;
    let person_images = sqlx::query_as::<_, StoredImage>(
        "SELECT person_id AS owner_id, image_type, image_index, path, width, height, blurhash \
         FROM person_images",
    )
    .fetch_all(pool)
    .await?;
    Ok(MetadataState {
        items,
        studios,
        people,
        links,
        item_images,
        person_images,
    })
}

/// An item's extras (spec 5.5, 5.6), or None when it has no `item_metadata_extras` row.
pub async fn item_extras(pool: &SqlitePool, item: Uuid) -> Result<Option<ItemExtras>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Option<String>, Option<f64>, Option<String>, bool, String)>(
        "SELECT original_title, critic_rating, imdb_id, lock_data, locked_fields \
         FROM item_metadata_extras WHERE item_id = ?",
    )
    .bind(item.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(original_title, critic_rating, imdb_id, lock_data, locked_fields)| ItemExtras {
            original_title,
            critic_rating,
            imdb_id,
            lock_data,
            locked_fields,
        },
    ))
}

const ITEM_IMAGE_DELETE: &str = "DELETE FROM item_images WHERE item_id = ? AND image_type = ?";
const ITEM_IMAGE_INSERT: &str = "INSERT INTO item_images \
     (item_id, image_type, image_index, path, provider, width, height, blurhash) \
     VALUES (?, ?, ?, ?, 'jellyfin', ?, ?, ?)";
const PERSON_IMAGE_DELETE: &str =
    "DELETE FROM person_images WHERE person_id = ? AND image_type = ?";
const PERSON_IMAGE_INSERT: &str = "INSERT INTO person_images \
     (person_id, image_type, image_index, path, provider, width, height, blurhash) \
     VALUES (?, ?, ?, ?, 'jellyfin', ?, ?, ?)";

/// Applies a metadata run in one transaction (spec 5.10), in this order: items, extras, studios, new
/// people, links, images. Item columns are only ever set by the UPDATE; the lock columns never are.
pub async fn write_metadata(pool: &SqlitePool, writes: &MetadataWrites) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    for item in &writes.items {
        sqlx::query(
            "UPDATE media_items SET name = ?, sort_name = ?, overview = ?, tagline = ?, \
             official_rating = ?, community_rating = ?, premiere_date = ?, end_date = ?, \
             production_year = ?, genres = ?, tags = ?, tmdb_id = ?, tvdb_id = ? WHERE id = ?",
        )
        .bind(&item.name)
        .bind(&item.sort_name)
        .bind(&item.overview)
        .bind(&item.tagline)
        .bind(&item.official_rating)
        .bind(item.community_rating)
        .bind(&item.premiere_date)
        .bind(&item.end_date)
        .bind(item.production_year)
        .bind(&item.genres)
        .bind(&item.tags)
        .bind(item.tmdb_id)
        .bind(item.tvdb_id)
        .bind(item.id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    for extras in &writes.extras {
        sqlx::query(
            "INSERT INTO item_metadata_extras (item_id, original_title, critic_rating, imdb_id) \
             VALUES (?, ?, ?, ?) ON CONFLICT(item_id) DO UPDATE SET \
             original_title = excluded.original_title, critic_rating = excluded.critic_rating, \
             imdb_id = excluded.imdb_id",
        )
        .bind(extras.item_id.to_string())
        .bind(&extras.original_title)
        .bind(extras.critic_rating)
        .bind(&extras.imdb_id)
        .execute(&mut *tx)
        .await?;
    }
    for studio in &writes.studios {
        let item = studio.item_id.to_string();
        sqlx::query("DELETE FROM item_studios WHERE item_id = ?")
            .bind(&item)
            .execute(&mut *tx)
            .await?;
        for (id, name) in &studio.studios {
            sqlx::query("INSERT OR IGNORE INTO studios (id, name) VALUES (?, ?)")
                .bind(id)
                .bind(name)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO item_studios (item_id, studio_id) VALUES (?, ?)")
                .bind(&item)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    for person in &writes.new_people {
        sqlx::query("INSERT INTO people (id, name, type) VALUES (?, ?, ?)")
            .bind(person.id.to_string())
            .bind(&person.name)
            .bind(&person.kind)
            .execute(&mut *tx)
            .await?;
    }
    for link in &writes.links {
        let item = link.item_id.to_string();
        sqlx::query("DELETE FROM item_people WHERE item_id = ?")
            .bind(&item)
            .execute(&mut *tx)
            .await?;
        for person in &link.links {
            sqlx::query(
                "INSERT INTO item_people (item_id, person_id, type, role, sort_order) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&item)
            .bind(&person.person_id)
            .bind(&person.kind)
            .bind(&person.role)
            .bind(person.sort_order)
            .execute(&mut *tx)
            .await?;
        }
    }
    for images in &writes.images {
        let owner = images.owner_id.to_string();
        let (delete, insert) = if images.is_person {
            (PERSON_IMAGE_DELETE, PERSON_IMAGE_INSERT)
        } else {
            (ITEM_IMAGE_DELETE, ITEM_IMAGE_INSERT)
        };
        sqlx::query(delete)
            .bind(&owner)
            .bind(&images.image_type)
            .execute(&mut *tx)
            .await?;
        for row in &images.rows {
            sqlx::query(insert)
                .bind(&owner)
                .bind(&images.image_type)
                .bind(row.index)
                .bind(&row.path)
                .bind(row.width)
                .bind(row.height)
                .bind(&row.blurhash)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

/// A stored id (hyphenated text) as a `Uuid`.
fn parse_id(text: &str) -> Result<Uuid, sqlx::Error> {
    Uuid::parse_str(text).map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

/// A stored date (RFC 3339, as Rustyfin writes it) in UTC; text that doesn't parse is None.
fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}
