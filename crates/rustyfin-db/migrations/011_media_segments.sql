-- Migration 010: Media segments for intro/outro detection

-- Media segments table stores detected intros, outros, recaps, and credits
CREATE TABLE IF NOT EXISTS media_segments (
    id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    segment_type TEXT NOT NULL,  -- 'Intro', 'Outro', 'Credits', 'Recap'
    segment_start_ticks INTEGER NOT NULL,
    segment_end_ticks INTEGER NOT NULL,
    PRIMARY KEY (id, item_id, segment_type),
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);

-- Index for fast lookups by item
CREATE INDEX IF NOT EXISTS idx_media_segments_item_id ON media_segments(item_id);
