use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::parental::ParentalPolicy;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub id: Uuid,
    pub name: String,
    pub is_administrator: bool,
    pub is_owner: bool,
    pub is_disabled: bool,
    pub last_login_date: Option<DateTime<Utc>>,
    pub configuration: serde_json::Value,
    /// Jellyfin's `UserPolicy`, with the fields Rustyfin keeps.
    #[serde(default)]
    pub policy: serde_json::Value,
}

impl UserDto {
    pub fn new(u: User, flags: UserFlags) -> Self {
        let mut policy = serde_json::json!({
            "IsAdministrator": u.is_administrator,
            "IsDisabled": flags.is_disabled,
        });
        if let Some(policy) = policy.as_object_mut() {
            ParentalPolicy::default().write_policy_fields(policy);
        }
        Self {
            id: u.id,
            name: u.name,
            is_administrator: u.is_administrator,
            is_owner: flags.is_owner,
            is_disabled: flags.is_disabled,
            last_login_date: u.last_login_date,
            configuration: u.configuration,
            policy,
        }
    }

    /// Puts the user's parental limits into `Policy`.
    pub fn with_parental(mut self, parental: &ParentalPolicy) -> Self {
        if let Some(policy) = self.policy.as_object_mut() {
            parental.write_policy_fields(policy);
        }
        self
    }
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
    /// The parental fields: absent leaves the stored value, null clears it.
    #[serde(default, deserialize_with = "present")]
    pub max_parental_rating: Option<Option<i32>>,
    #[serde(default, deserialize_with = "present")]
    pub max_parental_sub_rating: Option<Option<i32>>,
    #[serde(default, deserialize_with = "present")]
    pub block_unrated_items: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    pub blocked_tags: Option<Option<Vec<String>>>,
    #[serde(default, deserialize_with = "present")]
    pub allowed_tags: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub access_schedules: Option<Vec<serde_json::Value>>,
}

/// A field that is present, null or not: `Some(None)` for null, while an absent field stays `None`.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
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
