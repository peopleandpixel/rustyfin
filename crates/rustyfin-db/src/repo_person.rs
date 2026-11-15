use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::person::{NameIdPair, PersonDto};
use sqlx::SqlitePool;
use uuid::Uuid;

/// The id of the music artist `name`: one person per lowercased name (spec 1.5).
pub fn music_artist_person_id(name: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("rustyfin-artist:{}", name.to_lowercase()).as_bytes(),
    )
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
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT p.id, p.name FROM people p
         WHERE p.id IN (
             SELECT ip.person_id FROM item_people ip
             JOIN media_items mi ON mi.id = ip.item_id
             WHERE ip.type = ? AND mi.media_type = ? AND (? IS NULL OR mi.library_id = ?)
         )
         ORDER BY p.name COLLATE NOCASE",
    )
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
        let query = format!(
            "SELECT ip.item_id, p.id, p.name FROM item_people ip JOIN people p ON p.id = ip.person_id \
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
    let rows = sqlx::query_as::<_, (String, String)>("SELECT id, name FROM people ORDER BY name")
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
