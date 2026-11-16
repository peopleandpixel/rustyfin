//! The `/Items` conditions of Jellyfin's `InternalItemsQuery` that `repo_media::get_items_filtered`
//! has no clause for: people, studios and artists by name or id, provider ids, picture size,
//! ratings, flags and numbers. `matching_ids` answers all of them with one query of bound
//! parameters; the list then keeps those ids (with parental limits and the count in its own SQL).

use rustyfin_model::parental::{RatingScore, rating_score};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Jellyfin's `HDWidth`, `UHDWidth` and `UHDHeight` (`BaseItemRepository`).
const HD_WIDTH: i64 = 1200;
const UHD_WIDTH: i64 = 3800;
const UHD_HEIGHT: i64 = 2100;

/// The item's picture width and height: the largest of its sources, or of their video streams.
const WIDTH_SQL: &str = "(SELECT MAX(COALESCE(ms.width, st.width)) FROM media_sources ms \
     LEFT JOIN media_streams st ON st.source_id = ms.id AND lower(st.codec_type) = 'video' \
     WHERE ms.item_id = media_items.id)";
const HEIGHT_SQL: &str = "(SELECT MAX(COALESCE(ms.height, st.height)) FROM media_sources ms \
     LEFT JOIN media_streams st ON st.source_id = ms.id AND lower(st.codec_type) = 'video' \
     WHERE ms.item_id = media_items.id)";

/// The conditions; each empty list or `None` filters nothing. Ids are as stored (lowercase,
/// dashed).
#[derive(Debug, Default, Clone)]
pub struct ItemFilter {
    /// Items crediting a person of the same name as one of these people (`PersonIds`).
    pub person_ids: Vec<String>,
    /// Items crediting a person of exactly this name (`Person`).
    pub person: Option<String>,
    /// Items of a studio with one of these names, any case (`Studios`).
    pub studios: Vec<String>,
    /// Artists (as `MusicArtist` item or person ids) and artist names: items credited to one
    /// as `Artist` or `AlbumArtist` (`ArtistIds`, `Artists`).
    pub artist_ids: Vec<String>,
    pub artist_names: Vec<String>,
    /// As `artist_ids`, as `AlbumArtist` only (`AlbumArtistIds`).
    pub album_artist_ids: Vec<String>,
    /// As `artist_ids`, as `Artist` only (`ContributingArtistIds`).
    pub contributing_artist_ids: Vec<String>,
    /// Items credited to none of these artists (`ExcludeArtistIds`).
    pub exclude_artist_ids: Vec<String>,
    /// Tracks of these albums, by id or by name (`AlbumIds`, `Albums`).
    pub album_ids: Vec<String>,
    pub album_names: Vec<String>,
    /// Items with one of these tags, or whose series has one (`Tags`, `IsKids`, …).
    pub tags: Vec<String>,
    /// Items without any of these tags, nor their series (`IsKids=false`, …).
    pub exclude_tags: Vec<String>,
    pub min_community_rating: Option<f64>,
    /// RFC 3339 instants (`MinPremiereDate`, `MaxPremiereDate`).
    pub min_premiere_date: Option<String>,
    pub max_premiere_date: Option<String>,
    /// `IsUnaired`: premiered at or after this instant (true) or before it (false).
    pub unaired: Option<(bool, String)>,
    pub has_imdb_id: Option<bool>,
    pub has_tmdb_id: Option<bool>,
    pub has_tvdb_id: Option<bool>,
    pub is_hd: Option<bool>,
    pub is_4k: Option<bool>,
    /// No item is known to be 3D, so true keeps none.
    pub is_3d: Option<bool>,
    pub min_width: Option<i64>,
    pub min_height: Option<i64>,
    pub max_width: Option<i64>,
    pub max_height: Option<i64>,
    pub has_subtitles: Option<bool>,
    pub has_overview: Option<bool>,
    pub has_official_rating: Option<bool>,
    /// `HasParentalRating=false` without a rating range: only the items without a rating score.
    pub no_parental_rating: bool,
    /// `MinOfficialRating`, `MaxOfficialRating`: items without a rating score pass.
    pub min_rating: Option<RatingScore>,
    pub max_rating: Option<RatingScore>,
    pub is_locked: Option<bool>,
    /// No item is a placeholder, so true keeps none.
    pub is_placeholder: Option<bool>,
    /// `IsMissing`, or `LocationTypes`/`ExcludeLocationTypes` as Jellyfin reads them.
    pub is_virtual: Option<bool>,
    /// Jellyfin's `VideoType` names; every video here is a `VideoFile`.
    pub video_types: Vec<String>,
    /// Sort names before this one, any case (`NameLessThan`).
    pub name_less_than: Option<String>,
    pub index_number: Option<i64>,
    pub parent_index_number: Option<i64>,
    /// Items with a picture of one of these types (`ImageTypes`).
    pub image_types: Vec<String>,
}

impl ItemFilter {
    /// Whether no condition is set.
    pub fn is_empty(&self) -> bool {
        self.person_ids.is_empty()
            && self.person.is_none()
            && self.studios.is_empty()
            && self.artist_ids.is_empty()
            && self.artist_names.is_empty()
            && self.album_artist_ids.is_empty()
            && self.contributing_artist_ids.is_empty()
            && self.exclude_artist_ids.is_empty()
            && self.album_ids.is_empty()
            && self.album_names.is_empty()
            && self.tags.is_empty()
            && self.exclude_tags.is_empty()
            && self.min_community_rating.is_none()
            && self.min_premiere_date.is_none()
            && self.max_premiere_date.is_none()
            && self.unaired.is_none()
            && self.has_imdb_id.is_none()
            && self.has_tmdb_id.is_none()
            && self.has_tvdb_id.is_none()
            && self.is_hd.is_none()
            && self.is_4k.is_none()
            && self.is_3d.is_none()
            && self.min_width.is_none()
            && self.min_height.is_none()
            && self.max_width.is_none()
            && self.max_height.is_none()
            && self.has_subtitles.is_none()
            && self.has_overview.is_none()
            && self.has_official_rating.is_none()
            && !self.no_parental_rating
            && self.min_rating.is_none()
            && self.max_rating.is_none()
            && self.is_locked.is_none()
            && self.is_placeholder.is_none()
            && self.is_virtual.is_none()
            && self.video_types.is_empty()
            && self.name_less_than.is_none()
            && self.index_number.is_none()
            && self.parent_index_number.is_none()
            && self.image_types.is_empty()
    }
}

/// Pushes `(a, b, …)` with each value bound.
fn push_list<T>(qb: &mut QueryBuilder<Sqlite>, values: impl IntoIterator<Item = T>)
where
    T: for<'t> sqlx::Encode<'t, Sqlite> + sqlx::Type<Sqlite>,
{
    qb.push("(");
    let mut list = qb.separated(", ");
    for value in values {
        list.push_bind(value);
    }
    list.push_unseparated(")");
}

/// Pushes ` AND id [NOT] IN (…)`: the items credited, as one of `types`, to an artist of the same
/// name as one of `ids` (a `MusicArtist` item or a person) or one of `names`, and the albums
/// holding such an item.
fn push_artists(
    qb: &mut QueryBuilder<Sqlite>,
    ids: &[String],
    names: &[String],
    types: &[&'static str],
    negate: bool,
) {
    qb.push(if negate {
        " AND id NOT IN ("
    } else {
        " AND id IN ("
    });
    // The credited items, then the albums of the credited tracks.
    for (i, (column, kind)) in [
        ("ip.item_id", "1"),
        ("t.parent_id", "t.media_type = 'Audio'"),
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            qb.push(" UNION ");
        }
        qb.push(format!(
            "SELECT {column} FROM item_people ip JOIN people p ON p.id = ip.person_id \
             JOIN media_items t ON t.id = ip.item_id WHERE {column} IS NOT NULL AND {kind} \
             AND ip.type IN "
        ));
        push_list(qb, types.iter().copied());
        qb.push(" AND lower(p.name) IN (SELECT lower(name) FROM people WHERE id IN ");
        push_list(qb, ids.to_vec());
        qb.push(" UNION SELECT lower(name) FROM media_items WHERE media_type = 'MusicArtist' AND id IN ");
        push_list(qb, ids.to_vec());
        qb.push(" UNION SELECT value FROM json_each(");
        let names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        qb.push_bind(serde_json::Value::from(names).to_string());
        qb.push("))");
    }
    qb.push(")");
}

/// Pushes whether the `tags` of the row `alias` hold one of `tags`, any case.
fn push_tag_match(qb: &mut QueryBuilder<Sqlite>, alias: &str, tags: &[String]) {
    qb.push("(");
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            qb.push(" OR ");
        }
        let escaped = tag
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        qb.push(format!("('|' || lower({alias}.tags) || '|') LIKE "));
        qb.push_bind(format!("%|{escaped}|%"));
        qb.push(" ESCAPE '\\'");
    }
    qb.push(")");
}

/// Pushes whether the item or its series (for a season or an episode) has one of `tags`.
fn push_tags(qb: &mut QueryBuilder<Sqlite>, tags: &[String]) {
    qb.push("(");
    push_tag_match(qb, "media_items", tags);
    qb.push(
        " OR (media_items.media_type IN ('Season', 'Episode') AND EXISTS (SELECT 1 FROM media_items s \
         WHERE s.media_type = 'Series' AND (s.id = media_items.parent_id OR s.id = \
         (SELECT x.parent_id FROM media_items x WHERE x.id = media_items.parent_id)) AND ",
    );
    push_tag_match(qb, "s", tags);
    qb.push(")))");
}

/// Pushes ` AND (<expr>) = <wanted>` for a yes/no condition.
fn push_flag(qb: &mut QueryBuilder<Sqlite>, expr: &str, wanted: Option<bool>) {
    if let Some(wanted) = wanted {
        qb.push(format!(" AND ({expr}) = "));
        qb.push_bind(wanted);
    }
}

/// The stored ratings the rating conditions leave out: those with a score outside
/// `min_rating`..`max_rating`, or with any score when only unrated items are wanted.
async fn excluded_ratings(pool: &SqlitePool, f: &ItemFilter) -> Result<Vec<String>, sqlx::Error> {
    let ratings: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT official_rating FROM media_items WHERE official_rating IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let key = |s: RatingScore| (s.score, s.sub_score.unwrap_or(0));
    let ranged = f.min_rating.is_some() || f.max_rating.is_some();
    Ok(ratings
        .into_iter()
        .filter(|rating| match rating_score(rating) {
            None => false,
            Some(_) if !ranged => f.no_parental_rating,
            Some(score) => {
                f.min_rating.is_some_and(|min| key(score) < key(min))
                    || f.max_rating.is_some_and(|max| key(score) > key(max))
            }
        })
        .collect())
}

/// The ids of the items that meet every condition of `f`, or `None` when it has none.
pub async fn matching_ids(
    pool: &SqlitePool,
    f: &ItemFilter,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    if f.is_empty() {
        return Ok(None);
    }
    // Extras are listed only when asked for by id, so they are never among the matches.
    let mut qb: QueryBuilder<Sqlite> =
        QueryBuilder::new("SELECT id FROM media_items WHERE extra_type IS NULL");
    if !f.person_ids.is_empty() {
        qb.push(
            " AND id IN (SELECT ip.item_id FROM item_people ip JOIN people p ON p.id = ip.person_id \
             WHERE p.name IN (SELECT name FROM people WHERE id IN ",
        );
        push_list(&mut qb, f.person_ids.clone());
        qb.push("))");
    }
    if let Some(person) = &f.person {
        qb.push(
            " AND id IN (SELECT ip.item_id FROM item_people ip JOIN people p ON p.id = ip.person_id \
             WHERE p.name = ",
        );
        qb.push_bind(person.clone());
        qb.push(")");
    }
    if !f.studios.is_empty() {
        qb.push(
            " AND id IN (SELECT s.item_id FROM item_studios s JOIN studios st ON st.id = s.studio_id \
             WHERE lower(st.name) IN ",
        );
        push_list(&mut qb, f.studios.iter().map(|s| s.to_lowercase()));
        qb.push(")");
    }
    if !f.artist_ids.is_empty() || !f.artist_names.is_empty() {
        push_artists(
            &mut qb,
            &f.artist_ids,
            &f.artist_names,
            &["Artist", "AlbumArtist"],
            false,
        );
    }
    if !f.album_artist_ids.is_empty() {
        push_artists(&mut qb, &f.album_artist_ids, &[], &["AlbumArtist"], false);
    }
    if !f.contributing_artist_ids.is_empty() {
        push_artists(&mut qb, &f.contributing_artist_ids, &[], &["Artist"], false);
    }
    if !f.exclude_artist_ids.is_empty() {
        push_artists(
            &mut qb,
            &f.exclude_artist_ids,
            &[],
            &["Artist", "AlbumArtist"],
            true,
        );
    }
    if !f.album_ids.is_empty() {
        qb.push(" AND parent_id IN ");
        push_list(&mut qb, f.album_ids.clone());
    }
    if !f.album_names.is_empty() {
        qb.push(" AND parent_id IN (SELECT id FROM media_items WHERE media_type = 'MusicAlbum' AND name IN ");
        push_list(&mut qb, f.album_names.clone());
        qb.push(")");
    }
    if !f.tags.is_empty() {
        qb.push(" AND ");
        push_tags(&mut qb, &f.tags);
    }
    if !f.exclude_tags.is_empty() {
        qb.push(" AND NOT ");
        push_tags(&mut qb, &f.exclude_tags);
    }
    if let Some(min) = f.min_community_rating {
        qb.push(" AND community_rating >= ");
        qb.push_bind(min);
    }
    if let Some(min) = &f.min_premiere_date {
        qb.push(" AND julianday(premiere_date) >= julianday(");
        qb.push_bind(min.clone());
        qb.push(")");
    }
    if let Some(max) = &f.max_premiere_date {
        qb.push(" AND julianday(premiere_date) <= julianday(");
        qb.push_bind(max.clone());
        qb.push(")");
    }
    if let Some((unaired, now)) = &f.unaired {
        qb.push(if *unaired {
            " AND julianday(premiere_date) >= julianday("
        } else {
            " AND julianday(premiere_date) < julianday("
        });
        qb.push_bind(now.clone());
        qb.push(")");
    }
    push_flag(
        &mut qb,
        "EXISTS (SELECT 1 FROM item_metadata_extras e WHERE e.item_id = media_items.id \
         AND COALESCE(e.imdb_id, '') != '')",
        f.has_imdb_id,
    );
    push_flag(&mut qb, "tmdb_id IS NOT NULL", f.has_tmdb_id);
    push_flag(&mut qb, "tvdb_id IS NOT NULL", f.has_tvdb_id);
    if f.is_hd.is_some() || f.is_4k.is_some() {
        // Jellyfin's bands: SD under HD_WIDTH, HD from it up to 4K, 4K from UHD width or height.
        let uhd = format!("({WIDTH_SQL} >= {UHD_WIDTH} OR {HEIGHT_SQL} >= {UHD_HEIGHT})");
        qb.push(" AND ((");
        qb.push_bind(f.is_hd == Some(false));
        qb.push(format!(" AND {WIDTH_SQL} < {HD_WIDTH}) OR ("));
        qb.push_bind(f.is_hd == Some(true));
        qb.push(format!(
            " AND {WIDTH_SQL} >= {HD_WIDTH} AND NOT {uhd}) OR ("
        ));
        qb.push_bind(f.is_4k == Some(true));
        qb.push(format!(" AND {uhd}))"));
    }
    for (sql, value) in [
        (format!(" AND {WIDTH_SQL} >= "), f.min_width),
        (format!(" AND {HEIGHT_SQL} >= "), f.min_height),
        (format!(" AND {WIDTH_SQL} <= "), f.max_width),
        (format!(" AND {HEIGHT_SQL} <= "), f.max_height),
    ] {
        if let Some(value) = value {
            qb.push(sql);
            qb.push_bind(value);
        }
    }
    if f.is_3d == Some(true) || f.is_placeholder == Some(true) {
        qb.push(" AND 0");
    }
    push_flag(
        &mut qb,
        "EXISTS (SELECT 1 FROM media_streams st JOIN media_sources ms ON ms.id = st.source_id \
         WHERE ms.item_id = media_items.id AND lower(st.codec_type) = 'subtitle')",
        f.has_subtitles,
    );
    push_flag(&mut qb, "COALESCE(overview, '') != ''", f.has_overview);
    push_flag(
        &mut qb,
        "COALESCE(official_rating, '') != ''",
        f.has_official_rating,
    );
    if f.no_parental_rating || f.min_rating.is_some() || f.max_rating.is_some() {
        let excluded = excluded_ratings(pool, f).await?;
        if !excluded.is_empty() {
            qb.push(" AND (official_rating IS NULL OR official_rating NOT IN ");
            push_list(&mut qb, excluded);
            qb.push(")");
        }
    }
    push_flag(
        &mut qb,
        "COALESCE((SELECT e.lock_data FROM item_metadata_extras e \
         WHERE e.item_id = media_items.id), 0) != 0",
        f.is_locked,
    );
    push_flag(&mut qb, "location_type = 'Virtual'", f.is_virtual);
    if !f.video_types.is_empty() {
        let video_file = f
            .video_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case("VideoFile"));
        qb.push(if video_file {
            " AND media_type IN ('Movie', 'Episode', 'Video')"
        } else {
            " AND 0"
        });
    }
    if let Some(name) = &f.name_less_than {
        qb.push(" AND lower(COALESCE(sort_name, name)) < ");
        qb.push_bind(name.to_lowercase());
    }
    if let Some(index) = f.index_number {
        qb.push(" AND index_number = ");
        qb.push_bind(index);
    }
    if let Some(index) = f.parent_index_number {
        qb.push(" AND parent_index_number = ");
        qb.push_bind(index);
    }
    if !f.image_types.is_empty() {
        qb.push(" AND id IN (SELECT item_id FROM item_images WHERE lower(image_type) IN ");
        push_list(&mut qb, f.image_types.iter().map(|t| t.to_lowercase()));
        qb.push(")");
    }
    let ids: Vec<String> = qb.build_query_scalar().fetch_all(pool).await?;
    Ok(Some(ids))
}

#[cfg(test)]
mod tests {
    use super::{ItemFilter, matching_ids};
    use rustyfin_model::parental::rating_score;
    use sqlx::SqlitePool;

    /// Made-up items, ids by name: movies a (HD, PG-13, Kids, TMDb and IMDb ids, overview,
    /// subtitle, Primary picture, studio Acme, actor Pat, locked), b (4K, R, Sports|Drama, 2030)
    /// and c (SD by its stream, unrated); series s (News) > season n > episodes e1, e2, e3
    /// (virtual); artist item band > album r > tracks t1 (Band) and t2 (Guest); trailer x of a.
    async fn seeded() -> SqlitePool {
        let pool = crate::test_pool().await;
        let item = |id: &str, kind: &str, parent: &str| {
            let parent = if parent.is_empty() {
                "NULL".to_string()
            } else {
                format!("'{parent}'")
            };
            format!(
                "INSERT INTO media_items (id, name, media_type, path, parent_id, library_id) \
                 VALUES ('{id}', '{id}', '{kind}', '/x/{id}', {parent}, 'lib')"
            )
        };
        let mut sql = vec![
            "INSERT INTO libraries (id, name, content_type) VALUES ('lib', 'L', 'movies')"
                .to_string(),
            item("a", "Movie", ""),
            item("b", "Movie", ""),
            item("c", "Movie", ""),
            item("s", "Series", ""),
            item("n", "Season", "s"),
            item("e1", "Episode", "n"),
            item("e2", "Episode", "n"),
            item("e3", "Episode", "n"),
            item("band", "MusicArtist", ""),
            item("r", "MusicAlbum", "band"),
            item("t1", "Audio", "r"),
            item("t2", "Audio", "r"),
            item("x", "Video", ""),
        ];
        sql.extend(
            [
                "UPDATE media_items SET tmdb_id = 1, overview = 'o', community_rating = 8.1, premiere_date = '2001-05-01T00:00:00Z', official_rating = 'PG-13', tags = 'Kids' WHERE id = 'a'",
                "UPDATE media_items SET community_rating = 5, premiere_date = '2030-01-01T00:00:00Z', official_rating = 'R', tags = 'Sports|Drama' WHERE id = 'b'",
                "UPDATE media_items SET overview = '' WHERE id = 'c'",
                "UPDATE media_items SET tags = 'News', tvdb_id = 2 WHERE id = 's'",
                "UPDATE media_items SET index_number = CAST(substr(id, 2) AS INTEGER), parent_index_number = 1 WHERE id LIKE 'e_'",
                "UPDATE media_items SET location_type = 'Virtual' WHERE id = 'e3'",
                "UPDATE media_items SET extra_type = 'Trailer', owner_id = 'a', tags = 'Kids' WHERE id = 'x'",
                "INSERT INTO item_metadata_extras (item_id, imdb_id, lock_data) VALUES ('a', 'tt1', 1)",
                "INSERT INTO item_images (item_id, image_type, image_index, path) VALUES ('a', 'Primary', 0, '/x/a.jpg')",
                "INSERT INTO studios (id, name) VALUES ('st', 'Acme')",
                "INSERT INTO item_studios (item_id, studio_id) VALUES ('a', 'st')",
                "INSERT INTO people (id, name) VALUES ('pat', 'Pat'), ('pband', 'Band'), ('guest', 'Guest'), ('pat2', 'Pat')",
                "INSERT INTO item_people (item_id, person_id, type) VALUES ('a', 'pat2', 'Actor'), ('t1', 'pband', 'Artist'), ('r', 'pband', 'AlbumArtist'), ('t2', 'guest', 'Artist')",
                "INSERT INTO media_sources (id, item_id, uid, path, width, height) VALUES ('sa', 'a', 'sa', '/x/a', 1920, 1080), ('sb', 'b', 'sb', '/x/b', 3840, 2160), ('sc', 'c', 'sc', '/x/c', NULL, NULL)",
                "INSERT INTO media_streams (id, source_id, codec_type, stream_index, width, height) VALUES ('vc', 'sc', 'Video', 0, 640, 480), ('sub', 'sa', 'Subtitle', 2, NULL, NULL)",
            ]
            .map(str::to_string),
        );
        for sql in sql {
            sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"));
        }
        pool
    }

    async fn found(pool: &SqlitePool, f: ItemFilter) -> Vec<String> {
        let mut ids = matching_ids(pool, &f).await.unwrap().expect("a condition");
        ids.sort();
        ids
    }

    fn list(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[tokio::test]
    async fn r3items_each_condition_keeps_its_items() {
        let pool = seeded().await;
        assert_eq!(
            matching_ids(&pool, &ItemFilter::default()).await.unwrap(),
            None
        );
        let cases: Vec<(ItemFilter, &[&str])> = vec![
            // People by the name of the person id given; studios by name, any case.
            (
                ItemFilter {
                    person_ids: list(&["pat"]),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    person: Some("Pat".into()),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    studios: list(&["acme"]),
                    ..Default::default()
                },
                &["a"],
            ),
            // Artists by item or person id, or by name; albums of credited tracks count.
            (
                ItemFilter {
                    artist_ids: list(&["band"]),
                    ..Default::default()
                },
                &["r", "t1"],
            ),
            (
                ItemFilter {
                    artist_ids: list(&["pband"]),
                    ..Default::default()
                },
                &["r", "t1"],
            ),
            (
                ItemFilter {
                    artist_names: list(&["GUEST"]),
                    ..Default::default()
                },
                &["r", "t2"],
            ),
            (
                ItemFilter {
                    album_artist_ids: list(&["pband"]),
                    ..Default::default()
                },
                &["r"],
            ),
            (
                ItemFilter {
                    contributing_artist_ids: list(&["guest"]),
                    ..Default::default()
                },
                &["r", "t2"],
            ),
            (
                ItemFilter {
                    album_ids: list(&["r"]),
                    ..Default::default()
                },
                &["t1", "t2"],
            ),
            (
                ItemFilter {
                    album_names: list(&["r"]),
                    ..Default::default()
                },
                &["t1", "t2"],
            ),
            // Tags: any of them, any case, with the series' for its seasons and episodes; never
            // an extra.
            (
                ItemFilter {
                    tags: list(&["kids"]),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    tags: list(&["Drama", "Kids"]),
                    ..Default::default()
                },
                &["a", "b"],
            ),
            (
                ItemFilter {
                    tags: list(&["News"]),
                    ..Default::default()
                },
                &["e1", "e2", "e3", "n", "s"],
            ),
            (
                ItemFilter {
                    tags: list(&["Dra"]),
                    ..Default::default()
                },
                &[],
            ),
            (
                ItemFilter {
                    min_community_rating: Some(6.0),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    min_premiere_date: Some("2020-01-01 00:00:00".into()),
                    ..Default::default()
                },
                &["b"],
            ),
            (
                ItemFilter {
                    max_premiere_date: Some("2010-01-01 00:00:00".into()),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    unaired: Some((true, "2026-01-01 00:00:00".into())),
                    ..Default::default()
                },
                &["b"],
            ),
            (
                ItemFilter {
                    has_imdb_id: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    has_tmdb_id: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    has_tvdb_id: Some(true),
                    ..Default::default()
                },
                &["s"],
            ),
            (
                ItemFilter {
                    is_hd: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    is_hd: Some(false),
                    ..Default::default()
                },
                &["c"],
            ),
            (
                ItemFilter {
                    is_4k: Some(true),
                    ..Default::default()
                },
                &["b"],
            ),
            (
                ItemFilter {
                    min_width: Some(1000),
                    ..Default::default()
                },
                &["a", "b"],
            ),
            (
                ItemFilter {
                    max_height: Some(1080),
                    ..Default::default()
                },
                &["a", "c"],
            ),
            (
                ItemFilter {
                    has_subtitles: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    has_overview: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    has_official_rating: Some(true),
                    ..Default::default()
                },
                &["a", "b"],
            ),
            (
                ItemFilter {
                    is_locked: Some(true),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    is_3d: Some(true),
                    ..Default::default()
                },
                &[],
            ),
            (
                ItemFilter {
                    is_placeholder: Some(true),
                    ..Default::default()
                },
                &[],
            ),
            (
                ItemFilter {
                    is_virtual: Some(true),
                    ..Default::default()
                },
                &["e3"],
            ),
            (
                ItemFilter {
                    video_types: list(&["Iso"]),
                    ..Default::default()
                },
                &[],
            ),
            (
                ItemFilter {
                    name_less_than: Some("Bb".into()),
                    ..Default::default()
                },
                &["a", "b", "band"],
            ),
            (
                ItemFilter {
                    index_number: Some(2),
                    ..Default::default()
                },
                &["e2"],
            ),
            (
                ItemFilter {
                    parent_index_number: Some(1),
                    ..Default::default()
                },
                &["e1", "e2", "e3"],
            ),
            (
                ItemFilter {
                    image_types: list(&["primary"]),
                    ..Default::default()
                },
                &["a"],
            ),
        ];
        for (filter, expected) in cases {
            assert_eq!(
                found(&pool, filter.clone()).await,
                list(expected),
                "{filter:?}"
            );
        }
        // Exclusions keep the items without what they name.
        let kept = found(
            &pool,
            ItemFilter {
                exclude_tags: list(&["news"]),
                ..Default::default()
            },
        )
        .await;
        assert!(kept.contains(&"a".to_string()) && !kept.contains(&"e1".to_string()));
        let kept = found(
            &pool,
            ItemFilter {
                exclude_artist_ids: list(&["band"]),
                ..Default::default()
            },
        )
        .await;
        assert!(kept.contains(&"t2".to_string()) && !kept.contains(&"t1".to_string()));
        // A rating range keeps the unrated items; without one, HasParentalRating=false keeps only them.
        let ranged = |min: Option<&str>, max: Option<&str>, none: bool| ItemFilter {
            min_rating: min.and_then(rating_score),
            max_rating: max.and_then(rating_score),
            no_parental_rating: none,
            ..Default::default()
        };
        let movies = |ids: Vec<String>| -> Vec<String> {
            ids.into_iter()
                .filter(|id| ["a", "b", "c"].contains(&id.as_str()))
                .collect()
        };
        assert_eq!(
            movies(found(&pool, ranged(None, Some("PG-13"), false)).await),
            list(&["a", "c"])
        );
        assert_eq!(
            movies(found(&pool, ranged(Some("R"), None, false)).await),
            list(&["b", "c"])
        );
        assert_eq!(
            movies(found(&pool, ranged(None, None, true)).await),
            list(&["c"])
        );
    }
}
