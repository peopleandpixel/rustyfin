use chrono::{DateTime, Utc};

/// What a photo's header and EXIF block say, as Jellyfin's photo provider reads it. A field the file
/// doesn't give is `None`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoInfo {
    /// The picture's size in pixels as stored, before any EXIF rotation
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub software: Option<String>,
    /// Jellyfin's `ImageOrientation` name, such as "TopLeft" or "RightTop"
    pub orientation: Option<String>,
    /// Seconds
    pub exposure_time: Option<f64>,
    /// Millimetres
    pub focal_length: Option<f64>,
    /// The f-number
    pub aperture: Option<f64>,
    pub iso_speed_rating: Option<i32>,
    /// Degrees, south negative
    pub latitude: Option<f64>,
    /// Degrees, west negative
    pub longitude: Option<f64>,
    /// Metres, below sea level negative
    pub altitude: Option<f64>,
    /// When the photo was taken (EXIF `DateTimeOriginal`, else `DateTime`), read as UTC
    pub date_taken: Option<DateTime<Utc>>,
}

/// Jellyfin's `ImageOrientation` name of an EXIF orientation value (1-8); `None` for any other.
pub fn orientation_name(value: u32) -> Option<&'static str> {
    Some(match value {
        1 => "TopLeft",
        2 => "TopRight",
        3 => "BottomRight",
        4 => "BottomLeft",
        5 => "LeftTop",
        6 => "RightTop",
        7 => "RightBottom",
        8 => "LeftBottom",
        _ => return None,
    })
}
