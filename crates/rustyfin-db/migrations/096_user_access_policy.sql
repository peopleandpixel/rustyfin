-- The UserPolicy fields on access that an administrator sets per user: library access, access
-- schedules, hidden, remote access, playback and transcoding switches, deletion, the session and
-- remote bitrate limits. One JSON object with Jellyfin's key names; a missing key has Jellyfin's
-- default for a new user. The old user_access_schedules table was never written.
ALTER TABLE users ADD COLUMN access_policy TEXT NOT NULL DEFAULT '{}';
