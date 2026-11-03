-- Optimization migration: composite indexes for common query patterns

-- Media items: composite indexes for the most common query patterns
-- get_items_by_library: WHERE library_id = ? ORDER BY sort_name, name
CREATE INDEX IF NOT EXISTS idx_media_items_lib_sort ON media_items(library_id, sort_name, name);

-- get_children / get_children_paged: WHERE parent_id = ? ORDER BY index_number, sort_name, name
CREATE INDEX IF NOT EXISTS idx_media_items_parent_order ON media_items(parent_id, index_number, sort_name, name);

-- get_items_filtered: WHERE library_id = ? AND media_type = ?
CREATE INDEX IF NOT EXISTS idx_media_items_lib_type ON media_items(library_id, media_type);

-- get_items_filtered: WHERE parent_id = ? AND media_type IN (...)
CREATE INDEX IF NOT EXISTS idx_media_items_parent_type ON media_items(parent_id, media_type);

-- get_item_by_path: WHERE path = ?
CREATE INDEX IF NOT EXISTS idx_media_items_path ON media_items(path);

-- get_items_filtered: WHERE production_year IN (...)
CREATE INDEX IF NOT EXISTS idx_media_items_year ON media_items(production_year);

-- get_latest_items: WHERE media_type = ? AND library_id = ? ORDER BY date_created DESC
CREATE INDEX IF NOT EXISTS idx_media_items_lib_created ON media_items(library_id, date_created DESC);

-- get_similar_items: WHERE media_type = ? AND library_id = ?
CREATE INDEX IF NOT EXISTS idx_media_items_type_lib ON media_items(media_type, library_id);

-- User item data: common filters for "continue watching" and "unplayed" queries
-- Queries like: WHERE user_id = ? AND played = 0
CREATE INDEX IF NOT EXISTS idx_user_item_data_user_played ON user_item_data(user_id, played);

-- Queries like: WHERE user_id = ? AND is_favorite = 1
CREATE INDEX IF NOT EXISTS idx_user_item_data_user_favorite ON user_item_data(user_id, is_favorite);

-- Queries like: WHERE user_id = ? ORDER BY last_played_date DESC
CREATE INDEX IF NOT EXISTS idx_user_item_data_user_last_played ON user_item_data(user_id, last_played_date DESC);

-- Media sources: queries often filter by item_id and check specific properties
CREATE INDEX IF NOT EXISTS idx_media_sources_item_protocol ON media_sources(item_id, protocol);

-- Media streams: queries often filter by source_id and codec_type together
CREATE INDEX IF NOT EXISTS idx_media_streams_source_type ON media_streams(source_id, codec_type);

-- Media streams: queries for default/forced subtitles
CREATE INDEX IF NOT EXISTS idx_media_streams_source_default ON media_streams(source_id, is_default, forced);

-- Sessions: cleanup queries often filter by last_activity
CREATE INDEX IF NOT EXISTS idx_sessions_last_activity ON sessions(last_activity);

-- Devices: cleanup queries often filter by last_seen
CREATE INDEX IF NOT EXISTS idx_devices_last_seen ON devices(last_seen DESC);

-- Notifications: queries for unread notifications
CREATE INDEX IF NOT EXISTS idx_notifications_user_unread ON notifications(user_id, is_read);

-- Activity log: queries often filter by user and date range
CREATE INDEX IF NOT EXISTS idx_activity_log_user_date ON activity_log(user_id, date DESC);
