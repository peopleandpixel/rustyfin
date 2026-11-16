use serde::{Deserialize, Serialize};

/// Represents a media source (file) for a media item, matching Jellyfin's `MediaSourceResponse`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceResponse {
    /// Unique ID for this media source
    pub id: String,
    /// UUID string of the source
    pub uid: String,
    /// File container format (e.g., "mkv", "mp4", "mp3")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Protocol type: "File", "Http", "Rtmp", "Rtsp", "Udp", "Rtp", "Ftp"
    pub protocol: String,
    /// Full path to the media file
    pub path: String,
    /// Source type: "Default", "Gaps"
    #[serde(default = "default_source_type")]
    pub r#type: String,
    /// Runtime in ticks (1 tick = 100 ns)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_time_ticks: Option<i64>,
    /// File size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    /// Total bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    /// Video bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate: Option<u32>,
    /// Audio bitrate in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate: Option<u32>,
    /// Video width in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Video height in pixels
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Aspect ratio string (e.g., "16:9")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    /// Whether the source is remote
    #[serde(default)]
    pub is_remote: bool,
    /// Media state: "Missing", "SubtitlesOutOfSync"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_state: Option<String>,
    /// Array of media streams (video, audio, subtitle tracks)
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
    /// Whether direct play is supported
    #[serde(default)]
    pub supports_transcoding: bool,
    #[serde(default)]
    pub supports_direct_stream: bool,
    #[serde(default)]
    pub supports_direct_play: bool,
}

fn default_source_type() -> String {
    "Default".to_string()
}

/// Represents a single media stream (video, audio, or subtitle track), matching Jellyfin's `MediaStream`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    /// Index of the stream within the media source
    pub index: i32,
    /// Codec name as ffprobe gives it (e.g., "h264", "aac", "hdmv_pgs_subtitle"); sent with
    /// Jellyfin's names for picture subtitles (see `jellyfin_codec`)
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_jellyfin_codec"
    )]
    pub codec: Option<String>,
    /// Codec type as ffprobe gives it: "video", "audio", "subtitle", "data". Rustyfin's own
    /// field; Jellyfin clients read `Type`
    pub codec_type: String,
    /// Language code as the file tags it (e.g., "eng", "ger"); sent as ISO 639-2/T, as Jellyfin
    /// does (see `jellyfin_language`)
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_jellyfin_language"
    )]
    pub language: Option<String>,
    /// Time base fraction (e.g., "1/1000")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_base: Option<String>,
    /// Codec time base (e.g., "1/48000")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_time_base: Option<String>,
    /// Codec tag string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_tag: Option<String>,
    /// Whether the stream is external (separate file)
    #[serde(default)]
    pub is_external: bool,
    /// Path to external subtitle file, if applicable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    // Video-specific fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_frame_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_frame_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Jellyfin's MediaStreamType: "Video", "EmbeddedImage", "Audio", "Subtitle" or "Data" (see
    /// `stream_type`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_transfer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_primaries: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localized_uri: Option<String>,
    /// Whether this is the default stream
    #[serde(default)]
    pub is_default: bool,
    /// Whether this stream is forced
    #[serde(default)]
    pub is_forced: bool,
    /// Whether video is interlaced
    #[serde(default)]
    pub is_interlaced: bool,
    /// Whether subtitles have embedded bitmaps
    #[serde(default)]
    pub embedded_bitmaps: bool,
}

/// What identifies a media stream across rescans. A remembered track choice is offered only where
/// the stream at its index has the same key, so a replaced file whose index now names another
/// stream (a commentary, a dub) does not hand the choice to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamIdentity<'a> {
    pub codec_type: &'a str,
    pub codec: Option<&'a str>,
    pub language: Option<&'a str>,
    pub title: Option<&'a str>,
    pub is_external: bool,
    pub path: Option<&'a str>,
    pub is_default: bool,
    pub is_forced: bool,
}

impl StreamIdentity<'_> {
    /// The key as canonical JSON: the codec type, codec and language lowercased; the title; the
    /// external flag; an external stream's file name (else null); the default and forced flags.
    pub fn key(&self) -> String {
        let file_name = self
            .path
            .filter(|_| self.is_external)
            .and_then(|path| path.rsplit(['/', '\\']).next());
        serde_json::json!([
            self.codec_type.to_ascii_lowercase(),
            self.codec.map(str::to_ascii_lowercase),
            self.language.map(str::to_ascii_lowercase),
            self.title,
            self.is_external,
            file_name,
            self.is_default,
            self.is_forced,
        ])
        .to_string()
    }
}

impl MediaStream {
    /// This stream's identity (see `StreamIdentity`).
    pub fn identity(&self) -> StreamIdentity<'_> {
        StreamIdentity {
            codec_type: &self.codec_type,
            codec: self.codec.as_deref(),
            language: self.language.as_deref(),
            title: self.title.as_deref(),
            is_external: self.is_external,
            path: self.path.as_deref(),
            is_default: self.is_default,
            is_forced: self.is_forced,
        }
    }

    /// Jellyfin's MediaStreamType for an ffprobe codec type and codec: "Video", "EmbeddedImage"
    /// (a picture codec, as cover art is), "Audio", "Subtitle" or "Data"; None for any other (an
    /// attachment, which Jellyfin leaves out of MediaStreams).
    pub fn stream_type(codec_type: &str, codec: Option<&str>) -> Option<String> {
        let picture = codec.is_some_and(|c| {
            ["mjpeg", "png", "gif", "webp"]
                .iter()
                .any(|p| p.eq_ignore_ascii_case(c))
        });
        let kind = match codec_type.to_ascii_lowercase().as_str() {
            "video" if picture => "EmbeddedImage",
            "video" => "Video",
            "audio" => "Audio",
            "subtitle" => "Subtitle",
            "data" => "Data",
            _ => return None,
        };
        Some(kind.to_string())
    }
}

/// The name Jellyfin gives a codec: its own names for ffprobe's picture and teletext subtitle
/// codecs (ProbeResultNormalizer.NormalizeSubtitleCodec), any other name unchanged.
pub fn jellyfin_codec(codec: &str) -> &str {
    match codec.to_ascii_lowercase().as_str() {
        "dvb_subtitle" => "DVBSUB",
        "dvb_teletext" => "DVBTXT",
        "dvd_subtitle" => "DVDSUB",
        "hdmv_pgs_subtitle" => "PGSSUB",
        _ => codec,
    }
}

/// The ISO 639-2 bibliographic codes and their terminology codes, which Jellyfin reports.
const BIBLIOGRAPHIC_LANGUAGES: [(&str, &str); 20] = [
    ("alb", "sqi"),
    ("arm", "hye"),
    ("baq", "eus"),
    ("bur", "mya"),
    ("chi", "zho"),
    ("cze", "ces"),
    ("dut", "nld"),
    ("fre", "fra"),
    ("geo", "kat"),
    ("ger", "deu"),
    ("gre", "ell"),
    ("ice", "isl"),
    ("mac", "mkd"),
    ("mao", "mri"),
    ("may", "msa"),
    ("per", "fas"),
    ("rum", "ron"),
    ("slo", "slk"),
    ("tib", "bod"),
    ("wel", "cym"),
];

/// A language code as Jellyfin reports it: an ISO 639-2 bibliographic code (as Matroska files
/// tag them, "ger") becomes the terminology code ("deu"); any other code is unchanged.
pub fn jellyfin_language(language: &str) -> &str {
    BIBLIOGRAPHIC_LANGUAGES
        .iter()
        .find(|(b, _)| b.eq_ignore_ascii_case(language))
        .map_or(language, |(_, t)| t)
}

/// The spellings of a language code that name the same language, lowercased: the code, and for
/// an ISO 639-2 language with a bibliographic and a terminology code, both ("ger" and "deu").
/// Stored streams keep the file's code while the API reports the terminology one, so a filter
/// matches either.
pub fn language_forms(language: &str) -> Vec<String> {
    let code = language.trim().to_ascii_lowercase();
    let mut forms = vec![code.clone()];
    if let Some((b, t)) = BIBLIOGRAPHIC_LANGUAGES
        .iter()
        .find(|(b, t)| *b == code || *t == code)
    {
        forms = vec![b.to_string(), t.to_string()];
    }
    forms
}

fn serialize_jellyfin_codec<S: serde::Serializer>(
    codec: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    codec.as_deref().map(jellyfin_codec).serialize(serializer)
}

fn serialize_jellyfin_language<S: serde::Serializer>(
    language: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    language
        .as_deref()
        .map(jellyfin_language)
        .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r1play_stream_json_uses_jellyfin_names() {
        let stream = MediaStream {
            codec: Some("hdmv_pgs_subtitle".into()),
            codec_type: "subtitle".into(),
            language: Some("ger".into()),
            r#type: MediaStream::stream_type("subtitle", Some("hdmv_pgs_subtitle")),
            ..Default::default()
        };
        let json = serde_json::to_value(&stream).unwrap();
        assert_eq!(json["Codec"], "PGSSUB");
        assert_eq!(json["Language"], "deu");
        assert_eq!(json["Type"], "Subtitle");
        // Rustyfin's own field stays for its web client
        assert_eq!(json["CodecType"], "subtitle");
    }

    #[test]
    fn r3playback_language_forms_cover_both_iso_639_2_codes() {
        assert_eq!(language_forms("ger"), ["ger", "deu"]);
        assert_eq!(language_forms(" DEU "), ["ger", "deu"]);
        assert_eq!(language_forms("Fre"), ["fre", "fra"]);
        assert_eq!(language_forms("eng"), ["eng"]);
        assert_eq!(language_forms("de"), ["de"]);
    }

    #[test]
    fn r1play_unlisted_codes_pass_through() {
        assert_eq!(jellyfin_codec("subrip"), "subrip");
        assert_eq!(jellyfin_codec("ass"), "ass");
        assert_eq!(jellyfin_language("eng"), "eng");
        assert_eq!(jellyfin_language("CHI"), "zho");
        assert_eq!(jellyfin_language("und"), "und");
        let kind = |t, c| MediaStream::stream_type(t, Some(c));
        assert_eq!(kind("Audio", "aac").as_deref(), Some("Audio"));
        assert_eq!(kind("data", "bin_data").as_deref(), Some("Data"));
        assert_eq!(kind("video", "hevc").as_deref(), Some("Video"));
        assert_eq!(kind("video", "mjpeg").as_deref(), Some("EmbeddedImage"));
        assert_eq!(kind("attachment", "ttf"), None);
    }
}
