-- The filter trigger recounts an item's old and new rating each time its rating is written. With no
-- index on official_rating, each recount scanned media_items, so writing many items in one
-- transaction (the Jellyfin import) took time that grew with the square of the item count.
CREATE INDEX IF NOT EXISTS idx_media_items_official_rating ON media_items(official_rating);
