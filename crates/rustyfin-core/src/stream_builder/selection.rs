//! The default audio and subtitle tracks a user's settings pick, as Jellyfin's
//! `MediaSourceManager.SetDefaultAudioAndSubtitleStreamIndices` and `MediaStreamSelector` pick
//! them, and the subtitle scores that StreamBuilder's own default subtitle reads.

use rustyfin_model::device_profile::{SubtitleDeliveryMethod, SubtitleProfile};

use super::{SourceFacts, StreamFacts, StreamKind};
use crate::subtitle::is_image_subtitle;

/// Jellyfin's `SubtitlePlaybackMode`, the user's `SubtitleMode` setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SubtitleMode {
    #[default]
    Default,
    Always,
    OnlyForced,
    None,
    Smart,
}

impl SubtitleMode {
    /// The mode a user configuration names, ignoring case; Default for any other value.
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "always" => Self::Always,
            "onlyforced" => Self::OnlyForced,
            "none" => Self::None,
            "smart" => Self::Smart,
            _ => Self::Default,
        }
    }
}

/// The user settings that decide a video's default tracks. The language lists are already
/// normalized as Jellyfin's `NormalizeLanguage` does (a language's three-letter codes); empty
/// means no preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPreferences {
    /// `PlayDefaultAudioTrack`.
    pub play_default_audio_track: bool,
    /// `AudioLanguagePreference` is `OriginalLanguage`.
    pub original_language_audio: bool,
    /// `AudioLanguagePreference`, normalized.
    pub audio_languages: Vec<String>,
    /// `SubtitleMode`.
    pub subtitle_mode: SubtitleMode,
    /// `SubtitleLanguagePreference`, normalized.
    pub subtitle_languages: Vec<String>,
}

impl Default for TrackPreferences {
    /// Jellyfin's defaults for a new user.
    fn default() -> Self {
        Self {
            play_default_audio_track: true,
            original_language_audio: false,
            audio_languages: Vec::new(),
            subtitle_mode: SubtitleMode::Default,
            subtitle_languages: Vec::new(),
        }
    }
}

/// Which settings chose a source's default audio stream (Jellyfin's `AudioIndexSource`). They
/// limit the audio streams a transcode may copy instead of the default one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioIndexSource {
    /// `PlayDefaultAudioTrack` was on: only default streams are candidates.
    pub default: bool,
    /// An audio language was preferred: only streams in the default stream's language are.
    pub language: bool,
    /// The user's remembered track is the default: it is the only candidate.
    pub user: bool,
}

impl SourceFacts {
    /// Sets the default audio and subtitle streams and the subtitle scores from a user's
    /// settings, as Jellyfin does for a video when no remembered track applies.
    pub fn apply_preferences(&mut self, prefs: &TrackPreferences) {
        self.apply_preferences_and_remembered(prefs, None, None);
    }

    /// As `apply_preferences`, with the user's remembered audio and subtitle streams (already
    /// checked against this source; subtitle -1 is off) as the defaults where given, as Jellyfin's
    /// `SetDefaultAudioAndSubtitleStreamIndices` does: the subtitle default then follows the
    /// remembered audio's language.
    pub fn apply_preferences_and_remembered(
        &mut self,
        prefs: &TrackPreferences,
        audio: Option<i32>,
        subtitle: Option<i32>,
    ) {
        if let Some(index) = audio {
            self.default_audio_stream_index = Some(index);
            self.audio_index_source = Some(AudioIndexSource {
                user: true,
                ..AudioIndexSource::default()
            });
        } else if prefs.original_language_audio && prefs.play_default_audio_track {
            // The item's original language isn't known, so no language is preferred
            self.default_audio_stream_index = default_audio_index(&self.streams, &[], true);
            self.audio_index_source = Some(AudioIndexSource::default());
        } else {
            self.default_audio_stream_index = default_audio_index(
                &self.streams,
                &prefs.audio_languages,
                prefs.play_default_audio_track,
            );
            self.audio_index_source = Some(AudioIndexSource {
                default: prefs.play_default_audio_track,
                language: !prefs.audio_languages.is_empty(),
                user: false,
            });
        }
        let audio_language = self.default_audio_stream_index.and_then(|index| {
            self.streams
                .iter()
                .find(|s| s.kind == StreamKind::Audio && s.index == index)
                .and_then(|s| s.language.clone())
        });
        // A remembered subtitle (off included) is the default and nothing is scored, as Jellyfin's
        // SetDefaultSubtitleStreamIndex returns before scoring, so the builder's tie-break for an
        // external stream can't replace it
        if subtitle.is_some() {
            self.default_subtitle_stream_index = subtitle;
            return;
        }
        let languages = &prefs.subtitle_languages;
        let mode = prefs.subtitle_mode;
        self.default_subtitle_stream_index =
            default_subtitle_index(&self.streams, languages, mode, audio_language.as_deref());
        set_subtitle_scores(
            &mut self.streams,
            languages,
            mode,
            audio_language.as_deref(),
        );
    }
}

/// Whether a stream's codec is a text subtitle (Jellyfin's `IsTextSubtitleStream`, which its
/// `SupportsExternalStream` follows for subtitles).
fn is_text_subtitle(stream: &StreamFacts) -> bool {
    stream.kind == StreamKind::Subtitle && !is_image_subtitle(&stream.codec)
}

/// Jellyfin's `GetStreamScore`: the language's place in the preferences, then the forced,
/// default, external-stream, text and external flags, one digit each.
pub(super) fn stream_score(stream: &StreamFacts, languages: &[String]) -> i32 {
    let place = stream.language.as_deref().and_then(|language| {
        languages
            .iter()
            .position(|l| l.eq_ignore_ascii_case(language))
    });
    let mut score = place.map_or(1, |index| 101 - index as i32);
    let text = is_text_subtitle(stream);
    for flag in [
        stream.is_forced,
        stream.is_default,
        text,
        text,
        stream.is_external,
    ] {
        score = score * 10 + if flag { 2 } else { 1 };
    }
    score
}

/// The streams of `kind`, best score first; equal scores keep their order.
fn sorted_streams<'a>(
    streams: &'a [StreamFacts],
    kind: StreamKind,
    languages: &[String],
) -> Vec<&'a StreamFacts> {
    let mut sorted: Vec<&StreamFacts> = streams.iter().filter(|s| s.kind == kind).collect();
    sorted.sort_by_key(|s| std::cmp::Reverse(stream_score(s, languages)));
    sorted
}

/// Jellyfin's `GetDefaultAudioStreamIndex`: the first default stream when `prefer_default`,
/// else the best scored one.
fn default_audio_index(
    streams: &[StreamFacts],
    languages: &[String],
    prefer_default: bool,
) -> Option<i32> {
    let sorted = sorted_streams(streams, StreamKind::Audio, languages);
    prefer_default
        .then(|| sorted.iter().find(|s| s.is_default))
        .flatten()
        .or(sorted.first())
        .map(|s| s.index)
}

/// Whether `language` is preferred; no preference counts every language.
fn matches_language(language: Option<&str>, languages: &[String]) -> bool {
    languages.is_empty() || language.is_some_and(|l| contains(languages, l))
}

/// Whether `languages` holds `language`, ignoring case.
fn contains(languages: &[String], language: &str) -> bool {
    languages.iter().any(|l| l.eq_ignore_ascii_case(language))
}

/// Jellyfin's `IsLanguageUndefined`.
fn is_language_undefined(language: Option<&str>) -> bool {
    language.is_none_or(|l| {
        l.is_empty()
            || ["und", "unknown", "undetermined", "mul", "zxx"]
                .iter()
                .any(|u| u.eq_ignore_ascii_case(l))
    })
}

/// Jellyfin's `BehaviorOnlyForced`: the forced streams in a preferred or undefined language,
/// preferred ones first, then undefined ones.
fn only_forced<'a>(sorted: &[&'a StreamFacts], languages: &[String]) -> Vec<&'a StreamFacts> {
    let mut forced: Vec<&StreamFacts> = sorted
        .iter()
        .copied()
        .filter(|s| {
            s.is_forced
                && (matches_language(s.language.as_deref(), languages)
                    || is_language_undefined(s.language.as_deref()))
        })
        .collect();
    forced.sort_by_key(|s| {
        (
            !matches_language(s.language.as_deref(), languages),
            !is_language_undefined(s.language.as_deref()),
        )
    });
    forced
}

/// Whether the audio language is one of the preferred subtitle languages.
fn audio_in_languages(audio_language: Option<&str>, languages: &[String]) -> bool {
    audio_language.is_some_and(|l| contains(languages, l))
}

/// Jellyfin's `MediaStreamSelector.GetDefaultSubtitleStreamIndex`.
pub(super) fn default_subtitle_index(
    streams: &[StreamFacts],
    languages: &[String],
    mode: SubtitleMode,
    audio_language: Option<&str>,
) -> Option<i32> {
    if mode == SubtitleMode::None {
        return None;
    }
    let mut sorted: Vec<&StreamFacts> = streams
        .iter()
        .filter(|s| s.kind == StreamKind::Subtitle)
        .collect();
    // External, then default, then full tracks in a preferred language, then forced ones in a
    // preferred or undefined language, then any forced one
    sorted.sort_by_key(|s| {
        let language = s.language.as_deref();
        let preferred = matches_language(language, languages);
        (
            !s.is_external,
            !s.is_default,
            !(!s.is_forced && preferred),
            !(s.is_forced && preferred),
            !(s.is_forced && is_language_undefined(language)),
            !s.is_forced,
        )
    });
    let flagged = |s: &&&StreamFacts| s.is_external || s.is_default || s.is_forced;
    let stream = match mode {
        SubtitleMode::Default => sorted.iter().find(flagged).copied(),
        SubtitleMode::Smart if !audio_in_languages(audio_language, languages) => sorted
            .iter()
            .find(|s| matches_language(s.language.as_deref(), languages))
            .copied(),
        SubtitleMode::Smart | SubtitleMode::OnlyForced => {
            only_forced(&sorted, languages).first().copied()
        }
        SubtitleMode::Always => sorted
            .iter()
            .find(|s| !s.is_forced && matches_language(s.language.as_deref(), languages))
            .copied()
            .or_else(|| only_forced(&sorted, languages).first().copied()),
        SubtitleMode::None => None,
    };
    stream.map(|s| s.index)
}

/// Jellyfin's `SetSubtitleStreamScores`: scores the subtitle streams the mode would load.
fn set_subtitle_scores(
    streams: &mut [StreamFacts],
    languages: &[String],
    mode: SubtitleMode,
    audio_language: Option<&str>,
) {
    for stream in streams.iter_mut() {
        stream.score = None;
    }
    if mode == SubtitleMode::None {
        return;
    }
    let sorted = sorted_streams(streams, StreamKind::Subtitle, languages);
    let preferred = |s: &&StreamFacts| matches_language(s.language.as_deref(), languages);
    let loaded: Vec<&StreamFacts> = match mode {
        SubtitleMode::Default => sorted
            .iter()
            .copied()
            .filter(|s| s.is_external || s.is_default || s.is_forced)
            .collect(),
        SubtitleMode::Smart if !audio_in_languages(audio_language, languages) => {
            sorted.iter().copied().filter(preferred).collect()
        }
        SubtitleMode::Smart | SubtitleMode::OnlyForced => only_forced(&sorted, languages),
        SubtitleMode::Always => sorted
            .iter()
            .copied()
            .filter(|s| !s.is_forced && preferred(s))
            .collect(),
        SubtitleMode::None => Vec::new(),
    };
    let scores: Vec<(i32, i32)> = loaded
        .iter()
        .map(|s| (s.index, stream_score(s, languages)))
        .collect();
    for stream in streams.iter_mut() {
        if stream.kind == StreamKind::Subtitle
            && let Some((_, score)) = scores.iter().find(|(index, _)| *index == stream.index)
        {
            stream.score = Some(*score);
        }
    }
}

/// StreamBuilder's `GetDefaultSubtitleStreamIndex`: when several subtitle streams share the top
/// score, the first that a profile delivers as an external file of its own codec; otherwise the
/// source's default subtitle.
pub(super) fn builder_default_subtitle(
    source: &SourceFacts,
    profiles: &[SubtitleProfile],
) -> Option<i32> {
    let subtitles = || {
        source
            .streams
            .iter()
            .filter(|s| s.kind == StreamKind::Subtitle)
    };
    let top = subtitles().filter_map(|s| s.score).max().unwrap_or(-1);
    let tied: Vec<&StreamFacts> = subtitles().filter(|s| s.score == Some(top)).collect();
    if tied.len() > 1 {
        let external = tied.iter().find(|s| {
            profiles.iter().any(|p| {
                p.method == SubtitleDeliveryMethod::External
                    && p.format.eq_ignore_ascii_case(&s.codec)
            })
        });
        if let Some(stream) = external {
            return Some(stream.index);
        }
    }
    source.default_subtitle_stream_index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(index: i32, language: Option<&str>, flags: &str) -> StreamFacts {
        StreamFacts {
            index,
            kind: StreamKind::Subtitle,
            codec: if flags.contains('p') {
                "pgssub"
            } else {
                "subrip"
            }
            .to_string(),
            language: language.map(str::to_string),
            is_external: flags.contains('x'),
            is_default: flags.contains('d'),
            is_forced: flags.contains('f'),
            ..Default::default()
        }
    }

    fn audio(index: i32, language: Option<&str>, is_default: bool) -> StreamFacts {
        StreamFacts {
            index,
            kind: StreamKind::Audio,
            codec: "aac".to_string(),
            language: language.map(str::to_string),
            is_default,
            ..Default::default()
        }
    }

    fn langs(list: &[&str]) -> Vec<String> {
        list.iter().map(|l| l.to_string()).collect()
    }

    /// Default mode, no preferences: an external stream, then a default one, then a forced one;
    /// plain streams are never picked. A default stream beats a forced one, as in Jellyfin.
    #[test]
    fn r3gaps3_default_mode_prefers_external_then_default_then_forced() {
        let none: Vec<String> = Vec::new();
        let pick = |streams: &[StreamFacts]| {
            default_subtitle_index(streams, &none, SubtitleMode::Default, Some("eng"))
        };
        assert_eq!(
            pick(&[sub(2, Some("eng"), "f"), sub(3, Some("eng"), "d")]),
            Some(3)
        );
        assert_eq!(
            pick(&[sub(2, Some("eng"), "f"), sub(3, Some("eng"), "")]),
            Some(2)
        );
        assert_eq!(
            pick(&[sub(0, None, "x"), sub(3, Some("eng"), "d")]),
            Some(0)
        );
        assert_eq!(
            pick(&[sub(2, Some("eng"), ""), sub(3, Some("eng"), "")]),
            None
        );
        assert_eq!(
            default_subtitle_index(&[sub(2, Some("eng"), "d")], &none, SubtitleMode::None, None),
            None
        );
    }

    /// Smart: when the audio isn't in a preferred subtitle language, the first stream in one;
    /// when it is, only a forced one. OnlyForced takes a preferred or undefined forced stream.
    /// Always takes a full stream in a preferred language, else a forced one.
    #[test]
    fn r3gaps3_smart_always_and_only_forced_modes() {
        let eng = langs(&["eng"]);
        let streams = [
            sub(2, Some("fre"), ""),
            sub(3, Some("eng"), "f"),
            sub(4, Some("eng"), ""),
            sub(5, None, "f"),
        ];
        let pick = |mode, audio| default_subtitle_index(&streams, &eng, mode, audio);
        assert_eq!(pick(SubtitleMode::Smart, Some("jpn")), Some(4));
        assert_eq!(pick(SubtitleMode::Smart, Some("eng")), Some(3));
        assert_eq!(pick(SubtitleMode::OnlyForced, Some("jpn")), Some(3));
        assert_eq!(pick(SubtitleMode::Always, Some("eng")), Some(4));
        let forced_only = [sub(2, Some("fre"), ""), sub(5, Some("und"), "f")];
        assert_eq!(
            default_subtitle_index(&forced_only, &eng, SubtitleMode::Always, Some("eng")),
            Some(5)
        );
    }

    /// Jellyfin's score digits: language place, forced, default, external stream, text, external.
    #[test]
    fn r3gaps3_stream_scores_follow_jellyfin() {
        let eng = langs(&["fre", "eng"]);
        assert_eq!(stream_score(&sub(2, Some("eng"), "d"), &eng), 10_012_221);
        assert_eq!(stream_score(&sub(2, Some("ger"), "fx"), &eng), 121_222);
        assert_eq!(stream_score(&sub(2, None, "p"), &eng), 111_111);
    }

    /// The default audio stream: the first default one with PlayDefaultAudioTrack, else the one
    /// in the best preferred language. A language preference limits copy candidates to it.
    #[test]
    fn r3gaps3_default_audio_follows_the_preferences() {
        let mut source = SourceFacts {
            streams: vec![
                audio(1, Some("jpn"), true),
                audio(2, Some("eng"), false),
                sub(3, Some("eng"), "f"),
            ],
            ..Default::default()
        };
        source.apply_preferences(&TrackPreferences::default());
        assert_eq!(source.default_audio_stream_index, Some(1));
        assert_eq!(
            source.audio_index_source,
            Some(AudioIndexSource {
                default: true,
                ..AudioIndexSource::default()
            })
        );
        assert_eq!(source.default_subtitle_stream_index, Some(3));

        let prefs = TrackPreferences {
            play_default_audio_track: false,
            audio_languages: langs(&["eng"]),
            subtitle_mode: SubtitleMode::OnlyForced,
            subtitle_languages: langs(&["fre"]),
            ..Default::default()
        };
        source.apply_preferences(&prefs);
        assert_eq!(source.default_audio_stream_index, Some(2));
        assert_eq!(
            source.audio_index_source,
            Some(AudioIndexSource {
                language: true,
                ..AudioIndexSource::default()
            })
        );
        assert_eq!(source.default_subtitle_stream_index, None);
    }

    /// Two subtitles tied on the top score: the builder takes the first one a profile delivers
    /// as an external file; with no such profile, the selector's default stays.
    #[test]
    fn r3gaps3_tied_scores_prefer_an_external_delivery() {
        let mut source = SourceFacts {
            streams: vec![
                audio(1, Some("eng"), true),
                sub(2, Some("eng"), "fp"),
                sub(3, Some("eng"), "f"),
            ],
            ..Default::default()
        };
        // Two forced streams: the selector takes the first, and their text digits score them
        // apart, so make them tie
        source.apply_preferences(&TrackPreferences::default());
        assert_eq!(source.default_subtitle_stream_index, Some(2));
        source.streams[1].score = source.streams[2].score;
        let srt = SubtitleProfile {
            format: "subrip".to_string(),
            method: SubtitleDeliveryMethod::External,
            didl_mode: String::new(),
            language: String::new(),
            container: String::new(),
        };
        assert_eq!(builder_default_subtitle(&source, &[srt]), Some(3));
        assert_eq!(builder_default_subtitle(&source, &[]), Some(2));
    }

    /// A remembered subtitle, off included, is not scored, as Jellyfin's
    /// SetDefaultSubtitleStreamIndex returns first: the builder's tie-break for an external
    /// delivery can't replace it.
    #[test]
    fn r3prefs_remembered_subtitle_survives_tied_scores() {
        let streams = vec![
            audio(1, Some("eng"), true),
            sub(2, Some("eng"), "d"),
            sub(3, Some("eng"), "d"),
        ];
        let subrip = SubtitleProfile {
            format: "subrip".to_string(),
            method: SubtitleDeliveryMethod::External,
            didl_mode: String::new(),
            language: String::new(),
            container: String::new(),
        };
        let prefs = TrackPreferences::default();
        let mut source = SourceFacts {
            streams,
            ..Default::default()
        };
        // Not remembered: the two default streams tie and the first external delivery wins
        source.apply_preferences(&prefs);
        assert_eq!(
            builder_default_subtitle(&source, &[subrip.clone()]),
            Some(2)
        );
        for remembered in [-1, 3] {
            for stream in &mut source.streams {
                stream.score = None;
            }
            source.apply_preferences_and_remembered(&prefs, None, Some(remembered));
            assert_eq!(
                builder_default_subtitle(&source, &[subrip.clone()]),
                Some(remembered)
            );
        }
    }
}
