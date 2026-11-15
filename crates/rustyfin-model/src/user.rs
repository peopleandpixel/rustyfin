use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_administrator: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub configuration: serde_json::Value,
}

/// The account flags `User` doesn't carry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserFlags {
    pub is_owner: bool,
    pub is_disabled: bool,
}

/// A user as Jellyfin's `UserDto`. `IsAdministrator`, `IsOwner` and `IsDisabled` are Rustyfin's
/// own flat fields, which its web client reads; Jellyfin clients read `Policy`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub server_id: String,
    pub is_administrator: bool,
    pub is_owner: bool,
    pub is_disabled: bool,
    #[serde(default)]
    pub has_password: bool,
    #[serde(default)]
    pub has_configured_password: bool,
    #[serde(default)]
    pub has_configured_easy_password: bool,
    #[serde(default)]
    pub enable_auto_login: bool,
    pub last_login_date: Option<DateTime<Utc>>,
    pub configuration: serde_json::Value,
    #[serde(default)]
    pub policy: serde_json::Value,
}

impl UserDto {
    pub fn new(u: User, flags: UserFlags, server_id: &str) -> Self {
        let has_password = !u.password_hash.is_empty();
        Self {
            id: u.id,
            name: u.name,
            server_id: server_id.to_string(),
            is_administrator: u.is_administrator,
            is_owner: flags.is_owner,
            is_disabled: flags.is_disabled,
            has_password,
            has_configured_password: has_password,
            has_configured_easy_password: false,
            enable_auto_login: false,
            last_login_date: u.last_login_date,
            configuration: jellyfin_configuration(&u.configuration),
            policy: jellyfin_policy(u.is_administrator, flags.is_disabled),
        }
    }
}

/// Jellyfin's `UserPolicy` for an account. Rustyfin has no per-user library, device or feature
/// restrictions, so every access flag is on but downloading, which Rustyfin has no route for; the
/// flags for what only administrators may do here (deleting items, managing collections and
/// subtitles, controlling others' sessions) follow `is_admin`. Only `IsAdministrator` and
/// `IsDisabled` are read back by `POST /Users/{id}/Policy`.
pub fn jellyfin_policy(is_admin: bool, is_disabled: bool) -> serde_json::Value {
    // Two literals: one is past serde_json::json!'s macro recursion limit.
    let mut policy = serde_json::json!({
        "IsAdministrator": is_admin,
        "IsHidden": false,
        "EnableCollectionManagement": is_admin,
        "EnableSubtitleManagement": is_admin,
        "EnableLyricManagement": is_admin,
        "IsDisabled": is_disabled,
        "BlockedTags": [],
        "AllowedTags": [],
        "EnableUserPreferenceAccess": true,
        "AccessSchedules": [],
        "BlockUnratedItems": [],
        "EnableRemoteControlOfOtherUsers": is_admin,
        "EnableSharedDeviceControl": true,
        "EnableRemoteAccess": true,
        "EnableLiveTvManagement": is_admin,
        "EnableLiveTvAccess": true,
        "EnableMediaPlayback": true,
        "EnableAudioPlaybackTranscoding": true,
        "EnableVideoPlaybackTranscoding": true,
        "EnablePlaybackRemuxing": true,
    });
    let rest = serde_json::json!({
        "ForceRemoteSourceTranscoding": false,
        "EnableContentDeletion": is_admin,
        "EnableContentDeletionFromFolders": [],
        // No /Items/{id}/Download route yet; Jellyfin Web would offer a Download that 404s.
        "EnableContentDownloading": false,
        "EnableSyncTranscoding": true,
        "EnableMediaConversion": true,
        "EnabledDevices": [],
        "EnableAllDevices": true,
        "EnabledChannels": [],
        "EnableAllChannels": true,
        "EnabledFolders": [],
        "EnableAllFolders": true,
        "InvalidLoginAttemptCount": 0,
        "LoginAttemptsBeforeLockout": -1,
        "MaxActiveSessions": 0,
        "EnablePublicSharing": true,
        "BlockedMediaFolders": [],
        "BlockedChannels": [],
        "RemoteClientBitrateLimit": 0,
        "AuthenticationProviderId": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider",
        "PasswordResetProviderId": "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider",
        "SyncPlayAccess": "CreateAndJoinGroups",
    });
    if let (Some(policy), serde_json::Value::Object(rest)) = (policy.as_object_mut(), rest) {
        policy.extend(rest);
    }
    policy
}

/// Jellyfin's `UserConfiguration` defaults, with the keys the account has stored over them.
pub fn jellyfin_configuration(stored: &serde_json::Value) -> serde_json::Value {
    let mut config = serde_json::json!({
        "AudioLanguagePreference": "",
        "PlayDefaultAudioTrack": true,
        "SubtitleLanguagePreference": "",
        "DisplayMissingEpisodes": false,
        "GroupedFolders": [],
        "SubtitleMode": "Default",
        "DisplayCollectionsView": false,
        "EnableLocalPassword": false,
        "OrderedViews": [],
        "LatestItemsExcludes": [],
        "MyMediaExcludes": [],
        "HidePlayedInLatest": true,
        "RememberAudioSelections": true,
        "RememberSubtitleSelections": true,
        "EnableNextEpisodeAutoPlay": true,
        "CastReceiverId": "F007D354",
    });
    if let (Some(config), Some(stored)) = (config.as_object_mut(), stored.as_object()) {
        for (key, value) in stored {
            config.insert(key.clone(), value.clone());
        }
    }
    config
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicUserDto {
    pub id: Uuid,
    pub name: String,
}

impl From<User> for PublicUserDto {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            name: u.name,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateUserRequest {
    pub name: String,
    pub password: String,
    /// Optional; the new user is an administrator only when this is true.
    pub is_administrator: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChangePasswordRequest {
    /// Left out by a user without a password yet, and by an admin changing someone else's (AC25).
    pub current_pw: Option<String>,
    pub new_pw: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePolicyRequest {
    pub is_administrator: Option<bool>,
    pub is_disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ForgotPasswordRequest {
    pub entered_username: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ForgotPasswordResult {
    pub action: String,
    pub pin_file: String,
    pub pin_expiration_date: String,
}

/// Request to redeem a reset PIN with no username (`/Users/ForgotPassword/Pin`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ForgotPasswordPinRequest {
    pub pin: String,
}

/// Request to validate a password reset PIN code
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResetPasswordRequest {
    pub pin: String,
    pub username: String,
    pub temp_password: String,
}

/// Request to update a user's password after PIN validation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePasswordRequest {
    pub temp_password: String,
    pub new_password: String,
}

/// User security information
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserSecurityInfo {
    pub user_id: Uuid,
    pub name: String,
    pub is_administrator: bool,
    pub authentication_provider_id: String,
    pub authentication_provider_description: String,
    pub password_reset_provider_id: String,
    pub password_reset_provider_description: String,
    pub supports_two_step: bool,
}

/// User preferences
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserPreferences {
    pub chart_missing_episodes: String,
    pub chart_played_episodes: bool,
    pub group_episodes_by_season: bool,
    pub group_by_collection: bool,
    pub hide_audio_playlists_in_music: bool,
    pub hide_culture_titles: bool,
    pub hide_episodes: bool,
    pub hide_empty_series: bool,
    pub hide_seasons: bool,
    pub limit_field: String,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
}

/// User parental control periods
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserAccessSchedule {
    pub day_of_week: String,
    pub start_hour: u32,
    pub end_hour: u32,
}

/// Request to update user name
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateUserNameRequest {
    pub name: String,
}

/// Request to set easy PIN password
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EasyPasswordRequest {
    pub easy_password: String,
}

/// User avatar image info
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserImageInfo {
    pub user_id: Uuid,
    pub image_type: String,
    pub id: String,
}

/// The form a username is compared by (Unicode D146): NFD, full case folding, NFKC, then folding
/// and NFKC again. It is only for comparing, never stored or shown.
pub fn name_key(name: &str) -> String {
    let nfd: String = name.nfd().collect();
    let folded = UniCase::new(nfd.as_str()).to_folded_case();
    let nfkc: String = folded.as_str().nfkc().collect();
    let refolded = UniCase::new(nfkc.as_str()).to_folded_case();
    refolded.as_str().nfkc().collect()
}

/// The longest user name, in UTF-16 code units as .NET counts string lengths. Jellyfin declares
/// the same limit for its `User.Username`. Bytes don't count.
pub const MAX_NAME_UNITS: usize = 255;

/// Whether a name is over `MAX_NAME_UNITS`. It reads at most 256 units, so a huge name costs no
/// more to check than one just over the limit.
pub fn name_too_long(name: &str) -> bool {
    name.encode_utf16().nth(MAX_NAME_UNITS).is_some()
}

/// An entered name, folded once, to match against stored names (`names_match`, AC63). A name over
/// the limit has no key, so it is never folded and matches only an exactly equal name (AC61, AC62).
pub struct NameMatch<'a> {
    name: &'a str,
    key: Option<String>,
}

impl<'a> NameMatch<'a> {
    /// Folds `name`, unless it is over the limit.
    pub fn new(name: &'a str) -> Self {
        let key = (!name_too_long(name)).then(|| name_key(name));
        Self { name, key }
    }

    /// Whether `stored` matches the entered name: exactly equal, or both within the limit with
    /// equal keys. A stored name over the limit is never folded.
    pub fn matches(&self, stored: &str) -> bool {
        if self.name == stored {
            return true;
        }
        match &self.key {
            Some(key) => !name_too_long(stored) && name_key(stored) == *key,
            None => false,
        }
    }
}

/// Whether two usernames match under Unicode caseless matching (D146): any case, full case
/// folding (ß matches ss), compatibility forms and composed or decomposed accents all match.
/// Accents, spaces and digits still count. Over the limit (`MAX_NAME_UNITS`) names match only
/// exactly.
pub fn names_match(a: &str, b: &str) -> bool {
    NameMatch::new(a).matches(b)
}
