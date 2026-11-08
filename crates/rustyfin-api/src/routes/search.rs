use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use rustyfin_model::search::SearchHintResult;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Search/Hints", get(search_hints))
        .route("/Search", get(full_search))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchQuery {
    pub search_term: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct FullSearchQuery {
    pub term: Option<String>,
    #[serde(default)]
    pub search_filter: Option<String>,
    #[serde(default)]
    pub include_item_types: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: i64,
    #[serde(default)]
    pub start_index: Option<i64>,
    #[serde(default)]
    pub genre_ids: Option<String>,
    #[serde(default)]
    pub artist_ids: Option<String>,
    #[serde(default)]
    pub album_artist_ids: Option<String>,
    #[serde(default)]
    pub album_ids: Option<String>,
    #[serde(default)]
    pub studio_ids: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub featured: Option<bool>,
    #[serde(default)]
    pub is_favorite: Option<bool>,
    #[serde(default)]
    pub is_unplayed: Option<bool>,
    #[serde(default)]
    pub is_played: Option<bool>,
    #[serde(default)]
    pub is_movie: Option<bool>,
    #[serde(default)]
    pub is_series: Option<bool>,
    #[serde(default)]
    pub is_episode: Option<bool>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
}

fn default_search_limit() -> i64 {
    100
}

/// The key of a `/Search` answer. It holds every input `full_search` applies: the term exactly as
/// sent, the parsed item types as a set (sorted, deduplicated `item_kind()` names), the start index
/// and the limit. `full_search` applies no other `FullSearchQuery` field (`IsFavorite`, `IsPlayed`,
/// ... are ignored) and nothing that depends on the user, so a filter that does needs the user in
/// this key. The `search2:` tag keeps rows that the L2 table saved under the old format from ever
/// matching, and the free-text term comes last so that no term can make one key look like another.
fn full_search_cache_key(
    term: &str,
    item_types: &[MediaType],
    start_index: usize,
    limit: usize,
) -> String {
    let mut kinds: Vec<&str> = item_types.iter().map(|t| t.item_kind()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let kinds = kinds.join(",");
    format!("search2:{kinds}:{start_index}:{limit}:{term}")
}

/// The key of a `/Search/Hints` answer. Its only inputs are the term exactly as sent and the limit,
/// since `SearchQuery` has no other fields. The `hints2:` tag keeps rows that the L2 table saved
/// under the old format from ever matching, and the free-text term comes last so that no term can
/// make one key look like another.
fn search_hints_cache_key(term: &str, limit: i64) -> String {
    format!("hints2:{limit}:{term}")
}

async fn search_hints(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchHintResult>, ApiError> {
    let term = query.search_term.unwrap_or_default();
    if term.is_empty() {
        return Ok(Json(SearchHintResult {
            search_hints: vec![],
            total_record_count: 0,
        }));
    }

    // Check cache first
    let cache_key = search_hints_cache_key(&term, query.limit);
    if let Some(cached) = state.cache.search_results.get(&cache_key)
        && let Ok(result) = serde_json::from_value(cached.clone())
    {
        return Ok(Json(result));
    }

    let hints = rustyfin_db::repo_search::search_hints(&state.db_ro, &term, query.limit).await?;
    let count = hints.len() as i64;
    let result = SearchHintResult {
        search_hints: hints,
        total_record_count: count,
    };
    // Cache the result
    if let Ok(json) = serde_json::to_value(&result) {
        state
            .cache
            .search_results
            .insert(cache_key.clone(), json.clone());
        // Persist to L2 cache for survival across restarts (fire-and-forget)
        let cache = state.cache.clone();
        let key = cache_key.clone();
        let value = json.clone();
        tokio::spawn(async move {
            cache.save_search_to_l2(&key, &value).await;
        });
    }
    Ok(Json(result))
}

/// Serializes a `/Search` answer once. The reply, the cache and L2 all hold this value, so a
/// repeated search gets back exactly what the first one did.
fn search_answer(result: &QueryResult) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(result)
        .map_err(|e| ApiError::Internal(format!("could not serialize the search answer: {e}")))
}

/// The item types that `/Search` looks in: those named in `IncludeItemTypes`, matched without
/// ASCII case and with spaces trimmed as Jellyfin's binder does, and each bound once however
/// often it is named, sorted by name. Unknown names are skipped. Without the parameter, movies,
/// series, episodes and audio.
fn parse_item_types(include_item_types: Option<&str>) -> Vec<MediaType> {
    if let Some(types) = include_item_types {
        let mut item_types: Vec<MediaType> = types
            .split(',')
            .filter_map(|t| {
                let t = t.trim();
                [
                    ("Movie", MediaType::Movie),
                    ("Series", MediaType::Series),
                    ("Episode", MediaType::Episode),
                    ("Audio", MediaType::Audio),
                    ("Photo", MediaType::Photo),
                ]
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(t))
                .map(|(_, kind)| kind)
            })
            .collect();
        item_types.sort_unstable_by_key(|t| t.item_kind());
        item_types.dedup_by_key(|t| t.item_kind());
        item_types
    } else {
        vec![
            MediaType::Movie,
            MediaType::Series,
            MediaType::Episode,
            MediaType::Audio,
        ]
    }
}

async fn full_search(
    State(state): State<AppState>,
    Query(query): Query<FullSearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let term = query.term.unwrap_or_default();
    if term.is_empty() {
        let empty = QueryResult {
            items: vec![],
            total_record_count: 0,
            start_index: query.start_index.unwrap_or(0),
        };
        return Ok(Json(search_answer(&empty)?));
    }

    let start_index = query.start_index.unwrap_or(0) as usize;
    let limit = query.limit as usize;

    let item_types = parse_item_types(query.include_item_types.as_deref());

    // Check cache first. The cached value is the reply as it was first serialized.
    let cache_key = full_search_cache_key(&term, &item_types, start_index, limit);
    if let Some(cached) = state.cache.search_results.get(&cache_key) {
        return Ok(Json(cached));
    }

    let items = rustyfin_db::repo_search::search_items(
        &state.db_ro,
        &term,
        &item_types,
        (start_index + limit) as i64,
    )
    .await?;

    let total = items.len() as i64;
    let end = start_index.saturating_add(limit).min(items.len());
    let page: Vec<MediaItem> = if start_index < items.len() {
        items[start_index..end].to_vec()
    } else {
        Vec::new()
    };

    let result = QueryResult {
        items: page,
        total_record_count: total,
        start_index: start_index as i64,
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
            parse_item_types(Some("Series,movie, MOVIE ,Movie,Bogus")),
            [MediaType::Movie, MediaType::Series]
        );
    }

    /// More names than SQLite's 32,766 variables, so binding each name made the query fail.
    #[test]
    fn b27_forty_thousand_movie_names_are_one_type() {
        let names = "Movie,".repeat(40_000);
        assert_eq!(parse_item_types(Some(&names)), [MediaType::Movie]);
    }

    #[test]
    fn b27_without_names_search_looks_in_movies_series_episodes_and_audio() {
        assert_eq!(
            parse_item_types(None),
            [
                MediaType::Movie,
                MediaType::Series,
                MediaType::Episode,
                MediaType::Audio
            ]
        );
    }
}
