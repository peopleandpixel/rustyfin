-- Migration 015: Covering index for library grid view
-- Eliminates table lookups for the most common query: browsing a library with media_type filter
-- The index contains all columns needed for the grid view, so SQLite can answer the query
-- using only the index (Index-Only Scan), avoiding reading the large overview column.

-- Covering index for: SELECT * FROM media_items WHERE library_id = ? AND media_type = ? ORDER BY sort_name
-- Includes the most frequently displayed columns for the library grid view
CREATE INDEX IF NOT EXISTS idx_media_items_lib_type_covering 
ON media_items(library_id, media_type, sort_name, id, name, production_year, community_rating, date_created);

-- Covering index for: get_children WHERE parent_id = ? ORDER BY index_number
CREATE INDEX IF NOT EXISTS idx_media_items_parent_covering 
ON media_items(parent_id, index_number, id, name, media_type, production_year, community_rating);
