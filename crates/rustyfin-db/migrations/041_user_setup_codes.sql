-- One-time setup codes of users the Jellyfin import created (B20). Such a user signs in once with
-- the code and chooses a password; only an argon2 hash of the code is stored.
CREATE TABLE IF NOT EXISTS user_setup_codes (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
);
