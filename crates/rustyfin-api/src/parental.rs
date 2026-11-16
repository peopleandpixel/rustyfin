//! Parental limits on the API: which items the caller may see.
//!
//! Every route that returns, counts or streams items asks here. Lists drop the hidden items (or
//! hand `HiddenItems::json` to the SQL that pages them, so counts stay right), and routes on one
//! item answer 404 for a hidden one, as for an item that doesn't exist, so a limited user learns
//! nothing about it. The API key (the nil user id) has no limits.

use std::collections::HashSet;

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{RawPathParams, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use rustyfin_model::auth::Claims;
use rustyfin_model::parental::ParentalPolicy;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// The caller's parental policy, or `None` when it hides nothing.
pub async fn policy(state: &AppState, claims: &Claims) -> Result<Option<ParentalPolicy>, ApiError> {
    if claims.sub.is_nil() {
        return Ok(None);
    }
    let policy = rustyfin_db::repo_parental::get_policy(&state.db_ro, claims.sub).await?;
    Ok(policy.is_restricted().then_some(policy))
}

/// The items hidden from the caller. Empty for a caller without limits.
#[derive(Debug, Default)]
pub struct HiddenItems {
    /// The ids as stored, for SQL to compare.
    stored: HashSet<String>,
    ids: HashSet<Uuid>,
    restricted: bool,
}

impl HiddenItems {
    pub async fn for_caller(state: &AppState, claims: &Claims) -> Result<Self, ApiError> {
        match policy(state, claims).await? {
            None => Ok(Self::default()),
            Some(policy) => {
                let stored =
                    rustyfin_db::repo_parental::hidden_item_ids(&state.db_ro, &policy).await?;
                let ids = stored
                    .iter()
                    .filter_map(|id| Uuid::parse_str(id).ok())
                    .collect();
                Ok(Self {
                    stored,
                    ids,
                    restricted: true,
                })
            }
        }
    }

    /// The hidden ids as stored.
    pub fn stored_ids(&self) -> &HashSet<String> {
        &self.stored
    }

    /// True when the caller has limits, so answers shared between users (caches) don't fit.
    pub fn is_restricted(&self) -> bool {
        self.restricted
    }

    /// The hidden item ids.
    pub fn ids(&self) -> &HashSet<Uuid> {
        &self.ids
    }

    pub fn hides(&self, id: Uuid) -> bool {
        self.ids.contains(&id)
    }

    /// Drops the hidden entries of a list.
    pub fn retain<T>(&self, items: &mut Vec<T>, id: impl Fn(&T) -> Uuid) {
        if !self.ids.is_empty() {
            items.retain(|item| !self.hides(id(item)));
        }
    }

    /// The hidden ids as a JSON array for SQL's `json_each`, or `None` when nothing is hidden.
    pub fn json(&self) -> Option<String> {
        (!self.stored.is_empty()).then(|| {
            serde_json::Value::from(self.stored.iter().cloned().collect::<Vec<_>>()).to_string()
        })
    }
}

/// The path parameters that name an item.
const ITEM_PARAMS: [&str; 4] = ["id", "itemId", "item_id", "seriesId"];

/// Middleware for the authenticated routes, inside `require_auth`: a route whose path names an
/// item answers 404 when the caller's limits hide that item. This covers every route on one item
/// (details, images, PlaybackInfo, streams, HLS, subtitles, trickplay, segments, similar items,
/// children, a series' seasons and episodes), present and future. A path id that isn't a media
/// item (a user, playlist or collection) passes.
pub async fn item_gate(
    State(state): State<AppState>,
    params: Result<RawPathParams, RawPathParamsRejection>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if let (Ok(params), Some(claims)) = (params, req.extensions().get::<Claims>()) {
        for (key, value) in &params {
            if ITEM_PARAMS.contains(&key)
                && let Ok(id) = Uuid::parse_str(value)
            {
                ensure_visible(&state, claims, id).await?;
            }
        }
    }
    Ok(next.run(req).await)
}

/// Answers 404 when the caller's limits hide the item, as for an item that doesn't exist.
pub async fn ensure_visible(
    state: &AppState,
    claims: &Claims,
    item_id: Uuid,
) -> Result<(), ApiError> {
    if let Some(policy) = policy(state, claims).await?
        && rustyfin_db::repo_parental::is_item_hidden(&state.db_ro, &policy, item_id).await?
    {
        return Err(ApiError::NotFound("Item not found".into()));
    }
    Ok(())
}
