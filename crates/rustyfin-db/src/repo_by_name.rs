use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use rustyfin_model::media::MediaType;
use sha1::{Digest, Sha1};
use sqlx::SqlitePool;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

/// The four lists that are read by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ByNameKind {
    /// Genres of the items that are not music.
    Genre,
    /// Genres of the audio items and music albums.
    MusicGenre,
    /// Studio names.
    Studio,
    /// Production years, named by their number.
    Year,
}

impl ByNameKind {
    /// The kind as Jellyfin names it. The entry ids are built from this name.
    pub fn as_str(self) -> &'static str {
        match self {
            ByNameKind::Genre => "Genre",
            ByNameKind::MusicGenre => "MusicGenre",
            ByNameKind::Studio => "Studio",
            ByNameKind::Year => "Year",
        }
    }
}

/// The namespace of the entry ids, `bba0cef5-b539-4903-ad04-cdc4497f4ca3`.
pub const BY_NAME_NAMESPACE: Uuid = Uuid::from_u128(0xbba0cef5b5394903ad04cdc4497f4ca3);

/// Folds a name for comparison and ids, so "Ação", "ACAO" and a decomposed "Ação" are one name.
///
/// The fold, in order:
/// 1. The characters in `DELETED` are deleted.
/// 2. The name is trimmed and lowercased, and every "ς" becomes "σ".
/// 3. The name is decomposed (NFD), every combining mark from U+0300 to U+036F is dropped, and the
///    rest is composed again (NFC).
/// 4. The letters that Jellyfin's Diacritics.NET tables fold but Unicode does not decompose are
///    swapped by `unaccented`.
///
/// It follows Jellyfin 10.11's clean value, `value.RemoveDiacritics().ToLowerInvariant()`
/// (`Jellyfin.Server.Implementations/Item/BaseItemRepository.cs:1438-1446`), and folds every
/// character of those tables as Jellyfin does, but it folds more:
/// - Jellyfin doesn't trim, and keeps a "ς" that is already lowercase, so it lists "δράμας" apart
///   from "ΔΡΆΜΑΣ" ("δραμασ"), where this fold makes them one name.
/// - Jellyfin keeps the combining marks of a decomposed name, so it lists a decomposed "Ação" apart
///   from a composed one. This fold makes them one name, by choice.
/// - Step 3 also folds characters that the tables leave alone: rarer letters such as "ĉ" and
///   polytonic Greek lose their marks, "≠" becomes "=", and NFC turns the CJK compatibility
///   ideographs into the unified ones.
///
/// One exception: Jellyfin folds 17 two-code-point sequences, each a table letter followed by a
/// second mark, together with another spelling that this fold keeps apart (U+0622 U+0654 and
/// U+0627 U+0654, for one). They are U+0622, U+0623, U+0625 or U+0671 followed by U+0653, U+0654
/// or U+0655; U+0624, U+0626, U+0649 or U+06C2 followed by U+0654; and U+0B94 followed by U+0BD7.
///
/// The fold is frozen: every entry id is built from it (`by_name_id`) and clients store those ids, so
/// a change would change ids that clients hold.
pub fn fold_name(name: &str) -> String {
    let lower = name
        .replace(DELETED, "")
        .trim()
        .to_lowercase()
        .replace('ς', "σ");
    lower
        .nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .nfc()
        .map(unaccented)
        .collect()
}

/// The characters that Jellyfin deletes from a name: U+FFFD, which
/// `Jellyfin.Extensions.StringExtensions.RemoveDiacritics` strips, and "¡", "¿" and "⸘", which the
/// Spanish table of Diacritics.NET maps to nothing.
const DELETED: [char; 4] = ['\u{fffd}', '¡', '¿', '⸘'];

/// The lowercase letters that Jellyfin's Diacritics.NET tables fold and Unicode does not decompose.
fn unaccented(c: char) -> char {
    match c {
        'ø' => 'o',
        'đ' => 'd',
        'ı' => 'i',
        'ł' => 'l',
        // Arabic alef with madda, hamza above or below, and wasla, to alef.
        '\u{622}' | '\u{623}' | '\u{625}' | '\u{671}' => '\u{627}',
        // Arabic waw with hamza to waw; yeh with hamza and alef maksura to yeh.
        '\u{624}' => '\u{648}',
        '\u{626}' | '\u{649}' => '\u{64a}',
        // Arabic heh goal with hamza to heh goal.
        '\u{6c2}' => '\u{6c1}',
        // Tamil au to o.
        '\u{b94}' => '\u{b92}',
        other => other,
    }
}

/// The id of the entry `name` of `kind`: a UUIDv5 of `<kind>:<folded name>` in `BY_NAME_NAMESPACE`.
pub fn by_name_id(kind: ByNameKind, name: &str) -> Uuid {
    folded_id(kind, &fold_name(name))
}

/// `by_name_id` of a name that `fold_name` has already folded.
fn folded_id(kind: ByNameKind, folded: &str) -> Uuid {
    // The uuid crate's v5 feature is off (its dependencies are not cached), so the SHA-1 is taken here.
    let mut hasher = Sha1::new();
    hasher.update(BY_NAME_NAMESPACE.as_bytes());
    hasher.update(format!("{}:{}", kind.as_str(), folded).as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_sha1_bytes(bytes).into_uuid()
}

/// The kind of the genres of an item of type `media`: music genres for audio and albums, genres for
/// every other item. Jellyfin picks the id of a genre on an item the same way, by whether the item
/// has music genres (`Emby.Server.Implementations/Dto/DtoService.cs:1027-1035`).
pub fn genre_kind_of(media: MediaType) -> ByNameKind {
    if matches!(media, MediaType::Audio | MediaType::MusicAlbum) {
        ByNameKind::MusicGenre
    } else {
        ByNameKind::Genre
    }
}

/// Splits a `genres` value into names. The value is `|`-separated, or a JSON array of strings from
/// before migration 014. Names are trimmed, empty names are dropped, and a name that folds like an
/// earlier one is dropped too.
pub fn split_genres(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    // Only a JSON array of strings parses here; any other text is split on `|`.
    let names: Vec<String> =
        serde_json::from_str(raw).unwrap_or_else(|_| raw.split('|').map(str::to_string).collect());
    let mut seen = HashSet::new();
    names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty() && seen.insert(fold_name(name)))
        .map(str::to_string)
        .collect()
}

/// The counts of the items behind an entry, by type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ByNameCounts {
    /// Distinct items with the name.
    pub items: i64,
    /// Movies with the name.
    pub movies: i64,
    /// Series with the name.
    pub series: i64,
    /// Episodes with the name.
    pub episodes: i64,
    /// Music albums with the name.
    pub albums: i64,
    /// Audio items with the name.
    pub songs: i64,
}

/// One entry of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByNameEntry {
    /// The list the entry is in.
    pub kind: ByNameKind,
    /// The id from `by_name_id`.
    pub id: Uuid,
    /// The most common spelling among all the rows of the entry, whatever the filter; a tie goes
    /// to the spelling that comes first in code-point order, so "DRAMA" wins over "Drama" and
    /// "Acao" over "Ação".
    pub name: String,
    /// The counts of the items behind the entry.
    pub counts: ByNameCounts,
}

/// The filter of a list. Unset and empty fields filter nothing.
#[derive(Debug, Clone, Default)]
pub struct ByNameFilter {
    /// Only items of this library, or below this item.
    pub parent_id: Option<Uuid>,
    /// Keep only items of these types, by canonical `BaseItemKind` name, such as `Movie`.
    pub include_item_types: Vec<&'static str>,
    /// Drop items of these types, by canonical `BaseItemKind` name.
    pub exclude_item_types: Vec<&'static str>,
    /// Keep only items of these media types, by canonical media type name, such as `Video`.
    pub media_types: Vec<&'static str>,
    /// Keep names that contain this text.
    pub search_term: Option<String>,
    /// Keep names that start with this text.
    pub name_starts_with: Option<String>,
    /// Keep names that sort at or after this text.
    pub name_starts_with_or_greater: Option<String>,
    /// Keep names that sort before this text.
    pub name_less_than: Option<String>,
    /// Sort in reverse order.
    pub descending: bool,
    /// The number of entries to skip after sorting.
    pub start_index: usize,
    /// The number of entries to return; `None` returns all of them.
    pub limit: Option<usize>,
}

/// One page of a list.
#[derive(Debug, Clone, Default)]
pub struct ByNamePage {
    /// The entries of the page, in order.
    pub items: Vec<ByNameEntry>,
    /// The number of entries that pass the filters, before paging.
    pub total: usize,
}

/// Lists the entries of `kind` that pass `filter`, sorted and paged. An entry is named the same
/// in every list, whatever the filter.
pub async fn list(
    pool: &SqlitePool,
    kind: ByNameKind,
    filter: &ByNameFilter,
) -> Result<ByNamePage, sqlx::Error> {
    // Every row of the kind takes part in naming its entry, so a scope or type filter can't
    // change the name. Only the rows in scope that pass the type filters are counted, and an
    // entry without such a row is left out.
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for row in name_rows(pool, kind, filter.parent_id).await? {
        if !in_kind(kind, row.media) {
            continue;
        }
        let group = groups.entry(fold_name(&row.name)).or_default();
        if row.in_scope && passes_type_filters(filter, row.media) {
            group.items.insert(row.item, row.media);
        }
        *group.spellings.entry(row.name).or_default() += 1;
    }

    let mut entries: Vec<ByNameEntry> = groups
        .into_iter()
        .filter(|(folded, group)| !group.items.is_empty() && passes_name_filters(filter, folded))
        .map(|(_, group)| {
            let name = group.name();
            ByNameEntry {
                kind,
                id: by_name_id(kind, &name),
                counts: counts_of(&group.items),
                name,
            }
        })
        .collect();
    entries.sort_by_cached_key(|entry| order_key(kind, &entry.name));
    if filter.descending {
        entries.reverse();
    }

    let total = entries.len();
    let items = entries
        .into_iter()
        .skip(filter.start_index)
        .take(filter.limit.unwrap_or(usize::MAX))
        .collect();
    Ok(ByNamePage { items, total })
}

/// The entry of `kind` named `name`, or `None`. The name is folded before the lookup. When nothing
/// matches and the name has a `-`, that `-` may stand for `&`, `/` or `?`, as Jellyfin's slugs write it.
pub async fn get_by_name(
    pool: &SqlitePool,
    kind: ByNameKind,
    name: &str,
) -> Result<Option<ByNameEntry>, sqlx::Error> {
    // A year is its positive number, so `01999` finds 1999, while `0`, `-5` and `abc` find nothing.
    let folded = match kind {
        ByNameKind::Year => match name.trim().parse::<i64>() {
            Ok(year) if year > 0 => year.to_string(),
            _ => return Ok(None),
        },
        _ => fold_name(name),
    };
    let mut candidates = vec![folded.clone()];
    if folded.contains('-') {
        candidates.extend(["&", "/", "?"].map(|sep| folded.replace('-', sep)));
    }
    let page = list(pool, kind, &ByNameFilter::default()).await?;
    for candidate in candidates {
        if let Some(entry) = page
            .items
            .iter()
            .find(|entry| fold_name(&entry.name) == candidate)
        {
            return Ok(Some(entry.clone()));
        }
    }
    Ok(None)
}

/// The entry with `id` in any of the four lists, or `None`. The lists are read unscoped.
pub async fn get_by_id(pool: &SqlitePool, id: Uuid) -> Result<Option<ByNameEntry>, sqlx::Error> {
    let kinds = [
        ByNameKind::Genre,
        ByNameKind::MusicGenre,
        ByNameKind::Studio,
        ByNameKind::Year,
    ];
    for kind in kinds {
        let page = list(pool, kind, &ByNameFilter::default()).await?;
        if let Some(entry) = page.items.into_iter().find(|entry| entry.id == id) {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// The ids of the items that `/Items` keeps for `GenreIds` and `StudioIds`, or `None` when both
/// lists are empty. As in Jellyfin (`JellyfinQueryHelperExtensions.cs:92-111`), an item is kept when
/// one of its genres, or studios, folds like the name of an entry whose id is listed: the ids of one
/// list match any of them, both lists must match, and an id that names no entry matches nothing.
/// A genre id may be a genre's or a music genre's, since both name the same genre values.
pub async fn item_ids_with(
    pool: &SqlitePool,
    genre_ids: &[Uuid],
    studio_ids: &[Uuid],
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let mut kept: Option<HashSet<String>> = None;
    for (kind, ids) in [
        (ByNameKind::Genre, genre_ids),
        (ByNameKind::Studio, studio_ids),
    ] {
        if ids.is_empty() {
            continue;
        }
        let kinds: &[ByNameKind] = match kind {
            ByNameKind::Genre => &[ByNameKind::Genre, ByNameKind::MusicGenre],
            _ => &[kind],
        };
        let ids: HashSet<Uuid> = ids.iter().copied().collect();
        // Whether each folded name is listed, so that each name is hashed once.
        let mut listed: HashMap<String, bool> = HashMap::new();
        let mut items = HashSet::new();
        for row in name_rows(pool, kind, None).await? {
            let hit = *listed
                .entry(fold_name(&row.name))
                .or_insert_with_key(|folded| {
                    kinds
                        .iter()
                        .any(|&kind| ids.contains(&folded_id(kind, folded)))
                });
            if hit {
                items.insert(row.item);
            }
        }
        kept = Some(match kept {
            Some(kept) => kept.intersection(&items).cloned().collect(),
            None => items,
        });
    }
    Ok(kept.map(|items| items.into_iter().collect()))
}

/// The rows behind one entry: the number of rows with each spelling, among all its rows, and each
/// counted item with its type.
#[derive(Default)]
struct Group {
    spellings: BTreeMap<String, usize>,
    items: BTreeMap<String, MediaType>,
}

impl Group {
    /// The most common spelling; a tie goes to the smallest in code-point order ("DRAMA" before
    /// "Drama", "Acao" before "Ação").
    fn name(&self) -> String {
        self.spellings
            .iter()
            .max_by(|(a, a_rows), (b, b_rows)| a_rows.cmp(b_rows).then_with(|| b.cmp(a)))
            .map(|(spelling, _)| spelling.clone())
            .unwrap_or_default()
    }
}

/// One item and one of its names, as `name_rows` reads them.
struct NameRow {
    item: String,
    media: MediaType,
    name: String,
    /// Whether the item is in the scope of the list.
    in_scope: bool,
}

// The SQL below reads every item, so that the names don't depend on the scope. `?1` is the parent
// id, or NULL for every item, and the last column is 1 for an item in scope. Items below the
// parent come from the recursive `sub` table; the parent itself is never in scope. A value of
// another SQLite type leaves its row out through the `typeof` guards, so no row fails a query.

/// The genre rows: item id, media type, raw `genres` and scope of each item that has genres.
const GENRE_ROWS: &str = "WITH RECURSIVE sub(id) AS (
        SELECT id FROM media_items WHERE parent_id = ?1
        UNION SELECT c.id FROM media_items c JOIN sub ON c.parent_id = sub.id)
    SELECT m.id, m.media_type, m.genres,
        CASE WHEN ?1 IS NULL OR (m.id <> ?1 AND (m.library_id = ?1 OR m.id IN (SELECT id FROM sub)))
            THEN 1 ELSE 0 END
    FROM media_items m
    WHERE typeof(m.id) = 'text' AND typeof(m.media_type) = 'text' AND typeof(m.genres) = 'text'
        AND m.genres NOT IN ('', '[]')";

/// The studio rows: studio name, item id, media type and scope of each link of an item.
const STUDIO_ROWS: &str = "WITH RECURSIVE sub(id) AS (
        SELECT id FROM media_items WHERE parent_id = ?1
        UNION SELECT c.id FROM media_items c JOIN sub ON c.parent_id = sub.id)
    SELECT s.name, m.id, m.media_type,
        CASE WHEN ?1 IS NULL OR (m.id <> ?1 AND (m.library_id = ?1 OR m.id IN (SELECT id FROM sub)))
            THEN 1 ELSE 0 END
    FROM item_studios i
    JOIN studios s ON s.id = i.studio_id
    JOIN media_items m ON m.id = i.item_id
    WHERE typeof(s.name) = 'text' AND typeof(m.id) = 'text' AND typeof(m.media_type) = 'text'";

/// The year rows: item id, media type, production year and scope of each item with a year above 0.
const YEAR_ROWS: &str = "WITH RECURSIVE sub(id) AS (
        SELECT id FROM media_items WHERE parent_id = ?1
        UNION SELECT c.id FROM media_items c JOIN sub ON c.parent_id = sub.id)
    SELECT m.id, m.media_type, m.production_year,
        CASE WHEN ?1 IS NULL OR (m.id <> ?1 AND (m.library_id = ?1 OR m.id IN (SELECT id FROM sub)))
            THEN 1 ELSE 0 END
    FROM media_items m
    WHERE typeof(m.id) = 'text' AND typeof(m.media_type) = 'text'
      AND typeof(m.production_year) = 'integer' AND m.production_year > 0";

/// The rows that feed `kind`, one per item and name.
async fn name_rows(
    pool: &SqlitePool,
    kind: ByNameKind,
    parent_id: Option<Uuid>,
) -> Result<Vec<NameRow>, sqlx::Error> {
    let parent = parent_id.map(|id| id.to_string());
    let row = |item: String, media: &str, name: String, scope: i64| NameRow {
        item,
        media: stored_media_type(media),
        name,
        in_scope: scope != 0,
    };
    let mut rows = Vec::new();
    match kind {
        ByNameKind::Genre | ByNameKind::MusicGenre => {
            let found = sqlx::query_as::<_, (String, String, String, i64)>(GENRE_ROWS)
                .bind(parent)
                .fetch_all(pool)
                .await?;
            for (item, media, genres, scope) in found {
                for name in split_genres(&genres) {
                    rows.push(row(item.clone(), &media, name, scope));
                }
            }
        }
        ByNameKind::Studio => {
            let found = sqlx::query_as::<_, (String, String, String, i64)>(STUDIO_ROWS)
                .bind(parent)
                .fetch_all(pool)
                .await?;
            for (name, item, media, scope) in found {
                let name = name.trim();
                if !name.is_empty() {
                    rows.push(row(item, &media, name.to_string(), scope));
                }
            }
        }
        ByNameKind::Year => {
            let found = sqlx::query_as::<_, (String, String, i64, i64)>(YEAR_ROWS)
                .bind(parent)
                .fetch_all(pool)
                .await?;
            for (item, media, year, scope) in found {
                rows.push(row(item, &media, year.to_string(), scope));
            }
        }
    }
    Ok(rows)
}

/// The type of a stored `media_type`. The pre-014 JSON form, such as `"Movie"`, reads as `Movie`.
fn stored_media_type(stored: &str) -> MediaType {
    MediaType::from_str(stored.trim_matches('"')).unwrap_or(MediaType::Unknown)
}

/// Whether an item of type `media` feeds `kind`. Genres skip music, music genres take only audio and
/// albums, and studios and years take every item.
fn in_kind(kind: ByNameKind, media: MediaType) -> bool {
    match kind {
        ByNameKind::Genre | ByNameKind::MusicGenre => genre_kind_of(media) == kind,
        ByNameKind::Studio | ByNameKind::Year => true,
    }
}

/// Whether an item of type `media` passes the item type and media kind filters. The lists hold
/// canonical names, and an empty list filters nothing.
fn passes_type_filters(filter: &ByNameFilter, media: MediaType) -> bool {
    let item_kind = media.item_kind();
    (filter.include_item_types.is_empty() || filter.include_item_types.contains(&item_kind))
        && !filter.exclude_item_types.contains(&item_kind)
        && (filter.media_types.is_empty() || filter.media_types.contains(&media.media_kind()))
}

/// Whether a folded name passes the name filters. A blank filter value filters nothing.
fn passes_name_filters(filter: &ByNameFilter, folded: &str) -> bool {
    // The filter text, folded. `None` when the text is blank.
    let text = |value: &Option<String>| value.as_deref().map(fold_name).filter(|t| !t.is_empty());
    text(&filter.search_term).is_none_or(|t| folded.contains(t.as_str()))
        && text(&filter.name_starts_with).is_none_or(|t| folded.starts_with(t.as_str()))
        && text(&filter.name_starts_with_or_greater).is_none_or(|t| folded >= t.as_str())
        && text(&filter.name_less_than).is_none_or(|t| folded < t.as_str())
}

/// The sort key of an entry: years by their number, and the other kinds by folded name.
fn order_key(kind: ByNameKind, name: &str) -> (i64, String) {
    match kind {
        ByNameKind::Year => (name.parse().unwrap_or(0), String::new()),
        _ => (0, fold_name(name)),
    }
}

/// The counts of an entry from its items and their types.
fn counts_of(items: &BTreeMap<String, MediaType>) -> ByNameCounts {
    let mut counts = ByNameCounts {
        items: items.len() as i64,
        ..ByNameCounts::default()
    };
    for media in items.values() {
        match media {
            MediaType::Movie => counts.movies += 1,
            MediaType::Series => counts.series += 1,
            MediaType::Episode => counts.episodes += 1,
            MediaType::MusicAlbum => counts.albums += 1,
            MediaType::Audio => counts.songs += 1,
            _ => {}
        }
    }
    counts
}
