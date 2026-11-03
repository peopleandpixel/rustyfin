-- Add blurhash column to item_images for pre-computed blurhash values
-- This avoids computing blurhash on every item detail request
ALTER TABLE item_images ADD COLUMN blurhash TEXT;
