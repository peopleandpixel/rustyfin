-- The token whose request made a session for another device than the token's own: a Cast receiver
-- signs with the sender's token and names its own DeviceId. NULL for every other session. Ending
-- the token (sign-out, revocation) ends these sessions with it, and the server caps how many one
-- token keeps.
ALTER TABLE sessions ADD COLUMN via_token TEXT REFERENCES access_tokens(token_hash) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_sessions_via_token ON sessions(via_token);
