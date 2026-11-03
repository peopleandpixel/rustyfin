-- Fix local images that were incorrectly assigned from shared directories.
-- Deletes ALL local provider images so the next library scan re-populates them
-- with correct name matching. This fixes the bug where ~100 films showed the same
-- cover (e.g., "Saw-poster.jpg" was assigned to all movies in /share/videos/S/).
--
-- Provider images (tmdb, omdb, fanart) are kept intact.
-- Local images will be re-scanned on the next library scan with proper filename matching.

DELETE FROM item_images WHERE provider = 'local';
