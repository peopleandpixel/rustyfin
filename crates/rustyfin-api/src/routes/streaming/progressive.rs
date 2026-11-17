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
/// first. `source` is numbered as PlaybackInfo numbers it (`subtitle::number_streams`), the
/// numbers a client sends.
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
/// source's is higher. `source` is numbered as PlaybackInfo numbers it.
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
    let encoders: Vec<&'static str> = wanted.iter().filter_map(|c| audio_encoder(c)).collect();
    if encoders.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "No audio codec Rustyfin encodes is asked for in {container}"
        )));
    }

    let stream = source.and_then(|source| audio_stream(params, source));
    let externals = source.map_or(0, |source| {
        source
            .media_streams
            .iter()
            .filter(|s| s.is_external)
            .count() as i32
    });
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
        // ffmpeg maps the stream by its index in the file
        audio_stream_index: stream
            .and_then(|s| crate::routes::subtitle::file_index(s.index, externals))
            .and_then(|index| u32::try_from(index).ok()),
        start_time_ticks: param(params, "StartTimeTicks")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        source_audio_channels: source_channels,
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
    // The track's own codec, when the client lists it, is copied within the limits
    if source_codec.as_ref().is_some_and(|c| wanted.contains(c))
        && fits(bitrate, source_bitrate)
        && fits(channels, source_channels)
        && fits(rate, source_rate)
    {
        profile.copy_audio = true;
        return Ok(profile);
    }

    // Else the first listed codec Rustyfin encodes; under a bitrate limit the first lossy one,
    // as a lossless encode can't keep to a bitrate
    let encoder = encoders
        .iter()
        .copied()
        .find(|e| bitrate.is_none() || !matches!(*e, "flac" | "alac" | "pcm_s16le"))
        .unwrap_or(encoders[0]);
    profile.audio_codec = encoder.to_string();
    let max_kbps = if encoder == "libmp3lame" { 320 } else { 512 };
    profile.max_audio_bitrate_kbps = bitrate.map_or(192, |bps| (bps / 1000).clamp(32, max_kbps));
    profile.audio_channels = output_channels(channels, source_channels, encoder);
    profile.audio_sample_rate = output_sample_rate(rate, source_rate, encoder);
    Ok(profile)
}

/// The channels of an encoded audio stream: the fewest of the client's limit, the source's and
/// what the encoder writes (MP3 and MP2 stereo, AC-3 5.1, the others 7.1), so a stream is never
/// upmixed and ffmpeg never asks an encoder for more than it takes. Stereo when nothing is known.
pub(super) fn output_channels(
    limit: Option<u32>,
    source: Option<u32>,
    encoder: &str,
) -> Option<u32> {
    let encoder_max = match encoder {
        "libmp3lame" | "mp2" => 2,
        "ac3" | "eac3" => 6,
        _ => 8,
    };
    if limit.is_none() && source.is_none() {
        return Some(2);
    }
    [limit, source, Some(encoder_max)]
        .into_iter()
        .flatten()
        .filter(|&channels| channels > 0)
        .min()
}

/// The sample rate of an encoded audio stream: the client's limit (0 is none) when the source's
/// is higher or unknown, snapped down to a rate the encoder takes (Opus 48, 24, 16, 12 or 8 kHz;
/// MP3 the MPEG rates); None keeps the source's rate, which ffmpeg fits to the encoder itself.
pub(super) fn output_sample_rate(
    limit: Option<u32>,
    source: Option<u32>,
    encoder: &str,
) -> Option<u32> {
    let limit = limit.filter(|&rate| rate > 0)?;
    if source.is_some_and(|source| source <= limit) {
        return None;
    }
    let rates: &[u32] = match encoder {
        "libopus" => &[48000, 24000, 16000, 12000, 8000],
        "libmp3lame" => &[48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000],
        _ => return Some(limit),
    };
    Some(
        rates
            .iter()
            .copied()
            .find(|&rate| rate <= limit)
            .unwrap_or(rates[rates.len() - 1]),
    )
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

/// Who asks for a stream: their user policy and whether they are on the local network. The
/// remote bitrate limit and the transcoding and remuxing switches apply to them, as on the HLS
/// route.
pub(crate) struct Caller {
    pub access: rustyfin_model::parental::AccessPolicy,
    pub local: bool,
}

impl Caller {
    /// The caller of a request: no claims (a test's or the server's own request) has the default
    /// policy, and a request the middleware didn't mark is local.
    pub(crate) async fn of(
        state: &AppState,
        claims: Option<&rustyfin_model::auth::Claims>,
        local: Option<&crate::middleware::ClientIsLocal>,
    ) -> Result<Self, ApiError> {
        let access = match claims {
            Some(claims) => crate::parental::caller_access(state, claims).await?,
            None => Default::default(),
        };
        Ok(Self {
            access,
            local: local.is_none_or(|local| local.0),
        })
    }

    /// A requested bitrate within the remote limit (`parental::remote_bitrate`).
    pub(crate) fn bitrate(&self, requested: Option<u32>) -> Option<u32> {
        crate::parental::remote_bitrate(&self.access, self.local, requested)
    }

    /// `params` with the audio bitrate a progressive audio transcode reads (`AudioBitrate`,
    /// else `MaxAudioBitrate`, else `MaxStreamingBitrate`) within the remote limit, written as
    /// `AudioBitrate`. Unchanged when no limit applies.
    pub(super) fn clamp_audio(&self, params: &HashMap<String, String>) -> HashMap<String, String> {
        let mut params = params.clone();
        let requested = number(
            &params,
            &["AudioBitrate", "MaxAudioBitrate", "MaxStreamingBitrate"],
        );
        let clamped = self.bitrate(requested);
        if clamped != requested
            && let Some(bitrate) = clamped
        {
            params.retain(|name, _| !name.eq_ignore_ascii_case("AudioBitrate"));
            params.insert("AudioBitrate".to_string(), bitrate.to_string());
        }
        params
    }
}

/// The response of a progressive transcode: 200 with the container's type and ffmpeg's output
/// as a chunked body, once ffmpeg has written its first bytes. HEAD gets the headers only and
/// starts nothing. The caller's policy is checked first, as on the HLS route: a transcode that
/// re-encodes needs the transcoding switch for the item's kind, and one that only copies needs
/// `EnablePlaybackRemuxing` (403 otherwise). A container Rustyfin can't stream is a 400; an
/// ffmpeg that writes nothing is a 500 with its last stderr lines.
pub(super) async fn respond(
    state: &AppState,
    caller: &Caller,
    job: Job<'_>,
    head: bool,
) -> Result<Response, ApiError> {
    crate::parental::check_conversion(
        &caller.access,
        job.profile.audio_only,
        job.profile.copy_video,
        job.profile.copy_audio,
    )?;
    let (format, content_type) = progressive_format(job.container, job.profile.audio_only)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Rustyfin can't stream a transcode in {}",
                job.container
            ))
        })?;
    let headers = [
        (header::CONTENT_TYPE, content_type),
        (header::ACCEPT_RANGES, "none"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    if head {
        return Ok((StatusCode::OK, headers).into_response());
    }

    let manager = state.transcode_manager.clone();
    // A hardware transcode that writes nothing is run again in software (the file stays in
    // software from then on, logged once): at most two tries
    let mut tries = 0;
    let (guard, output, first) = loop {
        tries += 1;
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
                TranscodeError::Stopped => {
                    ApiError::NotFound("The play session was stopped".into())
                }
                e @ (TranscodeError::TooMany(_) | TranscodeError::ShuttingDown) => {
                    ApiError::ServiceUnavailable(e.to_string())
                }
                e => ApiError::Internal(format!("Transcode failed: {e}")),
            })?;
        let guard = EndOnDrop {
            manager: manager.clone(),
            playback_id,
        };
        let hardware = manager.progressive_uses_hardware(playback_id);
        let mut output = tokio_util::io::ReaderStream::with_capacity(stdout, 64 * 1024);
        match tokio::time::timeout(FIRST_BYTES_WAIT, output.next()).await {
            Ok(Some(Ok(bytes))) => break (guard, output, bytes),
            Ok(Some(Err(e))) => return Err(ApiError::Internal(format!("Transcode failed: {e}"))),
            Ok(None) => {
                let error = manager.progressive_failure(playback_id).await;
                if hardware && tries < 2 {
                    manager.note_progressive_hardware_failure(job.input_path, &error.to_string());
                    continue;
                }
                return Err(ApiError::Internal(format!("Transcode failed: {error}")));
            }
            Err(_) => {
                return Err(ApiError::ServiceUnavailable(
                    "The transcode wrote nothing in time".into(),
                ));
            }
        }
    };
    let first: Bytes = first;
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
        // Under a limit above the track's bitrate it is copied too
        let profile = audio_profile(
            &query(&[("AudioCodec", "flac,mp3"), ("AudioBitrate", "2000000")]),
            Some(&flac_source(2, 44100)),
            "mka",
        )
        .unwrap();
        assert!(profile.copy_audio);
        // Past the limit it is encoded, to the lossy codec the client lists, at the limit
        let profile = audio_profile(
            &query(&[("AudioCodec", "flac,mp3"), ("AudioBitrate", "320000")]),
            Some(&flac_source(2, 44100)),
            "mka",
        )
        .unwrap();
        assert!(!profile.copy_audio);
        assert_eq!(profile.audio_codec, "libmp3lame");
        assert_eq!(profile.max_audio_bitrate_kbps, 320);
    }

    /// Channels: never above the limit, the source or the encoder; sample rates: a limit of 0
    /// is none, and the limit snaps down to a rate the encoder takes.
    #[test]
    fn r3gaps2_channels_and_rates_fit_the_encoder() {
        assert_eq!(output_channels(Some(6), Some(2), "aac"), Some(2));
        assert_eq!(output_channels(Some(6), Some(6), "libmp3lame"), Some(2));
        assert_eq!(output_channels(Some(2), Some(6), "ac3"), Some(2));
        assert_eq!(output_channels(None, Some(8), "ac3"), Some(6));
        assert_eq!(output_channels(None, None, "aac"), Some(2));
        assert_eq!(output_sample_rate(Some(0), Some(96000), "aac"), None);
        assert_eq!(
            output_sample_rate(Some(44100), Some(96000), "libopus"),
            Some(24000)
        );
        assert_eq!(
            output_sample_rate(Some(44100), Some(96000), "libmp3lame"),
            Some(44100)
        );
        assert_eq!(
            output_sample_rate(Some(48000), Some(44100), "libopus"),
            None
        );
        assert_eq!(output_sample_rate(Some(5000), None, "libopus"), Some(8000));
    }

    /// A codec Rustyfin doesn't encode is a 400.
    #[test]
    fn r3gaps2_unknown_codec_is_refused() {
        assert!(audio_profile(&query(&[("AudioCodec", "dts")]), None, "mka").is_err());
    }
}
