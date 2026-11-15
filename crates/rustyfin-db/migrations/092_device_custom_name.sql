-- Jellyfin's device options: a name the administrator gives a device (GET/POST /Devices/Options).
-- Migration 022 meant to add more device columns, but 005 had made the table already, so its
-- CREATE TABLE IF NOT EXISTS never ran; the queries now use 005's columns.
ALTER TABLE devices ADD COLUMN custom_name TEXT;
