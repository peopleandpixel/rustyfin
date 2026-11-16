//! Parental limits on the API: which items the caller may see.
//!
//! Every route that returns, counts or streams items asks here. Lists drop the hidden items (or
//! hand `HiddenItems::json` to the SQL that pages them, so counts stay right), and routes on one
//! item answer 404 for a hidden one, as for an item that doesn't exist, so a limited user learns
//! nothing about it. The API key (the nil user id) has no limits.
//!
//! Library access (`EnabledFolders`) is applied per library, not per item: `HiddenItems` carries
//! the allowed library ids beside the ids that ratings and tags hide, so a library of any size
//! costs the same. `require_auth` reads the caller's policy once and keeps it for the request.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{RawPathParams, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use rustyfin_model::auth::Claims;
use rustyfin_model::media::MediaItem;
use rustyfin_model::parental::{AccessPolicy, ParentalPolicy};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

tokio::task_local! {
    /// The caller's policy, read once by `require_auth` for the request it serves.
    static REQUEST_POLICY: (Uuid, Arc<ParentalPolicy>);
}

/// Runs `fut` (the rest of a request) with `policy` as `user`'s policy for `user_policy`.
pub async fn with_request_policy<F: std::future::Future>(
    user: Uuid,
    policy: Arc<ParentalPolicy>,
    fut: F,
) -> F::Output {
    REQUEST_POLICY.scope((user, policy), fut).await
}

/// `user`'s stored policy: the one this request read already, else from the database.
pub async fn user_policy(state: &AppState, user: Uuid) -> Result<Arc<ParentalPolicy>, ApiError> {
    if let Ok(Some(policy)) =
        REQUEST_POLICY.try_with(|(id, policy)| (*id == user).then(|| policy.clone()))
    {
        return Ok(policy);
    }
    Ok(Arc::new(
        rustyfin_db::repo_parental::get_policy(&state.db_ro, user).await?,
    ))
}

/// The caller's access policy; Jellyfin's defaults for the API key, which has no limits.
pub async fn caller_access(state: &AppState, claims: &Claims) -> Result<AccessPolicy, ApiError> {
    if claims.sub.is_nil() {
        return Ok(AccessPolicy::default());
    }
    Ok(user_policy(state, claims.sub).await?.access.clone())
}

/// Jellyfin's `DefaultAuthorizationHandler` checks for a request that `require_auth` doesn't
/// guard (the socket upgrade): a user without remote access is refused outside the local network,
/// and a user who isn't an administrator outside their access schedule. The API key has no limits.
pub async fn check_access(state: &AppState, claims: &Claims, local: bool) -> Result<(), ApiError> {
    if claims.sub.is_nil() {
        return Ok(());
    }
    let policy = rustyfin_db::repo_parental::get_policy(&state.db_ro, claims.sub).await?;
    access_allowed(&policy.access, claims.is_admin, local)
}

/// Whether a user with this access policy may use the server now: remote access, and the
/// access schedule in the server's local time for a user who isn't an administrator.
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
        && !req
            .uri()
            .path()
            .to_ascii_lowercase()
            .ends_with("/playbackinfo")
        && !caller_access(&state, claims).await?.enable_media_playback
    {
        return Err(ApiError::Forbidden(
            "Media playback is not allowed for this user.".into(),
        ));
    }
    Ok(next.run(req).await)
}

/// The guard of a stream the server converts (the HLS transcode, the only route that starts
/// one): with `copy_video` and `copy_audio` as decided for the request, a stream that re-encodes
/// needs the transcoding switch for the item's kind, and one that only copies (a remux) needs
/// `EnablePlaybackRemuxing`. Jellyfin applies the video switch to video items, the audio switch
/// to audio items.
pub fn check_conversion(
    access: &AccessPolicy,
    audio_item: bool,
    copy_video: bool,
    copy_audio: bool,
) -> Result<(), ApiError> {
    let (transcodes, allowed) = if audio_item {
        (!copy_audio, access.enable_audio_playback_transcoding)
    } else {
        (
            !(copy_video && copy_audio),
            access.enable_video_playback_transcoding,
        )
    };
    if transcodes && !allowed {
        return Err(ApiError::Forbidden(
            "Playback that requires transcoding is not allowed for this user.".into(),
        ));
    }
    if !transcodes && !access.enable_playback_remuxing {
        return Err(ApiError::Forbidden(
            "Playback that requires remuxing is not allowed for this user.".into(),
        ));
    }
    Ok(())
}

/// A requested bitrate (bits per second) within `RemoteClientBitrateLimit` for a caller outside
/// the local network; a request without one gets the limit.
pub fn remote_bitrate(access: &AccessPolicy, local: bool, requested: Option<u32>) -> Option<u32> {
    let limit = u32::try_from(access.remote_client_bitrate_limit).unwrap_or(u32::MAX);
    if local || limit == 0 {
        return requested;
    }
    Some(requested.map_or(limit, |b| b.min(limit)))
}

/// The caller's parental policy, or `None` when it hides nothing.
pub async fn policy(
    state: &AppState,
    claims: &Claims,
) -> Result<Option<Arc<ParentalPolicy>>, ApiError> {
    if claims.sub.is_nil() {
        return Ok(None);
    }
    let policy = user_policy(state, claims.sub).await?;
    Ok(policy.is_restricted().then_some(policy))
}

/// An entry of a list that `HiddenItems::retain` filters: its id and its library.
pub trait Listed {
    fn item_id(&self) -> Uuid;
    fn library(&self) -> Uuid;
}

impl Listed for MediaItem {
    fn item_id(&self) -> Uuid {
        self.id
    }
    fn library(&self) -> Uuid {
        self.library_id
    }
}

/// The items hidden from the caller. Empty for a caller without limits.
#[derive(Debug, Default)]
pub struct HiddenItems {
    /// The ids that ratings and tags hide, as stored, for SQL to compare.
    stored: HashSet<String>,
    ids: HashSet<Uuid>,
    /// The libraries the caller may see, or `None` for every library.
    folders: Option<HashSet<Uuid>>,
    /// The same, as stored.
    folder_ids: Option<Vec<String>>,
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
                let folder_ids = policy.enabled_folders().map(<[String]>::to_vec);
                let folders = folder_ids.as_ref().map(|ids| {
                    ids.iter()
                        .filter_map(|id| Uuid::parse_str(id).ok())
                        .collect()
                });
                Ok(Self {
                    stored,
                    ids,
                    folders,
                    folder_ids,
                    restricted: true,
                })
            }
        }
    }

    /// The ids that ratings and tags hide, as stored.
    pub fn stored_ids(&self) -> &HashSet<String> {
        &self.stored
    }

    /// The library ids the caller may see, as stored, or `None` for every library.
    pub fn libraries(&self) -> Option<HashSet<String>> {
        self.folder_ids
            .as_ref()
            .map(|ids| ids.iter().cloned().collect())
    }

    /// The library ids the caller may see as a JSON array, or `None` for every library.
    pub fn libraries_json(&self) -> Option<String> {
        self.folder_ids
            .as_ref()
            .map(|ids| serde_json::Value::from(ids.clone()).to_string())
    }

    /// True when the caller has limits, so answers shared between users (caches) don't fit.
    pub fn is_restricted(&self) -> bool {
        self.restricted
    }

    /// Whether the item `id` in library `library` is hidden.
    pub fn hides(&self, id: Uuid, library: Uuid) -> bool {
        self.ids.contains(&id) || self.folders.as_ref().is_some_and(|f| !f.contains(&library))
    }

    /// The ids among `ids` that are hidden: by ratings and tags, or, with library access, in a
    /// library the caller may not see (an id that names no item counts as hidden then). One query
    /// for the libraries, for lists of ids (collections, playlists) without them.
    pub async fn hidden_among(
        &self,
        state: &AppState,
        ids: &[Uuid],
    ) -> Result<HashSet<Uuid>, ApiError> {
        let mut hidden: HashSet<Uuid> = ids
            .iter()
            .filter(|id| self.ids.contains(id))
            .copied()
            .collect();
        if let Some(folders) = &self.folders
            && !ids.is_empty()
        {
            let wanted =
                serde_json::Value::from(ids.iter().map(Uuid::to_string).collect::<Vec<_>>())
                    .to_string();
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT id, library_id FROM media_items WHERE id IN (SELECT value FROM json_each(?))",
            )
            .bind(wanted)
            .fetch_all(&state.db_ro)
            .await?;
            let libraries: std::collections::HashMap<Uuid, Uuid> = rows
                .into_iter()
                .filter_map(|(id, lib)| {
                    Some((Uuid::parse_str(&id).ok()?, Uuid::parse_str(&lib).ok()?))
                })
                .collect();
            hidden.extend(
                ids.iter()
                    .filter(|id| libraries.get(id).is_none_or(|lib| !folders.contains(lib))),
            );
        }
        Ok(hidden)
    }

    /// Drops the hidden ids of a list of item ids (`hidden_among`).
    pub async fn retain_ids(&self, state: &AppState, ids: &mut Vec<Uuid>) -> Result<(), ApiError> {
        if self.restricted {
            let hidden = self.hidden_among(state, ids).await?;
            ids.retain(|id| !hidden.contains(id));
        }
        Ok(())
    }

    /// Drops the hidden entries of a list.
    pub fn retain<T: Listed>(&self, items: &mut Vec<T>) {
        if !self.ids.is_empty() || self.folders.is_some() {
            items.retain(|item| !self.hides(item.item_id(), item.library()));
        }
    }

    /// The filter for SQL (`repo_parental::visible_sql`), or `None` when nothing is hidden.
    pub fn json(&self) -> Option<String> {
        rustyfin_db::repo_parental::filter_json(&self.stored, self.folder_ids.as_deref())
    }

    /// `json`, with the items `excluded` hidden too (a list's ExcludeItemIds).
    pub fn json_excluding(&self, excluded: &[Uuid]) -> Option<String> {
        if excluded.is_empty() {
            return self.json();
        }
        let mut hidden = self.stored.clone();
        hidden.extend(excluded.iter().map(Uuid::to_string));
        rustyfin_db::repo_parental::filter_json(&hidden, self.folder_ids.as_deref())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3parental_conversion_guard_follows_the_switches() {
        let open = AccessPolicy::default();
        let no_video = AccessPolicy {
            enable_video_playback_transcoding: false,
            ..AccessPolicy::default()
        };
        let no_remux = AccessPolicy {
            enable_playback_remuxing: false,
            ..AccessPolicy::default()
        };
        let no_audio = AccessPolicy {
            enable_audio_playback_transcoding: false,
            ..AccessPolicy::default()
        };
        // (policy, audio item, copy video, copy audio, allowed)
        for (access, audio, cv, ca, ok) in [
            (&open, false, false, false, true),
            (&no_video, false, false, true, false),
            (&no_video, false, true, false, false),
            (&no_video, false, true, true, true),
            (&no_remux, false, true, true, false),
            (&no_remux, false, false, true, true),
            (&no_audio, true, false, false, false),
            (&no_audio, false, true, false, true),
            (&no_video, true, false, false, true),
        ] {
            assert_eq!(
                check_conversion(access, audio, cv, ca).is_ok(),
                ok,
                "audio {audio} copy {cv}/{ca}"
            );
        }
    }

    #[test]
    fn r3parental_remote_bitrate_limit() {
        let limited = AccessPolicy {
            remote_client_bitrate_limit: 4_000_000,
            ..AccessPolicy::default()
        };
        assert_eq!(
            remote_bitrate(&limited, true, Some(9_000_000)),
            Some(9_000_000)
        );
        assert_eq!(
            remote_bitrate(&limited, false, Some(9_000_000)),
            Some(4_000_000)
        );
        assert_eq!(
            remote_bitrate(&limited, false, Some(1_000_000)),
            Some(1_000_000)
        );
        assert_eq!(remote_bitrate(&limited, false, None), Some(4_000_000));
        assert_eq!(remote_bitrate(&AccessPolicy::default(), false, None), None);
    }
}
