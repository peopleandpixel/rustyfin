-- 026: Auto-maintain materialized filter indexes via triggers
-- Keeps filter_year_index and filter_rating_index up-to-date on every
-- INSERT/UPDATE/DELETE to media_items.
-- Note: filter_genre_index is NOT auto-maintained by triggers because
-- SQLite cannot easily split pipe-delimited strings in triggers.
-- It is rebuilt by rebuild_filter_indexes() after scans.

-- Trigger: AFTER INSERT on media_items (year + rating only)
CREATE TRIGGER IF NOT EXISTS trg_media_items_insert
AFTER INSERT ON media_items
BEGIN
    -- Update year index
    INSERT OR REPLACE INTO filter_year_index (year, item_count)
    SELECT NEW.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = NEW.production_year
    GROUP BY NEW.production_year;

    -- Update rating index
    INSERT OR REPLACE INTO filter_rating_index (rating, item_count)
    SELECT NEW.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = NEW.official_rating
    GROUP BY NEW.official_rating;
END;

-- Trigger: AFTER DELETE on media_items
CREATE TRIGGER IF NOT EXISTS trg_media_items_delete
AFTER DELETE ON media_items
BEGIN
    -- Recount year index for deleted item's year
    INSERT OR REPLACE INTO filter_year_index (year, item_count)
    SELECT OLD.production_year, COUNT(*)
    FROM media_items
    WHERE production_year = OLD.production_year
    GROUP BY OLD.production_year;

    -- Delete year entry if count is 0
    DELETE FROM filter_year_index WHERE year = OLD.production_year AND item_count = 0;

    -- Recount rating index for deleted item's rating
    INSERT OR REPLACE INTO filter_rating_index (rating, item_count)
    SELECT OLD.official_rating, COUNT(*)
    FROM media_items
    WHERE official_rating = OLD.official_rating
    GROUP BY OLD.official_rating;

    -- Delete rating entry if count is 0
    DELETE FROM filter_rating_index WHERE rating = OLD.official_rating AND item_count = 0;
END;

-- Trigger: AFTER UPDATE on media_items
CREATE TRIGGER IF NOT EXISTS trg_media_items_update
AFTER UPDATE ON media_items
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
