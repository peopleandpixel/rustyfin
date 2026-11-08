-- The version of the probe code that wrote each source (repo_media_source::PROBE_VERSION).
-- Sources from before this column read 0, so the next scan reads their files again.
ALTER TABLE media_sources ADD COLUMN probe_version INTEGER NOT NULL DEFAULT 0;
