CREATE TABLE IF NOT EXISTS user_item_data (
    user_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    playback_position_ticks INTEGER NOT NULL DEFAULT 0,
    play_count INTEGER NOT NULL DEFAULT 0,
    is_favorite INTEGER NOT NULL DEFAULT 0,
    played INTEGER NOT NULL DEFAULT 0,
    last_played_date TEXT,
    PRIMARY KEY (user_id, item_id),
    FOREIGN KEY (user_id) REFERENCES users(id),
    FOREIGN KEY (item_id) REFERENCES media_items(id)
);
