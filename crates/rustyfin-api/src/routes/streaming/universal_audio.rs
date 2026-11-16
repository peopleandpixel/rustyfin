//! `GET /Audio/{id}/universal`: Jellyfin's universal audio stream, which music clients (Jellyfin
//! Web and Android, Finamp, Symfonium) play. A port of `UniversalAudioController`: the query's
//! capabilities become a device profile, the stream builder decides, and the answer is the audio
//! itself, not JSON: the file when it plays as it is, else the HLS transcode. A progressive (HTTP)
//! transcode is refused: Rustyfin's transcoder writes HLS only.

use crate::query::Query;
use axum::Extension;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rustyfin_core::stream_builder::{MediaKind, SourceFacts, StreamRequest, device_data};
use rustyfin_model::device_profile::{DeviceProfile, StreamProtocol};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    Version, head_file, requested_play_session_id, requested_source_id, select_version, serve_file,
    validate_path_against_library,
};
use crate::error::ApiError;
use crate::state::AppState;

/// The universal audio route.
pub(super) fn routes() -> Router<AppState> {
    Router::new().route(
        "/Audio/{id}/universal",
        get(audio_universal).head(head_universal),
    )
}

/// The value of query parameter `name`, in any ASCII case as Jellyfin binds it. Blank is absent.
fn param<'a>(params: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

/// The number in query parameter `name`; one that doesn't parse is absent.
fn number(params: &HashMap<String, String>, name: &str) -> Option<i64> {
    param(params, name).and_then(|value| value.parse().ok())
}

/// Jellyfin's `UniversalAudioController.GetDeviceProfile`, in Jellyfin's JSON: a direct play
/// profile per `Container` entry (`container|codec|codec`), one Streaming transcoding profile
/// (mp3 over HTTP unless the query says otherwise), and a codec profile with the query's sample
/// rate, bit depth and channel limits.
pub(super) fn universal_profile(params: &HashMap<String, String>) -> Value {
    let containers: Vec<&str> = param(params, "Container")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let direct_play: Vec<Value> = containers
        .iter()
        .map(|entry| {
            let mut parts = entry.split('|').filter(|part| !part.is_empty());
            let container = parts.next().unwrap_or_default();
            let codecs: Vec<&str> = parts.collect();
            let mut profile = json!({ "Type": "Audio", "Container": container });
            if !codecs.is_empty() {
                profile["AudioCodec"] = codecs.join(",").into();
            }
            profile
        })
        .collect();

    let mut transcoding = json!({
        "Type": "Audio",
        "Context": "Streaming",
        "Container": param(params, "TranscodingContainer").unwrap_or("mp3"),
        "AudioCodec": param(params, "AudioCodec").unwrap_or("mp3"),
        "Protocol": param(params, "TranscodingProtocol").unwrap_or("http"),
    });
    if let Some(channels) = number(params, "TranscodingAudioChannels") {
        transcoding["MaxAudioChannels"] = channels.to_string().into();
    }

    let conditions: Vec<Value> = [
        ("MaxAudioSampleRate", "AudioSampleRate"),
        ("MaxAudioBitDepth", "AudioBitDepth"),
        ("MaxAudioChannels", "AudioChannels"),
    ]
    .iter()
    .filter_map(|(name, property)| {
        number(params, name).map(|value| {
            json!({
                "Condition": "LessThanEqual",
                "Property": property,
                "Value": value.to_string(),
                "IsRequired": false,
            })
        })
    })
    .collect();
    let codec_profiles: Vec<Value> = if conditions.is_empty() {
        Vec::new()
    } else {
        vec![json!({
            "Type": "Audio",
            "Container": containers.join(","),
            "Conditions": conditions,
        })]
    };

    let mut profile = json!({
        "DirectPlayProfiles": direct_play,
        "TranscodingProfiles": [transcoding],
        "CodecProfiles": codec_profiles,
    });
    if let Some(bitrate) = number(params, "MaxStreamingBitrate") {
        profile["MaxStreamingBitrate"] = bitrate.into();
    }
    profile
}

/// What a universal audio request gets.
enum Answer {
    /// The file at this path, as it is.
    File(String),
    /// The HLS transcode at this URL.
    Hls(String),
}

/// The answer to a universal audio request: the file, with byte ranges, when the query's
/// containers and codecs take it as it is; else the HLS transcode the stream builder chose. A
/// request that allows only a progressive (HTTP) transcode is refused with 400: Rustyfin's
/// transcoder writes HLS only, and the file as it is (FLAC for an MP3 client) would not play.
async fn decide(
    state: &AppState,
    token: Option<String>,
    id: Uuid,
    params: &HashMap<String, String>,
) -> Result<Answer, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;
    let sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .unwrap_or_default();
    let requested = requested_source_id(params);
    let source = match select_version(id, &sources, requested)? {
        Version::Primary => sources.first(),
        Version::Source(index) => sources.get(index),
    };
    // The resolved path, which the file is opened by
    let file_path = source.map_or(item.path.clone(), |source| source.path.clone());
    let file_path = validate_path_against_library(state, &file_path).await?;
    let Some(source) = source else {
        // A track that was never probed plays as it is
        return Ok(Answer::File(file_path));
    };

    let profile = DeviceProfile::from_json(&universal_profile(params))
        .map_err(|e| ApiError::BadRequest(format!("Invalid universal audio query: {e}")))?;
    let mut facts = SourceFacts::from(source);
    facts.supports_direct_play = true;
    facts.supports_direct_stream = true;
    facts.supports_transcoding = true;
    // The client's play session, so that its reports and the transcode share one; a new one when
    // it sends none
    let play_session_id =
        requested_play_session_id(params).unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let request = StreamRequest {
        item_id: id.to_string(),
        media_kind: MediaKind::Audio,
        max_bitrate: number(params, "MaxStreamingBitrate")
            .and_then(|bitrate| i32::try_from(bitrate).ok()),
        start_time_ticks: number(params, "StartTimeTicks").unwrap_or(0),
        media_source_id: requested.map_or_else(String::new, |_| source.id.clone()),
        max_audio_channels: number(params, "MaxAudioChannels")
            .and_then(|channels| i32::try_from(channels).ok()),
        device_id: param(params, "DeviceId").unwrap_or_default().to_string(),
        play_session_id,
        access_token: token.unwrap_or_default(),
        ..StreamRequest::default()
    };
    let data = device_data(&facts, &profile, &request);

    // Jellyfin: "SupportsDirectStream actually means can direct play"
    if data.supports_direct_stream {
        return Ok(Answer::File(file_path));
    }
    match (data.transcoding_url, data.transcoding_sub_protocol) {
        (Some(url), StreamProtocol::Hls) => Ok(Answer::Hls(url)),
        _ => Err(ApiError::BadRequest(format!(
            "Item {id} needs a transcode for this client ({}), and Rustyfin transcodes audio over \
             HLS only: ask with TranscodingProtocol=hls",
            data.transcode_reasons.names().join(", ")
        ))),
    }
}

/// `GET /Audio/{id}/universal`: the file, or a redirect to the HLS transcode. Jellyfin answers the
/// HLS case with the master playlist itself; a redirect plays the same in the clients and keeps
/// one playlist route.
async fn audio_universal(
    State(state): State<AppState>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    match decide(&state, token.map(|Extension(t)| t.0), id, &params).await? {
        Answer::File(path) => serve_file(path, &headers).await,
        Answer::Hls(url) => Ok((StatusCode::FOUND, [(header::LOCATION, url)]).into_response()),
    }
}

/// `HEAD /Audio/{id}/universal`, which Jellyfin Web sends before it plays: the file's headers, or
/// the playlist's content type. It starts no transcode (axum would run GET for HEAD, and the
/// redirect's GET would start ffmpeg a second time).
async fn head_universal(
    State(state): State<AppState>,
    token: Option<Extension<rustyfin_auth::RequestToken>>,
    Path(id): Path<Uuid>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    match decide(&state, token.map(|Extension(t)| t.0), id, &params).await? {
        Answer::File(path) => head_file(&path).await,
        Answer::Hls(_) => Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
        )
            .into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Jellyfin Web's query becomes Jellyfin's profile: `container|codec` entries, an HLS AAC
    /// transcoding profile and the bitrate cap; parameter names in any case.
    #[test]
    fn r3decisions_universal_profile_is_jellyfins() {
        let profile = universal_profile(&query(&[
            ("container", "opus,webm|opus,mp3,m4a|aac,flac"),
            ("TranscodingContainer", "mp4"),
            ("transcodingProtocol", "hls"),
            ("AudioCodec", "aac"),
            ("MaxStreamingBitrate", "320000"),
            ("MaxAudioSampleRate", "48000"),
        ]));
        assert_eq!(
            profile["DirectPlayProfiles"],
            json!([
                {"Type": "Audio", "Container": "opus"},
                {"Type": "Audio", "Container": "webm", "AudioCodec": "opus"},
                {"Type": "Audio", "Container": "mp3"},
                {"Type": "Audio", "Container": "m4a", "AudioCodec": "aac"},
                {"Type": "Audio", "Container": "flac"},
            ])
        );
        assert_eq!(
            profile["TranscodingProfiles"],
            json!([{"Type": "Audio", "Context": "Streaming", "Container": "mp4",
                    "AudioCodec": "aac", "Protocol": "hls"}])
        );
        assert_eq!(profile["MaxStreamingBitrate"], json!(320000));
        assert_eq!(
            profile["CodecProfiles"][0]["Conditions"],
            json!([{"Condition": "LessThanEqual", "Property": "AudioSampleRate",
                    "Value": "48000", "IsRequired": false}])
        );
        DeviceProfile::from_json(&profile).unwrap();
    }

    /// With no transcoding parameters the transcode is MP3 over HTTP, as in Jellyfin.
    #[test]
    fn r3decisions_universal_profile_defaults_to_mp3_over_http() {
        let profile = universal_profile(&query(&[("Container", "flac")]));
        assert_eq!(profile["TranscodingProfiles"][0]["Container"], "mp3");
        assert_eq!(profile["TranscodingProfiles"][0]["AudioCodec"], "mp3");
        assert_eq!(profile["TranscodingProfiles"][0]["Protocol"], "http");
        assert_eq!(profile["CodecProfiles"], json!([]));
        assert!(profile.get("MaxStreamingBitrate").is_none());
    }
}
