//! What Jellyfin's instant mix (`Emby.Server.Implementations/Library/MusicManager.cs`) reads: the
//! genres of the tracks under an item, by an artist, or in a library, and every track.

use crate::repo_by_name::split_genres;
use sqlx::SqlitePool;
use uuid::Uuid;

async fn genres_of(pool: &SqlitePool, sql: &str, bind: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<Option<String>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string()))
        .bind(bind)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .flatten()
        .flat_map(|raw| split_genres(raw))
        .collect())
}

/// The genres of the tracks under the item `root`, at any depth.
pub async fn track_genres_under(pool: &SqlitePool, root: Uuid) -> Result<Vec<String>, sqlx::Error> {
    genres_of(
        pool,
        "WITH RECURSIVE sub(id) AS (SELECT ?1 UNION SELECT m.id FROM media_items AS m \
         JOIN sub ON m.parent_id = sub.id) \
         SELECT genres FROM media_items WHERE media_type = 'Audio' AND id IN (SELECT id FROM sub)",
        &root.to_string(),
    )
    .await
}

/// The genres of the tracks of the library `library`.
pub async fn track_genres_in_library(
    pool: &SqlitePool,
    library: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    genres_of(
        pool,
        "SELECT genres FROM media_items WHERE media_type = 'Audio' AND library_id = ?1",
        &library.to_string(),
    )
    .await
}

/// The genres of the tracks and albums the artist person `person` is credited on.
pub async fn artist_genres(pool: &SqlitePool, person: Uuid) -> Result<Vec<String>, sqlx::Error> {
    genres_of(
        pool,
        "SELECT m.genres FROM media_items AS m JOIN item_people AS ip ON ip.item_id = m.id \
         WHERE ip.person_id = ?1 AND ip.type IN ('Artist', 'AlbumArtist') \
         AND m.media_type IN ('Audio', 'MusicAlbum')",
        &person.to_string(),
    )
    .await
}

/// Every track that is not an extra, as (id, library id), in random order.
pub async fn all_tracks(pool: &SqlitePool) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, library_id FROM media_items WHERE media_type = 'Audio' AND extra_type IS NULL \
         ORDER BY RANDOM()",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|(id, lib)| Some((Uuid::parse_str(id).ok()?, Uuid::parse_str(lib).ok()?)))
        .collect())
}
