-- The version of each episode's file (size and modified time in seconds) when intro detection last
-- ran on it successfully, and the version of the detection that ran, so the daily run skips
-- files that haven't changed since and a changed detection runs on every file again.
CREATE TABLE IF NOT EXISTS media_segment_scans (
    item_id TEXT PRIMARY KEY NOT NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    detector_version INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);
