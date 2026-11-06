-- Plugin priority ordering
-- Lower number = higher priority (processed first).
-- Default priority is 100; built-in plugins can be reordered.
CREATE TABLE IF NOT EXISTS plugin_priority (
    plugin_id TEXT PRIMARY KEY,
    priority INTEGER NOT NULL DEFAULT 100,
    FOREIGN KEY (plugin_id) REFERENCES plugin_config(plugin_id) ON DELETE CASCADE
);
