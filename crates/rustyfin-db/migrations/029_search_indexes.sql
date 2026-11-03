-- Search performance optimization
-- Add indexes to improve LIKE fallback search performance and ensure FTS5 can be reindexed

-- Index for prefix LIKE queries (name LIKE 'query%') - enables B-tree index scan
CREATE INDEX IF NOT EXISTS idx_media_items_name ON media_items(name);

-- Index for media_type + name composite queries (search with type filter)
CREATE INDEX IF NOT EXISTS idx_media_items_type_name ON media_items(media_type, name);

-- Ensure FTS5 index can be reindexed if it becomes out of sync
-- The REINDEX command works on external content FTS5 tables
-- This migration ensures the trigger-based sync is correct by dropping and recreating triggers
DROP TRIGGER IF EXISTS media_items_ai;
DROP TRIGGER IF EXISTS media_items_ad;
DROP TRIGGER IF EXISTS media_items_au;

CREATE TRIGGER media_items_ai AFTER INSERT ON media_items BEGIN
    INSERT INTO media_items_fts(rowid, name, overview, genres)
    VALUES (new.rowid, new.name, COALESCE(new.overview, ''), new.genres);
END;

CREATE TRIGGER media_items_ad AFTER DELETE ON media_items BEGIN
    INSERT INTO media_items_fts(media_items_fts, rowid, name, overview, genres)
    VALUES ('delete', old.rowid, old.name, COALESCE(old.overview, ''), old.genres);
END;

CREATE TRIGGER media_items_au AFTER UPDATE ON media_items BEGIN
    INSERT INTO media_items_fts(media_items_fts, rowid, name, overview, genres)
    VALUES ('delete', old.rowid, old.name, COALESCE(old.overview, ''), old.genres);
    INSERT INTO media_items_fts(rowid, name, overview, genres)
    VALUES (new.rowid, new.name, COALESCE(new.overview, ''), new.genres);
END;
