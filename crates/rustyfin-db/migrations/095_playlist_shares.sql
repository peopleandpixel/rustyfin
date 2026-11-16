-- Jellyfin's playlist sharing: a public playlist (OpenAccess) every user may read, and the users
-- a playlist is shared with, each allowed to edit it or only to read it.
ALTER TABLE playlists ADD COLUMN open_access INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS playlist_shares (
    playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    can_edit INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (playlist_id, user_id)
);
