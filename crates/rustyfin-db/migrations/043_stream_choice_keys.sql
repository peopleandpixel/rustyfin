-- The key of the stream each remembered track index named when it was chosen (StreamIdentity::key
-- in rustyfin-model); NULL when none. An index is offered only where the stream at it has this key.
ALTER TABLE user_item_data ADD COLUMN audio_stream_key TEXT;
ALTER TABLE user_item_data ADD COLUMN subtitle_stream_key TEXT;
