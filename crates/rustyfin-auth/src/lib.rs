use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use rand_core::{OsRng, RngCore};
use rustyfin_db::{repo_session, repo_setup_code, repo_token, repo_user};
use rustyfin_model::auth::Claims;
use rustyfin_model::user::{User, name_key, name_too_long};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("user not found")]
    UserNotFound,
    #[error("invalid token")]
    InvalidToken,
    #[error("database error: {0}")]
    DbError(#[from] sqlx::Error),
    #[error("password hash error")]
    HashError,
}

// ── Password and PIN hashing ──

fn argon2_hash(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::HashError)?;
    Ok(hash.to_string())
}

fn argon2_verify(password: &str, hash: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash).map_err(|_| AuthError::HashError)?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Hashes a password. Argon2 is slow, so it runs on the blocking pool (M-15).
pub async fn hash_password(password: &str) -> Result<String, AuthError> {
    let p = password.to_owned();
    tokio::task::spawn_blocking(move || argon2_hash(&p))
        .await
        .map_err(|_| AuthError::HashError)?
}

/// Verifies a password against its hash. Runs on the blocking pool (M-15).
pub async fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
    let p = password.to_owned();
    let h = hash.to_owned();
    tokio::task::spawn_blocking(move || argon2_verify(&p, &h))
        .await
        .map_err(|_| AuthError::HashError)?
}

/// Hash a PIN for storage. Runs on the blocking pool to avoid blocking the async runtime.
pub async fn hash_pin(pin: &str) -> Result<String, AuthError> {
    let p = pin.to_owned();
    tokio::task::spawn_blocking(move || argon2_hash(&p))
        .await
        .map_err(|_| AuthError::HashError)?
}

/// Verify a PIN against its hash. Runs on the blocking pool to avoid blocking the async runtime.
pub async fn verify_pin(pin: &str, hash: &str) -> Result<bool, AuthError> {
    let p = pin.to_owned();
    let h = hash.to_owned();
    tokio::task::spawn_blocking(move || argon2_verify(&p, &h))
        .await
        .map_err(|_| AuthError::HashError)?
}

// ── Password-reset PINs (R1-01, M-17) ──

/// A reset PIN: 4 OsRng bytes as dash-grouped uppercase hex, e.g. `A1-B2-C3-D4`.
fn generate_reset_pin() -> String {
    let mut b = [0u8; 4];
    OsRng.fill_bytes(&mut b);
    format!("{:02X}-{:02X}-{:02X}-{:02X}", b[0], b[1], b[2], b[3])
}

/// Normalize a reset PIN for comparison and for use as the new password: trimmed and uppercased.
pub fn normalize_reset_pin(pin: &str) -> String {
    pin.trim().to_uppercase()
}

/// SHA-256 hex of the normalized PIN. Only this is stored, so a database leak exposes no live PIN.
fn reset_pin_hash(pin: &str) -> String {
    Sha256::digest(normalize_reset_pin(pin).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// File name that holds the plaintext reset PIN for `username`,
/// `passwordreset-<hex sha256(t)>.json` (AC65). `t` is `key:` and the name key for a name within
/// the limit, so names that match share one file. A longer name gives `name:` and its exact
/// spelling, so it matches only itself. The name is hashed so the file name leaks no username.
/// ForgotPassword names the file from the name it was asked with; the redeem removes the file
/// named from the stored name.
pub fn reset_pin_filename(username: &str) -> String {
    let t = if name_too_long(username) {
        format!("name:{username}")
    } else {
        format!("key:{}", name_key(username))
    };
    pin_file_named(&t)
}

/// The file name older servers used: `passwordreset-<hex sha256(UPPERCASE name)>.json`. Nothing
/// writes it now; the redeem also removes it, so a reset asked before the upgrade leaves no file.
pub fn legacy_reset_pin_filename(username: &str) -> String {
    pin_file_named(&username.to_uppercase())
}

/// `passwordreset-<hex sha256(t)>.json`, the PIN file name for the text `t`.
fn pin_file_named(t: &str) -> String {
    let h: String = Sha256::digest(t.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("passwordreset-{h}.json")
}

/// Generate a reset PIN for `user_id`, store only its hash (with the given RFC3339 `expires_at`).
/// Returns `Some(pin)`, the plaintext PIN once, so the caller can write it to the on-disk reset
/// file, when the user had no pending or voided reset. Returns `None` when one exists: that reset
/// stays in force and the generated PIN is discarded. NEVER log the return value.
pub async fn create_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    expires_at: &str,
) -> Result<Option<String>, AuthError> {
    let pin = generate_reset_pin();
    let stored =
        repo_user::store_reset_pin(pool, user_id, &reset_pin_hash(&pin), expires_at).await?;
    Ok(stored.then_some(pin))
}

/// Validate a reset PIN for one named user (`/Users/ResetPassword`). Returns the user id on success.
pub async fn validate_reset_pin(
    pool: &SqlitePool,
    user_id: Uuid,
    pin: &str,
) -> Result<Option<Uuid>, AuthError> {
    Ok(repo_user::validate_password_reset_pin(pool, user_id, &reset_pin_hash(pin)).await?)
}

/// Redeem a reset PIN given only the PIN (`/Users/ForgotPassword/Pin`). Returns the user id on success.
pub async fn redeem_reset_pin(pool: &SqlitePool, pin: &str) -> Result<Option<Uuid>, AuthError> {
    Ok(repo_user::redeem_reset_pin(pool, &reset_pin_hash(pin)).await?)
}

// ── One-time setup codes (B20) ──

/// Crockford's base32 alphabet: the characters of a setup code (no `I`, `L`, `O` or `U`).
const SETUP_CODE_ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A new one-time code: 12 characters of Crockford's base32 from `OsRng` (60 bits), shown as
/// `XXXX-XXXX-XXXX`.
pub fn generate_setup_code() -> String {
    let mut bytes = [0u8; 12];
    OsRng.fill_bytes(&mut bytes);
    let alphabet = SETUP_CODE_ALPHABET.as_bytes();
    // 32 characters and `b & 31`: each byte maps to one character with no bias.
    bytes
        .chunks(4)
        .map(|group| {
            group
                .iter()
                .map(|b| char::from(alphabet[usize::from(b & 31)]))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// A code's canonical form: dashes and whitespace dropped, uppercased, `I` and `L` read as `1`,
/// `O` as `0`. `None` unless exactly 12 Crockford characters remain.
pub fn normalize_setup_code(code: &str) -> Option<String> {
    // `to_ascii_uppercase`, not `to_uppercase`: a non-ASCII letter must not fold into `I` or `O`.
    let canonical: String = code
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect();
    let valid = canonical.len() == 12 && canonical.chars().all(|c| SETUP_CODE_ALPHABET.contains(c));
    valid.then_some(canonical)
}

/// An argon2 hash of the code's canonical form, on the blocking pool. `HashError` for a non-code.
pub async fn hash_setup_code(code: &str) -> Result<String, AuthError> {
    let canonical = normalize_setup_code(code).ok_or(AuthError::HashError)?;
    hash_password(&canonical).await
}

/// Whether `code` is the code hashed in `hash`, compared in canonical form. `false` for a
/// non-code.
pub async fn verify_setup_code(code: &str, hash: &str) -> Result<bool, AuthError> {
    match normalize_setup_code(code) {
        Some(canonical) => verify_password(&canonical, hash).await,
        None => Ok(false),
    }
}

/// A password hash no sign-in can match: argon2 of 32 `OsRng` bytes as hex, a secret then dropped.
pub async fn unusable_password_hash() -> Result<String, AuthError> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let secret: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    hash_password(&secret).await
}

// ── Opaque, server-side access tokens ──

/// A bearer token: 16 random bytes as 32 lowercase hex characters.
fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hex SHA-256 of a token. Only this hash is stored, so a database leak exposes no live token.
fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Device details recorded with a token. A non-empty `device_id` lets a new login on the same
/// device replace the old token, and lets logout remove that device's sessions.
#[derive(Debug, Clone, Default)]
pub struct TokenDevice {
    pub device_id: String,
    pub device_name: String,
    pub app_name: String,
    pub app_version: String,
}

/// The bearer token of the current request, inserted as a request extension so that handlers
/// can revoke it or keep it.
#[derive(Debug, Clone)]
pub struct RequestToken(pub String);

/// Issues a token for `user` and returns the plaintext once. Only its hash is stored.
/// A disabled or deleted user gets no token, with the answer of a wrong password.
pub async fn create_token(
    pool: &SqlitePool,
    user: &User,
    device: &TokenDevice,
) -> Result<String, AuthError> {
    let token = generate_token();
    let stored = repo_token::insert_token(
        pool,
        &token_hash(&token),
        user.id,
        &device.device_id,
        &device.device_name,
        &device.app_name,
        &device.app_version,
    )
    .await?;
    if !stored {
        return Err(AuthError::InvalidCredentials);
    }
    Ok(token)
}

/// Resolves a bearer token to its claims. Unknown tokens, and tokens of disabled or deleted
/// users, are rejected. Tokens do not expire, so `iat` is not tracked.
pub async fn validate_token(pool: &SqlitePool, token: &str) -> Result<Claims, AuthError> {
    let (uid, _) = repo_token::get_token(pool, &token_hash(token))
        .await?
        .ok_or(AuthError::InvalidToken)?;
    let user = repo_user::get_user_by_id(pool, uid)
        .await?
        .ok_or(AuthError::InvalidToken)?;
    if repo_user::is_disabled(pool, uid).await? {
        return Err(AuthError::InvalidToken);
    }
    Ok(Claims {
        sub: user.id,
        name: user.name,
        is_admin: user.is_administrator,
        iat: 0,
        exp: i64::MAX,
    })
}

/// The device id stored with a token. `None` for an unknown token or an empty device id.
pub async fn token_device_id(pool: &SqlitePool, token: &str) -> Result<Option<String>, AuthError> {
    let device = repo_token::get_token(pool, &token_hash(token))
        .await?
        .map(|(_, device_id)| device_id)
        .filter(|device_id| !device_id.is_empty());
    Ok(device)
}

/// Revokes a single token.
pub async fn revoke_token(pool: &SqlitePool, token: &str) -> Result<(), AuthError> {
    repo_token::delete_token(pool, &token_hash(token)).await?;
    Ok(())
}

/// Revokes a user's tokens, except `keep` when given. Returns how many were revoked.
pub async fn revoke_user_tokens(
    pool: &SqlitePool,
    user_id: Uuid,
    keep: Option<&str>,
) -> Result<u64, AuthError> {
    let keep_hash = keep.map(token_hash);
    Ok(repo_token::delete_user_tokens(pool, user_id, keep_hash.as_deref()).await?)
}

/// Logs out a token: revokes it and removes the sessions of its device.
pub async fn logout(pool: &SqlitePool, token: &str) -> Result<(), AuthError> {
    let hash = token_hash(token);
    if let Some((uid, device_id)) = repo_token::get_token(pool, &hash).await? {
        repo_token::delete_token(pool, &hash).await?;
        if !device_id.is_empty() {
            repo_session::delete_sessions_by_user_device(pool, uid, &device_id).await?;
        }
    }
    Ok(())
}

/// Checks a login and, if it succeeds, issues a token for `device`.
pub async fn authenticate(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    let user = check_password(pool, username, password).await?;
    issue_login_token(pool, user.id, device).await
}

/// Checks a user's name and password and returns the user. The name is found exactly, else by the
/// one name that matches it caselessly (`repo_user::find_user_for_sign_in`, AC57). A user without a
/// password (an OIDC sign-in makes one, AC23) gets the answer of a wrong password, not a hash error.
pub async fn check_password(
    pool: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<User, AuthError> {
    let user = repo_user::find_user_for_sign_in(pool, username)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    if user.password_hash.is_empty() || !verify_password(password, &user.password_hash).await? {
        return Err(AuthError::InvalidCredentials);
    }

    // A disabled account gets the same answer as a wrong password, and its last login is not updated.
    if repo_user::is_disabled(pool, user.id).await? {
        return Err(AuthError::InvalidCredentials);
    }

    Ok(user)
}

/// Issues a token for a user who signed in with an approved Quick Connect request instead of a
/// password. A missing or disabled user gets the same answer as a wrong password.
pub async fn authenticate_approved(
    pool: &SqlitePool,
    user_id: Uuid,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    let user = repo_user::get_user_by_id(pool, user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;
    if repo_user::is_disabled(pool, user.id).await? {
        return Err(AuthError::InvalidCredentials);
    }
    issue_login_token(pool, user.id, device).await
}

/// The hash a setup-code check verifies against when there is no code hash: argon2 with the same
/// parameters as code hashes, made once per process by `unusable_password_hash`.
static DUMMY_CODE_HASH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// Makes the dummy hash now, at startup, so the first setup-code check does not pay for it (B20
/// fix round 2, finding 4). Calling it again changes nothing.
pub async fn prepare_setup_code_checks() -> Result<(), AuthError> {
    DUMMY_CODE_HASH
        .get_or_try_init(unusable_password_hash)
        .await
        .map(|_| ())
}

/// The check of a setup code (B20 fix 5), shared by `accept_setup_code`, `pending_setup_user` and
/// `authenticate_with_setup_code`. The user is found as a password sign-in finds one, so a name
/// typed in another case still matches (B21 X2). Exactly one argon2 verification runs whoever is
/// asked about: against the user's code hash when the user exists and has one, else against the
/// dummy hash. A malformed code counts as wrong. So the answer time does not tell an unknown user,
/// a user without a code or a disabled user from a wrong code. Returns the user and the code hash
/// only when the user exists, is enabled, has a code and `code` is that code.
async fn check_setup_code(
    pool: &SqlitePool,
    username: &str,
    code: &str,
) -> Result<Option<(User, String)>, AuthError> {
    let user = repo_user::find_user_for_sign_in(pool, username).await?;
    let (hash, enabled) = match &user {
        Some(user) => (
            repo_setup_code::code_hash(pool, user.id).await?,
            !repo_user::is_disabled(pool, user.id).await?,
        ),
        None => (None, false),
    };
    let canonical = normalize_setup_code(code);
    let verified = match (&hash, &canonical) {
        (Some(hash), Some(canonical)) => verify_password(canonical, hash).await?,
        _ => {
            let dummy = DUMMY_CODE_HASH
                .get_or_try_init(unusable_password_hash)
                .await?;
            verify_password(canonical.as_deref().unwrap_or("000000000000"), dummy).await?;
            false
        }
    };
    Ok(match (user, hash) {
        (Some(user), Some(hash)) if verified && enabled => Some((user, hash)),
        _ => None,
    })
}

/// A setup code that `accept_setup_code` has checked: the pending user it belongs to and the code's
/// hash, which `redeem_setup_code` uses up. Holding one writes nothing.
pub struct VerifiedSetupCode {
    user: User,
    hash: String,
}

impl VerifiedSetupCode {
    /// The user whose setup code this is.
    pub fn user(&self) -> &User {
        &self.user
    }
}

/// Checks a setup code (B20 3.3, B21 X1) and writes nothing. A wrong code, an unknown user, a user
/// without a code, a disabled user or a malformed code gives `InvalidCredentials`, at the same cost
/// (B20 fix 5). The caller may refuse the sign-in before `redeem_setup_code` uses the code up.
pub async fn accept_setup_code(
    pool: &SqlitePool,
    username: &str,
    code: &str,
) -> Result<VerifiedSetupCode, AuthError> {
    match check_setup_code(pool, username, code).await? {
        Some((user, hash)) => Ok(VerifiedSetupCode { user, hash }),
        None => Err(AuthError::InvalidCredentials),
    }
}

/// Sets the password of the user a verified setup code belongs to and signs in (B20 3.3, 3.8).
/// `new_password` replaces the unusable password, and the code is used up, so it fails from then
/// on. A code that another request used first gives `InvalidCredentials`.
pub async fn redeem_setup_code(
    pool: &SqlitePool,
    verified: VerifiedSetupCode,
    new_password: &str,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    let VerifiedSetupCode { user, hash } = verified;
    let password_hash = hash_password(new_password).await?;
    if !repo_setup_code::redeem(pool, user.id, &hash, &password_hash).await? {
        return Err(AuthError::InvalidCredentials);
    }
    issue_login_token(pool, user.id, device).await
}

/// Sets the password of a pending user and signs in (B20 3.3, 3.8): `accept_setup_code` then
/// `redeem_setup_code`. `code` must be the user's current setup code.
pub async fn authenticate_with_setup_code(
    pool: &SqlitePool,
    username: &str,
    code: &str,
    new_password: &str,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    let verified = accept_setup_code(pool, username, code).await?;
    redeem_setup_code(pool, verified, new_password, device).await
}

/// The user whose pending setup code `code` is (B20 3.2, B21 X2), or `None`. Nothing is written;
/// an unknown user, a disabled user, a user without a code or a malformed code gives `None`, at the
/// same cost (B20 fix 5).
pub async fn pending_setup_user(
    pool: &SqlitePool,
    username: &str,
    code: &str,
) -> Result<Option<User>, AuthError> {
    Ok(check_setup_code(pool, username, code)
        .await?
        .map(|(user, _)| user))
}

/// Whether `code` is the current setup code of the enabled user `username` (B20 3.2, 3.8). See
/// `pending_setup_user`.
pub async fn setup_code_pending(
    pool: &SqlitePool,
    username: &str,
    code: &str,
) -> Result<bool, AuthError> {
    Ok(pending_setup_user(pool, username, code).await?.is_some())
}

/// Records a login and issues a token for `device`. The returned user is re-read after the login,
/// so its LastLoginDate is the new one (M-16). A new login on a device replaces that device's
/// previous token.
pub async fn issue_login_token(
    pool: &SqlitePool,
    user_id: Uuid,
    device: &TokenDevice,
) -> Result<(User, String), AuthError> {
    repo_user::update_last_login(pool, user_id).await?;
    let user = repo_user::get_user_by_id(pool, user_id)
        .await?
        .ok_or(AuthError::UserNotFound)?;

    if !device.device_id.is_empty() {
        repo_token::delete_user_device_tokens(pool, user.id, &device.device_id).await?;
    }

    let token = create_token(pool, &user, device).await?;
    Ok((user, token))
}

/// Names of enabled administrators whose password is still the default "admin".
pub async fn admins_with_default_password(pool: &SqlitePool) -> Result<Vec<String>, AuthError> {
    let mut names = Vec::new();
    for u in repo_user::get_all_users(pool).await? {
        if u.is_administrator
            && !repo_user::is_disabled(pool, u.id).await?
            && verify_password("admin", &u.password_hash)
                .await
                .unwrap_or(false)
        {
            names.push(u.name);
        }
    }
    Ok(names)
}

pub async fn create_initial_user(
    pool: &SqlitePool,
    name: &str,
    password: &str,
) -> Result<User, AuthError> {
    let hash = hash_password(password).await?;
    let user = User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: hash,
        is_administrator: true,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    };

    repo_user::create_user(pool, &user).await?;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── hash_password / verify_password ──

    #[tokio::test]
    async fn test_hash_password_returns_hash() {
        let hash = hash_password("secret123").await.unwrap();
        assert!(!hash.is_empty());
        assert!(hash.starts_with("$argon"));
    }

    #[tokio::test]
    async fn test_verify_password_correct() {
        let hash = hash_password("mypassword").await.unwrap();
        assert!(verify_password("mypassword", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_password_incorrect() {
        let hash = hash_password("mypassword").await.unwrap();
        assert!(!verify_password("wrongpassword", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_password_empty_hash() {
        let result = verify_password("test", "").await;
        assert!(result.is_err());
    }

    // Token lifecycle (opaque DB-backed), authenticate, and admin-default-password
    // checks live in tests/auth_tests.rs now that they need a live pool.

    // ── hash_pin / verify_pin ──

    #[tokio::test]
    async fn test_hash_and_verify_pin() {
        let pin = "123456";
        let hash = hash_pin(pin).await.unwrap();
        assert!(verify_pin(pin, &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_wrong_pin() {
        let pin = "123456";
        let hash = hash_pin(pin).await.unwrap();
        assert!(!verify_pin("654321", &hash).await.unwrap());
    }

    // ── B20: one-time setup codes ──

    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    #[test]
    fn b20_setup_codes_have_the_crockford_shape() {
        for _ in 0..200 {
            let code = generate_setup_code();
            let groups: Vec<&str> = code.split('-').collect();
            assert_eq!(groups.len(), 3, "{code}");
            for group in groups {
                assert_eq!(group.len(), 4, "{code}");
                assert!(group.chars().all(|c| CROCKFORD.contains(c)), "{code}");
            }
        }
    }

    #[test]
    fn b20_setup_codes_differ_and_use_the_whole_alphabet() {
        let codes: std::collections::HashSet<String> =
            (0..1000).map(|_| generate_setup_code()).collect();
        assert_eq!(codes.len(), 1000, "repeated codes");
        let seen: std::collections::HashSet<char> = codes.iter().flat_map(|c| c.chars()).collect();
        for c in CROCKFORD.chars() {
            assert!(seen.contains(&c), "{c} never appears in 1000 codes");
        }
    }

    #[test]
    fn b20_normalize_setup_codes() {
        let canonical = Some("ABCDEFGHJKMN".to_string());
        assert_eq!(normalize_setup_code("ABCD-EFGH-JKMN"), canonical);
        assert_eq!(normalize_setup_code(" abcd efgh-jkmn "), canonical);
        assert_eq!(normalize_setup_code("ABCDEFGHJKMN"), canonical);
        assert_eq!(
            normalize_setup_code("IL0O-ilo0-1234"),
            Some("110011001234".to_string())
        );
        for bad in [
            "",
            "ABCD-EFGH-JKM",
            "ABCD-EFGH-JKMNP",
            "ABCD-EFGH-JKMU",
            "ABCD_EFGH_JKMN",
        ] {
            assert_eq!(normalize_setup_code(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn b20_setup_code_hashes_verify_in_canonical_form() {
        let code = generate_setup_code();
        let hash = hash_setup_code(&code).await.unwrap();
        assert!(hash.starts_with("$argon2"), "{hash}");
        assert!(!hash.contains(&code) && !hash.contains(&code.replace('-', "")));
        assert!(verify_setup_code(&code, &hash).await.unwrap());
        let loose = code.to_lowercase().replace('-', " ");
        assert!(verify_setup_code(&loose, &hash).await.unwrap(), "{loose}");
        let mut other = generate_setup_code();
        while other == code {
            other = generate_setup_code();
        }
        assert!(!verify_setup_code(&other, &hash).await.unwrap());
        assert!(!verify_setup_code("not a code", &hash).await.unwrap());
        assert!(matches!(
            hash_setup_code("not a code").await,
            Err(AuthError::HashError)
        ));
    }

    #[tokio::test]
    async fn b20_unusable_password_hash_matches_nothing() {
        let first = unusable_password_hash().await.unwrap();
        let second = unusable_password_hash().await.unwrap();
        assert!(first.starts_with("$argon2"), "{first}");
        assert_ne!(first, second);
        for guess in ["", "admin", "password"] {
            assert!(!verify_password(guess, &first).await.unwrap(), "{guess:?}");
        }
    }

    // Fix round 2, 4: the server makes the dummy hash at startup, so the first setup-code check that
    // needs it runs one verification, like every later one.
    #[tokio::test]
    async fn b20_setup_code_checks_are_prepared_before_the_first_sign_in() {
        prepare_setup_code_checks().await.unwrap();
        let hash = DUMMY_CODE_HASH
            .get()
            .expect("no dummy hash after preparing");
        assert!(hash.starts_with("$argon2"), "{hash}");
        prepare_setup_code_checks().await.unwrap();
        assert_eq!(
            DUMMY_CODE_HASH.get(),
            Some(hash),
            "preparing again made a new hash"
        );
    }
}
