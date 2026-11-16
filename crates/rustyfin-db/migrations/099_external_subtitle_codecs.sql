-- External subtitle files were stored with their file extension as the codec. Jellyfin reports
-- the codec ffprobe reads from the file, which the scan now stores too: `subrip` for .srt,
-- `webvtt` for .vtt and `ass` for .ass and .ssa. Renames the stored ones; no file is read.
UPDATE media_streams
SET codec = CASE lower(codec)
    WHEN 'srt' THEN 'subrip'
    WHEN 'vtt' THEN 'webvtt'
    WHEN 'ssa' THEN 'ass'
    ELSE codec
END
WHERE is_external = 1 AND lower(codec) IN ('srt', 'vtt', 'ssa');
