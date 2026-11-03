-- Migration 021: Devices, Packages, SyncPlay, Backup, Dlna Streaming, Localization tables

-- Devices table for persistent device registration
CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    app_name TEXT NOT NULL DEFAULT '',
    app_version TEXT NOT NULL DEFAULT '',
    device_name TEXT NOT NULL DEFAULT '',
    last_activity_date TEXT NOT NULL DEFAULT (datetime('now')),
    image_path TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_devices_user ON devices(user_id);

-- Package repositories
CREATE TABLE IF NOT EXISTS package_repositories (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Installed packages
CREATE TABLE IF NOT EXISTS installed_packages (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '',
    repository_id TEXT REFERENCES package_repositories(id) ON DELETE SET NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    description TEXT NOT NULL DEFAULT '',
    installed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_installed_packages_repo ON installed_packages(repository_id);

-- SyncPlay groups
CREATE TABLE IF NOT EXISTS sync_play_groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    owner_user_id TEXT NOT NULL,
    is_locked INTEGER NOT NULL DEFAULT 0,
    play_method TEXT NOT NULL DEFAULT 'Sync',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- SyncPlay group members
CREATE TABLE IF NOT EXISTS sync_play_members (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL REFERENCES sync_play_groups(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    joined_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_sync_play_members_group ON sync_play_members(group_id);

-- DLNA streaming profiles
CREATE TABLE IF NOT EXISTS dlna_streaming_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    profile_xml TEXT NOT NULL DEFAULT '',
    container TEXT NOT NULL DEFAULT '',
    audio_codec TEXT NOT NULL DEFAULT '',
    video_codec TEXT NOT NULL DEFAULT '',
    direct_play_profiles TEXT NOT NULL DEFAULT '[]',
    transcoding_profiles TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Localization providers
CREATE TABLE IF NOT EXISTS localization_providers (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    provider_id TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    configuration TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Backup metadata
CREATE TABLE IF NOT EXISTS backups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    backup_path TEXT NOT NULL DEFAULT '',
    size_bytes INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
