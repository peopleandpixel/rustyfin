-- Chapter images, trickplay tile sets, and a one-time re-read of chapters.

-- When a chapter's image was made, for its ImageTag and ImageDateModified.
ALTER TABLE chapters ADD COLUMN image_date_modified TEXT;

-- Trickplay tile sets, one per item and thumbnail width, as Jellyfin's TrickplayInfo. The tiles
-- live in the data dir's trickplay folder, so no path is stored. The old table was never filled.
DROP TABLE IF EXISTS trickplay;
CREATE TABLE trickplay_infos (
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    tile_width INTEGER NOT NULL,
    tile_height INTEGER NOT NULL,
    thumbnail_count INTEGER NOT NULL,
    interval_ms INTEGER NOT NULL,
    bandwidth INTEGER NOT NULL,
    PRIMARY KEY (item_id, width)
);

-- Videos probed before chapters could be read have none stored. Each is read once more in the
-- background; the row goes when its chapters have been read.
CREATE TABLE chapter_rereads (
    item_id TEXT NOT NULL PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE
);
INSERT INTO chapter_rereads (item_id)
SELECT id FROM media_items
WHERE media_type IN ('Movie', 'Episode') AND path IS NOT NULL AND path != ''
  AND id NOT IN (SELECT item_id FROM chapters);
