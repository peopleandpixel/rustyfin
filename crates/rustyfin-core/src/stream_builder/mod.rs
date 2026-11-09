//! Stream building: the facts about a media source and the profile conditions that test them.
//! Ports the parts of Jellyfin's `StreamBuilder` and `ConditionProcessor` that the builder needs.

mod audio;
pub mod conditions;
mod facts;
mod subtitles;
mod url;
mod video;

use std::ops::{BitOr, BitOrAssign};

use rustyfin_model::device_profile::{
    DeviceProfile, EncodingContext, ProfileType, StreamProtocol, SubtitleDeliveryMethod,
};

pub use facts::{SourceFacts, StreamFacts};

/// How a stream is delivered. The order matches Jellyfin's values 0, 1 and 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PlayMethod {
    /// The stream is transcoded.
    #[default]
    Transcode,
    /// The streams are copied into a new container.
    DirectStream,
    /// The file is played as it is.
    DirectPlay,
}

/// Whether an item is video or audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaKind {
    /// A video item.
    #[default]
    Video,
    /// An audio item.
    Audio,
}

/// The kind of a stream in a media source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamKind {
    /// Any other stream, such as an attachment.
    #[default]
    Other,
    /// A video stream.
    Video,
    /// An audio stream.
    Audio,
    /// A subtitle stream.
    Subtitle,
}

/// Why a stream can't be copied as it is. Each reason is one bit, as in Jellyfin's
/// `TranscodeReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum TranscodeReason {
    /// The container isn't supported.
    ContainerNotSupported = 1 << 0,
    /// The video codec isn't supported.
    VideoCodecNotSupported = 1 << 1,
    /// The audio codec isn't supported.
    AudioCodecNotSupported = 1 << 2,
    /// The subtitle codec isn't supported.
    SubtitleCodecNotSupported = 1 << 3,
    /// The audio is in an external file.
    AudioIsExternal = 1 << 4,
    /// The secondary audio track isn't supported.
    SecondaryAudioNotSupported = 1 << 5,
    /// The video profile isn't supported.
    VideoProfileNotSupported = 1 << 6,
    /// The video level isn't supported.
    VideoLevelNotSupported = 1 << 7,
    /// The video resolution isn't supported.
    VideoResolutionNotSupported = 1 << 8,
    /// The video bit depth isn't supported.
    VideoBitDepthNotSupported = 1 << 9,
    /// The video frame rate isn't supported.
    VideoFramerateNotSupported = 1 << 10,
    /// The reference frame count isn't supported.
    RefFramesNotSupported = 1 << 11,
    /// The anamorphic video isn't supported.
    AnamorphicVideoNotSupported = 1 << 12,
    /// The interlaced video isn't supported.
    InterlacedVideoNotSupported = 1 << 13,
    /// The audio channel count isn't supported.
    AudioChannelsNotSupported = 1 << 14,
    /// The audio profile isn't supported.
    AudioProfileNotSupported = 1 << 15,
    /// The audio sample rate isn't supported.
    AudioSampleRateNotSupported = 1 << 16,
    /// The audio bit depth isn't supported.
    AudioBitDepthNotSupported = 1 << 17,
    /// The container bit rate is over the limit.
    ContainerBitrateExceedsLimit = 1 << 18,
    /// The video bit rate isn't supported.
    VideoBitrateNotSupported = 1 << 19,
    /// The audio bit rate isn't supported.
    AudioBitrateNotSupported = 1 << 20,
    /// The video stream's information is unknown.
    UnknownVideoStreamInfo = 1 << 21,
    /// The audio stream's information is unknown.
    UnknownAudioStreamInfo = 1 << 22,
    /// Direct play failed.
    DirectPlayError = 1 << 23,
    /// The video range type isn't supported.
    VideoRangeTypeNotSupported = 1 << 24,
    /// The video codec tag isn't supported.
    VideoCodecTagNotSupported = 1 << 25,
    /// There are more streams than the device allows.
    StreamCountExceedsLimit = 1 << 26,
    /// The video rotation isn't supported.
    VideoRotationNotSupported = 1 << 27,
}

impl TranscodeReason {
    /// Every reason, in ascending bit order.
    pub const ALL: [TranscodeReason; 28] = [
        TranscodeReason::ContainerNotSupported,
        TranscodeReason::VideoCodecNotSupported,
        TranscodeReason::AudioCodecNotSupported,
        TranscodeReason::SubtitleCodecNotSupported,
        TranscodeReason::AudioIsExternal,
        TranscodeReason::SecondaryAudioNotSupported,
        TranscodeReason::VideoProfileNotSupported,
        TranscodeReason::VideoLevelNotSupported,
        TranscodeReason::VideoResolutionNotSupported,
        TranscodeReason::VideoBitDepthNotSupported,
        TranscodeReason::VideoFramerateNotSupported,
        TranscodeReason::RefFramesNotSupported,
        TranscodeReason::AnamorphicVideoNotSupported,
        TranscodeReason::InterlacedVideoNotSupported,
        TranscodeReason::AudioChannelsNotSupported,
        TranscodeReason::AudioProfileNotSupported,
        TranscodeReason::AudioSampleRateNotSupported,
        TranscodeReason::AudioBitDepthNotSupported,
        TranscodeReason::ContainerBitrateExceedsLimit,
        TranscodeReason::VideoBitrateNotSupported,
        TranscodeReason::AudioBitrateNotSupported,
        TranscodeReason::UnknownVideoStreamInfo,
        TranscodeReason::UnknownAudioStreamInfo,
        TranscodeReason::DirectPlayError,
        TranscodeReason::VideoRangeTypeNotSupported,
        TranscodeReason::VideoCodecTagNotSupported,
        TranscodeReason::StreamCountExceedsLimit,
        TranscodeReason::VideoRotationNotSupported,
    ];

    /// Jellyfin's name for the reason, e.g. "AudioCodecNotSupported".
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContainerNotSupported => "ContainerNotSupported",
            Self::VideoCodecNotSupported => "VideoCodecNotSupported",
            Self::AudioCodecNotSupported => "AudioCodecNotSupported",
            Self::SubtitleCodecNotSupported => "SubtitleCodecNotSupported",
            Self::AudioIsExternal => "AudioIsExternal",
            Self::SecondaryAudioNotSupported => "SecondaryAudioNotSupported",
            Self::VideoProfileNotSupported => "VideoProfileNotSupported",
            Self::VideoLevelNotSupported => "VideoLevelNotSupported",
            Self::VideoResolutionNotSupported => "VideoResolutionNotSupported",
            Self::VideoBitDepthNotSupported => "VideoBitDepthNotSupported",
            Self::VideoFramerateNotSupported => "VideoFramerateNotSupported",
            Self::RefFramesNotSupported => "RefFramesNotSupported",
            Self::AnamorphicVideoNotSupported => "AnamorphicVideoNotSupported",
            Self::InterlacedVideoNotSupported => "InterlacedVideoNotSupported",
            Self::AudioChannelsNotSupported => "AudioChannelsNotSupported",
            Self::AudioProfileNotSupported => "AudioProfileNotSupported",
            Self::AudioSampleRateNotSupported => "AudioSampleRateNotSupported",
            Self::AudioBitDepthNotSupported => "AudioBitDepthNotSupported",
            Self::ContainerBitrateExceedsLimit => "ContainerBitrateExceedsLimit",
            Self::VideoBitrateNotSupported => "VideoBitrateNotSupported",
            Self::AudioBitrateNotSupported => "AudioBitrateNotSupported",
            Self::UnknownVideoStreamInfo => "UnknownVideoStreamInfo",
            Self::UnknownAudioStreamInfo => "UnknownAudioStreamInfo",
            Self::DirectPlayError => "DirectPlayError",
            Self::VideoRangeTypeNotSupported => "VideoRangeTypeNotSupported",
            Self::VideoCodecTagNotSupported => "VideoCodecTagNotSupported",
            Self::StreamCountExceedsLimit => "StreamCountExceedsLimit",
            Self::VideoRotationNotSupported => "VideoRotationNotSupported",
        }
    }

    /// The reason's bit in a [`TranscodeReasons`] set.
    pub fn bit(self) -> u32 {
        self as u32
    }
}

/// A set of [`TranscodeReason`]s, one bit each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TranscodeReasons(
    /// The reason bits, one per [`TranscodeReason`].
    pub u32,
);

impl TranscodeReasons {
    /// Adds a reason to the set.
    pub fn insert(&mut self, r: TranscodeReason) {
        self.0 |= r.bit();
    }

    /// Whether the set has the reason.
    pub fn contains(self, r: TranscodeReason) -> bool {
        (self.0 & r.bit()) != 0
    }

    /// Whether the two sets have a reason in common.
    pub fn intersects(self, other: TranscodeReasons) -> bool {
        (self.0 & other.0) != 0
    }

    /// Whether the set has no reasons.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The names of the reasons in the set, in ascending bit order.
    pub fn names(self) -> Vec<&'static str> {
        TranscodeReason::ALL
            .into_iter()
            .filter(|reason| self.contains(*reason))
            .map(TranscodeReason::as_str)
            .collect()
    }
}

impl From<TranscodeReason> for TranscodeReasons {
    fn from(reason: TranscodeReason) -> Self {
        Self(reason.bit())
    }
}

impl BitOr for TranscodeReasons {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOr<TranscodeReason> for TranscodeReasons {
    type Output = Self;

    fn bitor(self, rhs: TranscodeReason) -> Self {
        Self(self.0 | rhs.bit())
    }
}

impl BitOrAssign for TranscodeReasons {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitOrAssign<TranscodeReason> for TranscodeReasons {
    fn bitor_assign(&mut self, rhs: TranscodeReason) {
        self.0 |= rhs.bit();
    }
}

/// What a client asks for when it plays one item: Jellyfin's `MediaOptions`, for the fields that
/// the video decision reads. `Default` allows every method.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamRequest {
    /// The item to play.
    pub item_id: String,
    /// Whether the item is video or audio.
    pub media_kind: MediaKind,
    /// The bitrate limit in bits per second. `None` uses the device profile's limit.
    pub max_bitrate: Option<i32>,
    /// The position to start at, in ticks.
    pub start_time_ticks: i64,
    /// The media source the client chose. Empty when it chose none.
    pub media_source_id: String,
    /// The audio stream the client chose. It counts only for the source named above.
    pub audio_stream_index: Option<i32>,
    /// The subtitle stream the client chose. It counts only for the source named above.
    pub subtitle_stream_index: Option<i32>,
    /// The most audio channels the client accepts.
    pub max_audio_channels: Option<i32>,
    /// Whether the client may play the file as it is.
    pub enable_direct_play: bool,
    /// Whether the client may stream the file with a video copy.
    pub enable_direct_stream: bool,
    /// Whether the client may transcode.
    pub enable_transcoding: bool,
    /// Whether the video may be copied into the transcode.
    pub allow_video_stream_copy: bool,
    /// Whether the audio may be copied into the transcode.
    pub allow_audio_stream_copy: bool,
    /// Whether subtitles are burned in when the stream is transcoded.
    pub always_burn_in_subtitle: bool,
    /// The requesting device.
    pub device_id: String,
    /// The play session.
    pub play_session_id: String,
    /// The access token, written into the transcode URL.
    pub access_token: String,
}

impl Default for StreamRequest {
    fn default() -> Self {
        Self {
            item_id: String::new(),
            media_kind: MediaKind::Video,
            max_bitrate: None,
            start_time_ticks: 0,
            media_source_id: String::new(),
            audio_stream_index: None,
            subtitle_stream_index: None,
            max_audio_channels: None,
            enable_direct_play: true,
            enable_direct_stream: true,
            enable_transcoding: true,
            allow_video_stream_copy: true,
            allow_audio_stream_copy: true,
            always_burn_in_subtitle: false,
            device_id: String::new(),
            play_session_id: String::new(),
            access_token: String::new(),
        }
    }
}

/// How one subtitle stream reaches the client (SetDeviceSpecificSubtitleInfo's per-stream result).
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleDelivery {
    /// The index of the subtitle stream.
    pub index: i32,
    /// How the subtitle is delivered.
    pub method: SubtitleDeliveryMethod,
    /// The URL of the subtitle, when it is fetched separately.
    pub url: Option<String>,
}

/// What a device can do with one media source: Jellyfin's `SetDeviceSpecificData` result. The
/// transcode URL is set only when a transcoding profile was applied.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceData {
    /// Whether the device can play the file as it is.
    pub supports_direct_play: bool,
    /// Whether the device can stream the file with a video copy.
    pub supports_direct_stream: bool,
    /// Whether the device can transcode the file.
    pub supports_transcoding: bool,
    /// How the file plays, or `None` when it doesn't play.
    pub play_method: Option<PlayMethod>,
    /// The source container, normalized to one container.
    pub container: String,
    /// The transcode URL, relative to the server.
    pub transcoding_url: Option<String>,
    /// The protocol of the transcode.
    pub transcoding_sub_protocol: StreamProtocol,
    /// The container of the transcode.
    pub transcoding_container: Option<String>,
    /// Why the file can't play as it is.
    pub transcode_reasons: TranscodeReasons,
    /// The audio stream that plays.
    pub default_audio_stream_index: Option<i32>,
    /// The subtitle stream that plays.
    pub default_subtitle_stream_index: Option<i32>,
    /// How each subtitle reaches the client.
    pub subtitles: Vec<SubtitleDelivery>,
}

/// The suffix that the transcode URL gets for the request's stream-copy and burn-in flags (MIH
/// `SetDeviceSpecificData`). Each suffix is written only when its flag differs from the default.
/// The burn-in suffix is written only when `burn_in` is set: MIH reads it from StreamInfo, which
/// an audio plan never sets.
fn transcode_suffix(request: &StreamRequest, burn_in: bool) -> String {
    let mut suffix = String::new();
    if !request.allow_video_stream_copy {
        suffix.push_str("&allowVideoStreamCopy=false");
    }
    if !request.allow_audio_stream_copy {
        suffix.push_str("&allowAudioStreamCopy=false");
    }
    if burn_in && request.always_burn_in_subtitle {
        suffix.push_str("&alwaysBurnInSubtitleWhenTranscoding=true");
    }
    suffix
}

/// Jellyfin's `SetDeviceSpecificData` for one source (MIH steps 1 to 4): what the device can do
/// with `source`, and the transcode URL when the file needs one. The video plan and the audio plan
/// are ported. An audio request with no plan gets the flags of step 2 and the source's
/// own default streams.
pub fn device_data(
    source: &SourceFacts,
    profile: &DeviceProfile,
    request: &StreamRequest,
) -> DeviceData {
    // Step 1: the request's stream indexes count only for the source that it names.
    let selected = request.media_source_id.eq_ignore_ascii_case(&source.id);
    let options = video::VideoOptions {
        profile,
        request,
        audio_stream_index: request.audio_stream_index.filter(|_| selected),
        subtitle_stream_index: request.subtitle_stream_index.filter(|_| selected),
        max_bitrate: request.max_bitrate.or(profile.max_streaming_bitrate),
    };

    // Step 2: the request's flags. A direct stream needs a video copy too.
    let mut copy = source.clone();
    copy.supports_direct_play &= request.enable_direct_play;
    copy.supports_direct_stream &= request.enable_direct_stream && request.allow_video_stream_copy;
    copy.supports_transcoding &= request.enable_transcoding;

    // Step 3: the plan. With no audio plan (no audio stream, or a transcode the source may not
    // make), the source keeps the flags of step 2 and its own default streams (MIH 272-348).
    let plan = match request.media_kind {
        MediaKind::Video => video::plan_video(&copy, &options),
        MediaKind::Audio => match audio::plan_audio(&copy, &options) {
            Some(plan) => plan,
            None => {
                return DeviceData {
                    supports_direct_play: copy.supports_direct_play,
                    supports_direct_stream: copy.supports_direct_stream,
                    supports_transcoding: copy.supports_transcoding,
                    play_method: None,
                    container: source.container.clone(),
                    transcoding_url: None,
                    transcoding_sub_protocol: StreamProtocol::Http,
                    transcoding_container: None,
                    transcode_reasons: TranscodeReasons::default(),
                    default_audio_stream_index: source.default_audio_stream_index,
                    default_subtitle_stream_index: source.default_subtitle_stream_index,
                    subtitles: Vec::new(),
                };
            }
        },
    };

    // Step 4: the flags follow the plan. A direct stream is the direct play here, since the
    // DirectStream branch is off (MIH 280-283). A transcode URL needs an applied profile. The
    // profile is an HLS streaming one of the request's media kind.
    let direct = plan.direct_play;
    let transcode_type = match request.media_kind {
        MediaKind::Video => ProfileType::Video,
        MediaKind::Audio => ProfileType::Audio,
    };
    let profile_has_hls = profile.transcoding_profiles.iter().any(|tp| {
        tp.r#type == transcode_type
            && tp.context == EncodingContext::Streaming
            && tp.protocol == StreamProtocol::Hls
    });
    let mut supports_transcoding = plan.transcode.is_some() || profile_has_hls;
    let mut play_method = direct.then_some(PlayMethod::DirectPlay);
    let mut transcoding_url = None;
    if !direct && supports_transcoding {
        match &plan.transcode {
            Some(stream) => {
                play_method = Some(PlayMethod::Transcode);
                let burn_in = request.media_kind == MediaKind::Video;
                transcoding_url = Some(format!(
                    "{}{}",
                    stream.to_url(),
                    transcode_suffix(request, burn_in)
                ));
            }
            None => supports_transcoding = false,
        }
    }

    // Subtitles: one delivery per subtitle stream, in source order. An audio item has none.
    let deliveries = match request.media_kind {
        MediaKind::Video => subtitles::deliveries(source, profile, request, &plan),
        MediaKind::Audio => Vec::new(),
    };

    let (transcoding_container, transcoding_sub_protocol) = plan
        .transcode
        .as_ref()
        .map_or((None, StreamProtocol::Http), |stream| {
            (stream.container.clone(), stream.sub_protocol)
        });

    // Step 5: the defaults and reasons of the plan.
    DeviceData {
        supports_direct_play: direct,
        supports_direct_stream: direct,
        supports_transcoding,
        play_method,
        container: plan.container,
        transcoding_url,
        transcoding_sub_protocol,
        transcoding_container,
        transcode_reasons: plan.transcode_reasons,
        default_audio_stream_index: plan.audio_stream_index,
        default_subtitle_stream_index: plan.subtitle_stream_index,
        subtitles: deliveries,
    }
}
