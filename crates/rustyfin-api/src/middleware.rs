use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, Uri};
use axum::middleware::Next;
use axum::response::Response;
use dashmap::DashMap;
use ipnet::{IpNet, Ipv6Net};
use rustyfin_model::auth::Claims;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::error::ApiError;
use crate::state::AppState;

mod credentials;
pub use credentials::{MediaBrowserAuth, parse_media_browser};

/// Reads the credentials of a request and returns the claims they prove, plus the user token
/// (`None` for the configured API key, which has no token to revoke).
///
/// The token is the first non-blank one from these sources, in order: the `Authorization` header
/// (`Bearer <token>`, otherwise a `MediaBrowser` token), the `MediaBrowser` token of the
/// `X-Emby-Authorization` header, the `X-Emby-Token` header, the `X-MediaBrowser-Token` header,
/// then the `ApiKey` and `api_key` query parameters (see `credentials::request_token`). The
/// configured API key gives administrator claims. The `api_key` value and a `MediaBrowser` token
/// also count as sent when they are compared with that key, because *arr's Webhook URLs and its
/// Emby / Jellyfin connection carry the key as typed. Any other token must be a live user token.
/// Takes the request's headers and URI rather than the request: axum's `Body` is not `Sync`, so a
/// `&Request` cannot be held across the await.
async fn request_claims(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<(Claims, Option<String>), ApiError> {
    let presented = credentials::request_token(headers, uri).ok_or(ApiError::Unauthorized)?;

    // An empty token never authenticates, whatever the configured key.
    if presented.token.trim().is_empty() {
        return Err(ApiError::Unauthorized);
    }

    // Check if the token, or the value as sent (`api_key` or a `MediaBrowser` token), matches a
    // configured API key
    if let Some(ref api_key) = state.api_key
        && (presented.token == **api_key || presented.as_sent.as_deref() == Some(api_key.as_str()))
    {
        // API key auth: create admin claims
        return Ok((
            Claims {
                sub: uuid::Uuid::nil(),
                name: "ApiKey".to_string(),
                is_admin: true,
                iat: 0,
                exp: i64::MAX,
            },
            None,
        ));
    }

    // Otherwise the token must be a live user token
    let claims = rustyfin_auth::validate_token(&state.db_ro, &presented.token)
        .await
        .map_err(|_| ApiError::Unauthorized)?;
    Ok((claims, Some(presented.token)))
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let (claims, token) = request_claims(&state, req.headers(), req.uri()).await?;
    req.extensions_mut().insert(claims);
    if let Some(t) = token {
        req.extensions_mut().insert(rustyfin_auth::RequestToken(t));
    }
    Ok(next.run(req).await)
}

/// Guards the setup wizard routes. While the server has no users, anyone may use them. After that, only an
/// administrator may.
pub async fn require_setup_or_admin(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if !rustyfin_db::repo_user::users_exist(&state.db).await? {
        return Ok(next.run(req).await);
    }
    let (claims, _) = request_claims(&state, req.headers(), req.uri()).await?;
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

/// Guards the administrator routes. It runs after `require_auth`, which inserts the claims, so a
/// request without claims gets 401. A logged-in user who is not an administrator gets 403. The
/// configured API key carries administrator claims and passes.
pub async fn require_admin(req: Request, next: Next) -> Result<Response, ApiError> {
    let claims = req
        .extensions()
        .get::<Claims>()
        .ok_or(ApiError::Unauthorized)?;
    if !claims.is_admin {
        return Err(ApiError::Forbidden(
            "Administrator access is required.".to_string(),
        ));
    }
    Ok(next.run(req).await)
}

#[allow(dead_code)]
pub trait ClaimsExt {
    fn claims(&self) -> Option<&Claims>;
}

impl<T> ClaimsExt for axum::extract::Request<T> {
    fn claims(&self) -> Option<&Claims> {
        self.extensions().get::<Claims>()
    }
}

/// True if `ip` is on a local/private network: loopback, RFC1918, link-local (169.254/16),
/// IPv6 loopback/ULA (fc00::/7)/link-local (fe80::/10). An IPv4-mapped IPv6 address is checked as
/// its IPv4 address. Used to gate who may receive a reset PIN.
pub fn is_local_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return true;
            }
            let o = v6.octets();
            (o[0] & 0xfe) == 0xfc || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
        }
    }
}

/// Simple in-memory rate limiter using a sliding window per IP.
/// Uses DashMap for lock-striping to reduce contention under high concurrency.
/// Uses VecDeque for O(1) expired-entry pruning from the front.
#[derive(Clone)]
pub struct RateLimiter {
    /// Map of IP → deque of request timestamps (oldest first)
    state: Arc<DashMap<String, VecDeque<Instant>>>,
    /// Maximum requests allowed within the window
    max_requests: usize,
    /// Time window duration
    window: std::time::Duration,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window_secs: u64) -> Self {
        Self {
            state: Arc::new(DashMap::new()),
            max_requests,
            window: std::time::Duration::from_secs(window_secs),
        }
    }

    /// Periodically clean up expired entries to prevent memory leaks.
    pub fn start_cleanup(&self) {
        let state = self.state.clone();
        let window = self.window;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let now = Instant::now();
                state.retain(|_, timestamps| {
                    while let Some(front) = timestamps.front() {
                        if now.duration_since(*front) >= window {
                            timestamps.pop_front();
                        } else {
                            break;
                        }
                    }
                    !timestamps.is_empty()
                });
            }
        });
    }

    /// Prune expired timestamps from the front of the deque (oldest first, O(1) per pop).
    fn prune(entry: &mut VecDeque<Instant>, now: Instant, window: std::time::Duration) {
        while let Some(front) = entry.front() {
            if now.duration_since(*front) >= window {
                entry.pop_front();
            } else {
                break;
            }
        }
    }

    pub async fn check(&self, key: &str) -> bool {
        // A limit of 0 disables rate limiting
        if self.max_requests == 0 {
            return true;
        }

        let now = Instant::now();
        let mut entry = self.state.entry(key.to_string()).or_default();
        Self::prune(&mut entry, now, self.window);
        if entry.len() >= self.max_requests {
            return false;
        }
        entry.push_back(now);
        true
    }

    /// Whether the key's budget is used up. Prunes its expired timestamps, creates no entry for an
    /// unknown key and records nothing. A budget of 0 never limits.
    pub async fn is_limited(&self, key: &str) -> bool {
        if self.max_requests == 0 {
            return false;
        }

        let now = Instant::now();
        match self.state.get_mut(key) {
            Some(mut entry) => {
                Self::prune(&mut entry, now, self.window);
                entry.len() >= self.max_requests
            }
            None => false,
        }
    }

    /// Counts one request against the key without refusing it. A budget of 0 counts nothing.
    pub async fn record(&self, key: &str) {
        if self.max_requests == 0 {
            return;
        }

        let now = Instant::now();
        let mut entry = self.state.entry(key.to_string()).or_default();
        Self::prune(&mut entry, now, self.window);
        entry.push_back(now);
    }
}

/// Default login-endpoint budget: requests per window, per resolved client IP.
pub const DEFAULT_LOGIN_LIMIT: usize = 10;
/// Default login-endpoint window, in seconds.
pub const DEFAULT_LOGIN_WINDOW_SECS: u64 = 60;
/// Default resource-intensive endpoint budget (transcoding, streaming start): requests per window.
pub const DEFAULT_RESOURCE_LIMIT: usize = 30;
/// Default resource-intensive endpoint window, in seconds.
pub const DEFAULT_RESOURCE_WINDOW_SECS: u64 = 60;

/// Parse a comma-separated `RUSTYFIN_KNOWN_PROXIES` value into networks. Accepts CIDR blocks
/// (`10.0.0.0/8`) and bare host IPs (`192.168.1.1` → a /32 or /128). A bare IPv4-mapped IPv6 host
/// (`::ffff:10.0.0.7`) becomes its IPv4 address. Unparseable entries are dropped.
pub fn parse_known_proxies(csv: &str) -> Vec<IpNet> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            s.parse::<IpNet>().ok().or_else(|| {
                s.parse::<IpAddr>().ok().and_then(|ip| {
                    let ip = ip.to_canonical();
                    let bits = if ip.is_ipv4() { 32 } else { 128 };
                    IpNet::new(ip, bits).ok()
                })
            })
        })
        .collect()
}

/// Exactly 127.0.0.1 or ::1. NOT all of 127.0.0.0/8 — 127.0.0.2 is treated as a normal client.
fn is_exempt_loopback(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    ip == IpAddr::V4(Ipv4Addr::LOCALHOST) || ip == IpAddr::V6(Ipv6Addr::LOCALHOST)
}

/// A peer that may set `X-Forwarded-For`: a canonical address inside a configured known-proxy net.
/// Loopback is a trusted proxy only when it is listed there.
fn is_trusted_proxy(ip: IpAddr, known: &[IpNet]) -> bool {
    let ip = ip.to_canonical();
    known.iter().any(|n| n.contains(&ip))
}

/// The client a request is attributed to, from [`resolve_client`]. `ip` is canonical. An unresolved
/// request carries the connecting peer as `ip`.
#[derive(Debug, Clone, Copy)]
pub struct ClientAddr {
    pub ip: IpAddr,
    /// The connecting peer, canonical.
    peer: IpAddr,
    /// False when the forwarded chain could not be read to a client address, or when a peer that is
    /// not a trusted proxy sent a forwarding header.
    resolved: bool,
}

impl ClientAddr {
    /// On the local network (the reset-PIN gate). An unresolved request is never local.
    pub fn is_local(&self) -> bool {
        self.resolved && is_local_network(self.ip)
    }

    /// Exempt from the login limit: resolved, with the client and the peer both exact loopback.
    pub fn is_exempt(&self) -> bool {
        self.resolved && is_exempt_loopback(self.ip) && is_exempt_loopback(self.peer)
    }

    /// The login-limiter key: the IPv4 address (no port), or the IPv6 /56 network, so one site that
    /// rotates addresses or /64 subnets within its /56 shares one budget.
    pub fn limiter_key(&self) -> String {
        match self.ip {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => {
                Ipv6Net::new(v6, 56).map_or_else(|_| v6.to_string(), |net| net.trunc().to_string())
            }
        }
    }
}

/// Request headers that say the peer forwards for someone else. A peer that is not a trusted proxy
/// and sends any of them is never taken as the client.
const FORWARDING_HEADERS: [&str; 3] = ["x-forwarded-for", "forwarded", "x-real-ip"];

/// True when the request carries any of [`FORWARDING_HEADERS`], whatever its value. Presence alone
/// decides; no value is parsed.
fn has_forwarding_header(headers: &HeaderMap) -> bool {
    FORWARDING_HEADERS
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// Logs once per process when a local-network peer that is not a trusted proxy sends a forwarding
/// header. Header values are never logged.
fn warn_untrusted_forwarder(peer: IpAddr) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if is_local_network(peer) && !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "{peer} sent a forwarding header but is not in RUSTYFIN_KNOWN_PROXIES, so its requests are neither local nor exempt from the login limit and all its clients share one limit; if {peer} is a reverse proxy, add it to RUSTYFIN_KNOWN_PROXIES"
        );
    }
}

/// Parses one `X-Forwarded-For` entry (`ip`, `ip:port`, `[v6]` or `[v6]:port`) to a canonical
/// address. Empty or unreadable entries give `None`.
fn parse_forwarded_entry(entry: &str) -> Option<IpAddr> {
    let entry = entry.trim();
    let ip = entry
        .parse::<IpAddr>()
        .ok()
        .or_else(|| entry.parse::<SocketAddr>().ok().map(|sa| sa.ip()))
        .or_else(|| {
            let inner = entry.strip_prefix('[')?.strip_suffix(']')?;
            inner.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
        })?;
    Some(ip.to_canonical())
}

/// Resolves the client a request is attributed to. The peer is canonicalized first.
///
/// A peer that is not a trusted proxy is the client unless it sends `X-Forwarded-For`, `Forwarded`
/// or `X-Real-IP`; then the request is unresolved and the headers are not read. A trusted peer's
/// `X-Forwarded-For` lines are read last line first, each line right to left. The first entry
/// that is not a trusted proxy is the client, and entries to its left are never read. If an entry
/// cannot be read, or every hop is a trusted proxy, the request is unresolved: it is keyed on the
/// peer and is neither local nor exempt from the login limit.
pub fn resolve_client(peer: IpAddr, headers: &HeaderMap, known: &[IpNet]) -> ClientAddr {
    let peer = peer.to_canonical();
    let unresolved = ClientAddr {
        ip: peer,
        peer,
        resolved: false,
    };
    if !is_trusted_proxy(peer, known) {
        if has_forwarding_header(headers) {
            warn_untrusted_forwarder(peer);
            return unresolved;
        }
        return ClientAddr {
            ip: peer,
            peer,
            resolved: true,
        };
    }
    let lines: Vec<_> = headers.get_all("x-forwarded-for").into_iter().collect();
    for line in lines.iter().rev() {
        let Ok(text) = line.to_str() else {
            return unresolved;
        };
        for entry in text.rsplit(',') {
            let Some(addr) = parse_forwarded_entry(entry) else {
                return unresolved;
            };
            if !is_trusted_proxy(addr, known) {
                return ClientAddr {
                    ip: addr,
                    peer,
                    resolved: true,
                };
            }
        }
    }
    unresolved
}

/// Per-client budget for resource-intensive endpoints (transcoding, streaming start).
/// Configurable via `RUSTYFIN_RESOURCE_LIMIT` and `RUSTYFIN_RESOURCE_WINDOW`.
pub async fn rate_limit_resource(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let client = resolve_client(addr.ip(), req.headers(), &state.known_proxies);

    if !client.is_exempt() && !state.resource_limiter.check(&client.limiter_key()).await {
        tracing::warn!(ip = %client.ip, "resource rate limit exceeded");
        return Err(ApiError::RateLimited);
    }

    Ok(next.run(req).await)
}

/// Per-client budget for the login endpoints only (AuthenticateByName, ForgotPassword,
/// ForgotPassword/Pin, ResetPassword), mounted on those routes in `routes::build_router`. The client
/// comes from `resolve_client`, so X-Forwarded-For is read only from a trusted proxy, and a peer that
/// is not one but sends a forwarding header is never exempt. A request is exempt only when it
/// resolves to exact loopback and the peer is exact loopback. A budget of 0 disables the limit.
pub async fn rate_limit(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let client = resolve_client(addr.ip(), req.headers(), &state.known_proxies);

    // Every client that is not exempt draws on its limiter key: the IPv4 address, or the IPv6 /56.
    if !client.is_exempt() && !state.rate_limiter.check(&client.limiter_key()).await {
        tracing::warn!(ip = %client.ip, "rate limit exceeded");
        return Err(ApiError::RateLimited);
    }

    Ok(next.run(req).await)
}
