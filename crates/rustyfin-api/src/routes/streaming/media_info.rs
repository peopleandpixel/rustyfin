//! `GET` and `POST /Items/{id}/PlaybackInfo`: an item's media sources, each with what the caller's
//! device can play of it.

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Extension, Json};
use rustyfin_core::stream_builder::{
    DeviceData, MediaKind, SourceFacts, StreamRequest, device_data,
};
use rustyfin_model::auth::Claims;
use rustyfin_model::device_profile::{DeviceProfile, ProfileError, SubtitleDeliveryMethod};
use rustyfin_model::items_playback_info::{
    ItemsPlaybackInfoResponse, PlaybackErrorCode, PlaybackInfoDto,
};
use rustyfin_model::media::MediaType;
use rustyfin_model::media_source::MediaSourceResponse;
use serde_json::Value;
use uuid::Uuid;

use super::{Version, select_version};
use crate::error::ApiError;
use crate::state::AppState;

/// The PlaybackInfo routes of an item.
pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/Items/{id}/PlaybackInfo",
        get(get_items_playback_info).post(post_items_playback_info),
    )
}

/// `GET /Items/{id}/PlaybackInfo`. Only the `userId` query parameter is read; the sources come
/// back without device data.
async fn get_items_playback_info(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Path(id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Result<Json<ItemsPlaybackInfoResponse>, ApiError> {
    let user_pairs: Vec<(String, String)> = pairs
        .into_iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("userId"))
        .collect();
    let dto = PlaybackInfoDto::from_query_pairs(&user_pairs).map_err(bad_request)?;
    let token = token.map(|Extension(rt)| rt.0);
    let answer = playback_info(&state, &claims, token.as_deref(), &id, dto, false).await?;
    Ok(Json(answer))
}

/// `POST /Items/{id}/PlaybackInfo`. The query wins over the JSON body, field by field; the
/// body's device profile, or the one the caller's device posted, decides what the device can play.
async fn post_items_playback_info(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Path(id): Path<String>,
    Query(pairs): Query<Vec<(String, String)>>,
    body: Bytes,
) -> Result<Json<ItemsPlaybackInfoResponse>, ApiError> {
    let from_query = PlaybackInfoDto::from_query_pairs(&pairs).map_err(bad_request)?;
    let from_body = PlaybackInfoDto::from_body(&body).map_err(bad_request)?;
    let dto = from_query.or_body(from_body);
    let token = token.map(|Extension(rt)| rt.0);
    let answer = playback_info(&state, &claims, token.as_deref(), &id, dto, true).await?;
    Ok(Json(answer))
}

/// A request that does not parse is a 400.
fn bad_request(err: ProfileError) -> ApiError {
    ApiError::BadRequest(err.0)
}

/// The PlaybackInfo answer for item `id`. `with_profile` is set for POST, which may decide per
/// device; GET answers without device data.
async fn playback_info(
    state: &AppState,
    claims: &Claims,
    token: Option<&str>,
    id: &str,
    mut dto: PlaybackInfoDto,
    with_profile: bool,
) -> Result<ItemsPlaybackInfoResponse, ApiError> {
    let item_id =
        Uuid::parse_str(id).map_err(|_| ApiError::NotFound(format!("Item {id} not found")))?;
    crate::routes::playback::query_user(state, claims, dto.user_id).await?;
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, item_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {item_id} not found")))?;
    let kind = if matches!(item.media_type, MediaType::Audio) {
        MediaKind::Audio
    } else {
        MediaKind::Video
    };

    let mut sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &item_id)
            .await
            .unwrap_or_default();
    for source in sources.iter_mut() {
        let external = crate::routes::subtitle::external_streams(source).await;
        source.media_streams.extend(external);
    }

    // A blank MediaSourceId is absent: every source is answered
    let requested = dto
        .media_source_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let chosen: Vec<usize> = match requested {
        None => (0..sources.len()).collect(),
        Some(requested) => match select_version(item_id, &sources, Some(requested)) {
            Ok(Version::Primary) if !sources.is_empty() => vec![0],
            Ok(Version::Primary) => Vec::new(),
            Ok(Version::Source(index)) => vec![index],
            Err(_) => Vec::new(),
        },
    };
    if chosen.is_empty() {
        return Ok(ItemsPlaybackInfoResponse {
            media_sources: Vec::new(),
            play_session_id: None,
            error_code: Some(PlaybackErrorCode::NoCompatibleStream),
        });
    }

    let device_id = match token {
        Some(token) => rustyfin_auth::token_device_id(&state.db_ro, token)
            .await?
            .unwrap_or_default(),
        None => String::new(),
    };
    let profile = if !with_profile {
        None
    } else if let Some(profile) = dto.device_profile.take() {
        Some(profile)
    } else {
        stored_profile(state, claims, &device_id).await?
    };

    let play_session_id = Uuid::new_v4().simple().to_string();
    let mut media_sources = Vec::with_capacity(chosen.len());
    for index in chosen {
        let mut source = sources[index].clone();
        // The loader's flags are a placeholder; the device data below decides them when present
        source.supports_direct_play = true;
        source.supports_direct_stream = true;
        source.supports_transcoding = true;
        let value = match &profile {
            None => serde_json::to_value(&source),
            Some(profile) => {
                let request = StreamRequest {
                    item_id: item_id.to_string(),
                    media_kind: kind,
                    max_bitrate: dto.max_streaming_bitrate,
                    start_time_ticks: dto.start_time_ticks.unwrap_or(0),
                    media_source_id: if requested.is_some() {
                        source.id.clone()
                    } else {
                        String::new()
                    },
                    audio_stream_index: dto.audio_stream_index,
                    subtitle_stream_index: dto.subtitle_stream_index,
                    max_audio_channels: dto.max_audio_channels,
                    enable_direct_play: dto.enable_direct_play.unwrap_or(true),
                    enable_direct_stream: dto.enable_direct_stream.unwrap_or(true),
                    enable_transcoding: dto.enable_transcoding.unwrap_or(true),
                    allow_video_stream_copy: dto.allow_video_stream_copy.unwrap_or(true),
                    allow_audio_stream_copy: dto.allow_audio_stream_copy.unwrap_or(true),
                    always_burn_in_subtitle: dto
                        .always_burn_in_subtitle_when_transcoding
                        .unwrap_or(false),
                    device_id: device_id.clone(),
                    play_session_id: play_session_id.clone(),
                    access_token: token.unwrap_or_default().to_string(),
                };
                let data = device_data(&SourceFacts::from(&source), profile, &request);
                patch_source(&source, &data)
            }
        };
        media_sources
            .push(value.map_err(|e| ApiError::Internal(format!("Serialization error: {e}")))?);
    }

    Ok(ItemsPlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
        error_code: None,
    })
}

/// The device profile that the caller's device posted most recently. None when the device has
/// none, or when the stored JSON has no parsable `DeviceProfile` key.
async fn stored_profile(
    state: &AppState,
    claims: &Claims,
    device_id: &str,
) -> Result<Option<DeviceProfile>, ApiError> {
    if device_id.is_empty() {
        return Ok(None);
    }
    let Some(json) =
        rustyfin_db::repo_session::get_device_capabilities(&state.db_ro, claims.sub, device_id)
            .await?
    else {
        return Ok(None);
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&json) else {
        return Ok(None);
    };
    let Some((_, profile)) = map
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("DeviceProfile"))
    else {
        return Ok(None);
    };
    Ok(DeviceProfile::from_json(profile).ok())
}

/// The source as JSON, with the device's answer written in (the keys of the PlaybackInfo
/// contract).
fn patch_source(
    source: &MediaSourceResponse,
    data: &DeviceData,
) -> Result<Value, serde_json::Error> {
    let mut map = match serde_json::to_value(source)? {
        Value::Object(map) => map,
        other => return Ok(other),
    };
    map.insert(
        "SupportsDirectPlay".into(),
        data.supports_direct_play.into(),
    );
    map.insert(
        "SupportsDirectStream".into(),
        data.supports_direct_stream.into(),
    );
    map.insert(
        "SupportsTranscoding".into(),
        data.supports_transcoding.into(),
    );
    if let Some(url) = &data.transcoding_url {
        map.insert("TranscodingUrl".into(), url.clone().into());
    }
    map.insert(
        "TranscodingSubProtocol".into(),
        data.transcoding_sub_protocol.as_str().into(),
    );
    if let Some(container) = &data.transcoding_container {
        map.insert("TranscodingContainer".into(), container.clone().into());
    }
    map.insert(
        "TranscodeReasons".into(),
        data.transcode_reasons.names().into(),
    );
    if let Some(index) = data.default_audio_stream_index {
        map.insert("DefaultAudioStreamIndex".into(), index.into());
    }
    if let Some(index) = data.default_subtitle_stream_index {
        map.insert("DefaultSubtitleStreamIndex".into(), index.into());
    }
    map.insert("Container".into(), data.container.clone().into());

    if let Some(Value::Array(streams)) = map.get_mut("MediaStreams") {
        for delivery in &data.subtitles {
            let wanted = i64::from(delivery.index);
            let stream = streams
                .iter_mut()
                .find(|stream| stream.get("Index").and_then(Value::as_i64) == Some(wanted));
            let Some(object) = stream.and_then(Value::as_object_mut) else {
                continue;
            };
            object.insert("DeliveryMethod".into(), delivery.method.as_str().into());
            if matches!(delivery.method, SubtitleDeliveryMethod::External) {
                if let Some(url) = &delivery.url {
                    object.insert("DeliveryUrl".into(), url.clone().into());
                }
                object.insert("IsExternalUrl".into(), false.into());
            }
        }
    }
    Ok(Value::Object(map))
}
