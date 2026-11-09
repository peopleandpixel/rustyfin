//! The transcoding URL, a port of Jellyfin's `StreamInfo.ToUrl` for an HLS transcode.

use rustyfin_model::device_profile::{SubtitleDeliveryMethod, TranscodeSeekInfo};

use super::video::StreamInfo;

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

/// A bitrate in bits per second, as the URL writes it: kilobits per second, rounded down.
fn kbps(bits_per_second: i32) -> i64 {
    i64::from(bits_per_second) / 1000
}

impl StreamInfo {
    /// The transcode URL for this stream, as Jellyfin's `StreamInfo.ToUrl` writes it for an HLS
    /// transcode. The path is `/Videos/{item}/main.m3u8`, the token goes in `api_key`, and the
    /// bitrates are in kbps. Empty ids and tokens are left out.
    pub(super) fn to_url(&self) -> String {
        let mut params: Vec<String> = Vec::new();
        if !self.device_id.is_empty() {
            params.push(format!("DeviceId={}", self.device_id));
        }
        if !self.media_source_id.is_empty() {
            params.push(format!("MediaSourceId={}", self.media_source_id));
        }
        if !self.video_codecs.is_empty() {
            params.push(format!("VideoCodec={}", self.video_codecs.join(",")));
        }
        if !self.audio_codecs.is_empty() {
            params.push(format!("AudioCodec={}", self.audio_codecs.join(",")));
        }
        if let Some(index) = self.audio_stream_index {
            params.push(format!("AudioStreamIndex={index}"));
        }
        // The subtitle is named for a burn-in, or when it is not delivered as a file (SI 955-959).
        // Index -1 has no stream, so it is never named.
        let subtitle = self.subtitle_stream_index.zip(self.subtitle_method);
        if let Some((index, _)) = subtitle.filter(|(_, method)| {
            self.always_burn_in_subtitle || *method != SubtitleDeliveryMethod::External
        }) {
            params.push(format!("SubtitleStreamIndex={index}"));
        }
        if let Some(bitrate) = self.video_bitrate {
            params.push(format!("VideoBitrate={}", kbps(bitrate)));
        }
        if let Some(bitrate) = self.audio_bitrate {
            params.push(format!("AudioBitrate={}", kbps(bitrate)));
        }
        if let Some(rate) = self.audio_sample_rate {
            params.push(format!("AudioSampleRate={rate}"));
        }
        if let Some(rate) = self.max_framerate {
            params.push(format!("MaxFramerate={rate}"));
        }
        if let Some(width) = self.max_width {
            params.push(format!("MaxWidth={width}"));
        }
        if let Some(height) = self.max_height {
            params.push(format!("MaxHeight={height}"));
        }
        if let Some(container) = self.container.as_deref().filter(|c| !c.is_empty()) {
            params.push(format!("SegmentContainer={container}"));
        }
        if let Some(length) = self.segment_length {
            params.push(format!("SegmentLength={length}"));
        }
        if let Some(min) = self.min_segments {
            params.push(format!("MinSegments={min}"));
        }
        if self.start_position_ticks != 0 {
            params.push(format!("StartTimeTicks={}", self.start_position_ticks));
        }
        if !self.play_session_id.is_empty() {
            params.push(format!("PlaySessionId={}", self.play_session_id));
        }
        if !self.access_token.is_empty() {
            params.push(format!("api_key={}", self.access_token));
        }
        if self.require_non_anamorphic {
            params.push("RequireNonAnamorphic=True".to_string());
        }
        if let Some(channels) = self.transcoding_max_audio_channels {
            params.push(format!("TranscodingMaxAudioChannels={channels}"));
        }
        if self.enable_subtitles_in_manifest {
            params.push("EnableSubtitlesInManifest=True".to_string());
        }
        if self.enable_mpegts_m2ts_mode {
            params.push("EnableMpegtsM2TsMode=True".to_string());
        }
        if self.estimate_content_length {
            params.push("EstimateContentLength=True".to_string());
        }
        if self.transcode_seek_info == TranscodeSeekInfo::Bytes {
            params.push("TranscodeSeekInfo=Bytes".to_string());
        }
        if self.copy_timestamps {
            params.push("CopyTimestamps=True".to_string());
        }
        params.push(format!("RequireAvc={}", self.require_avc));
        params.push(format!(
            "EnableAudioVbrEncoding={}",
            self.enable_audio_vbr_encoding
        ));
        // SubtitleMethod is written for a chosen stream that isn't delivered as a file. Jellyfin
        // writes it for index -1 too (SI 1103-1112), but Rustyfin's transcoder takes
        // SubtitleMethod=Encode without an index as "burn in the first subtitle".
        if let Some((_, method)) = subtitle.filter(|(_, m)| *m != SubtitleDeliveryMethod::External)
        {
            params.push(format!("SubtitleMethod={}", method.as_str()));
        }
        for (name, value) in self.stream_options.iter() {
            // Spaces are stripped so that profile names such as "constrained baseline" need no escaping.
            params.push(format!("{name}={}", value.replace(' ', "")));
        }
        let reasons = self.transcode_reasons.names();
        if !reasons.is_empty() {
            params.push(format!("TranscodeReasons={}", reasons.join(",")));
        }
        if self.audio_only {
            params.push("AudioOnly=true".to_string());
        }

        let path = format!("/Videos/{}/main.m3u8", self.item_id);
        if params.is_empty() {
            path
        } else {
            format!("{path}?{}", params.join("&"))
        }
    }
}
