-- What a photo's header and EXIF block say: Jellyfin's Width, Height, CameraMake, CameraModel, Software,
-- ImageOrientation, ExposureTime, FocalLength, Aperture, IsoSpeedRating, Latitude, Longitude and Altitude.
-- One row per photo that a scan read, with NULL for what the file doesn't say; `stamp` is the file's
-- version when it was read. Photos stored before this are read at the next scan.
CREATE TABLE IF NOT EXISTS photo_metadata (
    item_id TEXT PRIMARY KEY NOT NULL,
    stamp INTEGER,
    width INTEGER,
    height INTEGER,
    camera_make TEXT,
    camera_model TEXT,
    software TEXT,
    orientation TEXT,
    exposure_time REAL,
    focal_length REAL,
    aperture REAL,
    iso_speed_rating INTEGER,
    latitude REAL,
    longitude REAL,
    altitude REAL,
    date_taken TEXT,
    FOREIGN KEY (item_id) REFERENCES media_items(id) ON DELETE CASCADE
);
