use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rustyfin_model::auth::Claims;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SyncPlayGroupDto {
    id: String,
    name: String,
    #[serde(rename = "OwnerId")]
    owner_id: String,
    #[serde(rename = "IsLocked")]
    is_locked: bool,
    #[serde(rename = "PlayMethod")]
    play_method: String,
    #[serde(rename = "MemberCount")]
    member_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SyncPlayMemberDto {
    id: String,
    #[serde(rename = "UserId")]
    user_id: String,
    #[serde(rename = "SessionId")]
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateGroupRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    #[serde(rename = "PlayMethod")]
    play_method: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JoinGroupRequest {
    #[serde(default)]
    #[serde(rename = "GroupId")]
    group_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PingRequest {
    #[serde(default)]
    #[serde(rename = "GroupId")]
    #[allow(dead_code)]
    group_id: String,
    #[serde(default)]
    ticks: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlayStateRequest {
    #[serde(default)]
    #[serde(rename = "GroupId")]
    #[allow(dead_code)]
    group_id: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/SyncPlay", get(get_status))
        .route("/SyncPlay/CreateGroup", post(create_group))
        .route("/SyncPlay/JoinGroup", post(join_group))
        .route("/SyncPlay/LeaveGroup", post(leave_group))
        .route("/SyncPlay/Ping", post(ping))
        .route("/SyncPlay/Play", post(play))
        .route("/SyncPlay/Pause", post(pause))
        .route("/SyncPlay/Seek", post(seek))
        .route(
            "/SyncPlay/Groups/{groupId}",
            get(get_group).delete(delete_group),
        )
        .route("/SyncPlay/Groups/{groupId}/Members", get(get_members))
        .route("/SyncPlay/Groups/{groupId}/Lock", post(lock_group))
        .route("/SyncPlay/Groups/{groupId}/Unlock", post(unlock_group))
}

async fn get_status(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let groups = rustyfin_db::repo_sync_play::get_groups(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<SyncPlayGroupDto> = groups
        .into_iter()
        .map(|g| SyncPlayGroupDto {
            id: g.id.clone(),
            name: g.name,
            owner_id: g.owner_user_id,
            is_locked: g.is_locked,
            play_method: g.play_method,
            member_count: 0,
        })
        .collect();

    Ok(Json(serde_json::json!({
        "Groups": items,
        "IsInGroup": false
    })))
}

async fn create_group(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<CreateGroupRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = Uuid::new_v4().to_string();
    let name = if req.name.is_empty() {
        "Sync Play Group".to_string()
    } else {
        req.name.clone()
    };
    let play_method = if req.play_method.is_empty() {
        "Sync".to_string()
    } else {
        req.play_method.clone()
    };

    rustyfin_db::repo_sync_play::create_group(
        &state.db,
        &id,
        &name,
        &claims.sub.to_string(),
        &play_method,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "Id": id,
        "Name": name,
        "PlayMethod": play_method
    })))
}

async fn join_group(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<JoinGroupRequest>,
) -> Result<StatusCode, ApiError> {
    let group = rustyfin_db::repo_sync_play::get_group(&state.db_ro, &req.group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Group not found".to_string()))?;

    if group.is_locked {
        return Err(ApiError::BadRequest("Group is locked".to_string()));
    }

    let member_id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();

    rustyfin_db::repo_sync_play::add_member(
        &state.db,
        &member_id,
        &req.group_id,
        &claims.sub.to_string(),
        &session_id,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn leave_group(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(req): Json<JoinGroupRequest>,
) -> Result<StatusCode, ApiError> {
    // Find and remove the member
    let members = rustyfin_db::repo_sync_play::get_members(&state.db_ro, &req.group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    for member in members {
        if member.user_id == claims.sub.to_string() {
            rustyfin_db::repo_sync_play::remove_member(&state.db, &member.id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            break;
        }
    }

    Ok(StatusCode::OK)
}

async fn ping(
    State(_state): State<AppState>,
    Json(_req): Json<PingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // SyncPlay ping is used to synchronize playback timing
    // Return the current server time in ticks
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ticks = now.as_millis() as i64 * 10_000;

    Ok(Json(serde_json::json!({
        "Ticks": ticks,
        "ServerTime": now.as_secs()
    })))
}

async fn play(
    State(_state): State<AppState>,
    Json(_req): Json<PlayStateRequest>,
) -> Result<StatusCode, ApiError> {
    // Broadcast play command to all group members via WebSocket
    tracing::info!("SyncPlay play command received");
    Ok(StatusCode::OK)
}

async fn pause(
    State(_state): State<AppState>,
    Json(_req): Json<PlayStateRequest>,
) -> Result<StatusCode, ApiError> {
    // Broadcast pause command to all group members via WebSocket
    tracing::info!("SyncPlay pause command received");
    Ok(StatusCode::OK)
}

async fn seek(
    State(_state): State<AppState>,
    Json(_req): Json<PingRequest>,
) -> Result<StatusCode, ApiError> {
    // Broadcast seek command to all group members via WebSocket
    tracing::info!("SyncPlay seek command received: {} ticks", _req.ticks);
    Ok(StatusCode::OK)
}

async fn get_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<SyncPlayGroupDto>, ApiError> {
    let group = rustyfin_db::repo_sync_play::get_group(&state.db_ro, &group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Group not found".to_string()))?;

    let members = rustyfin_db::repo_sync_play::get_members(&state.db_ro, &group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(SyncPlayGroupDto {
        id: group.id,
        name: group.name,
        owner_id: group.owner_user_id,
        is_locked: group.is_locked,
        play_method: group.play_method,
        member_count: members.len() as i64,
    }))
}

async fn delete_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_sync_play::delete_group(&state.db, &group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn get_members(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<Vec<SyncPlayMemberDto>>, ApiError> {
    let members = rustyfin_db::repo_sync_play::get_members(&state.db_ro, &group_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<SyncPlayMemberDto> = members
        .into_iter()
        .map(|m| SyncPlayMemberDto {
            id: m.id,
            user_id: m.user_id,
            session_id: m.session_id,
        })
        .collect();

    Ok(Json(items))
}

async fn lock_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_sync_play::lock_group(&state.db, &group_id, true)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn unlock_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_sync_play::lock_group(&state.db, &group_id, false)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}
