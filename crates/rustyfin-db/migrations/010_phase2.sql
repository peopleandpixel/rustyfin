-- Phase 2: Web UI Compatibility migration

-- Configuration table for server settings
CREATE TABLE IF NOT EXISTS configuration (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Media encoder settings
CREATE TABLE IF NOT EXISTS media_encoder (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    ffmpeg_path TEXT NOT NULL DEFAULT 'ffmpeg',
    ffprobe_path TEXT NOT NULL DEFAULT 'ffprobe',
    ffmpeg_version TEXT,
    ffprobe_version TEXT
);

-- People (actors, directors, writers, etc.)
CREATE TABLE IF NOT EXISTS people (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL DEFAULT 'Actor',
    date_of_birth TEXT,
    date_of_death TEXT,
    place_of_birth TEXT,
    overview TEXT,
    image_hash TEXT
);

-- Junction: items to people with roles
CREATE TABLE IF NOT EXISTS item_people (
    item_id TEXT NOT NULL,
    person_id TEXT NOT NULL,
    type TEXT NOT NULL DEFAULT 'Actor',
    role TEXT,
    PRIMARY KEY (item_id, person_id),
    FOREIGN KEY (item_id) REFERENCES media_items(id),
    FOREIGN KEY (person_id) REFERENCES people(id)
);

CREATE INDEX IF NOT EXISTS idx_item_people_person ON item_people(person_id);

-- Display preferences per user
CREATE TABLE IF NOT EXISTS display_preferences (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (user_id, key),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

-- Studios
CREATE TABLE IF NOT EXISTS studios (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    image_hash TEXT
);

-- Junction: items to studios
CREATE TABLE IF NOT EXISTS item_studios (
    item_id TEXT NOT NULL,
    studio_id TEXT NOT NULL,
    PRIMARY KEY (item_id, studio_id),
    FOREIGN KEY (item_id) REFERENCES media_items(id),
    FOREIGN KEY (studio_id) REFERENCES studios(id)
);

CREATE INDEX IF NOT EXISTS idx_item_studios_studio ON item_studios(studio_id);
