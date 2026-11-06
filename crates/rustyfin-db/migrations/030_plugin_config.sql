-- Plugin configuration storage
-- Each plugin stores its configuration as JSON in this table,
-- independent of the server's config.json.
CREATE TABLE IF NOT EXISTS plugin_config (
    plugin_id TEXT PRIMARY KEY,
    config_json TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
