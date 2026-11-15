-- Trickplay jobs that failed, by item, with the version (size and modified time in seconds) of
-- the file they failed on; a failed item is tried again only when its file changes, or a refresh
-- replaces all its images. Only kind 'trickplay' is written: a failed chapter image is marked in
-- its chapter row instead (an empty image_path).
CREATE TABLE video_image_failures (
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    file_mtime INTEGER NOT NULL,
    PRIMARY KEY (item_id, kind)
);

-- The file version (size and modified time in seconds) each item's tile set and chapter images
-- were made from. A re-probe keeps them while the file's version is the same.
CREATE TABLE video_image_versions (
    item_id TEXT NOT NULL PRIMARY KEY REFERENCES media_items(id) ON DELETE CASCADE,
    file_size INTEGER NOT NULL,
    file_mtime INTEGER NOT NULL
);
