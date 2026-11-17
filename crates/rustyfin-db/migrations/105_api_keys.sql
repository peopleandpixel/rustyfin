-- API keys an administrator creates in the dashboard (Jellyfin's /Auth/Keys). Each one gives
-- administrator access, like RUSTYFIN_API_KEY. Only the hex SHA-256 of a key is stored, so the key
-- itself is shown once, when it is created; `hint` is its last four characters, to tell keys apart.
CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    hint TEXT NOT NULL DEFAULT '',
    app_name TEXT NOT NULL DEFAULT '',
    date_created TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    date_last_activity TEXT
);
