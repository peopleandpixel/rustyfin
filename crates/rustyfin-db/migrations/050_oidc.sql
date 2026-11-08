-- OpenID Connect sign-in (B21): the one provider's settings (one row), the links between Rustyfin
-- users and provider identities, and the admin rights granted through a group.

CREATE TABLE IF NOT EXISTS oidc_settings (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    issuer_url TEXT NOT NULL DEFAULT '',
    client_id TEXT NOT NULL DEFAULT '',
    client_secret TEXT NOT NULL DEFAULT '',
    scopes TEXT NOT NULL DEFAULT '',
    button_label TEXT NOT NULL DEFAULT '',
    username_claim TEXT NOT NULL DEFAULT 'preferred_username',
    group_claim TEXT NOT NULL DEFAULT 'groups',
    allowed_group TEXT NOT NULL DEFAULT '',
    admin_group TEXT NOT NULL DEFAULT '',
    public_url TEXT NOT NULL DEFAULT '',
    allow_password_sign_in INTEGER NOT NULL DEFAULT 1
);

INSERT INTO oidc_settings (id) VALUES (1);

CREATE TABLE IF NOT EXISTS user_oidc_links (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    username TEXT NOT NULL,
    linked_at TEXT NOT NULL,
    last_sign_in_at TEXT,
    UNIQUE (issuer, subject)
);

ALTER TABLE users ADD COLUMN admin_from_group INTEGER NOT NULL DEFAULT 0;
