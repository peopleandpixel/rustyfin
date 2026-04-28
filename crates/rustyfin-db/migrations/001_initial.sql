CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_administrator INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_login_date TEXT
);

CREATE TABLE IF NOT EXISTS libraries (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    paths TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS media_items (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    sort_name TEXT,
    overview TEXT,
    media_type TEXT NOT NULL,
    path TEXT NOT NULL,
    parent_id TEXT REFERENCES media_items(id),
    library_id TEXT NOT NULL REFERENCES libraries(id),
    production_year INTEGER,
    community_rating REAL,
    official_rating TEXT,
    genres TEXT NOT NULL DEFAULT '[]',
    date_created TEXT NOT NULL DEFAULT (datetime('now')),
    premiere_date TEXT,
    runtime_ticks INTEGER,
    index_number INTEGER,
    parent_index_number INTEGER
);

CREATE INDEX IF NOT EXISTS idx_media_items_library ON media_items(library_id);
CREATE INDEX IF NOT EXISTS idx_media_items_parent ON media_items(parent_id);
CREATE INDEX IF NOT EXISTS idx_media_items_type ON media_items(media_type);
