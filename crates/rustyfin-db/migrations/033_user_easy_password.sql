-- The Easy PIN is kept as an argon2 hash in its own column, so no user response returns it.
ALTER TABLE users ADD COLUMN easy_password_hash TEXT;
-- Older versions stored the PIN in plain text in the user configuration; remove it there.
UPDATE users SET configuration = json_remove(configuration, '$.easyPassword')
WHERE json_valid(configuration) AND json_type(configuration, '$.easyPassword') IS NOT NULL;
