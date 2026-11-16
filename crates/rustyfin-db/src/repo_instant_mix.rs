//! What Jellyfin's instant mix (`Emby.Server.Implementations/Library/MusicManager.cs`) reads: the
//! genres of the tracks under an item, by an artist, or in a library, and the mix's tracks.

use crate::repo_by_name::{fold_name, split_genres};
use sqlx::SqlitePool;
use std::collections::HashSet;
use uuid::Uuid;

/// The most genres a mix looks for; a folder's tracks may have many.
pub const MAX_MIX_GENRES: usize = 64;

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
fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Up to `limit` random tracks (not extras) that have one of `genres` (any track when it names
/// none) and that the parental filter `visible_json` (`repo_parental::visible_sql`) lets through.
/// SQL keeps the tracks whose genres contain a genre's name (`LIKE`, a few times `limit` of them);
/// a genre then matches by its folded name, as `/Items?GenreIds=` matches.
pub async fn mix_tracks(
    pool: &SqlitePool,
    genres: &[String],
    visible_json: Option<&str>,
    limit: usize,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut wanted: Vec<String> = Vec::new();
    let mut folded: HashSet<String> = HashSet::new();
    for genre in genres {
        let fold = fold_name(genre);
        if !fold.is_empty() && folded.insert(fold) {
            wanted.push(genre.trim().to_string());
            if wanted.len() == MAX_MIX_GENRES {
                break;
            }
        }
    }
    let like = if wanted.is_empty() {
        "1".to_string()
    } else {
        let terms: Vec<String> = (0..wanted.len())
            .map(|i| format!("genres LIKE ?{} ESCAPE '\\'", i + 3))
            .collect();
        format!("({})", terms.join(" OR "))
    };
    let sql = format!(
        "SELECT id, genres FROM media_items \
         WHERE media_type = 'Audio' AND extra_type IS NULL AND {visible} AND {like} \
         ORDER BY RANDOM() LIMIT ?2",
        visible = crate::repo_parental::visible_sql("id", "library_id", "?1"),
    );
    let mut query = sqlx::query_as::<_, (String, Option<String>)>(sqlx::AssertSqlSafe(sql))
        .bind(visible_json)
        .bind(i64::try_from(limit.saturating_mul(4)).unwrap_or(i64::MAX));
    for genre in &wanted {
        query = query.bind(format!("%{}%", escape_like(genre)));
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter(|(_, raw)| {
            folded.is_empty()
                || raw.as_deref().is_some_and(|raw| {
                    split_genres(raw)
                        .iter()
                        .any(|g| folded.contains(&fold_name(g)))
                })
        })
        .filter_map(|(id, _)| Uuid::parse_str(&id).ok())
        .take(limit)
        .collect())
}
