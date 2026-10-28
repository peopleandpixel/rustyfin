-- Migration 020: Live TV tables

-- Tuner hosts (M3U, HDHomeRun, DVBT, etc.)
CREATE TABLE IF NOT EXISTS live_tv_tuner_hosts (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    type TEXT NOT NULL DEFAULT '',
    hostname TEXT NOT NULL DEFAULT '',
    port INTEGER NOT NULL DEFAULT 0,
    username TEXT NOT NULL DEFAULT '',
    password TEXT NOT NULL DEFAULT '',
    tuner_count INTEGER NOT NULL DEFAULT 0,
    is_ready INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Individual tuner instances
CREATE TABLE IF NOT EXISTS live_tv_tuners (
    id TEXT PRIMARY KEY NOT NULL,
    host_id TEXT NOT NULL REFERENCES live_tv_tuner_hosts(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    channel_url TEXT NOT NULL DEFAULT '',
    channel_id TEXT NOT NULL DEFAULT '',
    is_ready INTEGER NOT NULL DEFAULT 0,
    is_in_use INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_live_tv_tuners_host ON live_tv_tuners(host_id);

-- EPG listing providers (XMLTV, etc.)
CREATE TABLE IF NOT EXISTS live_tv_listing_providers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    type TEXT NOT NULL DEFAULT '',
    path TEXT NOT NULL DEFAULT '',
    is_ready INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Live TV channels
CREATE TABLE IF NOT EXISTS live_tv_channels (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    channel_number TEXT NOT NULL DEFAULT '',
    tuner_host_id TEXT REFERENCES live_tv_tuner_hosts(id) ON DELETE SET NULL,
    tuner_id TEXT REFERENCES live_tv_tuners(id) ON DELETE SET NULL,
    epg_id TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    logo_url TEXT NOT NULL DEFAULT '',
    is_radio INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_live_tv_channels_tuner_host ON live_tv_channels(tuner_host_id);
CREATE INDEX IF NOT EXISTS idx_live_tv_channels_tuner ON live_tv_channels(tuner_id);

-- EPG guide entries
CREATE TABLE IF NOT EXISTS live_tv_guide_entries (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL REFERENCES live_tv_channels(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    series_id TEXT NOT NULL DEFAULT '',
    season_display TEXT NOT NULL DEFAULT '',
    episode_display TEXT NOT NULL DEFAULT '',
    official_rating TEXT NOT NULL DEFAULT '',
    community_rating REAL,
    image_url TEXT NOT NULL DEFAULT '',
    genre TEXT NOT NULL DEFAULT '',
    is_series INTEGER NOT NULL DEFAULT 0,
    is_movie INTEGER NOT NULL DEFAULT 0,
    is_news INTEGER NOT NULL DEFAULT 0,
    is_live INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_live_tv_guide_entries_channel ON live_tv_guide_entries(channel_id);
CREATE INDEX IF NOT EXISTS idx_live_tv_guide_entries_start_time ON live_tv_guide_entries(start_time);
CREATE INDEX IF NOT EXISTS idx_live_tv_guide_entries_end_time ON live_tv_guide_entries(end_time);

-- One-time recording timers
CREATE TABLE IF NOT EXISTS live_tv_timers (
    id TEXT PRIMARY KEY NOT NULL,
    channel_id TEXT NOT NULL REFERENCES live_tv_channels(id) ON DELETE CASCADE,
    program_id TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    recording_path TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'Scheduled',
    recording_status TEXT NOT NULL DEFAULT 'Waiting',
    priority TEXT NOT NULL DEFAULT 'OneShot',
    keep_option TEXT NOT NULL DEFAULT 'Custom',
    max_record_days INTEGER NOT NULL DEFAULT 0,
    enable_email INTEGER NOT NULL DEFAULT 0,
    require_metadata INTEGER NOT NULL DEFAULT 0,
    ignore_missing_channels INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_live_tv_timers_channel ON live_tv_timers(channel_id);
CREATE INDEX IF NOT EXISTS idx_live_tv_timers_status ON live_tv_timers(status);

-- Recurring series timers
CREATE TABLE IF NOT EXISTS live_tv_series_timers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    series_id TEXT NOT NULL DEFAULT '',
    query TEXT NOT NULL DEFAULT '',
    search_until_time TEXT NOT NULL DEFAULT '',
    record_by_date TEXT NOT NULL DEFAULT '',
    ignore_words TEXT NOT NULL DEFAULT '',
    require_words TEXT NOT NULL DEFAULT '',
    recording_path TEXT NOT NULL DEFAULT '',
    priority TEXT NOT NULL DEFAULT 'OneShot',
    keep_option TEXT NOT NULL DEFAULT 'Custom',
    max_record_days INTEGER NOT NULL DEFAULT 0,
    enable_email INTEGER NOT NULL DEFAULT 0,
    command TEXT NOT NULL DEFAULT 'Create',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Recorded programs (stored as media items)
CREATE TABLE IF NOT EXISTS live_tv_recordings (
    id TEXT PRIMARY KEY NOT NULL,
    media_item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
    timer_id TEXT REFERENCES live_tv_timers(id) ON DELETE SET NULL,
    channel_id TEXT NOT NULL REFERENCES live_tv_channels(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    recording_path TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_live_tv_recordings_media_item ON live_tv_recordings(media_item_id);
CREATE INDEX IF NOT EXISTS idx_live_tv_recordings_timer ON live_tv_recordings(timer_id);
