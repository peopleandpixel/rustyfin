-- Migration 016: Add external ID columns for metadata provider integration
-- tmdb_id: TMDb ID for movies/TV shows (used for cast photos, fanart.tv movies)
-- tvdb_id: TheTVDB ID for TV shows (used for fanart.tv TV images)
ALTER TABLE media_items ADD COLUMN tmdb_id INTEGER;
ALTER TABLE media_items ADD COLUMN tvdb_id INTEGER;
