//! OpenID Connect sign-in: the saved settings, the settings the admin sees and saves, and the
//! public info the sign-in page reads (spec §3).

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The saved settings of the one provider. The client secret is kept as it is and never leaves
/// the server.
#[derive(Clone)]
pub struct OidcConfig {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// The scopes as typed; empty means the default.
    pub scopes: String,
    pub button_label: String,
    pub username_claim: String,
    pub group_claim: String,
    pub allowed_group: String,
    pub admin_group: String,
    pub public_url: String,
    pub allow_password_sign_in: bool,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            issuer_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scopes: String::new(),
            button_label: String::new(),
            username_claim: "preferred_username".to_string(),
            group_claim: "groups".to_string(),
            allowed_group: String::new(),
            admin_group: String::new(),
            public_url: String::new(),
            allow_password_sign_in: true,
        }
    }
}

impl OidcConfig {
    /// Whether a group is set: members may sign in (`AllowedGroup`) or may administer (`AdminGroup`).
    pub fn group_set(&self) -> bool {
        !self.allowed_group.is_empty() || !self.admin_group.is_empty()
    }
}

// The secret is left out on purpose: a settings struct must never reach a log line.
impl fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcConfig")
            .field("enabled", &self.enabled)
            .field("issuer_url", &self.issuer_url)
            .field("client_id", &self.client_id)
            .field("has_client_secret", &!self.client_secret.is_empty())
            .finish_non_exhaustive()
    }
}

/// The settings as the admin sees them. The secret shows only as set or not.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct OidcSettings {
    pub enabled: bool,
    pub issuer_url: String,
    pub client_id: String,
    pub has_client_secret: bool,
    pub scopes: String,
    pub effective_scopes: String,
    pub button_label: String,
    pub username_claim: String,
    pub group_claim: String,
    pub allowed_group: String,
    pub admin_group: String,
    pub public_url: String,
    pub callback_url: String,
    pub allow_password_sign_in: bool,
}

/// The fields of a save. An absent or null field keeps its value; a `ClientSecret` of `""` clears
/// the secret.
#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OidcSettingsUpdate {
    pub enabled: Option<bool>,
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Option<String>,
    pub button_label: Option<String>,
    pub username_claim: Option<String>,
    pub group_claim: Option<String>,
    pub allowed_group: Option<String>,
    pub admin_group: Option<String>,
    pub public_url: Option<String>,
    pub allow_password_sign_in: Option<bool>,
}

// Left without values on purpose: the body of a save holds the secret.
impl fmt::Debug for OidcSettingsUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcSettingsUpdate").finish_non_exhaustive()
    }
}

impl OidcSettingsUpdate {
    /// The settings after this save: the given fields over `current`. Text is trimmed, `PublicUrl`
    /// loses its trailing `/`s, and the secret is kept as sent.
    pub fn apply(self, current: OidcConfig) -> OidcConfig {
        OidcConfig {
            enabled: self.enabled.unwrap_or(current.enabled),
            issuer_url: trimmed(self.issuer_url, current.issuer_url),
            client_id: trimmed(self.client_id, current.client_id),
            client_secret: self.client_secret.unwrap_or(current.client_secret),
            scopes: trimmed(self.scopes, current.scopes),
            button_label: trimmed(self.button_label, current.button_label),
            username_claim: trimmed(self.username_claim, current.username_claim),
            group_claim: trimmed(self.group_claim, current.group_claim),
            allowed_group: trimmed(self.allowed_group, current.allowed_group),
            admin_group: trimmed(self.admin_group, current.admin_group),
            public_url: self.public_url.map_or(current.public_url, |url| {
                url.trim().trim_end_matches('/').to_string()
            }),
            allow_password_sign_in: self
                .allow_password_sign_in
                .unwrap_or(current.allow_password_sign_in),
        }
    }
}

/// The value sent, trimmed, or the current one when it was not sent.
fn trimmed(sent: Option<String>, current: String) -> String {
    sent.map_or(current, |value| value.trim().to_string())
}

/// What the sign-in page reads without signing in. An empty label means the translated default.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct OidcInfo {
    pub enabled: bool,
    pub button_label: String,
}

/// One account's link to the provider (AC30). The keys are PascalCase and none is left out: a
/// missing value is null.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AccountStatus {
    pub user_id: Uuid,
    pub user_name: String,
    pub is_administrator: bool,
    pub admin_from_group: bool,
    pub has_password: bool,
    pub linked: bool,
    pub provider_username: Option<String>,
    pub linked_at: Option<DateTime<Utc>>,
    pub last_sign_in_at: Option<DateTime<Utc>>,
    pub can_unlink: bool,
}
