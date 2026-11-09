-- The last episode of a file that holds several, such as `Show S01E03-E04.mkv`: Jellyfin's
-- IndexNumberEnd. NULL for other items. Episodes stored before this get theirs at the next scan.
ALTER TABLE media_items ADD COLUMN index_number_end INTEGER;

-- Jellyfin's LocationType: 'Virtual' for a season with no folder of its own, made for the episode files
-- in its series' folder, which has no Path then; 'FileSystem' for the rest. The season keeps its made-up
-- path here, as the scan finds it by that. Seasons stored before this get theirs at the next scan.
ALTER TABLE media_items ADD COLUMN location_type TEXT NOT NULL DEFAULT 'FileSystem';
