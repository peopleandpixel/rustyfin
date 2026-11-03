-- Migration 014: Indexes for genres and tags filtering
-- P12: Eliminates full table scans on genre/tags filters

-- Add tags column if it doesn't exist (used for user-defined tags on media items)
ALTER TABLE media_items ADD COLUMN tags TEXT NOT NULL DEFAULT '';

-- Composite index for library-scoped genre filtering (most common query pattern)
CREATE INDEX IF NOT EXISTS idx_media_items_lib_genres ON media_items(library_id, genres);

-- Composite index for library-scoped tags filtering
CREATE INDEX IF NOT EXISTS idx_media_items_lib_tags ON media_items(library_id, tags);

-- Single-column indexes for non-library-scoped genre/tags searches
CREATE INDEX IF NOT EXISTS idx_media_items_genres ON media_items(genres);
CREATE INDEX IF NOT EXISTS idx_media_items_tags ON media_items(tags);
