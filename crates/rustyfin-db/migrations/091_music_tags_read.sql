-- Audio items whose tags a music scan has read, with the size and modification time of the file
-- when it was read. A track without an artist link is read again only when its file has changed.
CREATE TABLE IF NOT EXISTS music_tags_read (
    item_id TEXT PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL
);
