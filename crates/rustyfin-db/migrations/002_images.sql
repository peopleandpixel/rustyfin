CREATE TABLE IF NOT EXISTS item_images (
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    image_type TEXT NOT NULL,
    image_index INTEGER NOT NULL DEFAULT 0,
    path TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'local',
    width INTEGER,
    height INTEGER,
    PRIMARY KEY (item_id, image_type, image_index)
);

CREATE INDEX IF NOT EXISTS idx_item_images_item ON item_images(item_id);
