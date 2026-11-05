use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use serde::Deserialize;
use uuid::Uuid;

use rustyfin_core::transcode::{HwAccel, TranscodeError, TranscodeProfile};
use rustyfin_model::media_source::MediaSourceResponse;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        // Jellyfin standard HLS master playlist endpoint
        .route("/Videos/{id}/main.m3u8", get(hls_master_playlist))
        // Jellyfin HLS segment endpoint: /Videos/{id}/hls/{playbackId}/{type}/{segmentId}.{ext}
        .route(
            "/Videos/{id}/hls/{playback_id}/{segment_type}/{segment}",
            get(hls_segment),
        )
        // Legacy segment endpoint (backward compat)
        .route("/Videos/{id}/hls/{segment}", get(hls_segment_legacy))
        .route("/Videos/{id}/Transcode", delete(stop_transcode))
        .route("/Videos/ActiveEncodings", delete(stop_active_encodings))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodeQuery {
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub hw_accel: Option<String>,
    pub segment_duration: Option<u32>,
    pub subtitle_stream_index: Option<u32>,
    pub burn_subtitles: Option<bool>,
    /// Audio stream index for multi-audio support
    pub audio_stream_index: Option<u32>,
    /// Audio-only transcoding (no video)
    pub audio_only: Option<bool>,
    /// Enable adaptive bitrate transcoding
    pub adaptive_bitrate: Option<bool>,
    /// Jellyfin play session id: the transcode session key
    pub play_session_id: Option<String>,
    /// Jellyfin device id: the session key when there is no play session id
    pub device_id: Option<String>,
}

impl TranscodeQuery {
    fn into_profile(self) -> TranscodeProfile {
        let mut p = TranscodeProfile::default();
        if let Some(v) = self.video_bitrate {
            p.max_video_bitrate_kbps = v;
        }
        if let Some(v) = self.audio_bitrate {
            p.max_audio_bitrate_kbps = v;
        }
        if let Some(v) = self.max_width {
            p.max_width = v;
        }
        if let Some(v) = self.max_height {
            p.max_height = v;
        }
        if let Some(v) = self.hw_accel {
            p.hw_accel = HwAccel::from_str(&v);
        }
        if let Some(v) = self.segment_duration {
            p.segment_duration = v;
        }
        if let Some(v) = self.subtitle_stream_index {
            p.subtitle_stream_index = Some(v);
        }
        if let Some(v) = self.burn_subtitles {
            p.burn_subtitles = v;
        }
        if let Some(v) = self.audio_stream_index {
            p.audio_stream_index = Some(v);
        }
        if let Some(v) = self.audio_only {
            p.audio_only = v;
        }
        if let Some(v) = self.adaptive_bitrate {
            p.adaptive_bitrate = v;
        }
        p
    }
}

/// What the transcode route knows about the source.
#[derive(Debug, Default)]
struct SourceInfo {
    video_codec: Option<String>,
    video_profile: Option<String>,
    video_bit_depth: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    video_bitrate: Option<u32>, // bits/s: stream, else source video_bitrate, else source bitrate
    audio_codecs: Vec<String>,  // lowercase, one per audio stream
    audio_bitrate: Option<u32>, // bits/s: first audio stream, else source audio_bitrate
}

impl SourceInfo {
    fn from_source(source: &MediaSourceResponse) -> Self {
        // The probe keeps ffprobe's lowercase stream types, so the types are matched without case
        let video = source
            .media_streams
            .iter()
            .find(|s| s.codec_type.eq_ignore_ascii_case("video"));
        let audio: Vec<_> = source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("audio"))
            .collect();
        Self {
            video_codec: video
                .and_then(|s| s.codec.as_deref())
                .map(str::to_ascii_lowercase),
            video_profile: video.and_then(|s| s.profile.clone()),
            video_bit_depth: video.and_then(|s| s.bit_depth),
            width: video.and_then(|s| s.width).or(source.width),
            height: video.and_then(|s| s.height).or(source.height),
            video_bitrate: video
                .and_then(|s| s.bit_rate)
                .or(source.video_bitrate)
                .or(source.bitrate),
            audio_codecs: audio
                .iter()
                .map(|s| s.codec.as_deref().unwrap_or_default().to_ascii_lowercase())
                .collect(),
            audio_bitrate: audio
                .first()
                .and_then(|s| s.bit_rate)
                .or(source.audio_bitrate),
        }
    }
}

/// Whether a client codec list takes one of `codecs`. An absent list, or one with no non-empty
/// entries, takes them all. Entries are split on commas, trimmed and compared lowercased.
fn client_takes(list: Option<&str>, codecs: &[&str]) -> bool {
    let names: Vec<String> = list
        .unwrap_or_default()
        .split(',')
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    names.is_empty() || names.iter().any(|name| codecs.contains(&name.as_str()))
}

/// Whether a source size is known and within a client limit. No limit always fits.
fn size_fits(limit: Option<u32>, size: Option<u32>) -> bool {
    limit.is_none_or(|max| size.is_some_and(|s| s <= max))
}

/// Whether a source bitrate (bits/s) is known and within a client limit (kbps).
/// No limit always fits.
fn bitrate_fits(limit_kbps: Option<u32>, bits_per_sec: Option<u32>) -> bool {
    limit_kbps
        .is_none_or(|kbps| bits_per_sec.is_some_and(|bps| u64::from(bps) <= u64::from(kbps) * 1000))
}

/// Whether a profile name marks a 10-bit, 4:2:2 or 4:4:4 stream (e.g. "High 10", "High 4:2:2").
fn is_deep_profile(profile: &str) -> bool {
    let profile = profile.to_ascii_lowercase();
    ["10", "422", "4:2:2", "444", "4:4:4"]
        .iter()
        .any(|mark| profile.contains(mark))
}

/// Which streams can be copied as they are, as (video, audio). A copy needs every rule to hold;
/// anything else is encoded.
fn stream_copy(query: &TranscodeQuery, source: &SourceInfo) -> (bool, bool) {
    let copy_video = !query.audio_only.unwrap_or(false)
        && !query.burn_subtitles.unwrap_or(false)
        && matches!(source.video_codec.as_deref(), Some("h264" | "avc" | "avc1"))
        && client_takes(query.video_codec.as_deref(), &["h264", "avc", "avc1"])
        && !source.video_profile.as_deref().is_some_and(is_deep_profile)
        && source.video_bit_depth.is_none_or(|bits| bits <= 8)
        && size_fits(query.max_width, source.width)
        && size_fits(query.max_height, source.height)
        && bitrate_fits(query.video_bitrate, source.video_bitrate);
    let copy_audio = matches!(source.audio_codecs.as_slice(), [codec] if codec == "aac")
        && client_takes(query.audio_codec.as_deref(), &["aac", "mp4a"])
        && query.audio_stream_index.is_none()
        && bitrate_fits(query.audio_bitrate, source.audio_bitrate);
    (copy_video, copy_audio)
}

async fn hls_master_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<TranscodeQuery>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    // Copy only what the client plays; with no source to read, nothing is copied
    let source =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .and_then(|sources| sources.into_iter().next());
    let info = source
        .as_ref()
        .map_or_else(SourceInfo::default, SourceInfo::from_source);
    let (copy_video, copy_audio) = stream_copy(&query, &info);
    // Every playlist reload passes here; the manager logs the ffmpeg command when it starts one
    tracing::debug!(item = %id, copy_video, copy_audio, "HLS playlist requested");

    let play_session_id = query.play_session_id.clone().unwrap_or_default();
    let device_id = query.device_id.clone().unwrap_or_default();
    let mut profile = query.into_profile();
    profile.copy_video = copy_video;
    profile.copy_audio = copy_audio;

    let (playlist_path, playback_id) = match state
        .transcode_manager
        .start_transcode(id, &item.path, &profile, &play_session_id, &device_id)
        .await
    {
        Ok(started) => started,
        // ffmpeg has not written the playlist yet; the client should ask again shortly
        Err(TranscodeError::PlaylistTimeout) => {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "5")],
            )
                .into_response());
        }
        Err(TranscodeError::FfmpegFailed(msg)) => {
            return Err(ApiError::Internal(format!("Transcode failed: {msg}")));
        }
        Err(e) => return Err(ApiError::Internal(format!("Transcode failed: {e}"))),
    };

    // Read the playlist and point its segments at the segment route, with this request's query
    let content = tokio::fs::read_to_string(&playlist_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read playlist: {e}")))?;
    let rewritten = rewrite_playlist(&content, &playback_id, raw_query.as_deref());

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/vnd.apple.mpegurl"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        rewritten,
    )
        .into_response())
}

/// Point each segment line of an FFmpeg playlist at the segment route:
/// `hls/{playback_id}/video/{segment}`, with `?{query}` when the query is non-empty. The query is
/// the playlist request's own, so the `api_key` in it authorises the segment requests. Empty lines
/// and `#` tags stay as they are.
fn rewrite_playlist(content: &str, playback_id: &Uuid, query: Option<&str>) -> String {
    let suffix = query
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    content
        .split('\n')
        .map(|line| {
            if line.is_empty() || line.starts_with('#') {
                line.to_string()
            } else {
                format!("hls/{playback_id}/video/{line}{suffix}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Handle Jellyfin HLS segment requests: /Videos/{id}/hls/{playbackId}/{type}/{segment}
async fn hls_segment(
    State(state): State<AppState>,
    Path((id, playback_id, segment_type, segment)): Path<(Uuid, Uuid, String, String)>,
) -> Result<Response, ApiError> {
    // segment_type is typically "video", "audio", or "sub" — we just ignore it
    // and serve the segment file by name
    let _ = segment_type;

    // The playback id names the session, which must belong to this item
    let segment_path = state
        .transcode_manager
        .segment_path(id, playback_id, &segment)
        .ok_or_else(|| ApiError::NotFound(format!("Segment {segment} not found for item {id}")))?;

    let data = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?;

    let content_type = if segment.ends_with(".ts") {
        "video/mp2t"
    } else if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        data,
    )
        .into_response())
}

/// Legacy segment endpoint for backward compatibility: /Videos/{id}/hls/{segment}
async fn hls_segment_legacy(
    State(state): State<AppState>,
    Path((id, segment)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let segment_path = state
        .transcode_manager
        .legacy_segment_path(id, &segment)
        .ok_or_else(|| ApiError::NotFound(format!("Segment {segment} not found for item {id}")))?;

    let data = tokio::fs::read(&segment_path)
        .await
        .map_err(|e| ApiError::Internal(format!("Cannot read segment: {e}")))?;

    let content_type = if segment.ends_with(".ts") {
        "video/mp2t"
    } else if segment.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        data,
    )
        .into_response())
}

/// Look up a query parameter by name, ignoring ASCII case (clients send both `PlaySessionId`
/// and `playSessionId`). A missing parameter reads as "".
fn query_value<'a>(query: &'a HashMap<String, String>, name: &str) -> &'a str {
    query
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value.as_str())
}

/// DELETE /Videos/{id}/Transcode: stop this item's transcodes for one play session or device.
async fn stop_transcode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let play_session_id = query_value(&query, "PlaySessionId");
    let device_id = query_value(&query, "DeviceId");
    state
        .transcode_manager
        .stop_play_session(Some(id), play_session_id, device_id)
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// DELETE /Videos/ActiveEncodings: stop the transcodes of a play session or device, any item.
/// A request that names neither is refused.
async fn stop_active_encodings(
    State(state): State<AppState>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let play_session_id = query_value(&query, "PlaySessionId");
    let device_id = query_value(&query, "DeviceId");
    if play_session_id.is_empty() && device_id.is_empty() {
        return Err(ApiError::BadRequest(
            "PlaySessionId or DeviceId is required".to_string(),
        ));
    }
    state
        .transcode_manager
        .stop_play_session(None, play_session_id, device_id)
        .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 8-bit H.264 High 1920x1080 source at 8 Mbit/s with one stream per `audio` codec.
    fn h264_with(audio: &[&str]) -> SourceInfo {
        SourceInfo {
            video_codec: Some("h264".into()),
            video_profile: Some("High".into()),
            video_bit_depth: Some(8),
            width: Some(1920),
            height: Some(1080),
            video_bitrate: Some(8_000_000),
            audio_codecs: audio.iter().map(|c| c.to_string()).collect(),
            audio_bitrate: Some(192_000),
        }
    }

    /// The lists of a browser that plays H.264 but not AC3.
    fn browser_query() -> TranscodeQuery {
        TranscodeQuery {
            video_codec: Some("h264,vp8,vp9,av1".into()),
            audio_codec: Some("aac,mp3,opus,flac".into()),
            ..Default::default()
        }
    }

    #[test]
    fn test_profile_encodes_h264_and_aac_whatever_the_lists_say() {
        let profile = TranscodeQuery {
            video_codec: Some("h264,hevc,vp8,vp9,av1".into()),
            audio_codec: Some("mp3,aac,opus,ac3,flac".into()),
            ..Default::default()
        }
        .into_profile();
        assert_eq!(profile.video_codec, "libx264");
        assert_eq!(profile.audio_codec, "aac");
        assert!(!profile.copy_video && !profile.copy_audio);
    }

    #[test]
    fn test_h264_with_ac3_copies_video_and_encodes_audio() {
        assert_eq!(
            stream_copy(&browser_query(), &h264_with(&["ac3"])),
            (true, false)
        );
    }

    #[test]
    fn test_h264_with_aac_copies_both() {
        assert_eq!(
            stream_copy(&browser_query(), &h264_with(&["aac"])),
            (true, true)
        );
        // A client without lists is assumed to play H.264 and AAC
        assert_eq!(
            stream_copy(&TranscodeQuery::default(), &h264_with(&["aac"])),
            (true, true)
        );
        // Limits the source meets keep the copy
        let query = TranscodeQuery {
            max_width: Some(1920),
            max_height: Some(1080),
            video_bitrate: Some(8000),
            audio_bitrate: Some(192),
            ..browser_query()
        };
        assert_eq!(stream_copy(&query, &h264_with(&["aac"])), (true, true));
    }

    #[test]
    fn test_video_is_encoded_when_it_must_be() {
        let encoded = |query: TranscodeQuery, source: SourceInfo| {
            assert!(!stream_copy(&query, &source).0, "{query:?} {source:?}");
        };
        // Not H.264, or a client that does not list it
        for codec in ["hevc", "mpeg4", "vp9"] {
            let source = SourceInfo {
                video_codec: Some(codec.into()),
                ..h264_with(&["aac"])
            };
            encoded(browser_query(), source);
        }
        let query = TranscodeQuery {
            video_codec: Some("hevc,vp9".into()),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        // 10-bit, 4:2:2 and 4:4:4 profiles, or more than 8 bits
        for profile in ["High 10", "High 4:2:2", "High 4:4:4 Predictive"] {
            let source = SourceInfo {
                video_profile: Some(profile.into()),
                ..h264_with(&["aac"])
            };
            encoded(browser_query(), source);
        }
        let source = SourceInfo {
            video_bit_depth: Some(10),
            ..h264_with(&["aac"])
        };
        encoded(browser_query(), source);
        // Larger than MaxWidth or MaxHeight, or of unknown size
        let query = TranscodeQuery {
            max_width: Some(1280),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            max_height: Some(720),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            max_width: Some(1920),
            ..browser_query()
        };
        let source = SourceInfo {
            width: None,
            ..h264_with(&["aac"])
        };
        encoded(query, source);
        // Over VideoBitrate (kbps), or of unknown bitrate
        let query = TranscodeQuery {
            video_bitrate: Some(4000),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            video_bitrate: Some(20000),
            ..browser_query()
        };
        let source = SourceInfo {
            video_bitrate: None,
            ..h264_with(&["aac"])
        };
        encoded(query, source);
        // Audio only, burned-in subtitles, or nothing known about the source
        let query = TranscodeQuery {
            audio_only: Some(true),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let query = TranscodeQuery {
            burn_subtitles: Some(true),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        encoded(browser_query(), SourceInfo::default());
    }

    #[test]
    fn test_audio_is_encoded_when_it_must_be() {
        let copied = |query: TranscodeQuery, source: SourceInfo| stream_copy(&query, &source).1;
        // Not AAC, or not exactly one audio stream
        assert!(!copied(browser_query(), h264_with(&["ac3"])));
        assert!(!copied(browser_query(), h264_with(&["aac", "aac"])));
        assert!(!copied(browser_query(), h264_with(&[])));
        // A client that does not list AAC, or a chosen audio stream
        let query = TranscodeQuery {
            audio_codec: Some("mp3,opus".into()),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac"])));
        let query = TranscodeQuery {
            audio_stream_index: Some(0),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac"])));
        // Over AudioBitrate (kbps), or of unknown bitrate
        let query = TranscodeQuery {
            audio_bitrate: Some(128),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac"])));
        let query = TranscodeQuery {
            audio_bitrate: Some(320),
            ..browser_query()
        };
        let source = SourceInfo {
            audio_bitrate: None,
            ..h264_with(&["aac"])
        };
        assert!(!copied(query, source));
        // "mp4a" in the list means AAC
        let query = TranscodeQuery {
            audio_codec: Some("mp4a".into()),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["aac"])));
    }

    #[test]
    fn test_source_info_reads_the_streams() {
        let source: rustyfin_model::media_source::MediaSourceResponse =
            serde_json::from_value(serde_json::json!({
                "Id": "source",
                "Uid": "source",
                "Protocol": "File",
                "Path": "/media/movie.mkv",
                "Bitrate": 9_000_000,
                "VideoBitrate": 8_000_000,
                "MediaStreams": [
                    { "Index": 0, "CodecType": "Video", "Codec": "h264", "Profile": "High",
                      "BitDepth": 8, "Width": 1920, "Height": 1080 },
                    { "Index": 1, "CodecType": "Audio", "Codec": "AC3", "BitRate": 640_000 },
                    { "Index": 2, "CodecType": "Audio", "Codec": "aac" },
                    { "Index": 3, "CodecType": "Subtitle", "Codec": "subrip" },
                ],
            }))
            .unwrap();
        let info = SourceInfo::from_source(&source);
        assert_eq!(info.video_codec.as_deref(), Some("h264"));
        assert_eq!(info.video_profile.as_deref(), Some("High"));
        assert_eq!(info.video_bit_depth, Some(8));
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        // No bitrate on the video stream: the source's video bitrate
        assert_eq!(info.video_bitrate, Some(8_000_000));
        assert_eq!(info.audio_codecs, ["ac3", "aac"]);
        assert_eq!(info.audio_bitrate, Some(640_000));
    }

    #[test]
    fn test_rewrite_points_segments_at_the_segment_route_with_auth() {
        let playback_id = Uuid::new_v4();
        let content = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n\
                       #EXTINF:6.000000,\nsegment_0000.ts\n#EXTINF:6.000000,\nsegment_0001.ts\n";
        let query = "PlaySessionId=psid&api_key=placeholder";
        let segment = |n: &str| format!("hls/{playback_id}/video/segment_{n}.ts?{query}");
        assert_eq!(
            rewrite_playlist(content, &playback_id, Some(query)),
            format!(
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n\
                 #EXTINF:6.000000,\n{}\n#EXTINF:6.000000,\n{}\n",
                segment("0000"),
                segment("0001")
            )
        );
    }

    #[test]
    fn test_rewrite_without_query() {
        let playback_id = Uuid::new_v4();
        let content = "#EXTM3U\n#EXTINF:6.000000,\nsegment_0000.ts\n\n#EXT-X-ENDLIST\n";
        let expected = format!(
            "#EXTM3U\n#EXTINF:6.000000,\nhls/{playback_id}/video/segment_0000.ts\n\n#EXT-X-ENDLIST\n"
        );
        assert_eq!(rewrite_playlist(content, &playback_id, None), expected);
        assert_eq!(rewrite_playlist(content, &playback_id, Some("")), expected);
    }
}
