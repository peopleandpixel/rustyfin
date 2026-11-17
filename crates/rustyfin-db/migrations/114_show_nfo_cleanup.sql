-- Older scans gave a season, and an episode file directly in its series' folder, the overview,
-- year, ratings, genres, runtime and images of the series' tvshow.nfo. The scan clears what
-- they still hold of that file once (scan_sync::settle_series), but only on the items stored
-- before this version: the ones listed here. A season or episode stored later never took the
-- file's values, so a value that equals the file's is its own and is kept.
CREATE TABLE show_nfo_cleanup (
    item_id TEXT PRIMARY KEY NOT NULL REFERENCES media_items(id) ON DELETE CASCADE
);

INSERT INTO show_nfo_cleanup (item_id)
SELECT id FROM media_items WHERE media_type IN ('Season', 'Episode');
