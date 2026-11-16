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
use rustyfin_model::parental::{AccessPolicy, ParentalPolicy};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// Jellyfin's `DefaultAuthorizationHandler` checks for every authenticated request: a user
/// without remote access is refused outside the local network, and a user who isn't an
/// administrator is refused outside their access schedule, in the server's local time. The API
/// key has no limits.
pub async fn check_access(state: &AppState, claims: &Claims, local: bool) -> Result<(), ApiError> {
    if claims.sub.is_nil() {
        return Ok(());
    }
    let policy = rustyfin_db::repo_parental::get_policy(&state.db_ro, claims.sub).await?;
    access_allowed(&policy.access, claims.is_admin, local)
}

/// Whether a user with this access policy may use the server now (see `check_access`).
pub fn access_allowed(access: &AccessPolicy, is_admin: bool, local: bool) -> Result<(), ApiError> {
    if !local && !access.enable_remote_access {
        return Err(ApiError::Forbidden(
            "Remote access is not allowed for this user.".into(),
        ));
    }
    if !is_admin && !access.schedule_allows_now() {
        return Err(ApiError::Forbidden(
            "User is not allowed access at this time.".into(),
        ));
    }
    Ok(())
}

/// The checks of a sign-in, before a token is issued: `access_allowed`, and Jellyfin's
/// `MaxActiveSessions`, counting the user's sessions on other devices than this one, which the
/// sign-in replaces.
pub async fn check_sign_in(
    state: &AppState,
    user: &rustyfin_model::user::User,
    device_id: &str,
    local: bool,
) -> Result<(), ApiError> {
    let access = rustyfin_db::repo_parental::get_policy(&state.db, user.id)
        .await?
        .access;
    access_allowed(&access, user.is_administrator, local)?;
    if access.max_active_sessions > 0 {
        let others = rustyfin_db::repo_session::session_device_ids(&state.db, user.id)
            .await?
            .into_iter()
            .filter(|d| d.is_empty() || d != device_id)
            .count();
        if others >= access.max_active_sessions as usize {
            return Err(ApiError::Forbidden(format!(
                "{} has reached the maximum of {} active sessions.",
                user.name, access.max_active_sessions
            )));
        }
    }
    Ok(())
}

/// Middleware for the stream, transcode and HLS routes: a user whose policy has no media
/// playback gets 403. PlaybackInfo passes, and answers Jellyfin's `NotAllowed` itself.
pub async fn require_playback(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if let Some(claims) = req.extensions().get::<Claims>()
        && !claims.sub.is_nil()
        && !req
            .uri()
            .path()
            .to_ascii_lowercase()
            .ends_with("/playbackinfo")
        && !rustyfin_db::repo_parental::get_policy(&state.db_ro, claims.sub)
            .await?
            .access
            .enable_media_playback
    {
        return Err(ApiError::Forbidden(
            "Media playback is not allowed for this user.".into(),
        ));
    }
    Ok(next.run(req).await)
}

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
