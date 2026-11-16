//! Progressive (HTTP) transcodes: ffmpeg's output piped as a chunked response, as Jellyfin's
//! progressive jobs answer `/Audio/{id}/universal` with an HTTP transcoding protocol,
//! `/Audio/{id}/stream.{container}` and `/Videos/{id}/stream` without `static=true` (FLAC to MP3
//! for Finamp, Chromecast and DLNA renderers, for example). The job is registered with the
//! transcode manager, so a stop by PlaySessionId or DeviceId and the session cleanup kill it;
//! ffmpeg is killed when the client goes away.

use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use rustyfin_core::transcode::{
    TranscodeError, TranscodeManager, TranscodeProfile, audio_encoder, progressive_format,
};
use rustyfin_model::media_source::{MediaSourceResponse, MediaStream};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// How long a progressive transcode may take to write its first bytes.
const FIRST_BYTES_WAIT: Duration = Duration::from_secs(30);

/// The value of query parameter `name`, in any ASCII case. Blank is absent.
pub(super) fn param<'a>(params: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

/// The number in the first of `names` that the query carries and that parses.
fn number(params: &HashMap<String, String>, names: &[&str]) -> Option<u32> {
    names
        .iter()
        .find_map(|name| param(params, name).and_then(|value| value.parse().ok()))
}

/// The most audio channels the client takes, from the names Jellyfin's URLs use.
pub(super) fn channel_limit(params: &HashMap<String, String>) -> Option<u32> {
    number(
        params,
        &[
            "AudioChannels",
            "TranscodingMaxAudioChannels",
            "MaxAudioChannels",
        ],
    )
    .filter(|&channels| channels > 0)
}

/// The audio codec a container implies when the request names none.
fn container_codec(container: &str) -> Option<&'static str> {
    Some(match container.to_ascii_lowercase().as_str() {
        "mp3" => "mp3",
        "aac" | "adts" | "m4a" | "m4b" | "mp4" => "aac",
        "ogg" | "oga" => "vorbis",
        "opus" | "webm" | "webma" => "opus",
        "flac" => "flac",
        "wav" => "wav",
        _ => return None,
    })
}

/// The audio stream a request plays: the one at `AudioStreamIndex`, else the default, else the
/// first.
fn audio_stream<'a>(
    params: &HashMap<String, String>,
    source: &'a MediaSourceResponse,
) -> Option<&'a MediaStream> {
    let audio = || {
        source
            .media_streams
            .iter()
            .filter(|s| s.codec_type.eq_ignore_ascii_case("audio") && !s.is_external)
    };
    let wanted = param(params, "AudioStreamIndex").and_then(|v| v.parse::<i32>().ok());
    wanted
        .and_then(|index| audio().find(|s| s.index == index))
        .or_else(|| audio().find(|s| s.is_default))
        .or_else(|| audio().next())
}

/// The profile of a progressive audio transcode into `container`. The track is copied when its
/// codec is the one asked for and fits the bitrate, channel and sample rate limits; else it is
/// encoded with the first asked-for codec Rustyfin encodes (`AudioCodec`, else the container's),
/// at `AudioBitrate` (else `MaxAudioBitrate`, else `MaxStreamingBitrate`; MP3 at most 320 kbps),
/// with no more channels than the source or the limit, and the limit's sample rate when the
/// source's is higher.
pub(super) fn audio_profile(
    params: &HashMap<String, String>,
    source: Option<&MediaSourceResponse>,
    container: &str,
) -> Result<TranscodeProfile, ApiError> {
    let implied = container_codec(container);
    let mut wanted: Vec<String> = param(params, "AudioCodec")
        .unwrap_or_default()
        .split(',')
        .map(|codec| codec.trim().to_ascii_lowercase())
        .filter(|codec| !codec.is_empty())
        .collect();
    // A container that implies a codec takes only that one
    if let Some(implied) = implied {
        wanted.retain(|codec| codec == implied || (implied == "aac" && codec == "mp4a"));
        if wanted.is_empty() {
            wanted.push(implied.to_string());
        }
    }
    let codec = wanted
        .iter()
        .find(|codec| audio_encoder(codec).is_some())
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "No audio codec Rustyfin encodes is asked for in {container}"
            ))
        })?;
    let encoder = audio_encoder(codec).unwrap_or("aac");

    let stream = source.and_then(|source| audio_stream(params, source));
    let source_channels = stream.and_then(|s| s.channels);
    let source_rate = stream.and_then(|s| s.sample_rate);
    let source_bitrate = stream
        .and_then(|s| s.bit_rate)
        .or(source.and_then(|s| s.bitrate));
    let bitrate = number(
        params,
        &["AudioBitrate", "MaxAudioBitrate", "MaxStreamingBitrate"],
    );
    let channels = channel_limit(params);
    let rate = number(params, &["AudioSampleRate", "MaxAudioSampleRate"]);

    let mut profile = TranscodeProfile {
        audio_only: true,
        audio_stream_index: stream.and_then(|s| u32::try_from(s.index).ok()),
        start_time_ticks: param(params, "StartTimeTicks")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        ..TranscodeProfile::default()
    };
    let source_codec = stream
        .and_then(|s| s.codec.as_deref())
        .map(str::to_ascii_lowercase);
    let fits = |limit: Option<u32>, value: Option<u32>| match (limit, value) {
        (Some(limit), Some(value)) => value <= limit,
        (Some(_), None) => false,
        (None, _) => true,
    };
    if source_codec.as_deref() == Some(codec.as_str())
        && fits(bitrate, source_bitrate)
        && fits(channels, source_channels)
        && fits(rate, source_rate)
    {
        profile.copy_audio = true;
        return Ok(profile);
    }

    profile.audio_codec = encoder.to_string();
    let max_kbps = if encoder == "libmp3lame" { 320 } else { 512 };
    profile.max_audio_bitrate_kbps = bitrate.map_or(192, |bps| (bps / 1000).clamp(32, max_kbps));
    // MP3 has at most two channels; never more than the source has
    let encoder_channels = if encoder == "libmp3lame" {
        Some(2)
    } else {
        None
    };
    profile.audio_channels = [channels, source_channels, encoder_channels]
        .into_iter()
        .flatten()
        .min()
        .or(Some(2));
    profile.audio_sample_rate = rate.filter(|&rate| source_rate.is_none_or(|src| src > rate));
    Ok(profile)
}

/// One progressive transcode to start.
pub(super) struct Job<'a> {
    pub item_id: Uuid,
    /// The file ffmpeg reads, already checked against the libraries
    pub input_path: &'a str,
    pub profile: TranscodeProfile,
    /// The output container, as the client names it (`mp3`, `ts`, `mkv`, ...)
    pub container: &'a str,
    pub play_session_id: String,
    pub device_id: String,
}

/// Ends a progressive transcode when the response body is dropped: the transcode finished, or
/// the client went away.
struct EndOnDrop {
    manager: TranscodeManager,
    playback_id: Uuid,
}

impl Drop for EndOnDrop {
    fn drop(&mut self) {
        let (manager, playback_id) = (self.manager.clone(), self.playback_id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if manager.end_progressive(playback_id).await {
                    tracing::debug!(%playback_id, "Progressive transcode ended");
                }
            });
        }
    }
}

/// The response of a progressive transcode: 200 with the container's type and ffmpeg's output
/// as a chunked body, once ffmpeg has written its first bytes. HEAD gets the headers only and
/// starts nothing. A container Rustyfin can't stream is a 400; an ffmpeg that writes nothing is
/// a 500 with its last stderr lines.
pub(super) async fn respond(
    state: &AppState,
    job: Job<'_>,
    head: bool,
) -> Result<Response, ApiError> {
    let (format, content_type) = progressive_format(job.container, job.profile.audio_only)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Rustyfin can't stream a transcode in {}",
                job.container
            ))
        })?;
    // Hook point: the user-policy guard of the HLS route (transcoding and remuxing permissions,
    // the remote bitrate clamp) belongs here, before anything starts, so it covers every
    // progressive path (universal audio, /Audio/{id}/stream, /Videos/{id}/stream)
    let headers = [
        (header::CONTENT_TYPE, content_type),
        (header::ACCEPT_RANGES, "none"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    if head {
        return Ok((StatusCode::OK, headers).into_response());
    }

    let manager = state.transcode_manager.clone();
    let (playback_id, stdout) = manager
        .start_progressive(
            job.item_id,
            job.input_path,
            &job.profile,
            format,
            &job.play_session_id,
            &job.device_id,
        )
        .await
        .map_err(|e| match e {
            TranscodeError::Stopped => ApiError::NotFound("The play session was stopped".into()),
            e => ApiError::Internal(format!("Transcode failed: {e}")),
        })?;
    let guard = EndOnDrop {
        manager: manager.clone(),
        playback_id,
    };

    let mut output = tokio_util::io::ReaderStream::with_capacity(stdout, 64 * 1024);
    let first: Bytes = match tokio::time::timeout(FIRST_BYTES_WAIT, output.next()).await {
        Ok(Some(Ok(bytes))) => bytes,
        Ok(Some(Err(e))) => return Err(ApiError::Internal(format!("Transcode failed: {e}"))),
        Ok(None) => {
            let error = manager.progressive_failure(playback_id).await;
            return Err(ApiError::Internal(format!("Transcode failed: {error}")));
        }
        Err(_) => {
            return Err(ApiError::ServiceUnavailable(
                "The transcode wrote nothing in time".into(),
            ));
        }
    };
    // The guard lives as long as the body: dropping it (the end, or a client that went away)
    // kills ffmpeg
    let body = futures_util::stream::once(async move { Ok(first) })
        .chain(output)
        .map(move |chunk| {
            let _ = &guard;
            chunk
        });
    Ok((StatusCode::OK, headers, Body::from_stream(body)).into_response())
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

    fn flac_source(channels: i32, rate: i32) -> MediaSourceResponse {
        serde_json::from_value(serde_json::json!({
            "Id": "s", "Uid": "s", "Protocol": "File", "Path": "/x.flac", "Container": "flac",
            "Bitrate": 900000,
            "MediaStreams": [{"Index": 0, "CodecType": "audio", "Codec": "flac",
                              "Channels": channels, "SampleRate": rate, "IsDefault": true}],
        }))
        .unwrap()
    }

    /// FLAC to MP3: encoded with LAME at the asked bitrate (capped at 320k), stereo at most.
    #[test]
    fn r3gaps2_flac_to_mp3_honours_bitrate_channels_and_rate() {
        let source = flac_source(6, 96000);
        let profile = audio_profile(
            &query(&[
                ("AudioCodec", "mp3"),
                ("AudioBitrate", "500000"),
                ("AudioSampleRate", "44100"),
            ]),
            Some(&source),
            "mp3",
        )
        .unwrap();
        assert!(profile.audio_only && !profile.copy_audio);
        assert_eq!(profile.audio_codec, "libmp3lame");
        assert_eq!(profile.max_audio_bitrate_kbps, 320);
        assert_eq!(profile.audio_channels, Some(2));
        assert_eq!(profile.audio_sample_rate, Some(44100));
        assert_eq!(profile.audio_stream_index, Some(0));
    }

    /// AAC keeps a mono source mono and its lower sample rate; a channel limit applies.
    #[test]
    fn r3gaps2_aac_never_upmixes_and_keeps_a_lower_rate() {
        let profile = audio_profile(
            &query(&[("AudioSampleRate", "48000")]),
            Some(&flac_source(1, 22050)),
            "aac",
        )
        .unwrap();
        assert_eq!(profile.audio_codec, "aac");
        assert_eq!(profile.audio_channels, Some(1));
        assert_eq!(profile.audio_sample_rate, None);
        let profile = audio_profile(
            &query(&[("TranscodingMaxAudioChannels", "2")]),
            Some(&flac_source(6, 48000)),
            "m4a",
        )
        .unwrap();
        assert_eq!(profile.audio_channels, Some(2));
    }

    /// The source's own codec within the limits is copied.
    #[test]
    fn r3gaps2_same_codec_is_copied() {
        let profile = audio_profile(
            &query(&[("AudioCodec", "flac")]),
            Some(&flac_source(2, 44100)),
            "flac",
        )
        .unwrap();
        assert!(profile.copy_audio);
        // ... but not past a bitrate limit
        let profile = audio_profile(
            &query(&[("AudioCodec", "flac"), ("AudioBitrate", "320000")]),
            Some(&flac_source(2, 44100)),
            "flac",
        )
        .unwrap();
        assert!(!profile.copy_audio);
        assert_eq!(profile.audio_codec, "flac");
    }

    /// A codec Rustyfin doesn't encode is a 400.
    #[test]
    fn r3gaps2_unknown_codec_is_refused() {
        assert!(audio_profile(&query(&[("AudioCodec", "dts")]), None, "mka").is_err());
    }
}
