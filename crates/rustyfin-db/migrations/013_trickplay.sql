-- Migration 012: Trickplay table for preview thumbnails

CREATE TABLE IF NOT EXISTS trickplay (
    id TEXT NOT NULL PRIMARY KEY,
    item_id TEXT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    interval_ticks INTEGER NOT NULL,
    thumbnail_count INTEGER NOT NULL,
    directory_path TEXT NOT NULL,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_trickplay_item_id ON trickplay(item_id);
