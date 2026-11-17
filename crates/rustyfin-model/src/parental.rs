//! Parental controls: the age a rating stands for, and a user's limits, compared as Jellyfin does
//! (`LocalizationManager.GetRatingScore` and `BaseItem.IsParentalAllowed`).

use serde::{Deserialize, Serialize};

/// Where a rating sits on the age scale: `score` is an age, and `sub_score` orders the ratings of
/// one age (TV-MA comes after R).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatingScore {
    #[serde(rename = "score")]
    pub score: i32,
    #[serde(rename = "subScore")]
    pub sub_score: Option<i32>,
}

impl RatingScore {
    const fn new(score: i32, sub_score: Option<i32>) -> Self {
        Self { score, sub_score }
    }
}

/// Values that mark an item as unrated rather than as an unknown rating.
const UNRATED_VALUES: [&str; 4] = ["n/a", "unrated", "not rated", "nr"];

type System = (&'static str, &'static [(&'static str, i32, Option<i32>)]);

/// Rating systems by country code, the US first: a rating that is in several systems takes the
/// first one's score, as Jellyfin falls back to the US system before the others.
const SYSTEMS: &[System] = &[
    (
        "us",
        &[
            ("Approved", 0, Some(0)),
            ("G", 0, Some(0)),
            ("TV-G", 0, Some(0)),
            ("TV-Y", 0, Some(0)),
            ("TV-Y7", 7, Some(0)),
            ("TV-Y7-FV", 7, Some(1)),
            ("PG", 10, Some(0)),
            ("TV-PG", 10, Some(0)),
            ("PG-13", 13, Some(0)),
            ("TV-14", 14, Some(0)),
            ("R", 17, Some(0)),
            ("NC-17", 17, Some(1)),
            ("TV-MA", 17, Some(1)),
            ("TV-X", 18, Some(0)),
            ("TV-AO", 18, Some(0)),
        ],
    ),
    (
        "gb",
        &[
            ("U", 0, None),
            ("UC", 0, None),
            ("PG", 10, None),
            ("12A", 12, None),
            ("12", 12, None),
            ("15", 15, None),
            ("18", 18, None),
            ("R18", 18, Some(1)),
        ],
    ),
    (
        "ca",
        &[
            ("G", 0, None),
            ("PG", 10, None),
            ("14A", 14, None),
            ("18A", 18, None),
            ("R", 18, None),
            ("A", 18, None),
        ],
    ),
    (
        "au",
        &[
            ("G", 0, None),
            ("PG", 10, None),
            ("M", 15, None),
            ("MA15+", 15, Some(1)),
            ("R18+", 18, None),
            ("X18+", 18, Some(1)),
        ],
    ),
    ("fr", &[("U", 0, None), ("TP", 0, None)]),
    ("nl", &[("AL", 0, None)]),
    ("de", &[]),
];

/// Ratings every system shares: adult and banned content, as Jellyfin's rating list adds them.
const SHARED: &[(&str, i32, Option<i32>)] = &[("XXX", 1000, None), ("Banned", 1001, None)];

/// The score of a stored rating, or `None` for an unrated item and for a rating no known system
/// has (Jellyfin treats both as unrated). Accepts what metadata sources write: "PG-13", "US-PG-13",
/// "US:R", "Rated R", "DE-16", "FSK 12", "12+", "TV-14-LV" and lists such as "SE:15 / SE:15+".
pub fn rating_score(rating: &str) -> Option<RatingScore> {
    let rating = rating.trim();
    if rating.is_empty() || is_unrated(rating) {
        return None;
    }
    if let Some(score) = single_score(rating) {
        return Some(score);
    }
    // A list of ratings: the first entry that resolves.
    if rating.contains('/') {
        return rating
            .split('/')
            .map(str::trim)
            .filter(|r| !r.is_empty() && !is_unrated(r))
            .find_map(single_score);
    }
    None
}

fn is_unrated(rating: &str) -> bool {
    UNRATED_VALUES
        .iter()
        .any(|u| u.eq_ignore_ascii_case(rating.trim()))
}

/// An age as a number: "12", "12+", or "-12" (French for "not under 12").
fn numeric_score(rating: &str) -> Option<RatingScore> {
    let n = rating.trim().trim_start_matches('-').trim_end_matches('+');
    n.parse::<i32>()
        .ok()
        .filter(|n| *n >= 0)
        .map(|n| RatingScore::new(n, None))
}

fn table_score(table: &[(&str, i32, Option<i32>)], rating: &str) -> Option<RatingScore> {
    table
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(rating))
        .map(|(_, score, sub)| RatingScore::new(*score, *sub))
}

/// A US TV rating with content descriptors ("TV-14-LV", "TV-MA L,S"): one step above the bare
/// rating, as Jellyfin's US table lists each combination.
fn tv_descriptor_score(rating: &str) -> Option<RatingScore> {
    let upper = rating.to_ascii_uppercase();
    for (base, score) in [("TV-PG", 10), ("TV-14", 14), ("TV-MA", 17)] {
        let Some(rest) = upper.strip_prefix(base) else {
            continue;
        };
        let rest = rest.trim_start_matches(['-', ' ']);
        if !rest.is_empty()
            && rest
                .chars()
                .all(|c| matches!(c, 'D' | 'L' | 'S' | 'V' | ',' | ' ' | '-'))
        {
            return Some(RatingScore::new(score, Some(1)));
        }
    }
    None
}

/// One rating, not a list: a number, a known rating (US first), or a country prefix and a rating.
fn single_score(rating: &str) -> Option<RatingScore> {
    if let Some(score) = numeric_score(rating) {
        return Some(score);
    }
    let rating = strip_rated(rating);
    if let Some(score) = known_score(rating) {
        return Some(score);
    }
    // "US:PG-13", "Germany: FSK-18", "DE-FSK-18": a country, then its rating.
    if !rating.contains('/') {
        for sep in [':', '-'] {
            let Some((country, rest)) = rating.split_once(sep) else {
                continue;
            };
            let (country, rest) = (country.trim(), rest.trim());
            if rest.is_empty() || is_unrated(rest) {
                continue;
            }
            let own = SYSTEMS
                .iter()
                .find(|(code, _)| code.eq_ignore_ascii_case(country))
                .and_then(|(_, table)| table_score(table, rest));
            if let Some(score) = own
                .or_else(|| numeric_score(rest))
                .or_else(|| known_score(rest))
            {
                return Some(score);
            }
        }
    }
    None
}

/// "Rated R" and "Rated: R" as metadata sometimes gives them.
fn strip_rated(rating: &str) -> &str {
    let lower = rating.to_ascii_lowercase();
    for prefix in ["rated :", "rated:", "rated "] {
        if lower.starts_with(prefix) {
            return rating[prefix.len()..].trim();
        }
    }
    rating
}

/// A rating found in a system's table, a TV rating with descriptors, or an FSK age.
fn known_score(rating: &str) -> Option<RatingScore> {
    SYSTEMS
        .iter()
        .find_map(|(_, table)| table_score(table, rating))
        .or_else(|| table_score(SHARED, rating))
        .or_else(|| tv_descriptor_score(rating))
        .or_else(|| {
            let upper = rating.to_ascii_uppercase();
            let age = upper.strip_prefix("FSK")?.trim_start_matches(['-', ' ']);
            numeric_score(age)
        })
}

/// The ratings `GET /Localization/ParentalRatings` offers: the US system plus the levels Jellyfin
/// adds for every country (unrated, 21, adult and banned), ordered by score.
pub fn parental_ratings() -> Vec<serde_json::Value> {
    let mut list: Vec<(String, Option<RatingScore>)> = SYSTEMS[0]
        .1
        .iter()
        .map(|(name, score, sub)| (name.to_string(), Some(RatingScore::new(*score, *sub))))
        .collect();
    list.push(("Unrated".into(), None));
    for (name, score) in [("21", 21), ("XXX", 1000), ("Banned", 1001)] {
        list.push((name.into(), Some(RatingScore::new(score, None))));
    }
    list.sort_by_key(|(_, s)| s.map(|s| (s.score, s.sub_score.unwrap_or(0))));
    list.into_iter()
        .map(|(name, score)| {
            serde_json::json!({
                "Name": name,
                "Value": score.map(|s| s.score),
                "RatingScore": score,
            })
        })
        .collect()
}

/// Jellyfin's `UnratedItem` values, the kinds of unrated item a user can be kept from. All are
/// accepted, as Jellyfin Web may send any of them, but `LiveTvChannel`, `LiveTvProgram` and
/// `ChannelContent` are kept without effect: the Live TV routes don't apply parental limits.
pub const UNRATED_ITEM_KINDS: [&str; 9] = [
    "Movie",
    "Trailer",
    "Series",
    "Music",
    "Book",
    "LiveTvChannel",
    "LiveTvProgram",
    "ChannelContent",
    "Other",
];

/// Stored `media_type` values and the unrated kind each counts as; `None` marks the folders an
/// unrated limit never hides (seasons follow their series and episodes). Any other type is "Other".
pub const UNRATED_KIND_OF_TYPE: &[(&str, Option<&str>)] = &[
    ("Movie", Some("Movie")),
    ("Trailer", Some("Trailer")),
    ("Series", Some("Series")),
    ("Episode", Some("Series")),
    ("Season", None),
    ("Audio", Some("Music")),
    ("MusicAlbum", Some("Music")),
    ("MusicVideo", Some("Music")),
    ("Book", Some("Book")),
    ("AudioBook", Some("Book")),
    ("TvChannel", Some("LiveTvChannel")),
    ("LiveTvChannel", Some("LiveTvChannel")),
    ("Program", Some("LiveTvProgram")),
    ("LiveTvProgram", Some("LiveTvProgram")),
    ("Folder", None),
    ("CollectionFolder", None),
    ("Genre", None),
    ("MusicGenre", None),
    ("MusicArtist", None),
    ("Person", None),
    ("Studio", None),
    ("Year", None),
    ("Playlist", None),
];

/// The unrated kind of a stored `media_type`, or `None` for a folder an unrated limit never hides.
pub fn unrated_kind(media_type: &str) -> Option<&'static str> {
    UNRATED_KIND_OF_TYPE
        .iter()
        .find(|(t, _)| *t == media_type)
        .map(|(_, kind)| *kind)
        .unwrap_or(Some("Other"))
}

/// A user's limits on what they see: the parental `UserPolicy` fields, and in `access` the
/// libraries, schedules and features an administrator allows them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParentalPolicy {
    /// The highest rating score the user may see; `None` for no limit.
    pub max_parental_rating: Option<i32>,
    /// With a rating at exactly `max_parental_rating`, the highest sub-score allowed.
    pub max_parental_sub_rating: Option<i32>,
    /// `UNRATED_ITEM_KINDS` hidden from the user while unrated.
    pub block_unrated_items: Vec<String>,
    /// Items with one of these tags, or under an item with one, are hidden.
    pub blocked_tags: Vec<String>,
    /// When not empty, only items under a top-level item with one of these tags are shown.
    pub allowed_tags: Vec<String>,
    /// Library access, access schedules and the feature switches.
    pub access: AccessPolicy,
}

impl ParentalPolicy {
    /// True when the policy hides anything.
    pub fn is_restricted(&self) -> bool {
        self.limits_ratings()
            || !self.blocked_tags.is_empty()
            || !self.allowed_tags.is_empty()
            || self.enabled_folders().is_some()
    }

    /// True when ratings or tags hide items (library access aside).
    pub fn filters_items(&self) -> bool {
        self.limits_ratings() || !self.blocked_tags.is_empty() || !self.allowed_tags.is_empty()
    }

    /// The library ids the user may see, or `None` for all of them.
    pub fn enabled_folders(&self) -> Option<&[String]> {
        (!self.access.enable_all_folders).then_some(self.access.enabled_folders.as_slice())
    }

    /// Whether the user may see library `id` (compared without case, as ids may be stored either way).
    pub fn allows_folder(&self, id: &str) -> bool {
        self.enabled_folders()
            .is_none_or(|ids| ids.iter().any(|f| f.eq_ignore_ascii_case(id)))
    }

    /// True when ratings matter: a maximum, or a kind of unrated item to hide.
    pub fn limits_ratings(&self) -> bool {
        self.max_parental_rating.is_some() || !self.block_unrated_items.is_empty()
    }

    /// Whether a rated item with this score is within the maximum (Jellyfin's comparison: a lower
    /// score passes, and at the maximum the sub-score decides).
    pub fn allows_score(&self, rating: RatingScore) -> bool {
        let Some(max) = self.max_parental_rating else {
            return true;
        };
        if rating.score != max {
            return rating.score < max;
        }
        match self.max_parental_sub_rating {
            None => true,
            Some(max_sub) => rating.sub_score.unwrap_or(0) <= max_sub,
        }
    }

    /// Whether an item with this stored rating passes the rating limits. An unrated item passes
    /// unless its kind is blocked while unrated.
    pub fn allows_rating(&self, rating: Option<&str>, unrated_kind: Option<&str>) -> bool {
        match rating.and_then(rating_score) {
            Some(score) => self.allows_score(score),
            None => !unrated_kind.is_some_and(|k| self.block_unrated_items.iter().any(|b| b == k)),
        }
    }

    /// The policy's tag lists as `tag_key`s.
    pub fn blocked_tag_keys(&self) -> Vec<String> {
        self.blocked_tags
            .iter()
            .map(|t| tag_key(t))
            .filter(|t| !t.is_empty())
            .collect()
    }

    pub fn allowed_tag_keys(&self) -> Vec<String> {
        self.allowed_tags
            .iter()
            .map(|t| tag_key(t))
            .filter(|t| !t.is_empty())
            .collect()
    }

    /// Writes the policy's fields into a Jellyfin `UserPolicy` object.
    pub fn write_policy_fields(&self, policy: &mut serde_json::Map<String, serde_json::Value>) {
        policy.insert("MaxParentalRating".into(), self.max_parental_rating.into());
        policy.insert(
            "MaxParentalSubRating".into(),
            self.max_parental_sub_rating.into(),
        );
        policy.insert(
            "BlockUnratedItems".into(),
            self.block_unrated_items.clone().into(),
        );
        policy.insert("BlockedTags".into(), self.blocked_tags.clone().into());
        policy.insert("AllowedTags".into(), self.allowed_tags.clone().into());
        if let Ok(serde_json::Value::Object(access)) = serde_json::to_value(&self.access) {
            for (key, value) in access {
                // EnableContentDeletion left unset follows the administrator flag already written.
                if !value.is_null() {
                    policy.insert(key, value);
                }
            }
        }
    }
}

/// The `UserPolicy` fields on access that an administrator sets per user, stored as given. The
/// defaults are Jellyfin's for a new user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccessPolicy {
    /// Left out of the public user list on the sign-in page.
    pub is_hidden: bool,
    /// May sign in and use the server from outside the local network.
    pub enable_remote_access: bool,
    pub enable_media_playback: bool,
    pub enable_audio_playback_transcoding: bool,
    pub enable_video_playback_transcoding: bool,
    /// May play a stream that is remuxed without transcoding (Jellyfin's direct stream).
    pub enable_playback_remuxing: bool,
    pub force_remote_source_transcoding: bool,
    /// May delete media items. `None` until set: administrators may, others not.
    pub enable_content_deletion: Option<bool>,
    /// Without `enable_content_deletion`, the libraries whose items the user may delete
    /// (Jellyfin's `EnableContentDeletionFromFolders`).
    pub enable_content_deletion_from_folders: Vec<String>,
    /// May download media files (`/Items/{id}/Download`); on for a new user, as in Jellyfin.
    pub enable_content_downloading: bool,
    /// The most sessions the user may have at once; 0 for no limit.
    pub max_active_sessions: i32,
    /// The highest bitrate a client outside the local network may stream at; 0 for no limit.
    pub remote_client_bitrate_limit: i64,
    pub enable_all_folders: bool,
    /// The library ids the user may see while `enable_all_folders` is off.
    pub enabled_folders: Vec<String>,
    /// When not empty, the times a user who isn't an administrator may use the server.
    pub access_schedules: Vec<AccessSchedule>,
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self {
            is_hidden: false,
            enable_remote_access: true,
            enable_media_playback: true,
            enable_audio_playback_transcoding: true,
            enable_video_playback_transcoding: true,
            enable_playback_remuxing: true,
            force_remote_source_transcoding: false,
            enable_content_deletion: None,
            enable_content_deletion_from_folders: Vec::new(),
            enable_content_downloading: true,
            max_active_sessions: 0,
            remote_client_bitrate_limit: 0,
            enable_all_folders: true,
            enabled_folders: Vec::new(),
            access_schedules: Vec::new(),
        }
    }
}

impl AccessPolicy {
    /// Jellyfin's `User.IsParentalScheduleAllowed`: with no schedule any time is allowed, else
    /// the local `weekday` and `hour` (hours since midnight, with fractions) must fall within one.
    pub fn schedule_allows(&self, weekday: chrono::Weekday, hour: f64) -> bool {
        self.access_schedules.is_empty()
            || self
                .access_schedules
                .iter()
                .any(|s| s.covers_day(weekday) && hour >= s.start_hour && hour <= s.end_hour)
    }

    /// `schedule_allows` at the server's local time now.
    pub fn schedule_allows_now(&self) -> bool {
        use chrono::{Datelike, Timelike};
        let now = chrono::Local::now();
        let hour = f64::from(now.hour())
            + f64::from(now.minute()) / 60.0
            + f64::from(now.second()) / 3600.0;
        self.schedule_allows(now.weekday(), hour)
    }
}

/// Jellyfin's `DynamicDayOfWeek` values.
pub const SCHEDULE_DAYS: [&str; 10] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Everyday",
    "Weekday",
    "Weekend",
];

/// One period of Jellyfin's `AccessSchedule`: a day (or Everyday, Weekday, Weekend) and the hours
/// from `start_hour` to `end_hour`, both 0 to 24, in halves or any fraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AccessSchedule {
    pub day_of_week: String,
    pub start_hour: f64,
    pub end_hour: f64,
}

impl AccessSchedule {
    /// The schedule with its day in Jellyfin's spelling, or `None` when the day is unknown or the
    /// hours aren't an ordered range within 0 to 24.
    pub fn validated(&self) -> Option<Self> {
        let day = SCHEDULE_DAYS
            .iter()
            .find(|d| d.eq_ignore_ascii_case(self.day_of_week.trim()))?;
        let hours = 0.0..=24.0;
        (hours.contains(&self.start_hour)
            && hours.contains(&self.end_hour)
            && self.start_hour <= self.end_hour)
            .then(|| Self {
                day_of_week: (*day).to_string(),
                ..self.clone()
            })
    }

    fn covers_day(&self, weekday: chrono::Weekday) -> bool {
        use chrono::Weekday::*;
        match self.day_of_week.as_str() {
            "Everyday" => true,
            "Weekday" => !matches!(weekday, Sat | Sun),
            "Weekend" => matches!(weekday, Sat | Sun),
            day => day == weekday_name(weekday),
        }
    }
}

fn weekday_name(weekday: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match weekday {
        Sun => "Sunday",
        Mon => "Monday",
        Tue => "Tuesday",
        Wed => "Wednesday",
        Thu => "Thursday",
        Fri => "Friday",
        Sat => "Saturday",
    }
}

/// A tag as limits compare it: trimmed, without case.
pub fn tag_key(tag: &str) -> String {
    tag.trim().to_lowercase()
}

/// The `tag_key`s of a stored `tags` value ("Kids|Holiday").
pub fn item_tag_keys(tags: &str) -> impl Iterator<Item = String> + '_ {
    tags.split('|').map(tag_key).filter(|t| !t.is_empty())
}

/// The canonical `UNRATED_ITEM_KINDS` name of a value, matched without case.
pub fn unrated_item_kind(value: &str) -> Option<&'static str> {
    UNRATED_ITEM_KINDS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(value.trim()))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(r: &str) -> Option<(i32, Option<i32>)> {
        rating_score(r).map(|s| (s.score, s.sub_score))
    }

    #[test]
    fn r2parental_scores_follow_jellyfin_lookup() {
        assert_eq!(score("G"), Some((0, Some(0))));
        assert_eq!(score("pg-13"), Some((13, Some(0))));
        assert_eq!(score("R"), Some((17, Some(0))));
        assert_eq!(score("TV-MA"), Some((17, Some(1))));
        assert_eq!(score("TV-14-LV"), Some((14, Some(1))));
        assert_eq!(score("US-PG-13"), Some((13, Some(0))));
        assert_eq!(score("US:R"), Some((17, Some(0))));
        assert_eq!(score("Rated R"), Some((17, Some(0))));
        assert_eq!(score("DE-16"), Some((16, None)));
        assert_eq!(score("FSK 12"), Some((12, None)));
        assert_eq!(score("DE-FSK-18"), Some((18, None)));
        assert_eq!(score("12+"), Some((12, None)));
        assert_eq!(score("-12"), Some((12, None)));
        assert_eq!(score("15"), Some((15, None)));
        assert_eq!(score("GB-15"), Some((15, None)));
        assert_eq!(score("MA15+"), Some((15, Some(1))));
        assert_eq!(score("SE:15 / SE:15+"), Some((15, None)));
        assert_eq!(score("NR / PG"), Some((10, Some(0))));
        assert_eq!(score("XXX"), Some((1000, None)));
        for unrated in [
            "",
            "  ",
            "NR",
            "Not Rated",
            "n/a",
            "Unrated",
            "US-NR",
            "Mystery",
        ] {
            assert_eq!(score(unrated), None, "{unrated:?}");
        }
    }

    #[test]
    fn r2parental_policy_compares_like_jellyfin() {
        let p = ParentalPolicy {
            max_parental_rating: Some(10),
            ..Default::default()
        };
        assert!(p.allows_rating(Some("PG"), Some("Movie")));
        assert!(p.allows_rating(Some("TV-PG-V"), Some("Series")));
        assert!(!p.allows_rating(Some("PG-13"), Some("Movie")));
        assert!(p.allows_rating(None, Some("Movie")));
        let p = ParentalPolicy {
            max_parental_rating: Some(17),
            max_parental_sub_rating: Some(0),
            block_unrated_items: vec!["Movie".into()],
            ..Default::default()
        };
        assert!(p.allows_rating(Some("R"), Some("Movie")));
        assert!(!p.allows_rating(Some("TV-MA"), Some("Series")));
        assert!(!p.allows_rating(None, Some("Movie")));
        assert!(!p.allows_rating(Some("Mystery"), Some("Movie")));
        assert!(p.allows_rating(None, Some("Series")));
        assert!(p.allows_rating(None, None));
    }

    #[test]
    fn r2parental_unrated_kinds_follow_item_types() {
        assert_eq!(unrated_kind("Episode"), Some("Series"));
        assert_eq!(unrated_kind("Season"), None);
        assert_eq!(unrated_kind("Photo"), Some("Other"));
        assert_eq!(unrated_kind("Video"), Some("Other"));
        assert_eq!(unrated_item_kind("movie"), Some("Movie"));
        assert_eq!(unrated_item_kind("Cartoon"), None);
    }

    #[test]
    fn r2parental_rating_list_is_ordered_with_unrated_first() {
        let list = parental_ratings();
        assert_eq!(list[0]["Name"], "Unrated");
        assert_eq!(list[0]["Value"], serde_json::Value::Null);
        let pg = list.iter().find(|r| r["Name"] == "PG").unwrap();
        assert_eq!(pg["Value"], 10);
        assert_eq!(pg["RatingScore"]["score"], 10);
        assert_eq!(list.last().unwrap()["Name"], "Banned");
    }

    fn schedule(day: &str, start: f64, end: f64) -> AccessSchedule {
        AccessSchedule {
            day_of_week: day.into(),
            start_hour: start,
            end_hour: end,
        }
    }

    #[test]
    fn r3parental_schedule_matches_jellyfin_days_and_hours() {
        use chrono::Weekday::*;
        let open = AccessPolicy::default();
        assert!(
            open.schedule_allows(Mon, 3.0),
            "no schedule allows any time"
        );
        let policy = AccessPolicy {
            access_schedules: vec![
                schedule("Weekday", 8.0, 17.5),
                schedule("Sunday", 10.0, 12.0),
            ],
            ..AccessPolicy::default()
        };
        assert!(policy.schedule_allows(Mon, 8.0), "start hour is inclusive");
        assert!(policy.schedule_allows(Fri, 17.5), "end hour is inclusive");
        assert!(!policy.schedule_allows(Fri, 17.6));
        assert!(
            !policy.schedule_allows(Sat, 9.0),
            "Weekday leaves out Saturday"
        );
        assert!(policy.schedule_allows(Sun, 11.0));
        assert!(!policy.schedule_allows(Sun, 13.0));
        let all = AccessPolicy {
            access_schedules: vec![schedule("Everyday", 0.0, 24.0)],
            ..AccessPolicy::default()
        };
        assert!(all.schedule_allows(Sat, 23.9));
    }

    #[test]
    fn r3parental_schedule_validation() {
        assert_eq!(
            schedule("weekend", 1.0, 2.0)
                .validated()
                .unwrap()
                .day_of_week,
            "Weekend"
        );
        assert!(schedule("Someday", 1.0, 2.0).validated().is_none());
        assert!(schedule("Monday", 5.0, 4.0).validated().is_none());
        assert!(schedule("Monday", -1.0, 4.0).validated().is_none());
        assert!(schedule("Monday", 0.0, 24.5).validated().is_none());
    }

    #[test]
    fn r3parental_library_access_restricts() {
        let mut policy = ParentalPolicy::default();
        assert!(!policy.is_restricted());
        assert!(policy.allows_folder("abc"));
        policy.access.enable_all_folders = false;
        policy.access.enabled_folders = vec!["ABC".into()];
        assert!(policy.is_restricted());
        assert!(policy.allows_folder("abc"));
        assert!(!policy.allows_folder("def"));
    }
}
