-- The lyrics of each audio item that has some, which its HasLyrics is read from: the lyric file
-- beside the track that the last scan found (`.lrc`, `.elrc` or `.txt` named like the track), a
-- lyric file uploaded into the server's data folder, and whether the track's tags hold lyrics.
-- A row with none of the three is deleted.
CREATE TABLE IF NOT EXISTS item_lyrics (
    item_id TEXT PRIMARY KEY NOT NULL,
    sidecar_path TEXT,
    uploaded_path TEXT,
    embedded INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);
