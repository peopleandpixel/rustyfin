//! The transcoding URL, a port of Jellyfin's `StreamInfo.ToUrl`.

use rustyfin_model::device_profile::{StreamProtocol, SubtitleDeliveryMethod, TranscodeSeekInfo};

use super::video::StreamInfo;
use crate::query_string::QueryBuilder;

/// The stream options that go into the URL, in insertion order. Setting a name again replaces
/// its value in place.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct StreamOptions {
    /// The option names and values, in insertion order.
    entries: Vec<(String, String)>,
}

impl StreamOptions {
    /// Sets the option `name` to `value`, replacing the value in place when `name` is set.
    pub(super) fn set(&mut self, name: &str, value: &str) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.0.eq_ignore_ascii_case(name))
        {
            entry.1 = value.to_string();
        } else {
            self.entries.push((name.to_string(), value.to_string()));
        }
    }

    /// Sets the option `name` for `qualifier`, as `qualifier-name`. An empty or missing qualifier
    /// sets the plain `name`.
    pub(super) fn set_qualified(&mut self, qualifier: Option<&str>, name: &str, value: &str) {
        match qualifier.filter(|q| !q.is_empty()) {
            Some(q) => self.set(&format!("{q}-{name}"), value),
            None => self.set(name, value),
        }
    }

    /// The value of the option `name`, if it is set.
    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.0.eq_ignore_ascii_case(name))
            .map(|entry| entry.1.as_str())
    }

    /// The value of the option `name` for `qualifier`. A missing or empty qualified value falls
    /// back to the plain `name`, as Jellyfin's `GetOption` does.
    pub(super) fn get_qualified(&self, qualifier: Option<&str>, name: &str) -> Option<&str> {
        let qualified = qualifier
            .filter(|q| !q.is_empty())
            .and_then(|q| self.get(&format!("{q}-{name}")));
        match qualified {
            Some(value) if !value.is_empty() => Some(value),
            _ => self.get(name),
        }
    }

    /// The options and their values, in insertion order.
    fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|entry| (entry.0.as_str(), entry.1.as_str()))
    }
}

impl StreamInfo {
    /// The transcode URL for this stream, as Jellyfin's `StreamInfo.ToUrl` writes it. An HLS
    /// transcode's path is `/Videos/{item}/master.m3u8`, also for audio; a progressive (HTTP)
    /// one's is `/audio/{item}/stream.{container}` or `/videos/{item}/stream.{container}`, with
    /// no SegmentContainer. The token goes in `ApiKey`, and the bitrates are in bits per second.
    /// Empty ids and tokens are left out. Every name and value is percent-encoded
    /// (`QueryBuilder`), so a `+`, `&` or `=` in a device id or token arrives as sent.
    pub(super) fn to_url(&self) -> String {
        let hls = self.sub_protocol == StreamProtocol::Hls;
        let mut params = QueryBuilder::new();
        if !self.device_id.is_empty() {
            params.pair("DeviceId", &self.device_id);
        }
        if !self.media_source_id.is_empty() {
            params.pair("MediaSourceId", &self.media_source_id);
        }
        if !self.video_codecs.is_empty() {
            params.pair("VideoCodec", self.video_codecs.join(","));
        }
        if !self.audio_codecs.is_empty() {
            params.pair("AudioCodec", self.audio_codecs.join(","));
        }
        if let Some(index) = self.audio_stream_index {
            params.pair("AudioStreamIndex", index);
        }
        // The subtitle is named for a burn-in, or when it is not delivered as a file (SI 955-959).
        // Index -1 has no stream, so it is never named.
        let subtitle = self.subtitle_stream_index.zip(self.subtitle_method);
        if let Some((index, _)) = subtitle.filter(|(_, method)| {
            self.always_burn_in_subtitle || *method != SubtitleDeliveryMethod::External
        }) {
            params.pair("SubtitleStreamIndex", index);
        }
        if let Some(bitrate) = self.video_bitrate {
            params.pair("VideoBitrate", bitrate);
        }
        if let Some(bitrate) = self.audio_bitrate {
            params.pair("AudioBitrate", bitrate);
        }
        if let Some(rate) = self.audio_sample_rate {
            params.pair("AudioSampleRate", rate);
        }
        if let Some(rate) = self.max_framerate {
            params.pair("MaxFramerate", rate);
        }
        if let Some(width) = self.max_width {
            params.pair("MaxWidth", width);
        }
        if let Some(height) = self.max_height {
            params.pair("MaxHeight", height);
        }
        let container = self.container.as_deref().filter(|c| !c.is_empty());
        if let Some(container) = container.filter(|_| hls) {
            params.pair("SegmentContainer", container);
        }
        if let Some(length) = self.segment_length.filter(|_| hls) {
            params.pair("SegmentLength", length);
        }
        if let Some(min) = self.min_segments.filter(|_| hls) {
            params.pair("MinSegments", min);
        }
        if self.start_position_ticks != 0 {
            params.pair("StartTimeTicks", self.start_position_ticks);
        }
        if !self.play_session_id.is_empty() {
            params.pair("PlaySessionId", &self.play_session_id);
        }
        if !self.access_token.is_empty() {
            params.pair("ApiKey", &self.access_token);
        }
        if self.require_non_anamorphic {
            params.pair("RequireNonAnamorphic", "True");
        }
        if let Some(channels) = self.transcoding_max_audio_channels {
            params.pair("TranscodingMaxAudioChannels", channels);
        }
        if self.enable_subtitles_in_manifest {
            params.pair("EnableSubtitlesInManifest", "True");
        }
        if self.enable_mpegts_m2ts_mode {
            params.pair("EnableMpegtsM2TsMode", "True");
        }
        if self.estimate_content_length {
            params.pair("EstimateContentLength", "True");
        }
        if self.transcode_seek_info == TranscodeSeekInfo::Bytes {
            params.pair("TranscodeSeekInfo", "Bytes");
        }
        if self.copy_timestamps {
            params.pair("CopyTimestamps", "True");
        }
        params.pair("RequireAvc", self.require_avc);
        params.pair("EnableAudioVbrEncoding", self.enable_audio_vbr_encoding);
        // SubtitleMethod is written for a chosen stream that isn't delivered as a file. Jellyfin
        // writes it for index -1 too (SI 1103-1112), but Rustyfin's transcoder takes
        // SubtitleMethod=Encode without an index as "burn in the first subtitle".
        if let Some((_, method)) = subtitle.filter(|(_, m)| *m != SubtitleDeliveryMethod::External)
        {
            params.pair("SubtitleMethod", method.as_str());
        }
        for (name, value) in self.stream_options.iter() {
            // Spaces are stripped so that profile names such as "constrained baseline" read as
            // one word
            params.pair(name, value.replace(' ', ""));
        }
        let reasons = self.transcode_reasons.names();
        if !reasons.is_empty() {
            params.pair("TranscodeReasons", reasons.join(","));
        }
        if self.audio_only && hls {
            params.pair("AudioOnly", "true");
        }

        let path = match (hls, self.audio_only) {
            (true, _) => format!("/Videos/{}/master.m3u8", self.item_id),
            (false, audio) => format!(
                "/{}/{}/stream{}",
                if audio { "audio" } else { "videos" },
                self.item_id,
                container.map(|c| format!(".{c}")).unwrap_or_default()
            ),
        };
        params.url(&path)
    }
}
