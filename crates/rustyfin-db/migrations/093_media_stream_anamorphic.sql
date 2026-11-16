-- The anamorphic flag of a video stream, which Jellyfin's playback decision tests (the
-- IsAnamorphic condition). The codec level goes in 007's `level` column, unused until now.
ALTER TABLE media_streams ADD COLUMN is_anamorphic INTEGER;
