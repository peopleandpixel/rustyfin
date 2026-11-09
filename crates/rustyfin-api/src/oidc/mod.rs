//! OpenID Connect sign-in (B21): the pure helpers the routes and the sign-in flow share (spec §5),
//! the provider check and its cache (AC3, AC6), and the settings the admin sees (spec §3).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flow::FlowStore;
use openidconnect::core::{CoreJsonWebKeySet, CoreProviderMetadata};
use rustyfin_model::oidc::{OidcConfig, OidcSettings};
use rustyfin_model::user::User;
pub use rustyfin_model::user::{NameMatch, name_key, name_too_long, names_match};
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::sync::RwLock;

pub mod flow;
pub mod signin;

#[cfg(test)]
mod tests;

/// The refusal for a password sign-in that the switch refuses, whoever the user is (AC21, AC41).
pub const PASSWORD_SIGN_IN_OFF: &str = "Password sign-in is turned off for this account.";

/// The refusal for a password reset that the switch refuses, whoever the user is (AC22, AC41).
pub const PASSWORD_RESET_OFF: &str = "Password reset is turned off for this account.";

/// The path of the provider's redirect, under the public URL.
const CALLBACK_PATH: &str = "/Auth/Oidc/Callback";

/// The path prefix of the flow endpoints, under the public URL's path.
const FLOW_PATH: &str = "/Auth/Oidc";

/// The signing algorithms Rustyfin verifies (AC9). Symmetric algorithms, `none` and ES512 are not
/// among them.
const ACCEPTED_ALGS: [&str; 9] = [
    "RS256", "RS384", "RS512", "PS256", "PS384", "PS512", "ES256", "ES384", "EdDSA",
];

/// The redirect URI for a public URL: the URL without trailing `/`s, then the callback path. Empty
/// without a public URL.
pub fn callback_url(public_url: &str) -> String {
    let base = public_url.trim_end_matches('/');
    if base.is_empty() {
        String::new()
    } else {
        format!("{base}{CALLBACK_PATH}")
    }
}

/// The path the flow cookie is sent on: the public URL's path, then `/Auth/Oidc`. A URL that does
/// not parse gives `/Auth/Oidc`.
pub fn cookie_path(public_url: &str) -> String {
    let prefix = reqwest::Url::parse(public_url)
        .map(|url| url.path().trim_end_matches('/').to_string())
        .unwrap_or_default();
    format!("{prefix}{FLOW_PATH}")
}

/// The scopes sent to the provider. A typed list is used as typed, split on spaces and commas,
/// with repeats dropped and `openid` first. An empty list is the default: `profile` and `email`,
/// plus `groups` when a group is set.
pub fn effective_scopes(configured: &str, group_set: bool) -> Vec<String> {
    let typed: Vec<&str> = configured
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|scope| !scope.is_empty())
        .collect();
    let requested: Vec<&str> = if typed.is_empty() {
        let mut defaults = vec!["profile", "email"];
        if group_set {
            defaults.push("groups");
        }
        defaults
    } else {
        typed
    };
    let mut scopes = vec!["openid".to_string()];
    for scope in requested {
        if !scopes.iter().any(|known| known == scope) {
            scopes.push(scope.to_string());
        }
    }
    scopes
}

/// The provider's signing algorithms Rustyfin accepts, in the provider's order. A provider that
/// lists none gets RS256, the OIDC default.
pub fn accepted_algs(supported: &[String]) -> Vec<String> {
    if supported.is_empty() {
        return vec!["RS256".to_string()];
    }
    supported
        .iter()
        .filter(|alg| ACCEPTED_ALGS.contains(&alg.as_str()))
        .cloned()
        .collect()
}

/// Whether the claim names the group. A string claim is one group and a list holds several; names
/// match exactly. A number, an empty group or a missing claim matches nothing.
pub fn in_group(claims: &Value, claim: &str, group: &str) -> bool {
    if group.is_empty() {
        return false;
    }
    match claims.get(claim) {
        Some(Value::String(name)) => name == group,
        Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(group)),
        _ => false,
    }
}

/// The username in the claim, trimmed. Blank text and non-text claims give None.
pub fn claim_username(claims: &Value, claim: &str) -> Option<String> {
    let name = claims.get(claim)?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The settings as the admin sees them. The secret shows only as set or not, and the two
/// read-only fields are computed here.
pub fn settings_answer(config: &OidcConfig) -> OidcSettings {
    OidcSettings {
        enabled: config.enabled,
        issuer_url: config.issuer_url.clone(),
        client_id: config.client_id.clone(),
        has_client_secret: !config.client_secret.is_empty(),
        scopes: config.scopes.clone(),
        effective_scopes: effective_scopes(&config.scopes, config.group_set()).join(" "),
        button_label: config.button_label.clone(),
        username_claim: config.username_claim.clone(),
        group_claim: config.group_claim.clone(),
        allowed_group: config.allowed_group.clone(),
        admin_group: config.admin_group.clone(),
        public_url: config.public_url.clone(),
        callback_url: callback_url(&config.public_url),
        allow_password_sign_in: config.allow_password_sign_in,
    }
}

/// Whether the password switch lets `user` use a password (AC21, AC22, AC41): always while the
/// switch is on; otherwise only an administrator whose rights were given by hand.
pub async fn password_allowed(
    pool: &SqlitePool,
    config: &OidcConfig,
    user: &User,
) -> Result<bool, sqlx::Error> {
    if config.allow_password_sign_in {
        return Ok(true);
    }
    if !user.is_administrator {
        return Ok(false);
    }
    Ok(!rustyfin_db::repo_user::admin_from_group(pool, user.id).await?)
}

/// How long a fetched discovery document is kept before a sign-in fetches it again.
const PROVIDER_TTL: Duration = Duration::from_secs(60 * 60);

/// The provider's discovery document and keys, once they have passed the checks of AC3.
#[derive(Debug)]
pub struct Provider {
    /// The issuer URL the document was fetched for.
    issuer: String,
    /// The discovery document, with the key set in it.
    pub metadata: CoreProviderMetadata,
    pub jwks_uri: String,
    pub userinfo_endpoint: Option<String>,
    /// The signing algorithms Rustyfin accepts from this provider (AC9).
    pub algs: Vec<String>,
    /// When the discovery document was fetched.
    fetched_at: Instant,
}

/// Fetches the provider at `issuer_url` and checks its discovery document and keys (AC3). The
/// error is a sentence for the admin; it holds no secret.
pub async fn discover(http: &reqwest::Client, issuer_url: &str) -> Result<Provider, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );
    let doc = fetch_json(http, &url, "discovery document").await?;

    if doc.get("issuer").and_then(Value::as_str) != Some(issuer_url) {
        return Err("The provider's issuer is not the issuer URL.".to_string());
    }
    // The flow needs these two endpoints too; the metadata parse below reads them again.
    endpoint(&doc, "authorization_endpoint")?;
    endpoint(&doc, "token_endpoint")?;
    let jwks_uri = endpoint(&doc, "jwks_uri")?;
    let userinfo_endpoint = doc
        .get("userinfo_endpoint")
        .and_then(Value::as_str)
        .map(str::to_string);

    let response_types = strings_of(&doc, "response_types_supported");
    if response_types.is_some_and(|types| !types.iter().any(|t| t == "code")) {
        return Err("The provider does not offer the code response type.".to_string());
    }
    let methods = strings_of(&doc, "code_challenge_methods_supported");
    if methods.is_some_and(|methods| !methods.iter().any(|m| m == "S256")) {
        return Err("The provider does not offer PKCE with S256.".to_string());
    }
    let supported = strings_of(&doc, "id_token_signing_alg_values_supported").unwrap_or_default();
    let algs = accepted_algs(&supported);
    if algs.is_empty() {
        return Err("The provider offers no signing algorithm Rustyfin accepts.".to_string());
    }

    let keys = fetch_keys(http, &jwks_uri).await?;
    let metadata: CoreProviderMetadata = serde_json::from_value(doc)
        .map_err(|_| "The provider's discovery document is not valid.".to_string())?;
    let metadata = metadata.set_jwks(keys);

    Ok(Provider {
        issuer: issuer_url.to_string(),
        metadata,
        jwks_uri,
        userinfo_endpoint,
        algs,
        fetched_at: Instant::now(),
    })
}

/// Fetches the key set at `jwks_uri`. A set with no keys is refused (AC3).
async fn fetch_keys(http: &reqwest::Client, jwks_uri: &str) -> Result<CoreJsonWebKeySet, String> {
    let jwks = fetch_json(http, jwks_uri, "key set").await?;
    let has_keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .is_some_and(|keys| !keys.is_empty());
    if !has_keys {
        return Err("The provider publishes no signing keys.".to_string());
    }
    serde_json::from_value(jwks).map_err(|_| "The provider's key set is not valid.".to_string())
}

/// GETs a JSON document from the provider. Redirects are not followed and the error names only
/// what was fetched.
async fn fetch_json(http: &reqwest::Client, url: &str, what: &str) -> Result<Value, String> {
    let response = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| format!("The provider's {what} could not be fetched."))?;
    response
        .json::<Value>()
        .await
        .map_err(|_| format!("The provider's {what} is not JSON."))
}

/// A required endpoint of the discovery document.
fn endpoint(doc: &Value, key: &str) -> Result<String, String> {
    doc.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("The provider's discovery document has no {key}."))
}

/// The strings of an array field, when the field is an array.
fn strings_of(doc: &Value, key: &str) -> Option<Vec<String>> {
    doc.get(key)?.as_array().map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

/// The HTTP client for the provider and the checked provider. Cloning shares the cache.
#[derive(Clone)]
pub struct OidcState {
    /// Follows no redirects (RFC 9700 §4.11) and gives up after 10 s.
    pub http: reqwest::Client,
    /// The checked provider. Saving the settings empties it.
    provider: Arc<RwLock<Option<Arc<Provider>>>>,
    /// The sign-in flows in progress (see `flow`).
    flows: Arc<Mutex<FlowStore>>,
}

impl OidcState {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("the provider HTTP client builds");
        Self {
            http,
            provider: Arc::new(RwLock::new(None)),
            flows: Arc::new(Mutex::new(FlowStore::default())),
        }
    }

    /// Empties the cache, so the next sign-in fetches the provider again.
    pub async fn forget(&self) {
        *self.provider.write().await = None;
    }

    /// The checked provider for `issuer_url`: the cached one while it is fresh, else fetched now.
    pub async fn provider(&self, issuer_url: &str) -> Result<Arc<Provider>, String> {
        let cached = self.provider.read().await.clone();
        if let Some(found) = cached
            .filter(|found| found.issuer == issuer_url && found.fetched_at.elapsed() < PROVIDER_TTL)
        {
            return Ok(found);
        }
        let fresh = Arc::new(discover(&self.http, issuer_url).await?);
        *self.provider.write().await = Some(fresh.clone());
        Ok(fresh)
    }

    /// Fetches the key set again, for a token that names a key the cache lacks (AC9). The new set
    /// replaces the cached provider only while the cache still holds `stale`.
    pub async fn refresh_keys(&self, stale: &Arc<Provider>) -> Result<Arc<Provider>, String> {
        let keys = fetch_keys(&self.http, &stale.jwks_uri).await?;
        let metadata = stale.metadata.clone().set_jwks(keys);
        let fresh = Arc::new(Provider {
            issuer: stale.issuer.clone(),
            metadata,
            jwks_uri: stale.jwks_uri.clone(),
            userinfo_endpoint: stale.userinfo_endpoint.clone(),
            algs: stale.algs.clone(),
            fetched_at: stale.fetched_at,
        });
        let mut cached = self.provider.write().await;
        if cached.as_ref().is_some_and(|held| Arc::ptr_eq(held, stale)) {
            *cached = Some(fresh.clone());
        }
        Ok(fresh)
    }
}

impl Default for OidcState {
    fn default() -> Self {
        Self::new()
    }
}
