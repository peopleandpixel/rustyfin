use crate::repo_by_name::{ByNameKind, by_name_id};
use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::metadata::MetadataPerson;
use rustyfin_model::person::{NameIdPair, PersonDto};
use sqlx::SqlitePool;
use uuid::Uuid;

/// The id of the music artist `name`: one person per folded name, with the id `by_name_id` gives it.
pub fn music_artist_person_id(name: &str) -> Uuid {
    by_name_id(ByNameKind::MusicArtist, name)
}

/// SQL for the id an artist shows under in the lists: the id of the `MusicArtist` item of the same
/// name (case-insensitive), so that the artist opens as that item, else the person id. Among
/// several such items, one in the library that `library_is` gives comes first, then the lowest
/// rowid. `library_is` is an SQL value (a bound `?`, or `NULL` when no library is known). The
/// expression reads the person as `p`.
pub fn artist_id_sql(library_is: &str) -> String {
    artist_id_sql_with("", library_is)
}

/// `artist_id_sql` for a query over `item_people ip`, where the library preferred is that of the
/// item `ip.item_id`. SQLite can't correlate the item's row from inside the ORDER BY of the
/// subquery, so the item is joined in the subquery's FROM.
pub fn item_artist_id_sql() -> String {
    artist_id_sql_with("JOIN media_items x ON x.id = ip.item_id", "x.library_id")
}

fn artist_id_sql_with(join: &str, library_is: &str) -> String {
    format!(
        "COALESCE((SELECT m.id FROM media_items m {join} WHERE m.media_type = 'MusicArtist' \
         AND m.name = p.name COLLATE NOCASE ORDER BY (m.library_id IS {library_is}) DESC, m.rowid LIMIT 1), p.id)"
    )
}

/// The MusicArtist item for person `id` when the person has an `Artist` or `AlbumArtist` link.
/// This lets an artist id that a list answers with (a person id, when the artist has no folder
/// item) open as an item.
pub async fn music_artist_by_id(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT p.name FROM people p WHERE p.id = ? AND EXISTS (
             SELECT 1 FROM item_people ip WHERE ip.person_id = p.id AND ip.type IN ('Artist', 'AlbumArtist'))",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(name.map(|name| MediaItem {
        id,
        name,
        media_type: MediaType::MusicArtist,
        ..MediaItem::default_dto_fields()
    }))
}

/// Links music items to their artists in one transaction (spec 1.5-1.6). Each `(item id, kind,
/// names)` gets one `item_people` row per name, of `kind` (`Artist` for tracks, `AlbumArtist` for
/// albums), in name order. A name that is already a person is reused; the rows are inserted with
/// `INSERT OR IGNORE`, so a repeated link adds nothing.
pub async fn link_music_artists(
    pool: &SqlitePool,
    links: &[(Uuid, &str, Vec<String>)],
) -> Result<(), sqlx::Error> {
    if links.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (item_id, kind, names) in links {
        for (order, name) in names.iter().enumerate() {
            let person_id = music_artist_person_id(name).to_string();
            sqlx::query("INSERT OR IGNORE INTO people (id, name, type) VALUES (?, ?, 'Artist')")
                .bind(&person_id)
                .bind(name)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "INSERT OR IGNORE INTO item_people (item_id, person_id, type, role, sort_order) \
                 VALUES (?, ?, ?, NULL, ?)",
            )
            .bind(item_id.to_string())
            .bind(&person_id)
            .bind(*kind)
            .bind(order as i64)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await
}

/// The music artists of `library` (all libraries when `None`): the people linked as `link_kind`
/// to items of `item_type` (a stored media type such as `"Audio"`), as MusicArtist items sorted
/// by name. Person ids are one per lowercased name, so each name appears once.
pub async fn get_music_artists(
    pool: &SqlitePool,
    link_kind: &str,
    item_type: &str,
    library: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let library = library.map(|id| id.to_string());
    let query = format!(
        "SELECT {}, p.name FROM people p
         WHERE p.id IN (
             SELECT ip.person_id FROM item_people ip
             JOIN media_items mi ON mi.id = ip.item_id
             WHERE ip.type = ? AND mi.media_type = ? AND (? IS NULL OR mi.library_id = ?)
         )
         ORDER BY p.name COLLATE NOCASE",
        artist_id_sql("?")
    );
    // The library is bound first for the id, as it comes first in the text.
    let rows = sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(query))
        .bind(&library)
        .bind(link_kind)
        .bind(item_type)
        .bind(&library)
        .bind(&library)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id, name)| {
            Some(MediaItem {
                id: Uuid::parse_str(&id).ok()?,
                name,
                media_type: MediaType::MusicArtist,
                ..MediaItem::default_dto_fields()
            })
        })
        .collect())
}

/// The people linked as `kind` to each of `item_ids`, in link order: `(item id, person id, name)`
/// rows, grouped by the item id string. Chunked to keep the SQL parameter count small.
pub async fn get_links_batch(
    pool: &SqlitePool,
    item_ids: &[Uuid],
    kind: &str,
) -> Result<std::collections::HashMap<String, Vec<(String, String)>>, sqlx::Error> {
    let mut map: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();
    for chunk in item_ids.chunks(100) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        // An artist with a folder item is listed under that item's id, in the item's library.
        let id_sql = item_artist_id_sql();
        let query = format!(
            "SELECT ip.item_id, CASE WHEN ip.type IN ('Artist', 'AlbumArtist') THEN {id_sql} ELSE p.id END, p.name \
             FROM item_people ip JOIN people p ON p.id = ip.person_id \
             WHERE ip.type = ? AND ip.item_id IN ({placeholders}) ORDER BY ip.item_id, ip.sort_order IS NULL, ip.sort_order, ip.rowid"
        );
        let mut q =
            sqlx::query_as::<_, (String, String, String)>(sqlx::AssertSqlSafe(query)).bind(kind);
        for id in chunk {
            q = q.bind(id.to_string());
        }
        for (item_id, pid, name) in q.fetch_all(pool).await? {
            map.entry(item_id).or_default().push((pid, name));
        }
    }
    Ok(map)
}

pub async fn get_all_persons(pool: &SqlitePool) -> Result<Vec<NameIdPair>, sqlx::Error> {
    // Music artists are stored as people of type 'Artist', but Jellyfin lists them only as
    // MusicArtist items, so they are left out here.
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, name FROM people WHERE type IS NOT 'Artist' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| NameIdPair { id: r.0, name: r.1 })
        .collect())
}

pub async fn get_person(pool: &SqlitePool, id: &str) -> Result<Option<PersonDto>, sqlx::Error> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64)>(
        "SELECT p.id, p.name, p.type,
         (SELECT COUNT(*) FROM item_people ip JOIN media_items m ON m.id = ip.item_id WHERE ip.person_id = p.id AND m.media_type = '\"Movie\"'),
         (SELECT COUNT(*) FROM item_people ip JOIN media_items m ON m.id = ip.item_id WHERE ip.person_id = p.id AND m.media_type = '\"Series\"')
         FROM people p WHERE p.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| PersonDto {
        name: r.1,
        id: r.0,
        role: None,
        person_type: Some(r.2),
        movie_count: r.3,
        series_count: r.4,
    }))
}

pub async fn get_person_items(
    pool: &SqlitePool,
    person_id: &str,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, super::repo_media::MediaItemRow>(
        "SELECT m.* FROM media_items m
         JOIN item_people ip ON ip.item_id = m.id
         WHERE ip.person_id = ?
         ORDER BY m.premiere_date DESC",
    )
    .bind(person_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Search persons by name, returning MediaItem objects
pub async fn search_persons(
    pool: &SqlitePool,
    name: &str,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let pattern = format!("%{}%", name);
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, name FROM people WHERE name LIKE ? ORDER BY name LIMIT ?",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut items = Vec::new();
    for (id, name) in rows {
        let item = MediaItem {
            id: uuid::Uuid::parse_str(&id).unwrap_or_default(),
            name: name.clone(),
            sort_name: Some(name.to_lowercase()),
            overview: None,
            media_type: rustyfin_model::media::MediaType::Unknown,
            path: String::new(),
            parent_id: None,
            library_id: uuid::Uuid::nil(),
            production_year: None,
            community_rating: None,
            official_rating: None,
            genres: vec![],
            date_created: chrono::Utc::now(),
            premiere_date: None,
            runtime_ticks: None,
            index_number: None,
            parent_index_number: None,
            media_sources: Vec::new(),
            ..MediaItem::default_dto_fields()
        };
        items.push(item);
    }
    Ok(items)
}

/// Credits `people` on an item that has no people yet, in the given order, in one transaction. A
/// person is matched by exact name, as the Jellyfin import does, or created; a new person's
/// photo URL becomes their Primary image. A person listed twice keeps the first credit. Returns
/// false, writing nothing, when the item already has people (from an NFO, an import or an
/// earlier refresh).
pub async fn set_item_people_if_none(
    pool: &SqlitePool,
    item_id: uuid::Uuid,
    people: &[MetadataPerson],
) -> Result<bool, sqlx::Error> {
    if people.is_empty() {
        return Ok(false);
    }
    let item = item_id.to_string();
    // IMMEDIATE: the read below and the writes after it run under the write lock, so concurrent
    // refreshes wait instead of failing with a deadlock on the upgrade from read to write
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let (credited,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM item_people WHERE item_id = ?")
        .bind(&item)
        .fetch_one(&mut *tx)
        .await?;
    if credited > 0 {
        return Ok(false);
    }
    for (order, person) in people.iter().enumerate() {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM people WHERE name = ? ORDER BY rowid LIMIT 1")
                .bind(&person.name)
                .fetch_optional(&mut *tx)
                .await?;
        let person_id = match existing {
            Some((id,)) => id,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO people (id, name, type) VALUES (?, ?, ?)")
                    .bind(&id)
                    .bind(&person.name)
                    .bind(&person.kind)
                    .execute(&mut *tx)
                    .await?;
                id
            }
        };
        if let Some(url) = &person.image_url {
            sqlx::query(
                "INSERT OR IGNORE INTO person_images (person_id, image_type, image_index, path, provider) \
                 VALUES (?, 'Primary', 0, ?, 'tmdb')",
            )
            .bind(&person_id)
            .bind(url)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO item_people (item_id, person_id, type, role, sort_order) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&item)
        .bind(&person_id)
        .bind(&person.kind)
        .bind(&person.role)
        .bind(order as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}
