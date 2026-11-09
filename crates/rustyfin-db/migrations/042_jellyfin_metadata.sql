-- Metadata imported from Jellyfin (spec 5.3).

-- Rustyfin's own extras per item: original title, critic rating, IMDb id and the locks (5.4, 5.5).
-- No foreign key: the trigger below deletes the row with its media item.
CREATE TABLE IF NOT EXISTS item_metadata_extras (
    item_id TEXT PRIMARY KEY NOT NULL,
    original_title TEXT,
    critic_rating REAL,
    imdb_id TEXT,
    lock_data INTEGER NOT NULL DEFAULT 0,
    locked_fields TEXT NOT NULL DEFAULT ''
);

-- REPLACE deletes don't fire this trigger (recursive_triggers is off), so a metadata refresh's
-- INSERT OR REPLACE keeps the extras row.
CREATE TRIGGER IF NOT EXISTS item_metadata_extras_ad AFTER DELETE ON media_items
BEGIN
    DELETE FROM item_metadata_extras WHERE item_id = OLD.id;
END;

-- The order of an item's people (5.7).
ALTER TABLE item_people ADD COLUMN sort_order INTEGER;

-- Images of people (5.8, 5.9), keyed like item_images.
CREATE TABLE IF NOT EXISTS person_images (
    person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    image_type TEXT NOT NULL,
    image_index INTEGER NOT NULL DEFAULT 0,
    path TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT 'local',
    width INTEGER,
    height INTEGER,
    blurhash TEXT,
    PRIMARY KEY (person_id, image_type, image_index)
);
