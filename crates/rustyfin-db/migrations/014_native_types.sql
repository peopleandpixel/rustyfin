-- Migration 013: Convert JSON-serialized media_type and genres to native types
-- Before: media_type stored as "\"Movie\"", genres stored as "[\"Action\",\"Sci-Fi\"]"
-- After: media_type stored as "Movie", genres stored as "Action|Sci-Fi"

-- Convert media_type from JSON-serialized to plain text
UPDATE media_items SET media_type = REPLACE(REPLACE(media_type, '"', ''), '\\"', '')
WHERE media_type LIKE '"%"';

-- Convert genres from JSON array to delimited string
-- Handle JSON arrays like ["Action","Sci-Fi"] → Action|Sci-Fi
UPDATE media_items SET genres = (
    SELECT REPLACE(REPLACE(REPLACE(REPLACE(genres, '[', ''), ']', ''), '"', ''), ', ', '|')
)
WHERE genres LIKE '[%]';

-- Also handle empty arrays
UPDATE media_items SET genres = '' WHERE genres = '[]';
