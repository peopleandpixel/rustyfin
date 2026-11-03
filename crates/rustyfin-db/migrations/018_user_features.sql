-- Migration 017: User images and parental control access schedules

-- User profile images
CREATE TABLE IF NOT EXISTS user_images (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    image_type TEXT NOT NULL DEFAULT 'Primary',
    path TEXT NOT NULL,
    width INTEGER,
    height INTEGER,
    date_created TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_user_images_user_id ON user_images(user_id);

-- Parental control access schedules
CREATE TABLE IF NOT EXISTS user_access_schedules (
    id TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    day_of_week TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_user_access_schedules_user_id ON user_access_schedules(user_id);
