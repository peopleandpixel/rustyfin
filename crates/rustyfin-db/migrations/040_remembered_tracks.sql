-- Remembered tracks: the audio and subtitle stream a user last chose for an item, offered as the
-- defaults of its video PlaybackInfo. NULL means nothing is remembered; -1 subtitle means off.
ALTER TABLE user_item_data ADD COLUMN audio_stream_index INTEGER;
ALTER TABLE user_item_data ADD COLUMN subtitle_stream_index INTEGER;
