//! The OIDC settings: the one row of `oidc_settings` (the migration inserts it). The client secret
//! is stored as it is (spec §4).

use chrono::{DateTime, Utc};
use rustyfin_model::oidc::{AccountStatus, OidcConfig};
use rustyfin_model::user::User;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct SettingsRow {
    enabled: bool,
    issuer_url: String,
    client_id: String,
    client_secret: String,
    scopes: String,
    button_label: String,
    username_claim: String,
    group_claim: String,
    allowed_group: String,
    admin_group: String,
    public_url: String,
    allow_password_sign_in: bool,
}

/// The saved settings.
pub async fn get_settings(pool: &SqlitePool) -> Result<OidcConfig, sqlx::Error> {
    let row = sqlx::query_as::<_, SettingsRow>(
        "SELECT enabled, issuer_url, client_id, client_secret, scopes, button_label, \
         username_claim, group_claim, allowed_group, admin_group, public_url, \
         allow_password_sign_in FROM oidc_settings WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;
    Ok(OidcConfig {
        enabled: row.enabled,
        issuer_url: row.issuer_url,
        client_id: row.client_id,
        client_secret: row.client_secret,
        scopes: row.scopes,
        button_label: row.button_label,
        username_claim: row.username_claim,
        group_claim: row.group_claim,
        allowed_group: row.allowed_group,
        admin_group: row.admin_group,
        public_url: row.public_url,
        allow_password_sign_in: row.allow_password_sign_in,
    })
}

/// Writes every setting.
pub async fn save_settings(pool: &SqlitePool, config: &OidcConfig) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE oidc_settings SET enabled = ?, issuer_url = ?, client_id = ?, client_secret = ?, \
         scopes = ?, button_label = ?, username_claim = ?, group_claim = ?, allowed_group = ?, \
         admin_group = ?, public_url = ?, allow_password_sign_in = ? WHERE id = 1",
    )
    .bind(i32::from(config.enabled))
    .bind(&config.issuer_url)
    .bind(&config.client_id)
    .bind(&config.client_secret)
    .bind(&config.scopes)
    .bind(&config.button_label)
    .bind(&config.username_claim)
    .bind(&config.group_claim)
    .bind(&config.allowed_group)
    .bind(&config.admin_group)
    .bind(&config.public_url)
    .bind(i32::from(config.allow_password_sign_in))
    .execute(pool)
    .await?;
    Ok(())
}

/// The user a provider account is linked to, if any.
pub async fn find_link(
    pool: &SqlitePool,
    issuer: &str,
    subject: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT user_id FROM user_oidc_links WHERE issuer = ? AND subject = ?")
            .bind(issuer)
            .bind(subject)
            .fetch_optional(pool)
            .await?;
    row.map(|(id,)| Uuid::parse_str(&id).map_err(|e| sqlx::Error::Decode(Box::new(e))))
        .transpose()
}

/// Whether the user is linked to a provider account.
pub async fn user_has_link(pool: &SqlitePool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let linked: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM user_oidc_links WHERE user_id = ?)")
            .bind(user_id.to_string())
            .fetch_one(pool)
            .await?;
    Ok(linked)
}

/// Links a user to a provider account. `linked_at` is `now`. `last_sign_in_at` is the sign-in that
/// made the link, or None for a profile link that has not signed in. The executor is a pool, or a
/// transaction the caller holds.
pub async fn create_link<'e, E>(
    executor: E,
    user_id: Uuid,
    issuer: &str,
    subject: &str,
    username: &str,
    now: DateTime<Utc>,
    last_sign_in_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let linked_at = now.to_rfc3339();
    sqlx::query(
        "INSERT INTO user_oidc_links (user_id, issuer, subject, username, linked_at, last_sign_in_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user_id.to_string())
    .bind(issuer)
    .bind(subject)
    .bind(username)
    .bind(&linked_at)
    .bind(last_sign_in_at.map(|at| at.to_rfc3339()))
    .execute(executor)
    .await?;
    Ok(())
}

/// What `settle_sign_in` did.
pub enum Settled {
    /// The sign-in or the link is settled, and the admin group applied.
    Done,
    /// The admin rights stay: the owner or the last enabled administrator keeps them.
    AdminKept,
    /// The link was gone, so nothing was written.
    LinkGone,
}

/// Settles a sign-in or a profile link in one IMMEDIATE transaction (AC55). When the link to
/// `issuer` and `subject` is gone, nothing is written and the result is `LinkGone`. Otherwise
/// `signed_in` records the sign-in through the link (the username changes only when the claim
/// gave one), and the admin group applies as `admin_member` says (AC17-AC19).
pub async fn settle_sign_in(
    pool: &SqlitePool,
    user_id: Uuid,
    issuer: &str,
    subject: &str,
    signed_in: Option<(DateTime<Utc>, Option<&str>)>,
    admin_member: bool,
) -> Result<Settled, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM user_oidc_links \
         WHERE user_id = ? AND issuer = ? AND subject = ?)",
    )
    .bind(user_id.to_string())
    .bind(issuer)
    .bind(subject)
    .fetch_one(&mut *tx)
    .await?;
    if !linked {
        tx.rollback().await?;
        return Ok(Settled::LinkGone);
    }
    if let Some((now, username)) = signed_in {
        sqlx::query(
            "UPDATE user_oidc_links SET last_sign_in_at = ?, username = COALESCE(?, username) \
             WHERE user_id = ?",
        )
        .bind(now.to_rfc3339())
        .bind(username)
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    let settled = if admin_member {
        crate::repo_user::grant_group_admin(&mut *tx, user_id).await?;
        Settled::Done
    } else if crate::repo_user::admin_from_group(&mut *tx, user_id).await?
        && !crate::repo_user::revoke_group_admin(&mut *tx, user_id).await?
    {
        Settled::AdminKept
    } else {
        Settled::Done
    };
    tx.commit().await?;
    Ok(settled)
}

/// Creates a user and links it to a provider account in one IMMEDIATE transaction, so a failed link
/// leaves no user behind. `taken` runs again over every user name inside that transaction, so two
/// simultaneous sign-ins cannot both take one name (AC52). It returns false, writing nothing, when
/// `taken` matches a name; true once the user and the link are committed.
pub async fn create_linked_user(
    pool: &SqlitePool,
    user: &User,
    issuer: &str,
    subject: &str,
    username: &str,
    now: DateTime<Utc>,
    taken: impl Fn(&str) -> bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM users")
        .fetch_all(&mut *tx)
        .await?;
    if names.iter().any(|name| taken(name)) {
        tx.rollback().await?;
        return Ok(false);
    }
    crate::repo_user::create_user(&mut *tx, user).await?;
    create_link(&mut *tx, user.id, issuer, subject, username, now, Some(now)).await?;
    tx.commit().await?;
    Ok(true)
}

/// What an unlink did: whether the account had a link, and whether it kept its administrator rights.
pub struct Unlinked {
    pub had_link: bool,
    pub admin_kept: bool,
}

/// Removes the link of a user and takes back the administrator rights the admin group gave it, in
/// one IMMEDIATE transaction (AC51). The owner and the last enabled administrator keep those rights,
/// and `admin_kept` says so. Rights given by hand stay.
pub async fn unlink_account(pool: &SqlitePool, user_id: Uuid) -> Result<Unlinked, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let deleted = sqlx::query("DELETE FROM user_oidc_links WHERE user_id = ?")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        tx.commit().await?;
        return Ok(Unlinked {
            had_link: false,
            admin_kept: false,
        });
    }
    let revoked = crate::repo_user::revoke_group_admin(&mut *tx, user_id).await?;
    let admin_kept = !revoked && crate::repo_user::admin_from_group(&mut *tx, user_id).await?;
    tx.commit().await?;
    Ok(Unlinked {
        had_link: true,
        admin_kept,
    })
}

/// The link status of every account, by name, or of the one account `only` names (AC30).
/// The CanUnlink rule (AC31, AC41) reads `allow_password_sign_in` and each account's
/// admin-group flag.
pub async fn account_statuses(
    pool: &SqlitePool,
    allow_password_sign_in: bool,
    only: Option<Uuid>,
) -> Result<Vec<AccountStatus>, sqlx::Error> {
    let rows: Vec<AccountRow> = sqlx::query_as(
        "SELECT u.id, u.name, u.is_administrator, u.admin_from_group, \
         (u.password_hash <> '') AS has_password, l.username, l.linked_at, l.last_sign_in_at \
         FROM users u LEFT JOIN user_oidc_links l ON l.user_id = u.id \
         WHERE ?1 IS NULL OR u.id = ?1 ORDER BY u.name",
    )
    .bind(only.map(|id| id.to_string()))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.into_status(allow_password_sign_in))
        .collect())
}

/// A row of `account_statuses`. SQLite gives the flags as integers.
#[derive(sqlx::FromRow)]
struct AccountRow {
    id: String,
    name: String,
    is_administrator: i32,
    admin_from_group: i32,
    has_password: i32,
    username: Option<String>,
    linked_at: Option<String>,
    last_sign_in_at: Option<String>,
}

impl AccountRow {
    fn into_status(self, allow_password_sign_in: bool) -> AccountStatus {
        let linked = self.username.is_some();
        let is_administrator = self.is_administrator != 0;
        let admin_from_group = self.admin_from_group != 0;
        let has_password = self.has_password != 0;
        AccountStatus {
            user_id: Uuid::parse_str(&self.id).unwrap_or_default(),
            user_name: self.name,
            is_administrator,
            admin_from_group,
            has_password,
            linked,
            provider_username: self.username,
            linked_at: self
                .linked_at
                .and_then(|at| at.parse::<DateTime<Utc>>().ok()),
            last_sign_in_at: self
                .last_sign_in_at
                .and_then(|at| at.parse::<DateTime<Utc>>().ok()),
            can_unlink: linked
                && has_password
                && (allow_password_sign_in || (is_administrator && !admin_from_group)),
        }
    }
}
