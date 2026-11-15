-- Trickplay and chapter-image jobs that failed, by item and kind, with the version (size and
-- modified time in seconds) of the file they failed on. A failed item is tried again only when
-- its file changes, or a refresh replaces all its images.
CREATE TABLE video_image_failures (
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    file_mtime INTEGER NOT NULL,
    PRIMARY KEY (item_id, kind)
);
