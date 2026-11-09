//! The sign-in itself (B21): the code exchange, the ID token checks, the claims and the account
//! rules. A refusal is a code and nothing else (AC5): no secret, token, state, nonce, verifier or
//! flow id, and no error text from openidconnect or oauth2, reaches a log or an answer.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use openidconnect::core::{CoreClient, CoreClientAuthMethod, CoreJwsSigningAlgorithm};
use openidconnect::{
    AuthType, AuthorizationCode, ClaimsVerificationError, ClientId, ClientSecret, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, Nonce, OAuth2TokenResponse, PkceCodeVerifier, RedirectUrl,
    RequestTokenError, SignatureVerificationError, TokenResponse,
};
use rustyfin_db::{repo_oidc, repo_user};
use rustyfin_model::oidc::OidcConfig;
use rustyfin_model::user::User;
use serde_json::{Map, Value};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::flow::{Flow, FlowKind};
use super::{
    NameMatch, OidcState, Provider, callback_url, claim_username, in_group, name_too_long,
    password_allowed,
};

/// How a sign-in ended without a user: the code the browser is sent, or a database failure.
#[derive(Debug)]
pub(crate) enum SignInError {
    /// Refused for this reason. The code names no secret.
    Refused(&'static str),
    /// Rustyfin could not use its database.
    Server,
}

impl From<sqlx::Error> for SignInError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(error = %error, "OIDC sign-in database error");
        Self::Server
    }
}

/// The OpenID Connect client for the provider and the configured client. With no secret set, the
/// client authenticates by its id alone. The authorization endpoint is set, and the token and
/// userinfo endpoints may be: the typestate `from_provider_metadata` returns.
pub(crate) fn client_for(
    provider: &Provider,
    config: &OidcConfig,
) -> CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
> {
    let secret =
        (!config.client_secret.is_empty()).then(|| ClientSecret::new(config.client_secret.clone()));
    CoreClient::from_provider_metadata(
        provider.metadata.clone(),
        ClientId::new(config.client_id.clone()),
        secret,
    )
    .set_auth_type(auth_type_of(provider))
}

/// The client authentication for the token endpoint: HTTP Basic, the default (RFC 6749 section
/// 2.3.1), unless the provider offers only `client_secret_post`.
fn auth_type_of(provider: &Provider) -> AuthType {
    let methods = provider.metadata.token_endpoint_auth_methods_supported();
    let offers = |method: CoreClientAuthMethod| methods.is_some_and(|list| list.contains(&method));
    if offers(CoreClientAuthMethod::ClientSecretPost)
        && !offers(CoreClientAuthMethod::ClientSecretBasic)
    {
        AuthType::RequestBody
    } else {
        AuthType::BasicAuth
    }
}

/// Signs in the user the code is for, or links the caller's account to the identity when the flow
/// is a link. `flow` is the flow the callback matched, and `code` is the code from the provider's
/// redirect. Returns the id of the account the sign-in or the link settled on.
pub(crate) async fn sign_in(
    oidc: &OidcState,
    pool: &SqlitePool,
    config: &OidcConfig,
    flow: &Flow,
    code: &str,
) -> Result<Uuid, SignInError> {
    let mut provider = oidc
        .provider(&config.issuer_url)
        .await
        .map_err(|_| SignInError::Refused("provider_error"))?;
    let redirect = RedirectUrl::new(callback_url(&config.public_url))
        .map_err(|_| SignInError::Refused("provider_error"))?;
    let client = client_for(&provider, config).set_redirect_uri(redirect);
    let exchanged = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|_| SignInError::Refused("provider_error"))?
        .set_pkce_verifier(PkceCodeVerifier::new(flow.verifier.clone()))
        .request_async(&oidc.http)
        .await;
    let token = match exchanged {
        Ok(token) => token,
        // An answer whose ID token does not parse is a bad ID token, unless the bad claim is the
        // username (see `unparsed_refusal`). Any other failure is the provider's: an outage, a
        // refused code, or an answer that is not a token response.
        Err(RequestTokenError::Parse(_, body)) if has_id_token(&body) => {
            return Err(SignInError::Refused(unparsed_refusal(
                &body,
                &config.username_claim,
            )));
        }
        Err(_) => return Err(SignInError::Refused("provider_error")),
    };
    let id_token = token
        .id_token()
        .ok_or(SignInError::Refused("invalid_token"))?;
    let access_token = token.access_token().secret().clone();

    let algs: Vec<CoreJwsSigningAlgorithm> = provider
        .algs
        .iter()
        .filter_map(|alg| serde_json::from_value(Value::String(alg.clone())).ok())
        .collect();
    let nonce = Nonce::new(flow.nonce.clone());
    let mut retried = false;
    loop {
        let client = client_for(&provider, config);
        let verifier = client.id_token_verifier().set_allowed_algs(algs.clone());
        match id_token.claims(&verifier, &nonce) {
            Ok(_) => break,
            Err(ClaimsVerificationError::SignatureVerification(
                SignatureVerificationError::NoMatchingKey,
            )) if !retried => {
                // A key the cache lacks: the provider may have rotated its keys. One refetch.
                retried = true;
                provider = oidc
                    .refresh_keys(&provider)
                    .await
                    .map_err(|_| SignInError::Refused("provider_error"))?;
            }
            Err(_) => return Err(SignInError::Refused("invalid_token")),
        }
    }

    let payload = payload_of(&id_token.to_string()).ok_or(SignInError::Refused("invalid_token"))?;
    let azp_ok = payload
        .get("azp")
        .is_none_or(|azp| azp.as_str() == Some(config.client_id.as_str()));
    if !azp_ok {
        return Err(SignInError::Refused("invalid_token"));
    }
    let subject = payload
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .ok_or(SignInError::Refused("invalid_token"))?
        .to_string();

    let linked = repo_oidc::find_link(pool, &config.issuer_url, &subject).await?;
    let mut claims = payload;
    let group_missing = config.group_set() && claims.get(config.group_claim.as_str()).is_none();
    let name_missing =
        linked.is_none() && claim_username(&claims, &config.username_claim).is_none();
    let wanted = if group_missing || name_missing {
        userinfo(oidc, &provider, &access_token, &subject).await?
    } else {
        None
    };
    if let Some(info) = wanted {
        fill_missing(&mut claims, info);
    }

    if !config.allowed_group.is_empty()
        && !in_group(&claims, &config.group_claim, &config.allowed_group)
    {
        return Err(SignInError::Refused("not_allowed"));
    }

    let user_id = match flow.kind {
        FlowKind::SignIn => sign_in_account(pool, config, linked, &subject, &claims).await?,
        FlowKind::Link(caller) => {
            link_profile(pool, config, caller, linked, &subject, &claims).await?
        }
    };
    // Only a sign-in through an existing link records a sign-in (AC55).
    let username = claim_username(&claims, &config.username_claim);
    let signed_in = if matches!(flow.kind, FlowKind::SignIn) && linked.is_some() {
        Some((Utc::now(), username.as_deref()))
    } else {
        None
    };
    let settled = repo_oidc::settle_sign_in(
        pool,
        user_id,
        &config.issuer_url,
        &subject,
        signed_in,
        in_group(&claims, &config.group_claim, &config.admin_group),
    )
    .await?;
    match settled {
        repo_oidc::Settled::Done => {}
        repo_oidc::Settled::AdminKept => tracing::warn!(
            user_id = %user_id,
            "admin rights kept: the admin group no longer grants them, and this user is the owner or the last enabled administrator"
        ),
        repo_oidc::Settled::LinkGone => return Err(SignInError::Refused("server_error")),
    }
    Ok(user_id)
}

/// The account a sign-in is for (AC44): the user the identity is linked to; else, when no user has
/// the name (it matches in any case and compatibility form, `NameMatch`), a new user. A claim over
/// the limit is refused (AC64). A taken name is refused (`name_taken`, or `name_taken_ask_admin`
/// when no matching account can sign in with a password), and the person links from the profile.
async fn sign_in_account(
    pool: &SqlitePool,
    config: &OidcConfig,
    linked: Option<Uuid>,
    subject: &str,
    claims: &Value,
) -> Result<Uuid, SignInError> {
    if let Some(user_id) = linked {
        if repo_user::is_disabled(pool, user_id).await? {
            return Err(SignInError::Refused("account_disabled"));
        }
        return Ok(user_id);
    }

    let username = claim_username(claims, &config.username_claim)
        .ok_or(SignInError::Refused("no_username"))?;
    if name_too_long(&username) {
        return Err(SignInError::Refused("no_username"));
    }
    let now = Utc::now();
    let matcher = NameMatch::new(&username);
    let same_name = users_named(pool, &matcher).await?;
    if same_name.is_empty() {
        let user = User {
            id: Uuid::new_v4(),
            name: username.clone(),
            password_hash: String::new(),
            is_administrator: false,
            created_at: now,
            last_login_date: None,
            configuration: Value::Object(Map::new()),
        };
        let created = repo_oidc::create_linked_user(
            pool,
            &user,
            &config.issuer_url,
            subject,
            &username,
            now,
            |name: &str| matcher.matches(name),
        )
        .await?;
        if created {
            return Ok(user.id);
        }
        // The name was taken between the first look and the insert (AC52): nothing was written.
        let matching = users_named(pool, &matcher).await?;
        return refuse_taken(pool, config, &matching).await;
    }
    // A taken name is refused whole (AC44): nothing is created, linked or granted here.
    refuse_taken(pool, config, &same_name).await
}

/// The users whose name matches the entered name (`NameMatch`), disabled ones included.
async fn users_named(pool: &SqlitePool, matcher: &NameMatch<'_>) -> Result<Vec<User>, sqlx::Error> {
    Ok(repo_user::get_all_users(pool)
        .await?
        .into_iter()
        .filter(|user| matcher.matches(&user.name))
        .collect())
}

/// Refuses a taken name: `name_taken` when one of the matching accounts is enabled, has a password
/// and may use it; otherwise `name_taken_ask_admin`, also for an empty list. Writes nothing.
async fn refuse_taken(
    pool: &SqlitePool,
    config: &OidcConfig,
    same_name: &[User],
) -> Result<Uuid, SignInError> {
    for user in same_name {
        if !repo_user::is_disabled(pool, user.id).await?
            && !user.password_hash.is_empty()
            && password_allowed(pool, config, user).await?
        {
            return Err(SignInError::Refused("name_taken"));
        }
    }
    Err(SignInError::Refused("name_taken_ask_admin"))
}

/// A profile link (AC29, AC8, AC12): the identity goes to the caller's own account. There is no name
/// matching and no new user. The identity must be linked to nobody, and the caller must have no link.
async fn link_profile(
    pool: &SqlitePool,
    config: &OidcConfig,
    caller: Uuid,
    linked: Option<Uuid>,
    subject: &str,
    claims: &Value,
) -> Result<Uuid, SignInError> {
    if linked.is_some() || repo_oidc::user_has_link(pool, caller).await? {
        return Err(SignInError::Refused("already_linked"));
    }
    if repo_user::is_disabled(pool, caller).await? {
        return Err(SignInError::Refused("account_disabled"));
    }
    let username = claim_username(claims, &config.username_claim)
        .ok_or(SignInError::Refused("no_username"))?;
    repo_oidc::create_link(
        pool,
        caller,
        &config.issuer_url,
        subject,
        &username,
        Utc::now(),
        None,
    )
    .await?;
    Ok(caller)
}

/// Whether the body of a failed token request holds an ID token: the provider answered, but the
/// ID token in the answer did not parse.
fn has_id_token(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).is_ok_and(|doc| doc.get("id_token").is_some())
}

/// The refusal for an ID token that did not parse. openidconnect reads the claims while it parses
/// the token, so a username claim that is not a string stops the parse too: that is no username.
/// The payload is read here unverified, and only to pick the refusal code.
fn unparsed_refusal(body: &[u8], username_claim: &str) -> &'static str {
    let payload = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|doc| doc.get("id_token")?.as_str().and_then(payload_of));
    match payload
        .as_ref()
        .and_then(|claims| claims.get(username_claim))
    {
        Some(username) if !username.is_string() => "no_username",
        _ => "invalid_token",
    }
}

/// The payload of a compact JWT, read as JSON. The caller has verified the token first.
fn payload_of(jwt: &str) -> Option<Value> {
    let part = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(part).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The claims the userinfo endpoint returns for the access token, or None when the provider has no
/// such endpoint. Its `sub` must be the ID token's.
async fn userinfo(
    oidc: &OidcState,
    provider: &Provider,
    access_token: &str,
    subject: &str,
) -> Result<Option<Map<String, Value>>, SignInError> {
    let Some(url) = provider.userinfo_endpoint.as_deref() else {
        return Ok(None);
    };
    let response = oidc
        .http
        .get(url)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| SignInError::Refused("provider_error"))?;
    if !response.status().is_success() {
        return Err(SignInError::Refused("provider_error"));
    }
    let info: Map<String, Value> = response
        .json()
        .await
        .map_err(|_| SignInError::Refused("provider_error"))?;
    if info.get("sub").and_then(Value::as_str) != Some(subject) {
        return Err(SignInError::Refused("invalid_token"));
    }
    Ok(Some(info))
}

/// Adds the userinfo claims the ID token lacks. The ID token's own claims win.
fn fill_missing(claims: &mut Value, info: Map<String, Value>) {
    if let Value::Object(mine) = claims {
        for (key, value) in info {
            mine.entry(key).or_insert(value);
        }
    }
}
