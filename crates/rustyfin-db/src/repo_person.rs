use rustyfin_model::person::NameIdPair;
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
        .map(|r| NameIdPair {
            id: r.0,
            name: r.1,
        })
        .collect())
}
