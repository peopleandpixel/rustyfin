use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rustyfin_model::search::SearchHintResult;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/Search/Hints", get(search_hints))
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

    let hints = rustyfin_db::repo_search::search_hints(&state.db, &term, query.limit).await?;
    let count = hints.len() as i64;
    Ok(Json(SearchHintResult {
        search_hints: hints,
        total_record_count: count,
    }))
}
