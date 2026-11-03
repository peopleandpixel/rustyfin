-- Migration 018: Password reset pins

CREATE TABLE IF NOT EXISTS password_reset_pins (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    pin_code TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_password_reset_pins_user_id ON password_reset_pins(user_id);
CREATE INDEX IF NOT EXISTS idx_password_reset_pins_pin_code ON password_reset_pins(pin_code);
