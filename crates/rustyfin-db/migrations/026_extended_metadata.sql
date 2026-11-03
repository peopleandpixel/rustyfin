-- Migration 025: Extended metadata fields from TMDb details endpoints
-- Adds columns for rich metadata that TMDb provides but was not previously stored.

ALTER TABLE media_items ADD COLUMN status TEXT;
ALTER TABLE media_items ADD COLUMN original_language TEXT;
ALTER TABLE media_items ADD COLUMN tagline TEXT;
ALTER TABLE media_items ADD COLUMN production_countries TEXT NOT NULL DEFAULT '[]';
ALTER TABLE media_items ADD COLUMN budget INTEGER;
ALTER TABLE media_items ADD COLUMN revenue INTEGER;
ALTER TABLE media_items ADD COLUMN homepage TEXT;
ALTER TABLE media_items ADD COLUMN end_date TEXT;
ALTER TABLE media_items ADD COLUMN vote_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE media_items ADD COLUMN popularity REAL;
