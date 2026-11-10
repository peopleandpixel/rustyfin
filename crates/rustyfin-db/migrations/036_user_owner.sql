-- Server owner flag. The partial unique index allows at most one owner.
ALTER TABLE users ADD COLUMN is_owner INTEGER NOT NULL DEFAULT 0;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_single_owner ON users(is_owner) WHERE is_owner = 1;
