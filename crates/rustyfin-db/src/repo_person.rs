use rustyfin_model::media::MediaItem;
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

pub async fn get_artists(pool: &SqlitePool) -> Result<Vec<NameIdPair>, sqlx::Error> {
    // Return distinct artists from Audio items (using parent name as album/artist proxy)
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT p.id, p.name FROM media_items p
         WHERE p.media_type IN ('\"MusicAlbum\"', '\"MusicArtist\"')
         ORDER BY p.name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| NameIdPair { id: r.0, name: r.1 })
        .collect())
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
