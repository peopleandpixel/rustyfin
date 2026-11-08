use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use std::collections::{BTreeSet, HashMap};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;
use uuid::Uuid;

use rustyfin_db::repo_search::{HintAncestor, HintCandidate};
use rustyfin_model::auth::Claims;
use rustyfin_model::image::{ImageType, ItemImage};
use rustyfin_model::media::{MediaItem, QueryResult};
use rustyfin_model::search::{SearchHint, SearchHintResult};

use crate::error::ApiError;
use crate::query;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Search/Hints", get(search_hints))
        .route("/Search", get(full_search))
}

/// The key of a `/Search` answer. It holds every input that `full_search` applies: the trimmed
/// term, the item kinds as a sorted set, the start index and the limit. Nothing else is applied,
/// and nothing depends on the user, so a parameter that did would need the user in this key. The
/// term comes last, so that no term can make one key look like another. The `search3:` tag keeps
/// the rows that L2 saved under `search2:` (kinds parsed the old way, term not trimmed) from ever
/// matching.
fn full_search_cache_key(term: &str, kinds: &[&str], start_index: usize, limit: usize) -> String {
    format!("search3:{}:{start_index}:{limit}:{term}", kinds.join(","))
}

/// A parsed `/Search/Hints` request. Its lists are sorted sets, so the order and the case of the
/// names in the URL change neither the filters nor the cache key.
struct HintsRequest {
    term: String,
    start_index: usize,
    limit: usize,
    user: Option<Uuid>,
    parent: Option<Uuid>,
    include_item_types: Vec<&'static str>,
    exclude_item_types: Vec<&'static str>,
    media_types: Vec<&'static str>,
    is_movie: Option<bool>,
    is_series: Option<bool>,
    is_news: Option<bool>,
    is_kids: Option<bool>,
    is_sports: Option<bool>,
    flags: IncludeFlags,
}

/// The `include*` flags. A flag that is false leaves its kinds out of the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IncludeFlags {
    people: bool,
    media: bool,
    genres: bool,
    studios: bool,
    artists: bool,
}

/// The kinds that a hints search may return, from `SearchManager.cs:387-449`.
struct KindFilter {
    include: Vec<&'static str>,
    exclude: Vec<&'static str>,
}

impl KindFilter {
    fn new(include: &[&'static str], exclude: &[&'static str], flags: IncludeFlags) -> Self {
        let mut exclude = exclude.to_vec();
        exclude.extend(["CollectionFolder", "Folder", "Year"]);
        if !flags.genres {
            exclude.extend(["Genre", "MusicGenre"]);
        }
        if !flags.people {
            exclude.push("Person");
        }
        if !flags.studios {
            exclude.push("Studio");
        }
        if !flags.artists {
            exclude.push("MusicArtist");
        }
        let mut include = include.to_vec();
        if !flags.media {
            add_group(&mut include, flags.genres, &["Genre", "MusicGenre"]);
            add_group(&mut include, flags.people, &["Person"]);
            add_group(&mut include, flags.studios, &["Studio"]);
            add_group(&mut include, flags.artists, &["MusicArtist"]);
        }
        Self {
            include: sorted_set(include),
            exclude: sorted_set(exclude),
        }
    }

    /// An include list that holds only excluded kinds passes nothing, as Jellyfin's final query does.
    fn passes(&self, kind: &str) -> bool {
        (self.include.is_empty() || self.include.contains(&kind)) && !self.exclude.contains(&kind)
    }
}

impl HintsRequest {
    /// Reads the parameters with their defaults and the 400 messages of `crate::query`.
    fn parse(params: &[(String, String)]) -> Result<Self, ApiError> {
        let term = query::first_value(params, "searchTerm")
            .ok_or_else(|| ApiError::BadRequest("searchTerm is required".to_string()))?;
        Ok(Self {
            term: term.to_string(),
            start_index: query::count_param(params, "startIndex")?.unwrap_or(0),
            limit: query::count_param(params, "limit")?.unwrap_or(100),
            user: query::uuid_param(params, "userId")?.filter(|id| !id.is_nil()),
            parent: query::parent_param(params, "parentId")?,
            include_item_types: sorted_set(query::item_kinds(params, "includeItemTypes")),
            exclude_item_types: sorted_set(query::item_kinds(params, "excludeItemTypes")),
            media_types: sorted_set(query::media_types(params, "mediaTypes")),
            is_movie: query::bool_param(params, "isMovie")?,
            is_series: query::bool_param(params, "isSeries")?,
            is_news: query::bool_param(params, "isNews")?,
            is_kids: query::bool_param(params, "isKids")?,
            is_sports: query::bool_param(params, "isSports")?,
            flags: IncludeFlags {
                people: query::bool_param(params, "includePeople")?.unwrap_or(true),
                media: query::bool_param(params, "includeMedia")?.unwrap_or(true),
                genres: query::bool_param(params, "includeGenres")?.unwrap_or(true),
                studios: query::bool_param(params, "includeStudios")?.unwrap_or(true),
                artists: query::bool_param(params, "includeArtists")?.unwrap_or(true),
            },
        })
    }

    /// The cache key. It holds the sets, the parent, the flags, the paging and the trimmed term,
    /// which comes last so that no term can make one key look like another. The user is not part
    /// of it: the user check runs before the cache.
    fn cache_key(&self) -> String {
        let tri = |flag: Option<bool>| match flag {
            None => '-',
            Some(true) => 't',
            Some(false) => 'f',
        };
        let bit = |flag: bool| if flag { 't' } else { 'f' };
        let flags: String = [
            self.is_movie,
            self.is_series,
            self.is_news,
            self.is_kids,
            self.is_sports,
        ]
        .into_iter()
        .map(tri)
        .chain(
            [
                self.flags.people,
                self.flags.media,
                self.flags.genres,
                self.flags.studios,
                self.flags.artists,
            ]
            .into_iter()
            .map(bit),
        )
        .collect();
        let parent = self
            .parent
            .map_or_else(|| "-".to_string(), |id| id.to_string());
        format!(
            "hints3:{}:{}:{}:{parent}:{flags}:{}:{}:{}",
            self.include_item_types.join(","),
            self.exclude_item_types.join(","),
            self.media_types.join(","),
            self.start_index,
            self.limit,
            self.term,
        )
    }

    /// The row filters that run after `KindFilter`: the media types, `isMovie`, `isSeries` and the
    /// tag flags. Library items are never Live TV programs, so `isMovie=true` keeps them only when
    /// the include list is empty or names Movie or Trailer, and `isSeries=true` keeps none.
    fn keeps(&self, filter: &KindFilter, kind: &str, tags: &[String]) -> bool {
        if !filter.passes(kind) {
            return false;
        }
        let media = query::media_type_of(kind);
        if !self.media_types.is_empty() && !self.media_types.contains(&media) {
            return false;
        }
        let names_movies = filter.include.is_empty()
            || filter
                .include
                .iter()
                .any(|name| matches!(*name, "Movie" | "Trailer"));
        if self.is_movie == Some(true) && !names_movies {
            return false;
        }
        if self.is_series == Some(true) {
            return false;
        }
        // A tag flag that is true needs one of its tags; one that is false needs none of them.
        let flag_tags = [
            (self.is_news, "News"),
            (self.is_kids, "Kids"),
            (self.is_sports, "Sports"),
        ];
        let named = |wanted: bool| -> Vec<String> {
            flag_tags
                .iter()
                .filter(|(flag, _)| *flag == Some(wanted))
                .map(|(_, tag)| clean_value(tag))
                .collect()
        };
        let (wanted, unwanted) = (named(true), named(false));
        if wanted.is_empty() && unwanted.is_empty() {
            return true;
        }
        let row: Vec<String> = tags.iter().map(|tag| clean_value(tag)).collect();
        if !wanted.is_empty() && !row.iter().any(|tag| wanted.contains(tag)) {
            return false;
        }
        !row.iter().any(|tag| unwanted.contains(tag))
    }
}

fn sorted_set<T: Ord>(mut values: Vec<T>) -> Vec<T> {
    values.sort_unstable();
    values.dedup();
    values
}

/// Adds the kinds of a group when the group is enabled and the include list is empty or already
/// holds one of them, so that a list of other kinds is left as it is.
fn add_group(include: &mut Vec<&'static str>, enabled: bool, members: &[&'static str]) {
    let open = include.is_empty() || members.iter().any(|member| include.contains(member));
    if enabled && open {
        include.extend_from_slice(members);
    }
}

/// The canonical `BaseItemKind` name of a stored `media_type`. A stored name that is not one is
/// treated as a folder, so the default filters leave it out.
fn stored_kind(stored: &str) -> &'static str {
    query::ITEM_KINDS
        .into_iter()
        .find(|kind| *kind == stored)
        .unwrap_or("Folder")
}

/// Jellyfin's clean value: NFD with the combining marks dropped, every character that is neither
/// alphanumeric nor whitespace read as a space, whitespace runs as one space, lowercase, trimmed.
fn clean_value(value: &str) -> String {
    let spaced: String = value
        .nfd()
        .filter(|ch| !is_combining_mark(*ch))
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect();
    spaced
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .to_lowercase()
}

/// SqlSearchProvider's score of a name for a clean term: 100 for the same clean value, 80 for a
/// prefix, 75 for the term followed by a space inside the name, and 50 otherwise.
fn match_score(clean_term: &str, name: &str) -> u8 {
    let clean_name = clean_value(name);
    if clean_name == clean_term {
        100
    } else if clean_name.starts_with(clean_term) {
        80
    } else if clean_name.contains(&format!("{clean_term} ")) {
        75
    } else {
        50
    }
}

/// Serializes a `/Search/Hints` answer once. The reply, the cache and L2 all hold this value.
fn hints_answer(result: &SearchHintResult) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(result)
        .map_err(|e| ApiError::Internal(format!("could not serialize the hints answer: {e}")))
}

/// The Jellyfin `Folder` subclasses. `IsFolder` is true for these kinds and absent for the rest.
const FOLDER_KINDS: [&str; 16] = [
    "AggregateFolder",
    "BasePluginFolder",
    "BoxSet",
    "Channel",
    "ChannelFolderItem",
    "CollectionFolder",
    "Folder",
    "ManualPlaylistsFolder",
    "MusicAlbum",
    "PhotoAlbum",
    "Playlist",
    "PlaylistsFolder",
    "Season",
    "Series",
    "UserRootFolder",
    "UserView",
];

/// The image of `image_type` with the lowest index that `item` has.
fn lowest_image(
    images: &HashMap<String, Vec<ItemImage>>,
    item: Uuid,
    image_type: ImageType,
) -> Option<&ItemImage> {
    images
        .get(&item.to_string())?
        .iter()
        .filter(|image| image.image_type == image_type)
        .min_by_key(|image| image.image_index)
}

/// The item that a hint takes an image of `image_type` from, with that image: `item` itself when
/// it has one, else its ancestors nearest first. With `series_first`, the nearest Series ancestor
/// with one comes before the other ancestors, as Jellyfin's thumb of an episode does.
fn image_owner<'a>(
    images: &'a HashMap<String, Vec<ItemImage>>,
    item: Uuid,
    ancestors: &[HintAncestor],
    image_type: ImageType,
    series_first: bool,
) -> Option<(Uuid, &'a ItemImage)> {
    let owner = |id: Uuid| lowest_image(images, id, image_type).map(|image| (id, image));
    if let Some(found) = owner(item) {
        return Some(found);
    }
    if series_first {
        let series = ancestors
            .iter()
            .filter(|ancestor| stored_kind(&ancestor.kind) == "Series")
            .find_map(|ancestor| owner(ancestor.id));
        if series.is_some() {
            return series;
        }
    }
    ancestors.iter().find_map(|ancestor| owner(ancestor.id))
}

/// The ancestor `level` steps up (0 is the parent), if it is a MusicAlbum.
fn music_album(ancestors: &[HintAncestor], level: usize) -> Option<&HintAncestor> {
    ancestors
        .get(level)
        .filter(|ancestor| stored_kind(&ancestor.kind) == "MusicAlbum")
}

/// The tag of an image: its type and its index, as Jellyfin's image tags are formed.
fn image_tag(image: &ItemImage) -> String {
    format!("{}_{}", image.image_type.as_str(), image.image_index)
}

/// The width over the height of an image, when both are known and above zero.
fn aspect_ratio(image: &ItemImage) -> Option<f64> {
    match (image.width, image.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            Some(f64::from(width) / f64::from(height))
        }
        _ => None,
    }
}

/// One hint of a page, from its candidate, its ancestors nearest first, and the images of the page
/// and of those ancestors. A field the item has no data for is left out of the JSON.
fn page_hint(
    candidate: HintCandidate,
    ancestors: &[HintAncestor],
    images: &HashMap<String, Vec<ItemImage>>,
) -> SearchHint {
    let id = candidate.id;
    let kind = stored_kind(&candidate.kind);
    let primary = lowest_image(images, id, ImageType::Primary);
    let thumb = image_owner(images, id, ancestors, ImageType::Thumb, kind == "Episode");
    let backdrop = image_owner(images, id, ancestors, ImageType::Backdrop, false);
    let series = match kind {
        "Episode" | "Season" => ancestors
            .iter()
            .find(|ancestor| stored_kind(&ancestor.kind) == "Series")
            .map(|ancestor| ancestor.name.clone()),
        _ => None,
    };
    let status = match kind {
        "Series" => candidate.status.filter(|text| !text.is_empty()),
        _ => None,
    };
    // A track names its album (the parent) and the album's artist (the grandparent, which scans
    // store as a MusicAlbum). A MusicAlbum under a MusicAlbum names its parent as its artist.
    let (album, artist) = match kind {
        "Audio" => (music_album(ancestors, 0), music_album(ancestors, 1)),
        "MusicAlbum" => (None, music_album(ancestors, 0)),
        _ => (None, None),
    };
    SearchHint {
        item_id: id,
        id,
        name: candidate.name,
        index_number: candidate.index_number,
        production_year: candidate.production_year,
        parent_index_number: candidate.parent_index_number,
        primary_image_tag: primary.map(image_tag),
        primary_image_aspect_ratio: primary.and_then(aspect_ratio),
        thumb_image_tag: thumb.map(|(_, image)| image_tag(image)),
        thumb_image_item_id: thumb.map(|(owner, _)| owner.simple().to_string()),
        backdrop_image_tag: backdrop.map(|(_, image)| image_tag(image)),
        backdrop_image_item_id: backdrop.map(|(owner, _)| owner.simple().to_string()),
        item_type: kind.to_string(),
        is_folder: FOLDER_KINDS.contains(&kind).then_some(true),
        run_time_ticks: candidate.runtime_ticks,
        media_type: query::media_type_of(kind).to_string(),
        end_date: candidate.end_date,
        series,
        status,
        album: album.map(|album| album.name.clone()),
        album_id: album.map(|album| album.id),
        album_artist: artist.map(|artist| artist.name.clone()),
        artists: artist
            .map(|artist| vec![artist.name.clone()])
            .unwrap_or_default(),
    }
}

/// The hints of one page. The page's ancestors are read in one query, then the images of its items
/// and of their ancestors, in chunks of 500 ids. An empty page reads nothing.
async fn page_hints(
    state: &AppState,
    page: Vec<HintCandidate>,
) -> Result<Vec<SearchHint>, ApiError> {
    if page.is_empty() {
        return Ok(Vec::new());
    }
    let pool = &state.db_ro;
    let ids: Vec<Uuid> = page.iter().map(|candidate| candidate.id).collect();
    let ancestors = rustyfin_db::repo_search::hint_ancestors(pool, &ids).await?;
    let image_ids: BTreeSet<Uuid> = ids
        .iter()
        .copied()
        .chain(ancestors.values().flatten().map(|ancestor| ancestor.id))
        .collect();
    let image_ids: Vec<Uuid> = image_ids.into_iter().collect();
    let mut images: HashMap<String, Vec<ItemImage>> = HashMap::new();
    for chunk in image_ids.chunks(500) {
        images.extend(rustyfin_db::repo_image::get_images_batch(pool, chunk).await?);
    }
    Ok(page
        .into_iter()
        .map(|candidate| {
            let item_ancestors = ancestors.get(&candidate.id).map_or(&[][..], Vec::as_slice);
            page_hint(candidate, item_ancestors, &images)
        })
        .collect())
}

/// Answers `/Search/Hints`: the user check, the cache, then the search. The answer is serialized
/// once, and the reply, the cache and L2 all hold that value.
async fn search_hints(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let request = HintsRequest::parse(&params)?;
    // Before the cache, so that a cached answer is never handed to a user it was not made for.
    if let Some(user) = request.user
        && user != claims.sub
        && !claims.is_admin
    {
        return Err(ApiError::Forbidden(
            "Only an administrator can search hints for another user.".to_string(),
        ));
    }

    let cache_key = request.cache_key();
    if let Some(cached) = state.cache.search_results.get(&cache_key) {
        return Ok(Json(cached));
    }

    // A term with no letters or digits (`"`, `*`) matches nothing. That answer is not cached.
    let clean_term = clean_value(&request.term);
    if clean_term.is_empty() {
        let empty = SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        };
        return Ok(Json(hints_answer(&empty)?));
    }

    let filter = KindFilter::new(
        &request.include_item_types,
        &request.exclude_item_types,
        request.flags,
    );
    let candidates =
        rustyfin_db::repo_search::hint_candidates(&state.db_ro, &request.term, request.parent)
            .await?;
    let mut hits: Vec<(u8, HintCandidate)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let kind = stored_kind(&candidate.kind);
            if !request.keeps(&filter, kind, &candidate.tags) {
                return None;
            }
            Some((match_score(&clean_term, &candidate.name), candidate))
        })
        .collect();
    // SqlSearchProvider's order: the score descending, then the id ascending.
    hits.sort_unstable_by(|(score_a, a), (score_b, b)| {
        score_b.cmp(score_a).then_with(|| a.id.cmp(&b.id))
    });
    let total_record_count = hits.len() as i64;
    let page: Vec<HintCandidate> = hits
        .into_iter()
        .skip(request.start_index)
        .take(request.limit)
        .map(|(_, candidate)| candidate)
        .collect();
    let search_hints = page_hints(&state, page).await?;

    let value = hints_answer(&SearchHintResult {
        search_hints,
        total_record_count,
    })?;
    state
        .cache
        .search_results
        .insert(cache_key.clone(), value.clone());
    // Persist to L2 cache for survival across restarts (fire-and-forget)
    let cache = state.cache.clone();
    let l2_value = value.clone();
    tokio::spawn(async move {
        cache.save_search_to_l2(&cache_key, &l2_value).await;
    });
    Ok(Json(value))
}

/// Serializes a `/Search` answer once. The reply, the cache and L2 all hold this value, so a
/// repeated search gets back exactly what the first one did.
fn search_answer(result: &QueryResult) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(result)
        .map_err(|e| ApiError::Internal(format!("could not serialize the search answer: {e}")))
}

/// The item kinds that `/Search` looks in: the `IncludeItemTypes` names that `item_kinds` reads
/// with Jellyfin's binder rules, sorted, each once. Folder, Year and CollectionFolder are never
/// searched. A list that parses to nothing is a missing list, which searches movies, series,
/// episodes and audio.
pub(crate) fn search_kinds(params: &[(String, String)]) -> Vec<&'static str> {
    let kinds: Vec<&'static str> = query::item_kinds(params, "IncludeItemTypes")
        .into_iter()
        .filter(|kind| !matches!(*kind, "Folder" | "Year" | "CollectionFolder"))
        .collect();
    let kinds = sorted_set(kinds);
    if kinds.is_empty() {
        vec!["Audio", "Episode", "Movie", "Series"]
    } else {
        kinds
    }
}

/// Answers `/Search`. The counts are read before the term, so a bad count is a 400 even when the
/// term is blank. A blank term gets the empty answer, which is not cached.
async fn full_search(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let start = query::count_param(&params, "StartIndex")?.unwrap_or(0);
    let limit = query::count_param(&params, "Limit")?.unwrap_or(100);

    let Some(term) =
        query::first_value(&params, "Term").or_else(|| query::first_value(&params, "SearchTerm"))
    else {
        let empty = QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: start as i64,
        };
        return Ok(Json(search_answer(&empty)?));
    };

    let kinds = search_kinds(&params);

    // Check cache first. The cached value is the reply as it was first serialized.
    let cache_key = full_search_cache_key(term, &kinds, start, limit);
    if let Some(cached) = state.cache.search_results.get(&cache_key) {
        return Ok(Json(cached));
    }

    // The first `start + limit` matches; the page is cut from them below.
    let fetch = i64::try_from(start.saturating_add(limit)).unwrap_or(i64::MAX);
    let items = rustyfin_db::repo_search::search_items(&state.db_ro, term, &kinds, fetch).await?;

    let total = items.len() as i64;
    let page: Vec<MediaItem> = items.into_iter().skip(start).take(limit).collect();

    let result = QueryResult {
        items: page,
        total_record_count: total,
        start_index: start as i64,
    };
    let value = search_answer(&result)?;
    // Cache the result
    state
        .cache
        .search_results
        .insert(cache_key.clone(), value.clone());
    // Persist to L2 cache for survival across restarts (fire-and-forget)
    let cache = state.cache.clone();
    let key = cache_key.clone();
    let l2_value = value.clone();
    tokio::spawn(async move {
        cache.save_search_to_l2(&key, &l2_value).await;
    });
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    // B27: cache keys

    #[test]
    fn b27_each_named_type_is_searched_once_in_any_case() {
        assert_eq!(
            search_kinds(&j6_params(&[(
                "IncludeItemTypes",
                "Series,movie, MOVIE ,Movie,Bogus"
            )])),
            ["Movie", "Series"]
        );
    }

    /// More names than SQLite's 32,766 variables, so binding each name made the query fail.
    #[test]
    fn b27_forty_thousand_movie_names_are_one_type() {
        let names = "Movie,".repeat(40_000);
        assert_eq!(
            search_kinds(&j6_params(&[("IncludeItemTypes", names.as_str())])),
            ["Movie"]
        );
    }

    #[test]
    fn b27_without_names_search_looks_in_movies_series_episodes_and_audio() {
        assert_eq!(search_kinds(&[]), ["Audio", "Episode", "Movie", "Series"]);
    }

    // J6: search parity

    const J6_ALL: IncludeFlags = IncludeFlags {
        people: true,
        media: true,
        genres: true,
        studios: true,
        artists: true,
    };

    fn j6_params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    fn j6_key(pairs: &[(&str, &str)]) -> String {
        match HintsRequest::parse(&j6_params(pairs)) {
            Ok(request) => request.cache_key(),
            Err(err) => panic!("{pairs:?}: {err:?}"),
        }
    }

    /// The 400 message `HintsRequest::parse` answers `pairs` with.
    fn j6_rejection(pairs: &[(&str, &str)]) -> String {
        match HintsRequest::parse(&j6_params(pairs)) {
            Err(ApiError::BadRequest(message)) => message,
            Err(err) => panic!("{pairs:?}: {err:?}"),
            Ok(_) => panic!("{pairs:?} parsed"),
        }
    }

    fn j6_included(include: &[&'static str], flags: IncludeFlags) -> Vec<&'static str> {
        KindFilter::new(include, &[], flags).include
    }

    fn j6_excluded(exclude: &[&'static str], flags: IncludeFlags) -> Vec<&'static str> {
        KindFilter::new(&[], exclude, flags).exclude
    }

    #[test]
    fn j6_hints_always_leave_out_years_folders_and_collection_folders() {
        let filter = KindFilter::new(&[], &[], J6_ALL);
        assert!(filter.include.is_empty());
        assert_eq!(filter.exclude, ["CollectionFolder", "Folder", "Year"]);
        for kind in [
            "Movie",
            "Series",
            "Season",
            "Episode",
            "Audio",
            "MusicAlbum",
            "Photo",
            "Genre",
            "MusicGenre",
            "Person",
            "Studio",
            "MusicArtist",
            "BoxSet",
        ] {
            assert!(filter.passes(kind), "{kind}");
        }
        for kind in ["CollectionFolder", "Folder", "Year"] {
            assert!(!filter.passes(kind), "{kind}");
        }
        // Naming them does not bring them back.
        let named = KindFilter::new(&["Folder", "Movie", "Year"], &[], J6_ALL);
        assert!(named.passes("Movie"));
        for kind in ["Folder", "Year", "Series"] {
            assert!(!named.passes(kind), "{kind}");
        }
    }

    #[test]
    fn j6_hints_include_flags_exclude_their_kinds() {
        assert_eq!(
            j6_excluded(
                &[],
                IncludeFlags {
                    genres: false,
                    ..J6_ALL
                }
            ),
            ["CollectionFolder", "Folder", "Genre", "MusicGenre", "Year"]
        );
        assert_eq!(
            j6_excluded(
                &[],
                IncludeFlags {
                    people: false,
                    ..J6_ALL
                }
            ),
            ["CollectionFolder", "Folder", "Person", "Year"]
        );
        assert_eq!(
            j6_excluded(
                &[],
                IncludeFlags {
                    studios: false,
                    ..J6_ALL
                }
            ),
            ["CollectionFolder", "Folder", "Studio", "Year"]
        );
        assert_eq!(
            j6_excluded(
                &[],
                IncludeFlags {
                    artists: false,
                    ..J6_ALL
                }
            ),
            ["CollectionFolder", "Folder", "MusicArtist", "Year"]
        );
        // includeMedia=false narrows the include list instead.
        assert_eq!(
            j6_excluded(
                &[],
                IncludeFlags {
                    media: false,
                    ..J6_ALL
                }
            ),
            ["CollectionFolder", "Folder", "Year"]
        );
        // Given kinds join the defaults, once each, sorted.
        let given = KindFilter::new(&[], &["Movie", "Year", "Audio", "Movie"], J6_ALL);
        assert_eq!(
            given.exclude,
            ["Audio", "CollectionFolder", "Folder", "Movie", "Year"]
        );
        assert!(!given.passes("Movie"));
        assert!(!given.passes("Audio"));
        assert!(given.passes("Series"));
    }

    #[test]
    fn j6_hints_without_media_include_only_the_first_enabled_group() {
        let no_media = IncludeFlags {
            media: false,
            ..J6_ALL
        };
        assert_eq!(j6_included(&[], no_media), ["Genre", "MusicGenre"]);
        let no_genres = IncludeFlags {
            genres: false,
            ..no_media
        };
        assert_eq!(j6_included(&[], no_genres), ["Person"]);
        let no_people = IncludeFlags {
            people: false,
            ..no_genres
        };
        assert_eq!(j6_included(&[], no_people), ["Studio"]);
        let no_studios = IncludeFlags {
            studios: false,
            ..no_people
        };
        assert_eq!(j6_included(&[], no_studios), ["MusicArtist"]);
        // A given list gains only the groups it already names.
        assert_eq!(j6_included(&["Studio"], no_media), ["Studio"]);
        assert_eq!(
            j6_included(&["Person", "Genre"], no_media),
            ["Genre", "MusicGenre", "Person"]
        );
        assert_eq!(j6_included(&["Movie"], no_media), ["Movie"]);
        // With media on, the given list stands as it is.
        assert_eq!(
            j6_included(&["Series", "Movie"], J6_ALL),
            ["Movie", "Series"]
        );
        assert!(j6_included(&[], J6_ALL).is_empty());
        // Every group off: nothing is added, so any kind the excludes leave passes.
        let none = IncludeFlags {
            artists: false,
            ..no_studios
        };
        let filter = KindFilter::new(&[], &[], none);
        assert!(filter.include.is_empty());
        assert!(filter.passes("Movie"));
        for kind in ["Genre", "MusicGenre", "Person", "Studio", "MusicArtist"] {
            assert!(!filter.passes(kind), "{kind}");
        }
    }

    #[test]
    fn j6_hints_never_pass_an_excluded_kind() {
        // An include list of excluded kinds passes nothing, as Jellyfin's final query does.
        let folders = KindFilter::new(&["Folder"], &[], J6_ALL);
        for kind in ["Folder", "Movie", "Series", "Audio"] {
            assert!(!folders.passes(kind), "{kind}");
        }
        let both = KindFilter::new(&["Movie", "Series"], &["Movie"], J6_ALL);
        assert!(!both.passes("Movie"));
        assert!(both.passes("Series"));
        assert!(!both.passes("Episode"));
        let no_people = IncludeFlags {
            people: false,
            ..J6_ALL
        };
        let people = KindFilter::new(&["Person"], &[], no_people);
        assert!(!people.passes("Person"));
        assert!(!people.passes("Movie"));
    }

    #[test]
    fn j6_clean_values_follow_jellyfin() {
        for (value, clean) in [
            ("Comet", "comet"),
            ("Comet-Hunters!", "comet hunters"),
            ("  The   Night\tSky ", "the night sky"),
            ("AC/DC", "ac dc"),
            ("Amélie", "amelie"),
            ("Café: Ünïcode", "cafe unicode"),
            ("2001: A Space Odyssey", "2001 a space odyssey"),
            ("東京物語", "東京物語"),
            ("\"*", ""),
            ("", ""),
        ] {
            assert_eq!(clean_value(value), clean, "{value:?}");
        }
    }

    #[test]
    fn j6_scores_follow_sql_search_provider() {
        for (name, score) in [
            ("Hunters", 100),
            ("HUNTERS!", 100),
            ("Hunters of Comets", 80),
            ("Huntersville", 80),
            ("Comet Hunters Season 1", 75),
            ("Comet Hunters", 50),
            ("Night Sky", 50),
        ] {
            assert_eq!(match_score("hunters", name), score, "{name}");
        }
        assert_eq!(match_score("amelie", "Amélie"), 100);
        assert_eq!(match_score("comet hunters", "Comet-Hunters"), 100);
    }

    #[test]
    fn j6_hints_keys_hold_sorted_sets_and_the_trimmed_term_last() {
        let key = j6_key(&[
            ("searchTerm", " comet "),
            ("includeItemTypes", "Series,movie,Movie"),
            ("excludeItemTypes", "Episode, audio"),
            ("mediaTypes", "Video,Audio,video"),
            ("userId", "6f1b2c3d-1111-4222-8333-444455556666"),
        ]);
        assert_eq!(
            key,
            "hints3:Movie,Series:Audio,Episode:Audio,Video:-:-----ttttt:0:100:comet"
        );
        // Names in any case, the same sets in another order, and no user give the same key.
        let same = j6_key(&[
            ("SEARCHTERM", "comet"),
            ("MediaTypes", "audio,VIDEO"),
            ("ExcludeItemTypes", "Audio,Episode"),
            ("IncludeItemTypes", "Movie,Series"),
        ]);
        assert_eq!(same, key);
        // The parent is the hyphenated id however it was sent; flags are isMovie, isSeries,
        // isNews, isKids, isSports, then the five include flags.
        let key = j6_key(&[
            ("searchTerm", "Comet"),
            ("parentId", "0b5a3c8e2a4f4c1d9e7b6d5f4a3b2c1d"),
            ("startIndex", "5"),
            ("limit", "10"),
            ("isMovie", "TRUE"),
            ("isSeries", "false"),
            ("isKids", "true"),
            ("includePeople", "false"),
            ("includeArtists", "False"),
        ]);
        assert_eq!(
            key,
            "hints3::::0b5a3c8e-2a4f-4c1d-9e7b-6d5f4a3b2c1d:tf-t-ftttf:5:10:Comet"
        );
    }

    #[test]
    fn j6_hints_keys_differ_in_every_applied_parameter() {
        let variants: &[&[(&str, &str)]] = &[
            &[],
            &[("startIndex", "1")],
            &[("limit", "5")],
            &[("parentId", "0b5a3c8e-2a4f-4c1d-9e7b-6d5f4a3b2c1d")],
            &[("includeItemTypes", "Movie")],
            &[("excludeItemTypes", "Movie")],
            &[("mediaTypes", "Video")],
            &[("isMovie", "true")],
            &[("isMovie", "false")],
            &[("isSeries", "true")],
            &[("isSeries", "false")],
            &[("isNews", "true")],
            &[("isNews", "false")],
            &[("isKids", "true")],
            &[("isKids", "false")],
            &[("isSports", "true")],
            &[("isSports", "false")],
            &[("includePeople", "false")],
            &[("includeMedia", "false")],
            &[("includeGenres", "false")],
            &[("includeStudios", "false")],
            &[("includeArtists", "false")],
        ];
        let mut keys = std::collections::HashSet::new();
        for extra in variants {
            let mut pairs = vec![("searchTerm", "comet")];
            pairs.extend_from_slice(extra);
            let key = j6_key(&pairs);
            assert!(keys.insert(key.clone()), "{extra:?} shares {key}");
        }
        assert!(keys.insert(j6_key(&[("searchTerm", "comets")])));
        // An explicit default, a blank value or a list that parses to nothing is the same
        // request, and the user is not part of it.
        let plain = j6_key(&[("searchTerm", "comet")]);
        for (name, value) in [
            ("includePeople", "true"),
            ("includeMedia", "TRUE"),
            ("startIndex", "0"),
            ("limit", "100"),
            ("limit", ""),
            ("parentId", "00000000-0000-0000-0000-000000000000"),
            ("includeItemTypes", "Bogus"),
            ("excludeItemTypes", ""),
            ("mediaTypes", "Movie"),
            ("isKids", ""),
            ("userId", "6f1b2c3d-1111-4222-8333-444455556666"),
        ] {
            let key = j6_key(&[("searchTerm", "comet"), (name, value)]);
            assert_eq!(key, plain, "{name}={value}");
        }
    }

    #[test]
    fn j6_hints_parse_rejects_bad_input() {
        let missing: [&[(&str, &str)]; 4] = [
            &[],
            &[("searchTerm", "")],
            &[("SearchTerm", "   ")],
            &[("term", "comet")],
        ];
        for pairs in missing {
            assert_eq!(j6_rejection(pairs), "searchTerm is required", "{pairs:?}");
        }
        for (name, value, message) in [
            (
                "startIndex",
                "-1",
                "startIndex must be a non-negative integer",
            ),
            ("limit", "-5", "limit must be a non-negative integer"),
            ("limit", "2147483648", "limit must be at most 2147483647"),
            ("parentId", "bogus", "parentId must be a UUID"),
            ("userId", "12", "userId must be a UUID"),
            ("isKids", "yes", "isKids must be true or false"),
            ("includePeople", "0", "includePeople must be true or false"),
        ] {
            let pairs = [("searchTerm", "comet"), (name, value)];
            assert_eq!(j6_rejection(&pairs), message, "{name}={value}");
        }
    }

    #[test]
    fn j6_search_kinds_parse_every_stored_kind() {
        // Every kind Rustyfin stores, Unknown aside, in any case and with any name case.
        for (name, kind) in [
            ("movie", "Movie"),
            ("SERIES", "Series"),
            ("season", "Season"),
            ("Episode", "Episode"),
            ("audio", "Audio"),
            ("musicalbum", "MusicAlbum"),
            ("PHOTO", "Photo"),
            ("genre", "Genre"),
        ] {
            for key in ["IncludeItemTypes", "includeItemTypes", "INCLUDEITEMTYPES"] {
                assert_eq!(
                    search_kinds(&j6_params(&[(key, name)])),
                    [kind],
                    "{key}={name}"
                );
            }
        }
        // Any other BaseItemKind parses too; Rustyfin stores none of them, so it finds nothing.
        assert_eq!(
            search_kinds(&j6_params(&[("IncludeItemTypes", "boxset")])),
            ["BoxSet"]
        );
        // Folder, Year and CollectionFolder are never searched.
        assert_eq!(
            search_kinds(&j6_params(&[(
                "IncludeItemTypes",
                "Folder,Movie,Year,CollectionFolder"
            )])),
            ["Movie"]
        );
        // A list that names nothing else is a missing list.
        for list in [
            "",
            " , ",
            "Bogus",
            "Unknown",
            "1",
            "Folder",
            "Year,CollectionFolder",
            "Folder,Bogus",
        ] {
            assert_eq!(
                search_kinds(&j6_params(&[("IncludeItemTypes", list)])),
                ["Audio", "Episode", "Movie", "Series"],
                "{list:?}"
            );
        }
        // A repeated key is not split: each value is one entry.
        assert_eq!(
            search_kinds(&j6_params(&[
                ("IncludeItemTypes", "Series"),
                ("includeItemTypes", "Movie"),
            ])),
            ["Movie", "Series"]
        );
        assert_eq!(
            search_kinds(&j6_params(&[
                ("IncludeItemTypes", "Movie,Series"),
                ("IncludeItemTypes", "Audio"),
            ])),
            ["Audio"]
        );
    }
}
