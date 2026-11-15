-- Parental limits per user, Jellyfin's UserPolicy fields of the same names. The lists are JSON
-- arrays of strings; a NULL maximum means no rating limit.
ALTER TABLE users ADD COLUMN max_parental_rating INTEGER;
ALTER TABLE users ADD COLUMN max_parental_sub_rating INTEGER;
ALTER TABLE users ADD COLUMN block_unrated_items TEXT NOT NULL DEFAULT '[]';
ALTER TABLE users ADD COLUMN blocked_tags TEXT NOT NULL DEFAULT '[]';
ALTER TABLE users ADD COLUMN allowed_tags TEXT NOT NULL DEFAULT '[]';
