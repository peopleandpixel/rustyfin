-- Saving an item now updates it in place with INSERT ... ON CONFLICT(id) DO UPDATE; INSERT OR
-- REPLACE deleted it first, and the delete cascaded to the rows that reference it. SQLite runs
-- that update with ABORT on conflict, which overrides the OR REPLACE of the statements in this
-- trigger, so recounting a year or rating already counted failed with "UNIQUE constraint failed".
-- The recounts now say what to do on a conflict themselves.
--
-- When the last item with a year or rating moved to another one or was deleted, the recount of
-- the old value selected no row: its count stayed, item_count = 0 never matched, and the filters
-- listed the value until the next scan rebuilt them. Both triggers now delete a value once no item
-- has it. Otherwise trg_media_items_update is that of 061 and trg_media_items_delete that of 027.
DROP TRIGGER IF EXISTS trg_media_items_update;
CREATE TRIGGER trg_media_items_update
AFTER UPDATE OF production_year, official_rating ON media_items
BEGIN
    -- Recount old year if it changed
    INSERT INTO filter_year_index (year, item_count)
    SELECT OLD.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = OLD.production_year
    GROUP BY OLD.production_year
    ON CONFLICT(year) DO UPDATE SET item_count = excluded.item_count;

    DELETE FROM filter_year_index
    WHERE year = OLD.production_year
      AND NOT EXISTS (SELECT 1 FROM media_items WHERE production_year = OLD.production_year);

    -- Recount new year
    INSERT INTO filter_year_index (year, item_count)
    SELECT NEW.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = NEW.production_year
    GROUP BY NEW.production_year
    ON CONFLICT(year) DO UPDATE SET item_count = excluded.item_count;

    -- Recount old rating if it changed
    INSERT INTO filter_rating_index (rating, item_count)
    SELECT OLD.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = OLD.official_rating
    GROUP BY OLD.official_rating
    ON CONFLICT(rating) DO UPDATE SET item_count = excluded.item_count;

    DELETE FROM filter_rating_index
    WHERE rating = OLD.official_rating
      AND NOT EXISTS (SELECT 1 FROM media_items WHERE official_rating = OLD.official_rating);

    -- Recount new rating
    INSERT INTO filter_rating_index (rating, item_count)
    SELECT NEW.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = NEW.official_rating
    GROUP BY NEW.official_rating
    ON CONFLICT(rating) DO UPDATE SET item_count = excluded.item_count;
END;

DROP TRIGGER IF EXISTS trg_media_items_delete;
CREATE TRIGGER trg_media_items_delete
AFTER DELETE ON media_items
BEGIN
    -- Recount year index for deleted item's year
    INSERT OR REPLACE INTO filter_year_index (year, item_count)
    SELECT OLD.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = OLD.production_year
    GROUP BY OLD.production_year;

    -- Delete year entry once no item has that year
    DELETE FROM filter_year_index
    WHERE year = OLD.production_year
      AND NOT EXISTS (SELECT 1 FROM media_items WHERE production_year = OLD.production_year);

    -- Recount rating index for deleted item's rating
    INSERT OR REPLACE INTO filter_rating_index (rating, item_count)
    SELECT OLD.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = OLD.official_rating
    GROUP BY OLD.official_rating;

    -- Delete rating entry once no item has that rating
    DELETE FROM filter_rating_index
    WHERE rating = OLD.official_rating
      AND NOT EXISTS (SELECT 1 FROM media_items WHERE official_rating = OLD.official_rating);
END;

-- Each save with INSERT OR REPLACE gave the item a new rowid, and the search entry for its old name,
-- overview and genres stayed at the old rowid. Rebuilding the search index from media_items once
-- drops those entries.
INSERT INTO media_items_fts(media_items_fts) VALUES('rebuild');
