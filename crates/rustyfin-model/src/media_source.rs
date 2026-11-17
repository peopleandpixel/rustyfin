use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a media source (file) for a media item, matching Jellyfin's `MediaSourceResponse`.
/// Serialized with the flags Jellyfin always sends (`source_flags`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", remote = "Self")]
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
    /// Array of media streams (video, audio, subtitle tracks), each sent with its DisplayTitle
    #[serde(default, serialize_with = "serialize_streams")]
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
/// Serialized with the flags Jellyfin always sends (`MediaStream::stream_flags`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", remote = "Self")]
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
    /// Frame rates as ffprobe gives them ("24000/1001"); sent as Jellyfin's floats, which its SDK
    /// clients require (see `serialize_frame_rate`), and read in either form
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_frame_rate",
        deserialize_with = "deserialize_frame_rate"
    )]
    pub average_frame_rate: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_frame_rate",
        deserialize_with = "deserialize_frame_rate"
    )]
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
    /// The codec level as ffprobe gives it (e.g. 41 for H.264 4.1, 153 for HEVC 5.1). Video only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<f64>,
    /// Whether the video's pixels are not square (a sample aspect ratio other than 1:1). Video
    /// only; unknown when ffprobe gives no sample aspect ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_anamorphic: Option<bool>,
    /// The Dolby Vision configuration record ffprobe reads from the stream. Video only.
    #[serde(flatten)]
    pub dovi: DoviConfig,
    /// Jellyfin's VideoRange ("SDR", "HDR") of a video stream (`MediaStream::video_range`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_range: Option<String>,
    /// Jellyfin's VideoRangeType ("SDR", "HDR10", "HLG", "DOVI", "DOVIWithHDR10", ...) of a
    /// video stream (`MediaStream::video_range`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_range_type: Option<String>,
}

/// A Dolby Vision configuration record, as ffprobe reads it from a stream's side data and as
/// Jellyfin's MediaStream sends it (`DvVersionMajor`, ..., `DvBlSignalCompatibilityId`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DoviConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dv_version_major: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dv_version_minor: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dv_profile: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dv_level: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpu_present_flag: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub el_present_flag: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bl_present_flag: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dv_bl_signal_compatibility_id: Option<i32>,
    /// Whether the stream's side data carries HDR10+ (SMPTE 2094-40) metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr10_plus_present_flag: Option<bool>,
}

impl DoviConfig {
    /// Whether the record is empty (the stream has none).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
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

    /// Jellyfin's VideoRange and VideoRangeType of a video stream, as its
    /// `MediaStream.GetVideoColorRange` gives them; None for a stream that isn't video. Dolby
    /// Vision (a profile 5, 7, 8 or 10 record with RPU and base layer, or a `dovi`, `dvh1`,
    /// `dvhe` or `dav1` tag) is typed by its profile and base layer compatibility: a
    /// compatibility id the profile doesn't define (8.6, 10.6, ...) is DOVIInvalid, and HDR10+
    /// metadata makes DOVIWithHDR10Plus or DOVIWithELHDR10Plus. Else HDR10+ metadata is HDR10Plus,
    /// the colour transfer gives HDR10 (PQ) or HLG, and anything else is SDR.
    pub fn video_range(&self) -> Option<(&'static str, &'static str)> {
        if !self.codec_type.eq_ignore_ascii_case("video") {
            return None;
        }
        let dv = &self.dovi;
        let tag = self
            .codec_tag
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let dovi_profile = matches!(dv.dv_profile, Some(5 | 7 | 8 | 10));
        let dovi_flags = dv.rpu_present_flag == Some(1)
            && dv.bl_present_flag == Some(1)
            && matches!(dv.dv_bl_signal_compatibility_id, Some(0 | 1 | 2 | 4 | 6));
        let hdr10_plus = dv.hdr10_plus_present_flag == Some(true);
        if (dovi_profile && dovi_flags) || matches!(tag.as_str(), "dovi" | "dvh1" | "dvhe" | "dav1")
        {
            return Some(match (dv.dv_profile, dv.dv_bl_signal_compatibility_id) {
                (Some(5), _) => ("HDR", "DOVI"),
                (Some(7), _) if hdr10_plus => ("HDR", "DOVIWithELHDR10Plus"),
                (Some(7), _) => ("HDR", "DOVIWithEL"),
                (Some(8 | 10), Some(1)) if hdr10_plus => ("HDR", "DOVIWithHDR10Plus"),
                (Some(8 | 10), Some(1)) => ("HDR", "DOVIWithHDR10"),
                (Some(8 | 10), Some(4)) => ("HDR", "DOVIWithHLG"),
                (Some(8 | 10), Some(2)) => ("SDR", "DOVIWithSDR"),
                (Some(10), Some(0)) => ("HDR", "DOVI"),
                (Some(8 | 10), _) => ("HDR", "DOVIInvalid"),
                _ => ("HDR", "DOVI"),
            });
        }
        if hdr10_plus {
            return Some(("HDR", "HDR10Plus"));
        }
        let transfer = self.color_transfer.as_deref().unwrap_or_default();
        Some(match transfer.to_ascii_lowercase().as_str() {
            "smpte2084" => ("HDR", "HDR10"),
            "arib-std-b67" => ("HDR", "HLG"),
            _ => ("SDR", "SDR"),
        })
    }

    /// Jellyfin's DisplayTitle (MediaStream.DisplayTitle), in English: for video the resolution,
    /// codec and range; for audio the language, codec, layout and Default and External; for a
    /// subtitle the language, Default, Forced, codec and External. A stream's own title comes
    /// first, followed by what it doesn't already say. `None` for other kinds of stream.
    pub fn display_title(&self) -> Option<String> {
        let kind = self
            .r#type
            .clone()
            .or_else(|| Self::stream_type(&self.codec_type, self.codec.as_deref()))?;
        let codec = self
            .codec
            .as_deref()
            .map(jellyfin_codec)
            .filter(|c| !c.is_empty());
        let language = self.language.as_deref().filter(|l| !l.is_empty());
        let language_text =
            |code: &str| first_to_upper(&language_name(code).unwrap_or_else(|| code.to_string()));
        let title = self.title.as_deref();
        let mut attributes = Vec::new();
        match kind.as_str() {
            "Audio" => {
                let special = |code: &str| {
                    ["mis", "mul", "und", "zxx"]
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(code))
                };
                if let Some(code) = language.filter(|code| !special(code)) {
                    attributes.push(language_text(code));
                }
                match self
                    .profile
                    .as_deref()
                    .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("lc"))
                {
                    Some(profile) => attributes.push(profile.to_string()),
                    None => attributes.extend(codec.map(audio_codec_name)),
                }
                match self.channel_layout.as_deref().filter(|l| !l.is_empty()) {
                    Some(layout) => attributes.push(first_to_upper(layout)),
                    None => attributes.extend(self.channels.map(|ch| format!("{ch} ch"))),
                }
                if self.is_default {
                    attributes.push("Default".to_string());
                }
                if self.is_external {
                    attributes.push("External".to_string());
                }
                Some(titled(title, attributes, " - "))
            }
            "Video" => {
                attributes.extend(self.resolution_text().map(str::to_string));
                attributes.extend(codec.map(str::to_uppercase));
                match self.dovi_title() {
                    Some(dovi) => attributes.push(dovi),
                    None => {
                        attributes.extend(self.video_range().map(|(range, _)| range.to_string()))
                    }
                }
                Some(titled(title, attributes, " "))
            }
            "Subtitle" => {
                attributes.push(language.map_or_else(|| "Und".to_string(), language_text));
                if self.is_default {
                    attributes.push("Default".to_string());
                }
                if self.is_forced {
                    attributes.push("Forced".to_string());
                }
                attributes.extend(codec.map(str::to_uppercase));
                if self.is_external {
                    attributes.push("External".to_string());
                }
                Some(titled(title, attributes, " - "))
            }
            _ => None,
        }
    }

    /// Jellyfin's resolution label of a video stream (MediaStream.GetResolutionText): the first
    /// size class whose width and height both hold the stream's.
    fn resolution_text(&self) -> Option<&'static str> {
        let (width, height) = (self.width?, self.height?);
        const CLASSES: [(u32, u32, &str, &str); 12] = [
            (256, 144, "144p", "144i"),
            (426, 240, "240p", "240i"),
            (640, 360, "360p", "360i"),
            (682, 384, "384p", "384i"),
            (720, 404, "404p", "404i"),
            (854, 480, "480p", "480i"),
            (960, 544, "540p", "540i"),
            (1024, 576, "576p", "576i"),
            (1280, 962, "720p", "720i"),
            (2560, 1440, "1080p", "1080i"),
            (4096, 3072, "4K", "4K"),
            (8192, 6144, "8K", "8K"),
        ];
        CLASSES
            .iter()
            .find(|(w, h, _, _)| width <= *w && height <= *h)
            .map(|(_, _, p, i)| if self.is_interlaced { *i } else { *p })
    }

    /// Jellyfin's VideoDoViTitle: "Dolby Vision Profile 8.1 (HDR10)" for a Dolby Vision record
    /// with RPU and base layer of profile 4, 5, 7, 8, 9 or 10.
    fn dovi_title(&self) -> Option<String> {
        let dv = &self.dovi;
        let profile = dv
            .dv_profile
            .filter(|p| matches!(p, 4 | 5 | 7 | 8 | 9 | 10))?;
        if dv.rpu_present_flag != Some(1) || dv.bl_present_flag != Some(1) {
            return None;
        }
        let compat = dv.dv_bl_signal_compatibility_id.unwrap_or(0);
        let mut title = format!("Dolby Vision Profile {profile}");
        if compat > 0 {
            title.push_str(&format!(".{compat}"));
        }
        title.push_str(match compat {
            1 | 6 => " (HDR10)",
            2 => " (SDR)",
            4 => " (HLG)",
            _ => "",
        });
        Some(title)
    }

    /// Sets `video_range` and `video_range_type` from the stream (`video_range`).
    pub fn fill_video_range(&mut self) {
        let range = self.video_range();
        self.video_range = range.map(|(range, _)| range.to_string());
        self.video_range_type = range.map(|(_, kind)| kind.to_string());
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

// --- The fields Jellyfin's MediaSourceInfo and MediaStream always carry. Its SDK clients (the
// Android app's player and downloads) refuse a PlaybackInfo or item without them. The derived
// impls are inherent (`remote = "Self"`); these add the missing keys, never replacing one. ---

/// Jellyfin's values for the source flags Rustyfin doesn't track: a plain file it opens itself,
/// streamed over HTTP unless PlaybackInfo says HLS
fn source_flags() -> [(&'static str, Value); 11] {
    [
        ("ReadAtNativeFramerate", Value::Bool(false)),
        ("IgnoreDts", Value::Bool(false)),
        ("IgnoreIndex", Value::Bool(false)),
        ("GenPtsInput", Value::Bool(false)),
        ("IsInfiniteStream", Value::Bool(false)),
        ("RequiresOpening", Value::Bool(false)),
        ("RequiresClosing", Value::Bool(false)),
        ("RequiresLooping", Value::Bool(false)),
        ("SupportsProbing", Value::Bool(true)),
        ("HasSegments", Value::Bool(false)),
        ("TranscodingSubProtocol", Value::String("http".into())),
    ]
}

/// Picture subtitles, which Jellyfin can't send as text (ffprobe's names and Jellyfin's)
const IMAGE_SUBTITLE_CODECS: [&str; 9] = [
    "hdmv_pgs_subtitle",
    "pgssub",
    "pgs",
    "dvd_subtitle",
    "dvdsub",
    "dvb_subtitle",
    "dvbsub",
    "vobsub",
    "xsub",
];

impl MediaStream {
    /// Jellyfin's `IsTextSubtitleStream`: a subtitle that isn't a picture
    pub fn is_text_subtitle(&self) -> bool {
        self.codec_type.eq_ignore_ascii_case("subtitle")
            && !self.codec.as_deref().is_some_and(|c| {
                IMAGE_SUBTITLE_CODECS
                    .iter()
                    .any(|i| i.eq_ignore_ascii_case(c))
            })
    }

    fn stream_flags(&self) -> [(&'static str, Value); 5] {
        let text = self.is_text_subtitle();
        [
            ("IsHearingImpaired", Value::Bool(false)),
            ("IsOriginal", Value::Bool(false)),
            ("IsTextSubtitleStream", Value::Bool(text)),
            // A text subtitle can be sent as its own file, as Jellyfin offers it
            ("SupportsExternalStream", Value::Bool(text)),
            ("Type", Value::String("Data".into())),
        ]
    }
}

/// What the derived `serialize` gives, with each of `flags` added where it is missing or null
fn with_flags<S: serde::Serializer>(
    value: Result<Value, serde_json::Error>,
    flags: impl IntoIterator<Item = (&'static str, Value)>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut value = value.map_err(serde::ser::Error::custom)?;
    if let Value::Object(map) = &mut value {
        for (key, flag) in flags {
            let slot = map.entry(key).or_insert(Value::Null);
            if slot.is_null() {
                *slot = flag;
            }
        }
    }
    value.serialize(serializer)
}

impl Serialize for MediaSourceResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        with_flags(
            MediaSourceResponse::serialize(self, serde_json::value::Serializer),
            source_flags(),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for MediaSourceResponse {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MediaSourceResponse::deserialize(deserializer)
    }
}

impl Serialize for MediaStream {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        with_flags(
            MediaStream::serialize(self, serde_json::value::Serializer),
            self.stream_flags(),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for MediaStream {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        MediaStream::deserialize(deserializer)
    }
}

/// A frame rate ("24000/1001", "25/1" or "23.976") as Jellyfin's float, rounded to 3 decimals as
/// Jellyfin does; null when it is no rate (ffprobe's "0/0")
fn frame_rate_value(text: &str) -> Option<f64> {
    let rate = match text.split_once('/') {
        Some((numerator, denominator)) => {
            numerator.trim().parse::<f64>().ok()? / denominator.trim().parse::<f64>().ok()?
        }
        None => text.trim().parse::<f64>().ok()?,
    };
    // An f64, so 24000/1001 goes out as 23.976 and not as an f32's 23.97599983215332
    (rate.is_finite() && rate > 0.0).then(|| (rate * 1000.0).round() / 1000.0)
}

fn serialize_frame_rate<S: serde::Serializer>(
    rate: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    rate.as_deref()
        .and_then(frame_rate_value)
        .serialize(serializer)
}

fn deserialize_frame_rate<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(text)) => Some(text),
            Some(serde_json::Value::Number(number)) => Some(number.to_string()),
            _ => None,
        },
    )
}

/// Media streams that serialize as Jellyfin sends them, each with its `DisplayTitle`, for a
/// hand-written `Serialize` (see `serialize_streams`).
pub struct TitledStreams<'a>(pub &'a [MediaStream]);

impl Serialize for TitledStreams<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_streams(self.0, serializer)
    }
}

/// Media sources that serialize with their `Name`, for a hand-written `Serialize` (see
/// `serialize_named_source`).
pub struct NamedSources<'a>(pub &'a [MediaSourceResponse]);

impl Serialize for NamedSources<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_named_sources(self.0, serializer)
    }
}

/// Media streams as Jellyfin sends them: each with its `DisplayTitle`.
pub fn serialize_streams<S: serde::Serializer>(
    streams: &[MediaStream],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Titled<'a> {
        #[serde(flatten)]
        stream: &'a MediaStream,
        #[serde(skip_serializing_if = "Option::is_none")]
        display_title: Option<String>,
    }
    serializer.collect_seq(streams.iter().map(|stream| Titled {
        stream,
        display_title: stream.display_title(),
    }))
}

/// A media source as Jellyfin sends it: with its `Name`, the file's name without its extension
/// (MediaSourceManager's GetMediaSourceName for a file).
pub fn serialize_named_source<S: serde::Serializer>(
    source: &MediaSourceResponse,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Named<'a> {
        #[serde(flatten)]
        source: &'a MediaSourceResponse,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<&'a str>,
    }
    let name = std::path::Path::new(&source.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty());
    Named { source, name }.serialize(serializer)
}

/// A list of media sources, each with its `Name` (`serialize_named_source`).
pub fn serialize_named_sources<S: serde::Serializer>(
    sources: &[MediaSourceResponse],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    struct One<'a>(&'a MediaSourceResponse);
    impl Serialize for One<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serialize_named_source(self.0, serializer)
        }
    }
    serializer.collect_seq(sources.iter().map(One))
}

/// ISO 639-2 codes with their English names (code, other code, two-letter code, name).
const LANGUAGE_NAMES: &str = include_str!("../../rustyfin-api/data/iso639-2.txt");

/// The English name of a language code (three letters, two letters, or a tag such as "en-US"),
/// as .NET's CultureInfo names a neutral culture: "Spanish" for "Spanish; Castilian".
fn language_name(code: &str) -> Option<String> {
    let base = code.split(['-', '_']).next().unwrap_or(code);
    LANGUAGE_NAMES.lines().find_map(|line| {
        let mut parts = line.split('|');
        let (b, t, two, name) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        [b, t, two]
            .iter()
            .any(|c| !c.is_empty() && c.eq_ignore_ascii_case(base))
            .then(|| {
                let name = name.split(';').next().unwrap_or(name);
                let name = name.split(" (").next().unwrap_or(name);
                name.strip_prefix("Modern ")
                    .unwrap_or(name)
                    .trim()
                    .to_string()
            })
    })
}

/// Jellyfin's name for an audio codec in a DisplayTitle (AudioCodec.GetFriendlyName).
fn audio_codec_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "ac3" => "Dolby Digital".to_string(),
        "eac3" => "Dolby Digital+".to_string(),
        "dca" => "DTS".to_string(),
        _ => codec.to_uppercase(),
    }
}

/// The first letter in upper case (Jellyfin's StringHelper.FirstToUpper).
fn first_to_upper(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The title, then each attribute it doesn't already hold, joined by `sep`; the attributes
/// alone without a title.
fn titled(title: Option<&str>, attributes: Vec<String>, sep: &str) -> String {
    match title.filter(|t| !t.is_empty()) {
        Some(title) => {
            let lower = title.to_lowercase();
            let mut out = title.to_string();
            for tag in attributes {
                if !lower.contains(&tag.to_lowercase()) {
                    out.push_str(" - ");
                    out.push_str(&tag);
                }
            }
            out
        }
        None => attributes.join(sep),
    }
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

    // Jellyfin's SDK clients (the Android app) refuse PlaybackInfo when a frame rate is a string
    #[test]
    fn r3android_frame_rates_are_floats() {
        let stream = MediaStream {
            codec_type: "video".into(),
            average_frame_rate: Some("24000/1001".into()),
            real_frame_rate: Some("25/1".into()),
            ..MediaStream::default()
        };
        let json = serde_json::to_value(&stream).unwrap();
        assert_eq!(json["AverageFrameRate"], serde_json::json!(23.976));
        assert_eq!(
            serde_json::to_string(&json["AverageFrameRate"]).unwrap(),
            "23.976"
        );
        assert_eq!(json["RealFrameRate"], serde_json::json!(25.0));

        let none = serde_json::to_value(MediaStream {
            average_frame_rate: Some("0/0".into()),
            ..MediaStream::default()
        })
        .unwrap();
        assert!(none["AverageFrameRate"].is_null());
        assert!(none.get("RealFrameRate").is_none());

        // Read back in either form, as ffprobe's fraction or Jellyfin's float
        let read: MediaStream = serde_json::from_value(serde_json::json!({
            "Index": 0, "CodecType": "video", "AverageFrameRate": "24/1", "RealFrameRate": 23.976,
        }))
        .unwrap();
        assert_eq!(read.average_frame_rate.as_deref(), Some("24/1"));
        assert_eq!(read.real_frame_rate.as_deref(), Some("23.976"));
        assert_eq!(frame_rate_value("24/1"), Some(24.0));
    }

    // The fields Jellyfin's SDK requires of MediaSourceInfo and MediaStream (jellyfin-sdk-kotlin's
    // non-null properties); the Android app's player refuses a PlaybackInfo without one
    #[test]
    fn r3android_sources_carry_the_fields_jellyfin_clients_require() {
        let source: MediaSourceResponse = serde_json::from_value(serde_json::json!({
            "Id": "s", "Uid": "s", "Protocol": "File", "Path": "/x.mkv", "Container": "mkv",
            "MediaStreams": [
                {"Index": 0, "CodecType": "video", "Codec": "h264", "Type": "Video"},
                {"Index": 1, "CodecType": "subtitle", "Codec": "subrip", "Type": "Subtitle"},
                {"Index": 2, "CodecType": "subtitle", "Codec": "hdmv_pgs_subtitle", "Type": "Subtitle"},
                {"Index": 3, "CodecType": "data"},
            ],
        }))
        .unwrap();
        let json = serde_json::to_value(&source).unwrap();
        for key in [
            "Protocol",
            "Type",
            "IsRemote",
            "ReadAtNativeFramerate",
            "IgnoreDts",
            "IgnoreIndex",
            "GenPtsInput",
            "SupportsTranscoding",
            "SupportsDirectStream",
            "SupportsDirectPlay",
            "IsInfiniteStream",
            "RequiresOpening",
            "RequiresClosing",
            "RequiresLooping",
            "SupportsProbing",
            "TranscodingSubProtocol",
            "HasSegments",
        ] {
            assert!(!json[key].is_null(), "source {key}");
        }
        assert_eq!(json["TranscodingSubProtocol"], "http");
        let streams = json["MediaStreams"].as_array().unwrap();
        for stream in streams {
            for key in [
                "IsInterlaced",
                "IsDefault",
                "IsForced",
                "IsHearingImpaired",
                "IsOriginal",
                "Type",
                "Index",
                "IsExternal",
                "IsTextSubtitleStream",
                "SupportsExternalStream",
            ] {
                assert!(!stream[key].is_null(), "stream {} {key}", stream["Index"]);
            }
        }
        let text: Vec<_> = streams
            .iter()
            .map(|s| s["IsTextSubtitleStream"].clone())
            .collect();
        assert_eq!(text, [false, true, false, false]);
        assert_eq!(streams[1]["SupportsExternalStream"], true);
        assert_eq!(streams[3]["Type"], "Data");
        // A value the source has is kept, and it reads back as before
        let back: MediaSourceResponse = serde_json::from_value(json).unwrap();
        assert_eq!(back.media_streams.len(), 4);
    }

    fn dv(profile: i32, compat: i32) -> MediaStream {
        MediaStream {
            codec_type: "video".into(),
            color_transfer: Some("smpte2084".into()),
            dovi: DoviConfig {
                dv_profile: Some(profile),
                dv_bl_signal_compatibility_id: Some(compat),
                rpu_present_flag: Some(1),
                bl_present_flag: Some(1),
                el_present_flag: Some(i32::from(profile == 7)),
                ..DoviConfig::default()
            },
            ..MediaStream::default()
        }
    }

    /// Jellyfin's VideoRangeType: Dolby Vision by profile and base layer (the reference server
    /// types a profile 7 file with compatibility 6 as DOVIWithEL), else the transfer.
    #[test]
    fn r3gaps2_video_range_types_dolby_vision_as_jellyfin() {
        let range = |s: &MediaStream| s.video_range().unwrap();
        assert_eq!(range(&dv(7, 6)), ("HDR", "DOVIWithEL"));
        assert_eq!(range(&dv(8, 1)), ("HDR", "DOVIWithHDR10"));
        assert_eq!(range(&dv(8, 4)), ("HDR", "DOVIWithHLG"));
        assert_eq!(range(&dv(8, 2)), ("SDR", "DOVIWithSDR"));
        assert_eq!(range(&dv(5, 0)), ("HDR", "DOVI"));
        assert_eq!(range(&dv(10, 0)), ("HDR", "DOVI"));
        // Compatibility ids the profile doesn't define
        assert_eq!(range(&dv(8, 6)), ("HDR", "DOVIInvalid"));
        assert_eq!(range(&dv(10, 6)), ("HDR", "DOVIInvalid"));
        let mut plus = dv(8, 1);
        plus.dovi.hdr10_plus_present_flag = Some(true);
        assert_eq!(range(&plus), ("HDR", "DOVIWithHDR10Plus"));
        plus.dovi = DoviConfig {
            hdr10_plus_present_flag: Some(true),
            ..DoviConfig::default()
        };
        assert_eq!(range(&plus), ("HDR", "HDR10Plus"));
        let mut tagged = dv(0, 0);
        tagged.dovi = DoviConfig::default();
        assert_eq!(range(&tagged), ("HDR", "HDR10"));
        tagged.codec_tag = Some("dvh1".into());
        assert_eq!(range(&tagged), ("HDR", "DOVI"));
        let audio = MediaStream {
            codec_type: "audio".into(),
            ..MediaStream::default()
        };
        assert_eq!(audio.video_range(), None);

        // The JSON carries Jellyfin's names
        let mut stream = dv(8, 1);
        stream.fill_video_range();
        let json = serde_json::to_value(&stream).unwrap();
        assert_eq!(json["DvProfile"], 8);
        assert_eq!(json["DvBlSignalCompatibilityId"], 1);
        assert_eq!(json["VideoRange"], "HDR");
        assert_eq!(json["VideoRangeType"], "DOVIWithHDR10");
        let back: MediaStream = serde_json::from_value(json).unwrap();
        assert_eq!(back.dovi, stream.dovi);
    }

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

    #[test]
    fn r3parental_display_titles_follow_jellyfin() {
        let stream = |kind: &str, codec: &str| MediaStream {
            codec_type: kind.to_string(),
            codec: Some(codec.to_string()),
            ..Default::default()
        };
        let video = MediaStream {
            width: Some(3840),
            height: Some(1600),
            ..stream("video", "hevc")
        };
        assert_eq!(video.display_title().as_deref(), Some("4K HEVC SDR"));
        let interlaced = MediaStream {
            width: Some(720),
            height: Some(576),
            is_interlaced: true,
            title: Some("Main 576i".to_string()),
            ..stream("video", "mpeg2video")
        };
        assert_eq!(
            interlaced.display_title().as_deref(),
            Some("Main 576i - MPEG2VIDEO - SDR")
        );
        let audio = MediaStream {
            language: Some("spa".to_string()),
            channels: Some(2),
            is_external: true,
            ..stream("audio", "eac3")
        };
        assert_eq!(
            audio.display_title().as_deref(),
            Some("Spanish - Dolby Digital+ - 2 ch - External")
        );
        let unknown = MediaStream {
            language: Some("und".to_string()),
            profile: Some("LC".to_string()),
            channel_layout: Some("stereo".to_string()),
            is_default: true,
            ..stream("audio", "aac")
        };
        assert_eq!(
            unknown.display_title().as_deref(),
            Some("AAC - Stereo - Default")
        );
        let subtitle = MediaStream {
            language: Some("gre".to_string()),
            is_default: true,
            ..stream("subtitle", "hdmv_pgs_subtitle")
        };
        assert_eq!(
            subtitle.display_title().as_deref(),
            Some("Greek - Default - PGSSUB")
        );
        assert_eq!(
            stream("subtitle", "ass").display_title().as_deref(),
            Some("Und - ASS")
        );
        assert_eq!(stream("data", "bin_data").display_title(), None);

        // Serialized with DisplayTitle, and a source with the file's name.
        let value = serde_json::to_value(crate::media::MediaItem {
            media_streams: vec![video.clone()],
            media_sources: vec![MediaSourceResponse {
                id: "s".into(),
                uid: "u".into(),
                container: Some("mkv".into()),
                protocol: "File".into(),
                path: "/films/A Made Up Film (2001)/A Made Up Film.mkv".into(),
                r#type: "Default".into(),
                run_time_ticks: None,
                size: None,
                bitrate: None,
                video_bitrate: None,
                audio_bitrate: None,
                width: None,
                height: None,
                aspect_ratio: None,
                is_remote: false,
                media_state: None,
                media_streams: vec![video],
                supports_transcoding: true,
                supports_direct_stream: true,
                supports_direct_play: true,
            }],
            ..crate::media::MediaItem::default_dto_fields()
        })
        .unwrap();
        assert_eq!(value["MediaStreams"][0]["DisplayTitle"], "4K HEVC SDR");
        let source = &value["MediaSources"][0];
        assert_eq!(source["Name"], "A Made Up Film", "{source}");
        assert_eq!(source["Container"], "mkv");
        assert_eq!(source["MediaStreams"][0]["DisplayTitle"], "4K HEVC SDR");
    }
}
