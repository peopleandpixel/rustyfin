//! The OIDC settings (spec §3): the admin's read and save, and the public info the sign-in page
//! reads before anyone signs in (AC28).

use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use openidconnect::core::CoreAuthenticationFlow;
use openidconnect::{CsrfToken, Nonce, PkceCodeChallenge, RedirectUrl, Scope};
use rustyfin_model::auth::Claims;
use rustyfin_model::oidc::{AccountStatus, OidcConfig, OidcInfo, OidcSettings, OidcSettingsUpdate};
use serde::Deserialize;
use uuid::Uuid;

use super::auth::{start_session, token_device};
use crate::error::ApiError;
use crate::oidc::flow::{FLOW_LIFETIME, Flow, FlowKind};
use crate::oidc::signin::{self, SignInError};
use crate::oidc::{callback_url, cookie_path, discover, effective_scopes, settings_answer};
use crate::state::AppState;

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/Auth/Oidc/Info", get(info))
}

pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Auth/Oidc/Settings",
            get(read_settings).post(save_settings),
        )
        .route("/Auth/Oidc/Accounts", get(list_accounts))
}

/// The sign-in endpoints (B21). The login router applies the login rate limit to them (routes/mod.rs).
pub fn login_routes() -> Router<AppState> {
    Router::new()
        .route("/Auth/Oidc/Start", post(start))
        .route("/Auth/Oidc/Callback", get(callback))
        .route("/Auth/Oidc/Complete", post(complete))
}

/// The account endpoints of a signed-in user (B21): the link of their own account, and the link
/// status of accounts. The protected router applies the auth layer (routes/mod.rs).
pub fn protected_routes() -> Router<AppState> {
    Router::new()
        .route("/Auth/Oidc/Link", post(link_start))
        .route("/Auth/Oidc/Accounts/{userId}", get(get_account))
        .route("/Auth/Oidc/Accounts/{userId}/Link", delete(unlink))
}

async fn info(State(state): State<AppState>) -> Result<Json<OidcInfo>, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    Ok(Json(OidcInfo {
        enabled: config.enabled,
        button_label: config.button_label,
    }))
}

async fn read_settings(State(state): State<AppState>) -> Result<Json<OidcSettings>, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    Ok(Json(settings_answer(&config)))
}

/// Saves the sent fields over the saved ones. The body is read as a JSON value first and then
/// converted, so a serde message (which can echo a value such as a numeric secret) is replaced by
/// a fixed sentence and never reaches the answer.
async fn save_settings(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<OidcSettings>, ApiError> {
    let update: OidcSettingsUpdate = serde_json::from_value(body).map_err(|_| {
        ApiError::BadRequest("The OIDC settings are not in the expected form.".to_string())
    })?;
    let current = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    let mut next = update.apply(current);
    // Kept without trailing slashes, so the callback and cookie paths join cleanly (AC3).
    next.public_url = next.public_url.trim_end_matches('/').to_string();
    check_fields(&next).map_err(ApiError::BadRequest)?;
    if next.enabled {
        // Always a fresh fetch: the cache is never used to accept a provider.
        discover(&state.oidc.http, &next.issuer_url)
            .await
            .map_err(ApiError::BadRequest)?;
    }
    rustyfin_db::repo_oidc::save_settings(&state.db, &next).await?;
    state.oidc.forget().await;
    Ok(Json(settings_answer(&next)))
}

/// The checks a save must pass when OIDC is on (AC3). Nothing is checked while it is off.
fn check_fields(config: &OidcConfig) -> Result<(), String> {
    if !config.enabled {
        return Ok(());
    }
    check_url("The issuer URL", &config.issuer_url)?;
    check_url("The public URL", &config.public_url)?;
    if config.client_id.is_empty() {
        return Err("The client ID is required.".to_string());
    }
    if config.username_claim.is_empty() {
        return Err("The username claim is required.".to_string());
    }
    if config.group_set() && config.group_claim.is_empty() {
        return Err("The group claim is required when a group is set.".to_string());
    }
    Ok(())
}

/// An absolute http or https URL with no query and no fragment.
fn check_url(label: &str, value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
        .ok_or_else(|| format!("{label} must be an absolute http or https URL."))?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{label} must not have a query or a fragment."));
    }
    Ok(())
}

/// The flow cookie: the browser's id for the sign-in in progress (AC6).
const FLOW_COOKIE: &str = "rustyfin_oidc";

/// What the provider sends the browser back with. Every field is optional: a missing one is a
/// refusal, not a rejected request.
#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `POST /Auth/Oidc/Start` – starts a sign-in: the provider's authorization URL, and the flow cookie.
async fn start(State(state): State<AppState>) -> Result<Response, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    ensure_on(&config)?;
    begin(&state, config, FlowKind::SignIn).await
}

/// `POST /Auth/Oidc/Link` – starts the link of the caller's account to the provider (AC29). The
/// answer is Start's; the flow it starts links the caller when the browser comes back.
async fn link_start(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Response, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    ensure_on(&config)?;
    if rustyfin_db::repo_oidc::user_has_link(&state.db, claims.sub).await? {
        return Err(ApiError::Conflict(
            "This account is already linked to the provider.".to_string(),
        ));
    }
    begin(&state, config, FlowKind::Link(claims.sub)).await
}

/// Refuses while OIDC is off.
fn ensure_on(config: &OidcConfig) -> Result<(), ApiError> {
    if config.enabled {
        return Ok(());
    }
    Err(ApiError::BadRequest(
        "OIDC sign-in is turned off.".to_string(),
    ))
}

/// Starts a flow of `kind`: the provider's authorization URL, and the flow cookie.
async fn begin(state: &AppState, config: OidcConfig, kind: FlowKind) -> Result<Response, ApiError> {
    let provider = state
        .oidc
        .provider(&config.issuer_url)
        .await
        .map_err(ApiError::BadGateway)?;
    let redirect = RedirectUrl::new(callback_url(&config.public_url)).map_err(|_| {
        ApiError::BadRequest("The public URL is not a valid redirect URI.".to_string())
    })?;
    let client = signin::client_for(&provider, &config).set_redirect_uri(redirect);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(challenge);
    // The client adds `openid` itself, so the configured list leaves it out.
    for scope in effective_scopes(&config.scopes, config.group_set())
        .into_iter()
        .filter(|scope| scope != "openid")
    {
        request = request.add_scope(Scope::new(scope));
    }
    let (url, csrf, nonce) = request.url();
    let flow = Flow {
        state: csrf.secret().clone(),
        nonce: nonce.secret().clone(),
        verifier: verifier.secret().clone(),
        created: Instant::now(),
        signed_in: None,
        kind,
    };
    let id = state.oidc.insert_flow(flow);
    Ok((
        [(header::SET_COOKIE, flow_cookie(&id, &config))],
        Json(serde_json::json!({ "AuthorizationUrl": url.as_str() })),
    )
        .into_response())
}

/// `GET /Auth/Oidc/Callback` – the browser comes back from the provider. Always a redirect: to the
/// page the flow is for, signed in or with an error code (AC9-AC11).
async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    // Until the flow is known, a refusal can only go to the sign-in page.
    let Some(flow_id) = flow_cookie_value(&headers) else {
        return Ok(refused(&config, "login", "state_mismatch"));
    };
    // Taking the flow ends it: a refusal leaves none behind, and a good sign-in puts it back.
    let Some(flow) = state.oidc.take_flow(&flow_id) else {
        return Ok(refused(&config, "login", "expired"));
    };
    let page = flow.kind.page();
    if flow.signed_in.is_some() {
        return Ok(refused(&config, page, "expired"));
    }
    if query.state.as_deref() != Some(flow.state.as_str()) {
        return Ok(refused(&config, page, "state_mismatch"));
    }
    if !config.enabled {
        return Ok(refused(&config, page, "disabled"));
    }
    if let Some(error) = query.error.as_deref() {
        let reason = if error == "access_denied" {
            "cancelled"
        } else {
            "provider_error"
        };
        return Ok(refused(&config, page, reason));
    }
    let Some(code) = query.code.as_deref() else {
        return Ok(refused(&config, page, "provider_error"));
    };
    match signin::sign_in(&state.oidc, &state.db, &config, &flow, code).await {
        Ok(user_id) => match flow.kind {
            FlowKind::SignIn => {
                state.oidc.put_flow(
                    flow_id,
                    Flow {
                        signed_in: Some(user_id),
                        ..flow
                    },
                );
                Ok(redirect(
                    format!("{}/web/#login/oidc", config.public_url),
                    None,
                ))
            }
            // A link hands out no session: the account is linked, and the flow is over.
            FlowKind::Link(_) => Ok(redirect(
                format!("{}/web/#settings/oidc-linked", config.public_url),
                Some(clear_flow_cookie(&config)),
            )),
        },
        Err(SignInError::Refused(reason)) => Ok(refused(&config, page, reason)),
        Err(SignInError::Server) => Ok(refused(&config, page, "server_error")),
    }
}

/// `POST /Auth/Oidc/Complete` – hands out the session of the flow the callback signed in (AC12).
/// Once: the flow is gone after it, and the flow cookie is cleared.
async fn complete(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user_id = flow_cookie_value(&headers)
        .and_then(|id| state.oidc.finish_flow(&id))
        .ok_or(ApiError::Unauthorized)?;
    if rustyfin_db::repo_user::is_disabled(&state.db, user_id).await? {
        return Err(ApiError::Unauthorized);
    }
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    let device = token_device(&headers);
    let (user, token) = rustyfin_auth::issue_login_token(&state.db, user_id, &device).await?;
    let answer = start_session(&state, &device, user, token).await;
    Ok((
        [(header::SET_COOKIE, clear_flow_cookie(&config))],
        Json(answer),
    )
        .into_response())
}

/// `GET /Auth/Oidc/Accounts` – every account's link status (AC30). Only admins reach it: the admin
/// router applies the admin layer.
async fn list_accounts(
    State(state): State<AppState>,
) -> Result<Json<Vec<AccountStatus>>, ApiError> {
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    let accounts =
        rustyfin_db::repo_oidc::account_statuses(&state.db, config.allow_password_sign_in, None)
            .await?;
    Ok(Json(accounts))
}

/// `GET /Auth/Oidc/Accounts/{userId}` – one account's link status (AC30). Its owner and admins may
/// read it. Anyone else gets a 403, even for an account that does not exist.
async fn get_account(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<AccountStatus>, ApiError> {
    if user_id != claims.sub && !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Only the account's owner or an administrator may see its link.".to_string(),
        ));
    }
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    let account = rustyfin_db::repo_oidc::account_statuses(
        &state.db,
        config.allow_password_sign_in,
        Some(user_id),
    )
    .await?
    .pop()
    .ok_or_else(|| ApiError::NotFound("No such account.".to_string()))?;
    Ok(Json(account))
}

/// `DELETE /Auth/Oidc/Accounts/{userId}/Link` – unlinks an account (AC31). Another user's link is the
/// admin's to remove. An owner removes their own link only when they can still sign in.
async fn unlink(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let own = user_id == claims.sub;
    if !own && !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Only an administrator may remove another account's link.".to_string(),
        ));
    }
    let config = rustyfin_db::repo_oidc::get_settings(&state.db).await?;
    let account = rustyfin_db::repo_oidc::account_statuses(
        &state.db,
        config.allow_password_sign_in,
        Some(user_id),
    )
    .await?
    .pop()
    .filter(|account| account.linked)
    .ok_or_else(|| ApiError::NotFound("This account has no link to remove.".to_string()))?;
    if own && !account.can_unlink {
        let reason = if account.has_password {
            "Password sign-in is turned off, so unlinking would leave this account no way in."
        } else {
            "This account has no password, so unlinking would leave it no way in."
        };
        return Err(ApiError::Conflict(reason.to_string()));
    }
    let unlinked = rustyfin_db::repo_oidc::unlink_account(&state.db, user_id).await?;
    if unlinked.admin_kept {
        tracing::warn!(
            user_id = %user_id,
            "admin rights kept after unlinking: the admin group gave them, and this user is the owner or the last enabled administrator"
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The flow cookie's value in the request, if the browser sent one.
fn flow_cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == FLOW_COOKIE)
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

/// The flow cookie for `id`. It lasts as long as the flow (AC6).
fn flow_cookie(id: &str, config: &OidcConfig) -> String {
    format!(
        "{FLOW_COOKIE}={id}; Path={}; Max-Age={}; HttpOnly; SameSite=Lax{}",
        cookie_path(&config.public_url),
        FLOW_LIFETIME.as_secs(),
        secure_attr(config),
    )
}

/// A `Set-Cookie` that removes the flow cookie, on the path it was set with.
fn clear_flow_cookie(config: &OidcConfig) -> String {
    format!(
        "{FLOW_COOKIE}=; Path={}; Max-Age=0; HttpOnly; SameSite=Lax{}",
        cookie_path(&config.public_url),
        secure_attr(config),
    )
}

/// `; Secure` when the public URL is https.
fn secure_attr(config: &OidcConfig) -> &'static str {
    if config.public_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    }
}

/// The redirect that ends a flow with `reason` on `page` (`login` or `settings`), and clears the flow
/// cookie (AC9).
fn refused(config: &OidcConfig, page: &str, reason: &str) -> Response {
    let location = format!("{}/web/#{page}/oidc-error/{reason}", config.public_url);
    redirect(location, Some(clear_flow_cookie(config)))
}

/// A 303 to `location`, with the cookie change when there is one.
fn redirect(location: String, set_cookie: Option<String>) -> Response {
    match set_cookie {
        Some(cookie) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, location), (header::SET_COOKIE, cookie)],
        )
            .into_response(),
        None => (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response(),
    }
}
