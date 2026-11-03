-- Fix image indices: ensure the first image of each type per item is at index 0.
-- This fixes the bug where provider images were stored starting at index 1,
-- causing 404 errors when the client requests index 0 (the default).

-- Re-index images using a window function to assign sequential indices per (item_id, image_type)
-- Ordered by provider priority: tmdb > omdb > fanart > local (prefer provider images at index 0)
UPDATE item_images SET image_index = new_idx FROM (
    SELECT 
        rowid,
        ROW_NUMBER() OVER (
            PARTITION BY item_id, image_type 
            ORDER BY 
                CASE provider 
                    WHEN 'tmdb' THEN 1 
                    WHEN 'omdb' THEN 2 
                    WHEN 'fanart' THEN 3 
                    WHEN 'local' THEN 4 
                    ELSE 5 
                END,
                image_index
        ) - 1 AS new_idx
    FROM item_images
) AS reindexed WHERE item_images.rowid = reindexed.rowid;
