-- The version of each episode's file (size and modified time in seconds) when intro detection last
-- ran on it successfully, so the daily run skips files that haven't changed since.
CREATE TABLE IF NOT EXISTS media_segment_scans (
    item_id TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);
