-- Collections & Playlists
CREATE TABLE IF NOT EXISTS collections (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS collection_items (
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (collection_id, item_id)
);

CREATE TABLE IF NOT EXISTS playlists (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    media_type TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS playlist_items (
    playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (playlist_id, item_id)
);

-- Activity log
CREATE TABLE IF NOT EXISTS activity_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    user_id TEXT,
    item_id TEXT,
    severity TEXT NOT NULL DEFAULT 'Information',
    overview TEXT,
    date TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_activity_log_date ON activity_log(date DESC);
CREATE INDEX IF NOT EXISTS idx_activity_log_user ON activity_log(user_id);

-- Notifications
CREATE TABLE IF NOT EXISTS notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    level TEXT NOT NULL DEFAULT 'Normal',
    is_read INTEGER NOT NULL DEFAULT 0,
    date TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_notifications_user ON notifications(user_id);

-- FTS for search (virtual table)
CREATE VIRTUAL TABLE IF NOT EXISTS media_items_fts USING fts5(
    name,
    overview,
    genres,
    content='media_items',
    content_rowid='rowid'
);

-- Triggers to keep FTS in sync
CREATE TRIGGER IF NOT EXISTS media_items_ai AFTER INSERT ON media_items BEGIN
    INSERT INTO media_items_fts(rowid, name, overview, genres)
    VALUES (new.rowid, new.name, COALESCE(new.overview, ''), new.genres);
END;

CREATE TRIGGER IF NOT EXISTS media_items_ad AFTER DELETE ON media_items BEGIN
    INSERT INTO media_items_fts(media_items_fts, rowid, name, overview, genres)
    VALUES ('delete', old.rowid, old.name, COALESCE(old.overview, ''), old.genres);
END;

CREATE TRIGGER IF NOT EXISTS media_items_au AFTER UPDATE ON media_items BEGIN
    INSERT INTO media_items_fts(media_items_fts, rowid, name, overview, genres)
    VALUES ('delete', old.rowid, old.name, COALESCE(old.overview, ''), old.genres);
    INSERT INTO media_items_fts(rowid, name, overview, genres)
    VALUES (new.rowid, new.name, COALESCE(new.overview, ''), new.genres);
END;
