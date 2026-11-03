-- Playstate migration: user ratings and pinned episodes

-- Add user rating and likes columns to user_item_data
ALTER TABLE user_item_data ADD COLUMN user_rating INTEGER DEFAULT NULL;
ALTER TABLE user_item_data ADD COLUMN likes INTEGER DEFAULT NULL;

-- Create pinned_episodes table for pinning current episodes
CREATE TABLE IF NOT EXISTS pinned_episodes (
    user_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    episode_id TEXT NOT NULL,
    season_id TEXT,
    season_index INTEGER,
    episode_index INTEGER,
    created_date TEXT NOT NULL,
    PRIMARY KEY (user_id, series_id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (series_id) REFERENCES media_items(id),
    FOREIGN KEY (episode_id) REFERENCES media_items(id),
    FOREIGN KEY (season_id) REFERENCES media_items(id)
);

-- Index for quick lookup by user
CREATE INDEX IF NOT EXISTS idx_pinned_episodes_user ON pinned_episodes(user_id);
