use rustyfin_model::media::MediaItem;
use rustyfin_model::person::{NameIdPair, PersonDto};
use sqlx::SqlitePool;

pub async fn get_genres(pool: &SqlitePool) -> Result<Vec<NameIdPair>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT value FROM media_items, json_each(media_items.genres) ORDER BY value",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| NameIdPair {
            id: r.0.clone(),
            name: r.0,
        })
        .collect())
}

pub async fn get_studios(pool: &SqlitePool) -> Result<Vec<NameIdPair>, sqlx::Error> {
    // Studios are not stored separately; return empty for now
    let _ = pool;
    Ok(vec![])
}

pub async fn get_artists(pool: &SqlitePool) -> Result<Vec<NameIdPair>, sqlx::Error> {
    // Return distinct artists from Audio items (using parent name as album/artist proxy)
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT p.id, p.name FROM media_items p
         WHERE p.media_type = '\"MusicAlbum\"'
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
