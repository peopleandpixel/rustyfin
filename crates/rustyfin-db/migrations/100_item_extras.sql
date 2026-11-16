-- Trailers, special features and theme media (Jellyfin's extras). An extra is a media item of its
-- own, so it plays like any other, but it belongs to the movie, series or season in `owner_id` and
-- is never listed as a library item. `extra_type` is Jellyfin's ExtraType (Trailer, ThemeSong,
-- ThemeVideo, BehindTheScenes, DeletedScene, Interview, Scene, Sample, Short, Featurette, Clip,
-- Unknown); NULL for every other item.
-- `owner_id` has no foreign key (SQLite can't add one with ALTER TABLE): the code keeps it valid.
-- Deleting an item deletes its extras (repo_media::delete_item_rows_in), and each scan sets every
-- extra's owner again (rustyfin_core::extras).
ALTER TABLE media_items ADD COLUMN extra_type TEXT;
ALTER TABLE media_items ADD COLUMN owner_id TEXT;

CREATE INDEX IF NOT EXISTS idx_media_items_owner ON media_items(owner_id) WHERE owner_id IS NOT NULL;
