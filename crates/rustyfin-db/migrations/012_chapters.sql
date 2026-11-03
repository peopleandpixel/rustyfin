-- Migration 011: Chapters table for video chapter markers

CREATE TABLE IF NOT EXISTS chapters (
    id TEXT NOT NULL PRIMARY KEY,
    item_id TEXT NOT NULL,
    chapter_number INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    start_ticks INTEGER NOT NULL,
    end_ticks INTEGER NOT NULL,
    image_path TEXT,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chapters_item_id ON chapters(item_id);
CREATE INDEX IF NOT EXISTS idx_chapters_start_ticks ON chapters(start_ticks);
