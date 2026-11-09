ALTER TABLE media_sources ADD COLUMN modified INTEGER;
CREATE INDEX IF NOT EXISTS idx_media_sources_path ON media_sources(path);
