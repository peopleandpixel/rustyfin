use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use serde::Deserialize;
use uuid::Uuid;

use rustyfin_core::subtitle::is_image_subtitle;
use rustyfin_core::transcode::{HwAccel, ImageSubtitle, TranscodeError, TranscodeProfile};
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};

use super::streaming::{Version, requested_source_id, select_version};
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
    /// Absolute index of the subtitle stream to burn in: absent is the first, negative is none
    pub subtitle_stream_index: Option<i32>,
    pub burn_subtitles: Option<bool>,
    /// Jellyfin's SubtitleMethod: "Encode" burns the subtitle in, in any case
    pub subtitle_method: Option<String>,
    /// Absolute index of the audio stream to play: absent, negative or not an audio stream of the
    /// source is the default audio stream
    pub audio_stream_index: Option<i32>,
    /// Audio-only transcoding (no video)
    pub audio_only: Option<bool>,
    /// Enable adaptive bitrate transcoding
    pub adaptive_bitrate: Option<bool>,
    /// Jellyfin play session id: the transcode session key
    pub play_session_id: Option<String>,
    /// Jellyfin device id: the session key when there is no play session id
    pub device_id: Option<String>,
    /// Where to start, in ticks: the resume or seek point Universal forwards
    pub start_time_ticks: Option<i64>,
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
        if let Some(v) = self.audio_only {
            p.audio_only = v;
        }
        if let Some(v) = self.adaptive_bitrate {
            p.adaptive_bitrate = v;
        }
        if let Some(v) = self.start_time_ticks {
            p.start_time_ticks = v;
        }
        p
    }
}

/// An audio or subtitle stream of the source that a transcode can play.
#[derive(Debug, Clone, Default)]
struct Track {
    index: u32,           // absolute stream index (MediaStream.Index)
    codec: String,        // lowercase
    bitrate: Option<u32>, // bits/s
    is_default: bool,
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
    audio: Vec<Track>,          // audio streams, in stream order
    audio_bitrate: Option<u32>, // bits/s: the source's audio_bitrate, for a track without its own
    subtitles: Vec<Track>,      // embedded subtitle streams, in stream order
}

impl SourceInfo {
    fn from_source(source: &MediaSourceResponse) -> Self {
        // The probe keeps ffprobe's lowercase stream types, so the types are matched without case
        let video = source
            .media_streams
            .iter()
            .find(|s| s.codec_type.eq_ignore_ascii_case("video"));
        let audio: Vec<Track> = source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("audio"))
            .filter_map(source_track)
            .collect();
        // An external subtitle file is not a stream of the source
        let subtitles: Vec<Track> = source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("subtitle") && !s.is_external)
            .filter_map(source_track)
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
            audio,
            audio_bitrate: source.audio_bitrate,
            subtitles,
        }
    }
}

/// A source stream as a track, with its codec lowercased. A negative index names no stream, so it
/// is no track.
fn source_track(stream: &MediaStream) -> Option<Track> {
    Some(Track {
        index: u32::try_from(stream.index).ok()?,
        codec: stream
            .codec
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase(),
        bitrate: stream.bit_rate,
        is_default: stream.is_default,
    })
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

/// What is burned into the picture: a picture subtitle overlaid by its absolute stream index, or a
/// text subtitle drawn by the subtitles filter at its place among the source's subtitles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BurnIn {
    Image(u32),
    Text(u32),
}

/// The tracks a transcode plays: the audio stream by its absolute index, and the burn-in.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tracks {
    audio: Option<u32>,
    burn_in: Option<BurnIn>,
}

/// Whether the request asks for a subtitle to be burned in: BurnSubtitles=true, or a
/// SubtitleMethod of Encode in any case.
fn burn_requested(query: &TranscodeQuery) -> bool {
    query.burn_subtitles == Some(true)
        || query
            .subtitle_method
            .as_deref()
            .is_some_and(|method| method.eq_ignore_ascii_case("encode"))
}

/// The tracks a transcode plays. The audio is the stream the request names, else the default one,
/// else the first. The burn-in is the subtitle the request names, else the first subtitle stream.
fn choose_tracks(query: &TranscodeQuery, source: &SourceInfo) -> Tracks {
    let requested = query
        .audio_stream_index
        .and_then(|index| u32::try_from(index).ok())
        .and_then(|index| source.audio.iter().find(|track| track.index == index));
    let audio = requested
        .or_else(|| source.audio.iter().find(|track| track.is_default))
        .or_else(|| source.audio.first())
        .map(|track| track.index);

    let burn_in = if burn_requested(query) {
        // The subtitle with its place among the subtitle streams. A negative index matches no
        // subtitle stream, so it burns nothing in.
        let subtitle = match query.subtitle_stream_index {
            None => source.subtitles.first().map(|track| (0, track)),
            Some(index) => source
                .subtitles
                .iter()
                .zip(0..)
                .find(|(track, _)| i64::from(track.index) == i64::from(index))
                .map(|(track, place)| (place, track)),
        };
        subtitle.map(|(place, track)| {
            if is_image_subtitle(&track.codec) {
                BurnIn::Image(track.index)
            } else {
                BurnIn::Text(place)
            }
        })
    } else {
        None
    };

    Tracks { audio, burn_in }
}

/// Which streams can be copied as they are, as (video, audio). A copy needs every rule to hold;
/// anything else is encoded.
fn stream_copy(query: &TranscodeQuery, source: &SourceInfo) -> (bool, bool) {
    let tracks = choose_tracks(query, source);
    let audio = tracks
        .audio
        .and_then(|index| source.audio.iter().find(|track| track.index == index));
    let copy_video = !query.audio_only.unwrap_or(false)
        && tracks.burn_in.is_none()
        && matches!(source.video_codec.as_deref(), Some("h264" | "avc" | "avc1"))
        && client_takes(query.video_codec.as_deref(), &["h264", "avc", "avc1"])
        && !source.video_profile.as_deref().is_some_and(is_deep_profile)
        && source.video_bit_depth.is_none_or(|bits| bits <= 8)
        && size_fits(query.max_width, source.width)
        && size_fits(query.max_height, source.height)
        && bitrate_fits(query.video_bitrate, source.video_bitrate);
    let copy_audio = audio.is_some_and(|track| {
        track.codec == "aac"
            && client_takes(query.audio_codec.as_deref(), &["aac", "mp4a"])
            && bitrate_fits(query.audio_bitrate, track.bitrate.or(source.audio_bitrate))
    });
    (copy_video, copy_audio)
}

/// The profile of a transcode: the query's settings, the streams it copies, and the tracks it
/// plays.
fn transcode_profile(query: TranscodeQuery, source: &SourceInfo) -> TranscodeProfile {
    let (copy_video, copy_audio) = stream_copy(&query, source);
    let tracks = choose_tracks(&query, source);
    let mut profile = query.into_profile();
    profile.copy_video = copy_video;
    profile.copy_audio = copy_audio;
    profile.audio_stream_index = tracks.audio;
    match tracks.burn_in {
        Some(BurnIn::Image(stream_index)) => {
            profile.image_subtitle = Some(ImageSubtitle {
                stream_index,
                video_size: source.width.zip(source.height),
            });
        }
        Some(BurnIn::Text(place)) => {
            profile.burn_subtitles = true;
            profile.subtitle_stream_index = Some(place);
        }
        None => {}
    }
    profile
}

/// ffmpeg's input for `version` of an item at `item_path`, and the source whose streams decide
/// stream copy.
fn hls_input<'a>(
    item_path: &'a str,
    sources: &'a [MediaSourceResponse],
    version: Version,
) -> (&'a str, Option<&'a MediaSourceResponse>) {
    match version {
        Version::Primary => (item_path, sources.first()),
        Version::Source(index) => (sources[index].path.as_str(), Some(&sources[index])),
    }
}

async fn hls_master_playlist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<TranscodeQuery>,
    Query(params): Query<HashMap<String, String>>,
    RawQuery(raw_query): RawQuery,
) -> Result<Response, ApiError> {
    let item = rustyfin_db::repo_media::get_item_by_id(&state.db, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Item {id} not found")))?;

    // The version the request names; an id that names none is refused before ffmpeg starts
    let sources =
        rustyfin_db::repo_media_source::get_media_sources_with_item_check(&state.db_ro, &id)
            .await
            .unwrap_or_default();
    let version = select_version(id, &sources, requested_source_id(&params))?;
    let (input_path, source) = hls_input(&item.path, &sources, version);

    // Copy only what the client plays; with no source to read, nothing is copied
    let info = source.map_or_else(SourceInfo::default, SourceInfo::from_source);
    let tracks = choose_tracks(&query, &info);
    let (copy_video, copy_audio) = stream_copy(&query, &info);
    // Every playlist reload passes here; the manager logs the ffmpeg command when it starts one
    tracing::debug!(
        item = %id,
        copy_video,
        copy_audio,
        audio = ?tracks.audio,
        burn_in = ?tracks.burn_in,
        "HLS playlist requested"
    );
    // Burn-in asked for by a stream index that no subtitle stream has: nothing is burned in
    if tracks.burn_in.is_none()
        && burn_requested(&query)
        && query.subtitle_stream_index.is_some_and(|index| index >= 0)
    {
        tracing::debug!(
            item = %id,
            subtitle = ?query.subtitle_stream_index,
            "Burn-in names no subtitle stream"
        );
    }

    let play_session_id = query.play_session_id.clone().unwrap_or_default();
    let device_id = query.device_id.clone().unwrap_or_default();
    let profile = transcode_profile(query, &info);

    let (playlist_path, playback_id) = match state
        .transcode_manager
        .start_transcode(id, input_path, &profile, &play_session_id, &device_id)
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
        Err(TranscodeError::Stopped) => {
            return Err(ApiError::NotFound("The play session was stopped".into()));
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

    /// An audio or subtitle stream at absolute index `index`.
    fn track(index: u32, codec: &str) -> Track {
        Track {
            index,
            codec: codec.into(),
            ..Default::default()
        }
    }

    /// An 8-bit H.264 High 1920x1080 source at 8 Mbit/s: the video is stream 0, then one audio
    /// stream per `audio` codec from index 1, none flagged default, with 192 kbit/s audio.
    fn h264_with(audio: &[&str]) -> SourceInfo {
        SourceInfo {
            video_codec: Some("h264".into()),
            video_profile: Some("High".into()),
            video_bit_depth: Some(8),
            width: Some(1920),
            height: Some(1080),
            video_bitrate: Some(8_000_000),
            audio: audio.iter().zip(1..).map(|(c, i)| track(i, c)).collect(),
            audio_bitrate: Some(192_000),
            subtitles: Vec::new(),
        }
    }

    /// Ghost Stories' streams: 1440x1080 H.264; FLAC Japanese (default) and English at 1 and 2;
    /// PGS at 3, 4 and 5; SubRip at 6 (default), 7 and 8.
    fn anime() -> SourceInfo {
        SourceInfo {
            width: Some(1440),
            audio: vec![
                Track {
                    is_default: true,
                    ..track(1, "flac")
                },
                track(2, "flac"),
            ],
            subtitles: vec![
                track(3, "hdmv_pgs_subtitle"),
                track(4, "hdmv_pgs_subtitle"),
                track(5, "hdmv_pgs_subtitle"),
                Track {
                    is_default: true,
                    ..track(6, "subrip")
                },
                track(7, "subrip"),
                track(8, "subrip"),
            ],
            ..h264_with(&[])
        }
    }

    /// The query a playlist request with `params` carries, parsed as the route parses it.
    fn query(params: &str) -> TranscodeQuery {
        let uri = format!("/Videos/item/main.m3u8?{params}").parse().unwrap();
        Query::<TranscodeQuery>::try_from_uri(&uri).unwrap().0
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
    fn test_start_time_ticks_reaches_the_profile() {
        // Universal forwards a resume or seek point as StartTimeTicks
        let query: TranscodeQuery =
            serde_json::from_value(serde_json::json!({ "StartTimeTicks": 6_000_000_000_i64 }))
                .unwrap();
        assert_eq!(query.into_profile().start_time_ticks, 6_000_000_000);
        assert_eq!(TranscodeQuery::default().into_profile().start_time_ticks, 0);
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
        // BurnSubtitles on a source without subtitle streams burns nothing in, so the copy stays
        let query = TranscodeQuery {
            burn_subtitles: Some(true),
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
        // Audio only, a subtitle to burn in, or nothing known about the source
        let query = TranscodeQuery {
            audio_only: Some(true),
            ..browser_query()
        };
        encoded(query, h264_with(&["aac"]));
        let with_subtitles = || SourceInfo {
            subtitles: vec![track(2, "subrip"), track(3, "dvd_subtitle")],
            ..h264_with(&["aac"])
        };
        let query = TranscodeQuery {
            burn_subtitles: Some(true),
            ..browser_query()
        };
        encoded(query, with_subtitles());
        for index in [2, 3] {
            let query = TranscodeQuery {
                subtitle_stream_index: Some(index),
                subtitle_method: Some("Encode".into()),
                ..browser_query()
            };
            encoded(query, with_subtitles());
        }
        encoded(browser_query(), SourceInfo::default());
    }

    #[test]
    fn test_audio_is_encoded_when_it_must_be() {
        let copied = |query: TranscodeQuery, source: SourceInfo| stream_copy(&query, &source).1;
        // The audio stream that plays is not AAC: the first when none is flagged, the chosen one,
        // or none at all
        assert!(!copied(browser_query(), h264_with(&["ac3"])));
        assert!(!copied(browser_query(), h264_with(&["ac3", "aac"])));
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(!copied(query, h264_with(&["aac", "ac3"])));
        assert!(!copied(browser_query(), h264_with(&[])));
        // A client that does not list AAC
        let query = TranscodeQuery {
            audio_codec: Some("mp3,opus".into()),
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
    fn test_the_aac_stream_that_plays_is_copied() {
        let copied = |query: TranscodeQuery, source: SourceInfo| stream_copy(&query, &source).1;
        // Chosen by its index beside other audio streams, or flagged as the default
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["ac3", "aac"])));
        let query = TranscodeQuery {
            audio_stream_index: Some(2),
            ..browser_query()
        };
        assert!(copied(query, h264_with(&["aac", "aac"])));
        let source = SourceInfo {
            audio: vec![
                track(1, "ac3"),
                Track {
                    is_default: true,
                    ..track(2, "aac")
                },
            ],
            ..h264_with(&[])
        };
        assert!(copied(browser_query(), source));
        // The stream's own bitrate counts against AudioBitrate before the source's
        let source = SourceInfo {
            audio: vec![Track {
                bitrate: Some(96_000),
                ..track(1, "aac")
            }],
            audio_bitrate: Some(640_000),
            ..h264_with(&[])
        };
        let query = TranscodeQuery {
            audio_bitrate: Some(128),
            ..browser_query()
        };
        assert!(copied(query, source));
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
                "AudioBitrate": 448_000,
                "MediaStreams": [
                    { "Index": 0, "CodecType": "Video", "Codec": "h264", "Profile": "High",
                      "BitDepth": 8, "Width": 1920, "Height": 1080 },
                    { "Index": 1, "CodecType": "Audio", "Codec": "AC3", "BitRate": 640_000 },
                    { "Index": 2, "CodecType": "Audio", "Codec": "aac", "IsDefault": true },
                    { "Index": 3, "CodecType": "Subtitle", "Codec": "subrip" },
                    { "Index": 4, "CodecType": "subtitle", "Codec": "hdmv_pgs_subtitle" },
                    { "Index": 5, "CodecType": "subtitle", "Codec": "srt", "IsExternal": true,
                      "Path": "/media/movie.en.srt" },
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
        // Each audio stream by its index, with a lowercase codec, its bitrate and its flag
        let tracks = |list: &[Track]| -> Vec<(u32, String, Option<u32>, bool)> {
            list.iter()
                .map(|t| (t.index, t.codec.clone(), t.bitrate, t.is_default))
                .collect()
        };
        assert_eq!(
            tracks(&info.audio),
            [
                (1, "ac3".to_string(), Some(640_000), false),
                (2, "aac".to_string(), None, true)
            ]
        );
        assert_eq!(info.audio_bitrate, Some(448_000));
        // The embedded subtitle streams; an external file is not a stream of the source
        assert_eq!(
            tracks(&info.subtitles),
            [
                (3, "subrip".to_string(), None, false),
                (4, "hdmv_pgs_subtitle".to_string(), None, false)
            ]
        );
    }

    #[test]
    fn test_negative_indexes_and_the_subtitle_method_parse() {
        // Jellyfin clients send -1 for "none"; as unsigned numbers that failed the whole query
        let q = query("AudioStreamIndex=-1&SubtitleStreamIndex=-1&SubtitleMethod=Encode");
        assert_eq!(q.audio_stream_index, Some(-1));
        assert_eq!(q.subtitle_stream_index, Some(-1));
        assert_eq!(q.subtitle_method.as_deref(), Some("Encode"));
    }

    #[test]
    fn test_audio_index_picks_that_audio_stream() {
        let audio = |params: &str| choose_tracks(&query(params), &anime()).audio;
        assert_eq!(audio("AudioStreamIndex=2"), Some(2));
        assert_eq!(audio("AudioStreamIndex=1"), Some(1));
        // Absent, negative, or not one of the audio streams: the default audio stream
        for params in [
            "",
            "AudioStreamIndex=-1",
            "AudioStreamIndex=0",
            "AudioStreamIndex=6",
            "AudioStreamIndex=40",
        ] {
            assert_eq!(audio(params), Some(1), "{params}");
        }
        // The default is the flagged stream, else the first; with no audio streams there is none
        let flagged = SourceInfo {
            audio: vec![
                track(1, "ac3"),
                Track {
                    is_default: true,
                    ..track(2, "aac")
                },
            ],
            ..anime()
        };
        assert_eq!(choose_tracks(&query(""), &flagged).audio, Some(2));
        let unflagged = h264_with(&["ac3", "aac"]);
        assert_eq!(choose_tracks(&query(""), &unflagged).audio, Some(1));
        for source in [h264_with(&[]), SourceInfo::default()] {
            let tracks = choose_tracks(&query("AudioStreamIndex=1"), &source);
            assert_eq!(tracks.audio, None);
        }
    }

    #[test]
    fn test_subtitle_burn_in_follows_the_method_and_the_stream() {
        let burn_in = |params: &str| choose_tracks(&query(params), &anime()).burn_in;
        // A picture subtitle is overlaid by its index; a text one goes through the subtitles
        // filter by its place among the subtitle streams
        let image = burn_in("SubtitleStreamIndex=4&SubtitleMethod=Encode");
        assert_eq!(image, Some(BurnIn::Image(4)));
        let text = burn_in("SubtitleStreamIndex=7&SubtitleMethod=Encode");
        assert_eq!(text, Some(BurnIn::Text(4)));
        // The method matches without case; BurnSubtitles=true asks for it too
        let image = burn_in("SubtitleStreamIndex=3&SubtitleMethod=encode");
        assert_eq!(image, Some(BurnIn::Image(3)));
        let text = burn_in("SubtitleStreamIndex=6&BurnSubtitles=true");
        assert_eq!(text, Some(BurnIn::Text(3)));
        // Without an index, the first subtitle stream, as BurnSubtitles did before
        assert_eq!(burn_in("BurnSubtitles=true"), Some(BurnIn::Image(3)));
        assert_eq!(burn_in("SubtitleMethod=Encode"), Some(BurnIn::Image(3)));
        // Nothing: no burn-in asked for, an index of -1, or one that names no embedded subtitle
        // stream (an audio stream, or an external file past the embedded streams)
        for params in [
            "SubtitleStreamIndex=4",
            "SubtitleStreamIndex=4&SubtitleMethod=Hls",
            "SubtitleStreamIndex=4&SubtitleMethod=External",
            "SubtitleStreamIndex=4&BurnSubtitles=false",
            "SubtitleStreamIndex=-1&SubtitleMethod=Encode",
            "SubtitleStreamIndex=1&SubtitleMethod=Encode",
            "SubtitleStreamIndex=9&SubtitleMethod=Encode",
        ] {
            assert_eq!(burn_in(params), None, "{params}");
        }
        let none = choose_tracks(&query("BurnSubtitles=true"), &h264_with(&["aac"]));
        assert_eq!(none.burn_in, None);
    }

    #[test]
    fn test_profile_carries_the_chosen_tracks() {
        // The English audio and the "English Dub PGS" picture subtitle, from 10 minutes in
        let params = "AudioStreamIndex=2&SubtitleStreamIndex=4&SubtitleMethod=Encode\
                      &StartTimeTicks=6000000000";
        let p = transcode_profile(query(params), &anime());
        assert_eq!(p.audio_stream_index, Some(2));
        let overlay = ImageSubtitle {
            stream_index: 4,
            video_size: Some((1440, 1080)),
        };
        assert_eq!(p.image_subtitle, Some(overlay));
        assert!(!p.burn_subtitles);
        assert_eq!(p.subtitle_stream_index, None);
        assert!(!p.copy_video && !p.copy_audio);
        assert_eq!(p.start_time_ticks, 6_000_000_000);
        // A text subtitle: the subtitles filter with its place among the subtitle streams
        let p = transcode_profile(
            query("SubtitleStreamIndex=8&SubtitleMethod=Encode"),
            &anime(),
        );
        assert_eq!(p.audio_stream_index, Some(1));
        assert!(p.burn_subtitles);
        assert_eq!(p.subtitle_stream_index, Some(5));
        assert_eq!(p.image_subtitle, None);
        assert!(!p.copy_video);
        // No subtitle: nothing burned in, so the video is copied
        let p = transcode_profile(query("SubtitleStreamIndex=-1&BurnSubtitles=true"), &anime());
        assert!(!p.burn_subtitles);
        assert_eq!((p.subtitle_stream_index, p.image_subtitle), (None, None));
        assert!(p.copy_video);
        // A picture subtitle on a video of unknown size is not scaled
        let unsized_source = SourceInfo {
            width: None,
            ..anime()
        };
        let p = transcode_profile(
            query("SubtitleStreamIndex=5&SubtitleMethod=Encode"),
            &unsized_source,
        );
        let overlay = ImageSubtitle {
            stream_index: 5,
            video_size: None,
        };
        assert_eq!(p.image_subtitle, Some(overlay));
        // The chosen AAC stream is copied, and its index still maps it
        let p = transcode_profile(query("AudioStreamIndex=2"), &h264_with(&["ac3", "aac"]));
        assert_eq!(p.audio_stream_index, Some(2));
        assert!(p.copy_audio);
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

    // ── B16: the transcode follows the version ──

    #[test]
    fn test_hls_input_follows_the_version() {
        use crate::routes::streaming::Version;

        let source = |path: &str, video: &str| -> MediaSourceResponse {
            serde_json::from_value(serde_json::json!({
                "Id": Uuid::new_v4().to_string(),
                "Uid": "source",
                "Container": "mkv",
                "Protocol": "File",
                "Path": path,
                "MediaStreams": [
                    { "Index": 0, "CodecType": "Video", "Codec": video,
                      "Width": 1920, "Height": 1080 },
                    { "Index": 1, "CodecType": "Audio", "Codec": "aac" },
                ],
            }))
            .unwrap()
        };
        // The primary is H.264, which the browser takes as it is; the other version is HEVC
        let primary = "/films/Film - 1080p.mkv";
        let sources = vec![
            source(primary, "h264"),
            source("/films/Film - 2160p.mkv", "hevc"),
        ];
        let query = browser_query();

        let (input, chosen) = hls_input(primary, &sources, Version::Primary);
        assert_eq!(input, primary);
        let chosen = chosen.expect("the primary's streams");
        assert_eq!(chosen.id, sources[0].id);
        let info = SourceInfo::from_source(chosen);
        assert_eq!(stream_copy(&query, &info), (true, true));

        let (input, chosen) = hls_input(primary, &sources, Version::Source(1));
        assert_eq!(input, "/films/Film - 2160p.mkv");
        let chosen = chosen.expect("the version's streams");
        assert_eq!(chosen.id, sources[1].id);
        // The HEVC version's video is encoded, not copied
        let info = SourceInfo::from_source(chosen);
        assert_eq!(stream_copy(&query, &info), (false, true));

        // A movie that was never probed: its own file, and no streams to copy
        let (input, chosen) = hls_input("/films/Bare.mkv", &[], Version::Primary);
        assert_eq!(input, "/films/Bare.mkv");
        assert!(chosen.is_none());
    }
}
