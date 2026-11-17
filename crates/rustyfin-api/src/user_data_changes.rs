//! `UserDataChanged` over the WebSocket, as Jellyfin's UserDataChangeNotifier sends it. Each write
//! of a user's played state, favourite, rating or resume point (but not a progress report) is held
//! until no other write has come for 500 ms; then each user's sockets get one message with the user
//! data of every item changed and of each item's parent, so that watched badges and
//! unplayed counts follow on the user's other devices.

use std::sync::{Arc, Mutex};

use rustyfin_model::media::{MediaItem, MediaType};
use rustyfin_model::ws_event::WsEvent;
use tokio::time::{Duration, Instant};
use uuid::Uuid;

use crate::state::AppState;

/// Jellyfin's UpdateDuration: how long the changes wait for more after the last one.
const UPDATE_DURATION: Duration = Duration::from_millis(500);

/// The changes waiting to be sent. One per `AppState` (in its `SessionRuntime`).
#[derive(Debug, Clone, Default)]
pub struct Pending(Arc<Mutex<Inner>>);

#[derive(Debug, Default)]
struct Inner {
    /// Each user's changed items, in order of their first change, and whether the change reached
    /// the item's episodes too (a Series or Season marked played or unplayed).
    changes: Vec<(Uuid, Vec<(Uuid, bool)>)>,
    /// When the changes are sent; set while a task waits to send them.
    deadline: Option<Instant>,
}

impl Pending {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The user's data on the item changed.
pub fn changed(state: &AppState, user_id: Uuid, item_id: Uuid) {
    add(state, user_id, item_id, false);
}

/// The user's played state changed on the item and, for a Series or Season, on its episodes.
pub fn played_changed(state: &AppState, user_id: Uuid, item_id: Uuid) {
    add(state, user_id, item_id, true);
}

fn add(state: &AppState, user_id: Uuid, item_id: Uuid, descendants: bool) {
    let start = {
        let mut inner = state.session_runtime.user_data.lock();
        let items = match inner.changes.iter().position(|(user, _)| *user == user_id) {
            Some(i) => &mut inner.changes[i].1,
            None => {
                inner.changes.push((user_id, Vec::new()));
                &mut inner.changes.last_mut().expect("just pushed").1
            }
        };
        match items.iter_mut().find(|(id, _)| *id == item_id) {
            Some(entry) => entry.1 |= descendants,
            None => items.push((item_id, descendants)),
        }
        let start = inner.deadline.is_none();
        inner.deadline = Some(Instant::now() + UPDATE_DURATION);
        start
    };
    if start {
        let state = state.clone();
        tokio::spawn(async move { send_when_quiet(state).await });
    }
}

/// Waits until no change has come for `UPDATE_DURATION`, then sends each user's changes.
async fn send_when_quiet(state: AppState) {
    loop {
        let deadline = state.session_runtime.user_data.lock().deadline;
        match deadline {
            Some(deadline) if Instant::now() < deadline => tokio::time::sleep_until(deadline).await,
            _ => break,
        }
    }
    let changes = {
        let mut inner = state.session_runtime.user_data.lock();
        inner.deadline = None;
        std::mem::take(&mut inner.changes)
    };
    for (user_id, items) in changes {
        match message(&state, user_id, &items).await {
            Ok(Some(event)) => crate::routes::websocket::broadcast_event(&state, event).await,
            Ok(None) => {}
            Err(e) => tracing::warn!(user = %user_id, error = ?e, "UserDataChanged not sent"),
        }
    }
}

/// The `UserDataChanged` message for the user's changed items: `{"UserId", "UserDataList"}`, each
/// entry a UserItemDataDto with its `ItemId`, as items answer it.
async fn message(
    state: &AppState,
    user_id: Uuid,
    changed: &[(Uuid, bool)],
) -> Result<Option<WsEvent>, crate::error::ApiError> {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut push = |id: Uuid| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };
    for &(item_id, descendants) in changed {
        let Some(item) = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id).await?
        else {
            continue;
        };
        push(item_id);
        // Jellyfin goes up one level, to the parent, for its indicators.
        if let Some(parent) = item.parent_id
            && rustyfin_db::repo_media::get_item_by_id(&state.db_ro, parent)
                .await?
                .is_some()
        {
            push(parent);
        }
        if descendants {
            for child in episodes_and_seasons(state, &item).await? {
                push(child);
            }
        }
    }
    if ids.is_empty() {
        return Ok(None);
    }
    let mut list = Vec::with_capacity(ids.len());
    for id in ids {
        let data =
            rustyfin_db::repo_playback::get_user_item_data(&state.db_ro, user_id, id).await?;
        let data = crate::routes::playback::folder_answer(state, user_id, id, data).await?;
        list.push(data);
    }
    Ok(Some(WsEvent::UserDataChanged {
        data: serde_json::json!({ "UserId": user_id.to_string(), "UserDataList": list }),
    }))
}

/// The seasons and episodes a Series or Season marked played or unplayed reaches.
async fn episodes_and_seasons(
    state: &AppState,
    item: &MediaItem,
) -> Result<Vec<Uuid>, sqlx::Error> {
    Ok(match item.media_type {
        MediaType::Series => {
            let mut ids: Vec<Uuid> = rustyfin_db::repo_media::get_children(&state.db_ro, item.id)
                .await?
                .into_iter()
                .filter(|child| child.media_type == MediaType::Season)
                .map(|season| season.id)
                .collect();
            ids.extend(
                rustyfin_db::repo_media::get_episodes_for_series(&state.db_ro, item.id, None)
                    .await?
                    .into_iter()
                    .map(|episode| episode.id),
            );
            ids
        }
        MediaType::Season => rustyfin_db::repo_media::get_children(&state.db_ro, item.id)
            .await?
            .into_iter()
            .filter(|child| child.media_type == MediaType::Episode)
            .map(|episode| episode.id)
            .collect(),
        _ => Vec::new(),
    })
}
