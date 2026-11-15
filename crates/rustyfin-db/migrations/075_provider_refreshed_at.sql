-- When a metadata provider last answered for the item (a movie or series identified or looked up by
-- id, a season or episode filled from its season). The "Refresh metadata" task and a series'
-- refresh skip items that have it, so an item TMDb has no premiere date for isn't asked again on
-- every run. NULL: never, or before this column existed (an upgraded library is asked once).
ALTER TABLE item_metadata_extras ADD COLUMN provider_refreshed_at TEXT;
