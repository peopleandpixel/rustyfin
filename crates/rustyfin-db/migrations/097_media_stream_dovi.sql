-- A video stream's Dolby Vision configuration record (JSON: DvProfile, DvBlSignalCompatibilityId,
-- RpuPresentFlag, ...), which Jellyfin's VideoRangeType (DOVIWithHDR10, DOVIWithEL, ...) is read
-- from. NULL when the stream has none, or was probed before PROBE_VERSION 3.
ALTER TABLE media_streams ADD COLUMN dovi_config TEXT;
