use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct DlnaStreamingProfileDto {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_xml: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<String>,
    #[serde(rename = "AudioCodec", skip_serializing_if = "Option::is_none")]
    audio_codec: Option<String>,
    #[serde(rename = "VideoCodec", skip_serializing_if = "Option::is_none")]
    video_codec: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateProfileRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    profile_xml: String,
    #[serde(default)]
    container: String,
    #[serde(default)]
    audio_codec: String,
    #[serde(default)]
    video_codec: String,
    #[serde(default)]
    direct_play_profiles: String,
    #[serde(default)]
    transcoding_profiles: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Dlna/Streaming/Profiles",
            get(get_profiles).post(create_profile),
        )
        .route(
            "/Dlna/Streaming/Profiles/{profileId}",
            get(get_profile).put(update_profile).delete(delete_profile),
        )
}

async fn get_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<DlnaStreamingProfileDto>>, ApiError> {
    let profiles = rustyfin_db::repo_misc::get_streaming_profiles(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<DlnaStreamingProfileDto> = profiles
        .into_iter()
        .map(|p| DlnaStreamingProfileDto {
            id: p.id,
            name: p.name,
            profile_xml: if p.profile_xml.is_empty() {
                None
            } else {
                Some(p.profile_xml)
            },
            container: if p.container.is_empty() {
                None
            } else {
                Some(p.container)
            },
            audio_codec: if p.audio_codec.is_empty() {
                None
            } else {
                Some(p.audio_codec)
            },
            video_codec: if p.video_codec.is_empty() {
                None
            } else {
                Some(p.video_codec)
            },
        })
        .collect();

    Ok(Json(items))
}

async fn get_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<Json<DlnaStreamingProfileDto>, ApiError> {
    let profile = rustyfin_db::repo_misc::get_streaming_profile(&state.db_ro, &profile_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("Profile not found".to_string()))?;

    Ok(Json(DlnaStreamingProfileDto {
        id: profile.id,
        name: profile.name,
        profile_xml: if profile.profile_xml.is_empty() {
            None
        } else {
            Some(profile.profile_xml)
        },
        container: if profile.container.is_empty() {
            None
        } else {
            Some(profile.container)
        },
        audio_codec: if profile.audio_codec.is_empty() {
            None
        } else {
            Some(profile.audio_codec)
        },
        video_codec: if profile.video_codec.is_empty() {
            None
        } else {
            Some(profile.video_codec)
        },
    }))
}

async fn create_profile(
    State(state): State<AppState>,
    Json(req): Json<CreateProfileRequest>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::new_v4().to_string();

    rustyfin_db::repo_misc::create_streaming_profile(
        &state.db,
        &id,
        &req.name,
        &req.profile_xml,
        &req.container,
        &req.audio_codec,
        &req.video_codec,
        &req.direct_play_profiles,
        &req.transcoding_profiles,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn update_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Json(req): Json<CreateProfileRequest>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_misc::update_streaming_profile(
        &state.db,
        &profile_id,
        &req.name,
        &req.profile_xml,
        &req.container,
        &req.audio_codec,
        &req.video_codec,
        &req.direct_play_profiles,
        &req.transcoding_profiles,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn delete_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_misc::delete_streaming_profile(&state.db, &profile_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}
