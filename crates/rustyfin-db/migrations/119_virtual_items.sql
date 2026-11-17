-- Jellyfin 12.4's IsVirtualItem: an episode or season that the TMDb missing-episode provider made
-- from TMDb's episode list, with no file behind it (an unaired or a missing episode). It is not the
-- same as LocationType Virtual: a season with no folder has no path either, but it holds real
-- episodes and is not a virtual item. Listings hide virtual items where Jellyfin does, and a scan
-- never removes one as a missing file; a file that arrives for one takes its row over.
ALTER TABLE media_items ADD COLUMN is_virtual_item INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_media_items_virtual ON media_items(is_virtual_item) WHERE is_virtual_item = 1;
