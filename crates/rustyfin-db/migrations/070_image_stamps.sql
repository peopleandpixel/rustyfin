-- Add the version of a local image's file to item_images, so that a replaced picture gets a new tag
-- and new resized copies. NULL for remote images and for rows not stamped yet.
ALTER TABLE item_images ADD COLUMN stamp INTEGER;
