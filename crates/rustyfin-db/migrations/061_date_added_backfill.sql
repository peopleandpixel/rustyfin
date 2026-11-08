-- Older versions dated each item by the time of its scan. This row marks the one-time backfill
-- that dates the items already stored by their files; the backfill deletes it when it finishes.
INSERT OR IGNORE INTO configuration (key, value) VALUES ('date_added_backfill', '');

-- The backfill moves each item with an UPDATE of date_created alone. The two triggers below
-- fired on every UPDATE of an item: the filter trigger recounted the item's year and rating
-- over the whole table, and the search trigger rewrote the item's search entry. They now fire
-- only when an UPDATE sets a column they read; their bodies are those of 027 and 029.
DROP TRIGGER IF EXISTS trg_media_items_update;
CREATE TRIGGER trg_media_items_update
AFTER UPDATE OF production_year, official_rating ON media_items
BEGIN
    -- Recount old year if it changed
    INSERT OR REPLACE INTO filter_year_index (year, item_count)
    SELECT OLD.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = OLD.production_year
    GROUP BY OLD.production_year;

    DELETE FROM filter_year_index WHERE year = OLD.production_year AND item_count = 0;

    -- Recount new year
    INSERT OR REPLACE INTO filter_year_index (year, item_count)
    SELECT NEW.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = NEW.production_year
    GROUP BY NEW.production_year;

    -- Recount old rating if it changed
    INSERT OR REPLACE INTO filter_rating_index (rating, item_count)
    SELECT OLD.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = OLD.official_rating
    GROUP BY OLD.official_rating;

    DELETE FROM filter_rating_index WHERE rating = OLD.official_rating AND item_count = 0;

    -- Recount new rating
    INSERT OR REPLACE INTO filter_rating_index (rating, item_count)
    SELECT NEW.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = NEW.official_rating
    GROUP BY NEW.official_rating;
END;

DROP TRIGGER IF EXISTS media_items_au;
CREATE TRIGGER media_items_au AFTER UPDATE OF name, overview, genres ON media_items BEGIN
    INSERT INTO media_items_fts(media_items_fts, rowid, name, overview, genres)
    VALUES ('delete', old.rowid, old.name, COALESCE(old.overview, ''), old.genres);
    INSERT INTO media_items_fts(rowid, name, overview, genres)
    VALUES (new.rowid, new.name, COALESCE(new.overview, ''), new.genres);
END;
