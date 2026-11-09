-- Disabled users cannot log in. The setup wizard uses this to disable a leftover admin account.
ALTER TABLE users ADD COLUMN is_disabled INTEGER NOT NULL DEFAULT 0;
