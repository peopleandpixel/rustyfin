//! The `/Items` conditions of Jellyfin's `InternalItemsQuery` that `repo_media::get_items_filtered`
//! has no clause for: people, studios and artists by name or id, provider ids, picture size,
//! ratings, flags and numbers. `filter_sql` makes them one SQL condition with bound values, which
//! the list query adds to its own, beside the parental limits and the count.

use rustyfin_model::parental::{RatingScore, rating_score};
use sqlx::SqlitePool;

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
    /// With `person_ids` or `person`: only credits of these kinds (Actor, Director, …), any case.
    pub person_types: Vec<String>,
    /// Items, or folders holding an item, with exactly one of these ratings (`OfficialRatings`).
    pub official_ratings: Vec<String>,
    /// Items, or folders holding an item, with an audio or a subtitle stream in one of these
    /// languages (`AudioLanguages`, `SubtitleLanguages`); `und` also matches a stream without one.
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
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
    /// `MinDateLastSaved(ForUser)`: an item is last saved when it was added or a metadata
    /// provider last refreshed it, whichever is later; no other write is timestamped.
    pub min_date_last_saved: Option<String>,
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
            && self.min_date_last_saved.is_none()
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
            && self.official_ratings.is_empty()
            && self.audio_languages.is_empty()
            && self.subtitle_languages.is_empty()
    }
}

/// A value bound to the condition, in the order of its `?`.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    Text(String),
    Int(i64),
    Real(f64),
}

/// The conditions as one SQL condition on a `media_items` row (unaliased, as
/// `repo_media::get_items_filtered` reads the table), with its values in order. Every list is
/// bound once, as a JSON array read with `json_each`, so no list can reach SQLite's limit on
/// parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterSql {
    pub sql: String,
    pub binds: Vec<Bound>,
}

impl FilterSql {
    fn push(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    fn text(&mut self, value: impl Into<String>) {
        self.sql.push('?');
        self.binds.push(Bound::Text(value.into()));
    }

    fn int(&mut self, value: i64) {
        self.sql.push('?');
        self.binds.push(Bound::Int(value));
    }

    /// ` AND (<cond>)` on the item or any of its descendants, as Jellyfin 12.4's
    /// `WhereItemOrDescendantMatches` (` AND NOT` for `WhereNeitherItemNorDescendantMatches`):
    /// `cond` gives the condition on a table alias, and `binds` are its values, bound for each use.
    fn item_or_descendant(&mut self, cond: impl Fn(&str) -> String, binds: &[Bound], wanted: bool) {
        self.push(if wanted { " AND (" } else { " AND NOT (" });
        self.push(&cond("media_items"));
        self.binds.extend(binds.iter().cloned());
        // The ancestors of every matching item, walked up once for the whole query.
        self.push(
            " OR id IN (WITH RECURSIVE up(id) AS (SELECT x.parent_id FROM media_items x \
             WHERE x.parent_id IS NOT NULL AND ",
        );
        self.push(&cond("x"));
        self.binds.extend(binds.iter().cloned());
        self.push(
            " UNION SELECT m.parent_id FROM media_items m JOIN up ON m.id = up.id \
             WHERE m.parent_id IS NOT NULL) SELECT id FROM up))",
        );
    }

    /// Items, or folders holding one, with a `kind` stream (`audio`, `subtitle`) in one of
    /// `languages` (lowercase), `und` also matching a stream without a language.
    fn stream_languages(&mut self, kind: &'static str, languages: &[String]) {
        let languages: Vec<String> = languages.iter().map(|l| l.to_lowercase()).collect();
        let und = languages.iter().any(|l| l == "und");
        let binds = [
            Bound::Text(serde_json::Value::from(languages).to_string()),
            Bound::Int(und.into()),
        ];
        self.item_or_descendant(
            |a| {
                format!(
                    "EXISTS (SELECT 1 FROM media_streams st JOIN media_sources ms ON ms.id = st.source_id \
                     WHERE ms.item_id = {a}.id AND lower(st.codec_type) = '{kind}' \
                     AND (lower(st.language) IN (SELECT value FROM json_each(?)) \
                     OR (? AND COALESCE(st.language, '') = '')))"
                )
            },
            &binds,
            true,
        );
    }

    /// ` AND lower(ip.type) IN (…)` for a credit query over `item_people ip`, when `types` names
    /// any (Jellyfin 12.4 applies `PersonTypes` to `PersonIds` and `Person`).
    fn person_types(&mut self, types: &[String]) {
        if !types.is_empty() {
            self.push(" AND lower(ip.type) IN ");
            self.list(types.iter().map(|t| t.to_lowercase()));
        }
    }

    /// `(SELECT value FROM json_each(?))`, with `values` bound as one JSON array.
    fn list<S: Into<String>>(&mut self, values: impl IntoIterator<Item = S>) {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        self.push("(SELECT value FROM json_each(");
        self.text(serde_json::Value::from(values).to_string());
        self.push("))");
    }

    /// ` AND (<expr>) = <wanted>` for a yes/no condition.
    fn flag(&mut self, expr: &str, wanted: Option<bool>) {
        if let Some(wanted) = wanted {
            self.push(&format!(" AND ({expr}) = "));
            self.int(wanted.into());
        }
    }

    /// ` AND id [NOT] IN (…)`: the items credited, as one of `types`, to an artist of the same
    /// name as one of `ids` (a `MusicArtist` item or a person) or one of `names`, and the albums
    /// holding such a track.
    fn artists(&mut self, ids: &[String], names: &[String], types: &[&str], negate: bool) {
        self.push(if negate {
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
                self.push(" UNION ");
            }
            self.push(&format!(
                "SELECT {column} FROM item_people ip JOIN people p ON p.id = ip.person_id \
                 JOIN media_items t ON t.id = ip.item_id WHERE {column} IS NOT NULL AND {kind} \
                 AND ip.type IN "
            ));
            self.list(types.iter().copied());
            self.push(" AND lower(p.name) IN (SELECT lower(name) FROM people WHERE id IN ");
            self.list(ids.iter().cloned());
            self.push(" UNION SELECT lower(name) FROM media_items WHERE media_type = 'MusicArtist' AND id IN ");
            self.list(ids.iter().cloned());
            self.push(" UNION SELECT value FROM json_each(");
            let names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
            self.text(serde_json::Value::from(names).to_string());
            self.push("))");
        }
        self.push(")");
    }

    /// Whether the item's own tags hold one of `tags`, any case (Jellyfin's `Tags`; only its
    /// `IncludeInheritedTags` reads the series' tags too).
    fn tags(&mut self, tags: &[String]) {
        self.push("EXISTS (SELECT 1 FROM json_each(");
        self.text(
            serde_json::Value::from(tags.iter().map(|t| t.to_lowercase()).collect::<Vec<_>>())
                .to_string(),
        );
        self.push(
            ") wanted WHERE instr('|' || lower(media_items.tags) || '|', '|' || wanted.value || '|') > 0)",
        );
    }
}

/// The rating Jellyfin's `InheritedParentalRatingValue` reads: the item's own, else its
/// parent's (an extra's owner's), else its grandparent's, as `repo_parental` inherits it; NULL
/// when none is set. A macro, so `repo_media`'s OfficialRating sort can `concat!` it.
macro_rules! inherited_rating_sql {
    () => {
        "COALESCE(\
         NULLIF(TRIM(COALESCE(media_items.official_rating, '')), ''), \
         (SELECT NULLIF(TRIM(COALESCE(p.official_rating, '')), '') FROM media_items p \
          WHERE p.id = COALESCE(media_items.parent_id, media_items.owner_id)), \
         (SELECT NULLIF(TRIM(COALESCE(g.official_rating, '')), '') FROM media_items p \
          JOIN media_items g ON g.id = p.parent_id \
          WHERE p.id = COALESCE(media_items.parent_id, media_items.owner_id)))"
    };
}
pub(crate) use inherited_rating_sql;
const INHERITED_RATING_SQL: &str = inherited_rating_sql!();

/// The ratings (trimmed, as `INHERITED_RATING_SQL` reads them) the rating conditions leave out:
/// those with a score outside `min_rating`..`max_rating`, or with any score when only unrated
/// items are wanted.
async fn excluded_ratings(pool: &SqlitePool, f: &ItemFilter) -> Result<Vec<String>, sqlx::Error> {
    let ratings: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT TRIM(official_rating) FROM media_items \
         WHERE TRIM(COALESCE(official_rating, '')) != ''",
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

/// The conditions of `f` as one SQL condition, or `None` when it has none.
pub async fn filter_sql(
    pool: &SqlitePool,
    f: &ItemFilter,
) -> Result<Option<FilterSql>, sqlx::Error> {
    if f.is_empty() {
        return Ok(None);
    }
    let mut q = FilterSql {
        sql: "(1 = 1".to_string(),
        binds: Vec::new(),
    };
    if !f.person_ids.is_empty() {
        q.push(
            " AND id IN (SELECT ip.item_id FROM item_people ip JOIN people p ON p.id = ip.person_id \
             WHERE p.name IN (SELECT name FROM people WHERE id IN ",
        );
        q.list(f.person_ids.iter().cloned());
        q.push(")");
        q.person_types(&f.person_types);
        q.push(")");
    }
    if let Some(person) = &f.person {
        q.push(
            " AND id IN (SELECT ip.item_id FROM item_people ip JOIN people p ON p.id = ip.person_id \
             WHERE p.name = ",
        );
        q.text(person.clone());
        q.person_types(&f.person_types);
        q.push(")");
    }
    if !f.official_ratings.is_empty() {
        let ratings = [Bound::Text(
            serde_json::Value::from(f.official_ratings.clone()).to_string(),
        )];
        q.item_or_descendant(
            |a| format!("{a}.official_rating IN (SELECT value FROM json_each(?))"),
            &ratings,
            true,
        );
    }
    if !f.audio_languages.is_empty() {
        q.stream_languages("audio", &f.audio_languages);
    }
    if !f.subtitle_languages.is_empty() {
        q.stream_languages("subtitle", &f.subtitle_languages);
    }
    if !f.studios.is_empty() {
        q.push(
            " AND id IN (SELECT s.item_id FROM item_studios s JOIN studios st ON st.id = s.studio_id \
             WHERE lower(st.name) IN ",
        );
        q.list(f.studios.iter().map(|s| s.to_lowercase()));
        q.push(")");
    }
    if !f.artist_ids.is_empty() || !f.artist_names.is_empty() {
        q.artists(
            &f.artist_ids,
            &f.artist_names,
            &["Artist", "AlbumArtist"],
            false,
        );
    }
    if !f.album_artist_ids.is_empty() {
        q.artists(&f.album_artist_ids, &[], &["AlbumArtist"], false);
    }
    if !f.contributing_artist_ids.is_empty() {
        q.artists(&f.contributing_artist_ids, &[], &["Artist"], false);
    }
    if !f.exclude_artist_ids.is_empty() {
        q.artists(&f.exclude_artist_ids, &[], &["Artist", "AlbumArtist"], true);
    }
    if !f.album_ids.is_empty() {
        q.push(" AND parent_id IN ");
        q.list(f.album_ids.iter().cloned());
    }
    if !f.album_names.is_empty() {
        q.push(" AND parent_id IN (SELECT id FROM media_items WHERE media_type = 'MusicAlbum' AND name IN ");
        q.list(f.album_names.iter().cloned());
        q.push(")");
    }
    if !f.tags.is_empty() {
        q.push(" AND ");
        q.tags(&f.tags);
    }
    if !f.exclude_tags.is_empty() {
        q.push(" AND NOT ");
        q.tags(&f.exclude_tags);
    }
    if let Some(min) = f.min_community_rating {
        q.push(" AND community_rating >= ?");
        q.binds.push(Bound::Real(min));
    }
    for (op, date) in [(">=", &f.min_premiere_date), ("<=", &f.max_premiere_date)] {
        if let Some(date) = date {
            q.push(&format!(" AND julianday(premiere_date) {op} julianday("));
            q.text(date.clone());
            q.push(")");
        }
    }
    if let Some(min) = &f.min_date_last_saved {
        q.push(
            " AND MAX(COALESCE(julianday(date_created), 0), COALESCE((SELECT julianday(e.provider_refreshed_at) \
             FROM item_metadata_extras e WHERE e.item_id = media_items.id), 0)) >= julianday(",
        );
        q.text(min.clone());
        q.push(")");
    }
    if let Some((unaired, now)) = &f.unaired {
        q.push(if *unaired {
            " AND julianday(premiere_date) >= julianday("
        } else {
            " AND julianday(premiere_date) < julianday("
        });
        q.text(now.clone());
        q.push(")");
    }
    q.flag(
        "EXISTS (SELECT 1 FROM item_metadata_extras e WHERE e.item_id = media_items.id \
         AND COALESCE(e.imdb_id, '') != '')",
        f.has_imdb_id,
    );
    q.flag("tmdb_id IS NOT NULL", f.has_tmdb_id);
    q.flag("tvdb_id IS NOT NULL", f.has_tvdb_id);
    if f.is_hd.is_some() || f.is_4k.is_some() {
        // Jellyfin's bands: SD under HD_WIDTH, HD from it up to 4K, 4K from UHD width or height.
        let uhd = format!("({WIDTH_SQL} >= {UHD_WIDTH} OR {HEIGHT_SQL} >= {UHD_HEIGHT})");
        q.push(" AND ((");
        q.int((f.is_hd == Some(false)).into());
        q.push(&format!(" AND {WIDTH_SQL} < {HD_WIDTH}) OR ("));
        q.int((f.is_hd == Some(true)).into());
        q.push(&format!(
            " AND {WIDTH_SQL} >= {HD_WIDTH} AND NOT {uhd}) OR ("
        ));
        q.int((f.is_4k == Some(true)).into());
        q.push(&format!(" AND {uhd}))"));
    }
    for (sql, value) in [
        (format!(" AND {WIDTH_SQL} >= "), f.min_width),
        (format!(" AND {HEIGHT_SQL} >= "), f.min_height),
        (format!(" AND {WIDTH_SQL} <= "), f.max_width),
        (format!(" AND {HEIGHT_SQL} <= "), f.max_height),
    ] {
        if let Some(value) = value {
            q.push(&sql);
            q.int(value);
        }
    }
    if f.is_3d == Some(true) || f.is_placeholder == Some(true) {
        q.push(" AND 0");
    }
    if let Some(wanted) = f.has_subtitles {
        q.item_or_descendant(
            |a| {
                format!(
                    "EXISTS (SELECT 1 FROM media_streams st JOIN media_sources ms ON ms.id = st.source_id \
                     WHERE ms.item_id = {a}.id AND lower(st.codec_type) = 'subtitle')"
                )
            },
            &[],
            wanted,
        );
    }
    q.flag("COALESCE(overview, '') != ''", f.has_overview);
    if let Some(wanted) = f.has_official_rating {
        q.item_or_descendant(
            |a| format!("COALESCE({a}.official_rating, '') != ''"),
            &[],
            wanted,
        );
    }
    if f.no_parental_rating || f.min_rating.is_some() || f.max_rating.is_some() {
        let excluded = excluded_ratings(pool, f).await?;
        if !excluded.is_empty() {
            q.push(&format!(
                " AND ({INHERITED_RATING_SQL} IS NULL OR {INHERITED_RATING_SQL} NOT IN "
            ));
            q.list(excluded);
            q.push(")");
        }
    }
    q.flag(
        "COALESCE((SELECT e.lock_data FROM item_metadata_extras e \
         WHERE e.item_id = media_items.id), 0) != 0",
        f.is_locked,
    );
    q.flag("location_type = 'Virtual'", f.is_virtual);
    if !f.video_types.is_empty() {
        let video_file = f
            .video_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case("VideoFile"));
        if video_file {
            q.item_or_descendant(
                |a| format!("{a}.media_type IN ('Movie', 'Episode', 'Video')"),
                &[],
                true,
            );
        } else {
            q.push(" AND 0");
        }
    }
    if let Some(name) = &f.name_less_than {
        q.push(" AND lower(COALESCE(sort_name, name)) < ");
        q.text(name.to_lowercase());
    }
    if let Some(index) = f.index_number {
        q.push(" AND index_number = ");
        q.int(index);
    }
    if let Some(index) = f.parent_index_number {
        q.push(" AND parent_index_number = ");
        q.int(index);
    }
    if !f.image_types.is_empty() {
        q.push(" AND id IN (SELECT item_id FROM item_images WHERE lower(image_type) IN ");
        q.list(f.image_types.iter().map(|t| t.to_lowercase()));
        q.push(")");
    }
    q.push(")");
    Ok(Some(q))
}

/// The distinct languages of the `kind` streams (`audio`, `subtitle`) of the items of
/// `item_types` (all when empty) in `library` (every library when `None`) that the hidden-items
/// filter `hidden_json` (`repo_parental::filter_json`) lets through, `und` for a stream without
/// one, as Jellyfin 12.4's `GetMediaStreamLanguages` lists them for `/Items/Filters2`.
pub async fn stream_languages(
    pool: &SqlitePool,
    kind: &str,
    library: Option<uuid::Uuid>,
    item_types: &[String],
    hidden_json: Option<&str>,
) -> Result<Vec<String>, sqlx::Error> {
    let sql = format!(
        "SELECT DISTINCT CASE WHEN COALESCE(st.language, '') = '' THEN 'und' ELSE st.language END \
         FROM media_streams st JOIN media_sources ms ON ms.id = st.source_id \
         JOIN media_items m ON m.id = ms.item_id \
         WHERE lower(st.codec_type) = ?1 AND m.extra_type IS NULL \
         AND (?2 IS NULL OR m.library_id = ?2) \
         AND (json_array_length(?3) = 0 OR m.media_type IN (SELECT value FROM json_each(?3))) \
         AND {visible}",
        visible = crate::repo_parental::visible_sql("m.id", "m.library_id", "?4"),
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(kind.to_lowercase())
        .bind(library.map(|id| id.to_string()))
        .bind(serde_json::Value::from(item_types.to_vec()).to_string())
        .bind(hidden_json)
        .fetch_all(pool)
        .await
}

/// The ids of the items, not extras, that meet every condition of `f`, or `None` when it has
/// none: `filter_sql` on its own, for the tests.
#[cfg(test)]
pub async fn matching_ids(
    pool: &SqlitePool,
    f: &ItemFilter,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let Some(filter) = filter_sql(pool, f).await? else {
        return Ok(None);
    };
    let sql = format!(
        "SELECT id FROM media_items WHERE extra_type IS NULL AND {}",
        filter.sql
    );
    let mut query = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql));
    for bound in filter.binds {
        query = match bound {
            Bound::Text(v) => query.bind(v),
            Bound::Int(v) => query.bind(v),
            Bound::Real(v) => query.bind(v),
        };
    }
    Ok(Some(query.fetch_all(pool).await?))
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
                "UPDATE media_items SET tags = 'News', tvdb_id = 2, official_rating = 'TV-MA' WHERE id = 's'",
                "UPDATE media_items SET date_created = '2000-01-01T00:00:00+00:00'",
                "UPDATE media_items SET date_created = '2026-01-01T00:00:00+00:00' WHERE id = 'a'",
                "INSERT INTO item_metadata_extras (item_id, provider_refreshed_at) VALUES ('b', '2026-02-01T10:00:00.123456+00:00')",
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
                // Episode 1: German subtitles and English audio; Alpha: audio without a language.
                "INSERT INTO media_sources (id, item_id, uid, path) VALUES ('se1', 'e1', 'se1', '/x/e1')",
                "INSERT INTO media_streams (id, source_id, codec_type, stream_index, language) VALUES ('e1s', 'se1', 'Subtitle', 2, 'ger'), ('e1a', 'se1', 'Audio', 1, 'eng'), ('aa', 'sa', 'Audio', 1, NULL)",
                "UPDATE media_items SET official_rating = 'TV-14' WHERE id = 'e2'",
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
                &["s"],
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
                // A folder counts when an item under it has one, as in Jellyfin 12.4.
                &["a", "e1", "n", "s"],
            ),
            (
                ItemFilter {
                    subtitle_languages: list(&["GER"]),
                    ..Default::default()
                },
                &["e1", "n", "s"],
            ),
            (
                ItemFilter {
                    audio_languages: list(&["eng"]),
                    ..Default::default()
                },
                &["e1", "n", "s"],
            ),
            (
                ItemFilter {
                    audio_languages: list(&["und"]),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    official_ratings: list(&["TV-14"]),
                    ..Default::default()
                },
                &["e2", "n", "s"],
            ),
            (
                ItemFilter {
                    video_types: list(&["VideoFile"]),
                    ..Default::default()
                },
                &["a", "b", "c", "e1", "e2", "e3", "n", "s"],
            ),
            (
                ItemFilter {
                    person_ids: list(&["pat"]),
                    person_types: list(&["actor"]),
                    ..Default::default()
                },
                &["a"],
            ),
            (
                ItemFilter {
                    person: Some("Pat".into()),
                    person_types: list(&["Director"]),
                    ..Default::default()
                },
                &[],
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
                &["a", "b", "e2", "n", "s"],
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
        assert!(kept.contains(&"a".to_string()) && kept.contains(&"e1".to_string()));
        assert!(!kept.contains(&"s".to_string()));
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
        // Seasons and episodes take the series' rating, as Jellyfin's inherited rating.
        let shows = |ids: Vec<String>| -> Vec<String> {
            ids.into_iter()
                .filter(|id| ["s", "n", "e1", "e2", "e3"].contains(&id.as_str()))
                .collect()
        };
        assert!(shows(found(&pool, ranged(None, Some("PG-13"), false)).await).is_empty());
        assert_eq!(
            shows(found(&pool, ranged(Some("TV-MA"), None, false)).await),
            list(&["e1", "e3", "n", "s"])
        );
        assert!(shows(found(&pool, ranged(None, None, true)).await).is_empty());
        // Last saved: added, or refreshed by a metadata provider, whichever is later.
        let saved = |min: &str| ItemFilter {
            min_date_last_saved: Some(min.to_string()),
            ..Default::default()
        };
        assert_eq!(
            found(&pool, saved("2025-06-01 00:00:00")).await,
            list(&["a", "b"])
        );
        assert_eq!(
            found(&pool, saved("2026-01-15 00:00:00")).await,
            list(&["b"])
        );
        // The stream languages of /Items/Filters2: `und` for a stream without one.
        let languages = |kind: &'static str, types: &'static [&'static str]| {
            let pool = pool.clone();
            async move {
                let types: Vec<String> = types.iter().map(|t| t.to_string()).collect();
                let mut found = super::stream_languages(&pool, kind, None, &types, None)
                    .await
                    .unwrap();
                found.sort();
                found
            }
        };
        assert_eq!(languages("audio", &[]).await, list(&["eng", "und"]));
        assert_eq!(languages("audio", &["Episode"]).await, list(&["eng"]));
        assert_eq!(languages("subtitle", &["Episode"]).await, list(&["ger"]));
        assert!(languages("subtitle", &["Audio"]).await.is_empty());
        // A list of 10,000 ids is one bound value, under SQLite's limit on parameters.
        let mut many: Vec<String> = (0..10_000).map(|i| format!("missing-{i}")).collect();
        many.push("pat".to_string());
        let wide = ItemFilter {
            person_ids: many.clone(),
            album_ids: many,
            ..Default::default()
        };
        assert!(found(&pool, wide.clone()).await.is_empty());
        let wide = ItemFilter {
            album_ids: Vec::new(),
            ..wide
        };
        assert_eq!(found(&pool, wide).await, list(&["a"]));
    }
}
