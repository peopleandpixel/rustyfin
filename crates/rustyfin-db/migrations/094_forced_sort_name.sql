-- Sort names made as Jellyfin makes them ("The Meridian" sorts as "meridian").

-- The sort name an admin, an NFO's <sorttitle> or a Jellyfin import forced, as written: Jellyfin's
-- ForcedSortName. media_items.sort_name stays the sort name in use, made from it when it is set.
ALTER TABLE item_metadata_extras ADD COLUMN forced_sort_name TEXT;

-- Until now a forced sort name lived only in media_items.sort_name, where an automatic one was the
-- lowercased name. One that is not keeps its value as the forced sort name. Episodes, seasons and
-- tracks are left out: their automatic sort names were made from the file name or the numbers.
-- SQLite's lower() folds only ASCII, so names with other characters are left to the startup pass
-- (repo_media::recompute_sort_names), which compares with Rust's lowercasing; the key below asks
-- it to.
INSERT INTO item_metadata_extras (item_id, forced_sort_name)
    SELECT id, sort_name FROM media_items
    WHERE sort_name IS NOT NULL AND sort_name != ''
      AND sort_name != lower(name)
      AND name NOT GLOB '*[^ -~]*'
      AND media_type NOT IN ('Episode', 'Season', 'Audio')
ON CONFLICT(item_id) DO UPDATE SET forced_sort_name = excluded.forced_sort_name;

INSERT OR REPLACE INTO configuration (key, value) VALUES ('sort_name_legacy_check', '1');
