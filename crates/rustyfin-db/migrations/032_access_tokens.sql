CREATE TABLE IF NOT EXISTS access_tokens (
    token_hash   TEXT PRIMARY KEY NOT NULL,
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id    TEXT NOT NULL DEFAULT '',
    device_name  TEXT NOT NULL DEFAULT '',
    app_name     TEXT NOT NULL DEFAULT '',
    app_version  TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_access_tokens_user ON access_tokens(user_id, device_id);
-- Harden password reset PINs (R1-01, M-17). This was folded into 032 from a separate migration.
-- PINs used to be 6 plaintext digits; now pin_code holds a SHA-256 hex of the normalized PIN.
-- Drop any legacy rows and add a per-reset failed-attempt counter (5-attempt cap).
DELETE FROM password_reset_pins;
ALTER TABLE password_reset_pins ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0;
DROP INDEX IF EXISTS idx_password_reset_pins_user_id;
-- One reset per user (the DELETE above guarantees no duplicates when the index is built).
CREATE UNIQUE INDEX IF NOT EXISTS idx_password_reset_pins_user ON password_reset_pins(user_id);
