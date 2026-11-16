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
}
