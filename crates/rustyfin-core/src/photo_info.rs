//! Reads what Jellyfin's photo provider reads from a photo: its size from the image header, and the
//! camera, orientation, exposure, place and date from its EXIF block. Nothing is decoded: the size
//! comes from the header in the first `image_proxy::HEADER_READ_MAX_BYTES` (64 KiB) of the file, and
//! the EXIF block from the first `EXIF_READ_MAX_BYTES` (256 KiB), so a huge or malformed file costs
//! two bounded reads.

use std::io::Read;
use std::path::Path;

use chrono::{NaiveDate, TimeZone, Utc};
use exif::{In, Tag, Value};
use rustyfin_model::photo::{PhotoInfo, orientation_name};

/// The most bytes of a photo read for its EXIF block. A JPEG keeps it in an APP1 segment of at most
/// 64 KiB near the start of the file.
pub const EXIF_READ_MAX_BYTES: u64 = 256 << 10;

/// A photo's file version (`image_proxy::file_stamp`) and its size, read from the first
/// `image_proxy::HEADER_READ_MAX_BYTES` of the file and nothing more. The stamp is `None` when the
/// file's metadata can't be read; the size is `None` when the header isn't in those bytes.
pub fn read_photo_size(path: &Path) -> (Option<i64>, Option<i32>, Option<i32>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (None, None, None);
    };
    let size = crate::image_proxy::header_dimensions(path);
    let (width, height) = match size {
        Some((w, h)) => (i32::try_from(w).ok(), i32::try_from(h).ok()),
        None => (None, None),
    };
    (Some(crate::image_proxy::file_stamp(&meta)), width, height)
}

/// A photo's file version (`image_proxy::file_stamp`) and what it says. The stamp is `None` when the
/// file's metadata can't be read, and then the info is empty.
pub fn read_photo(path: &Path) -> (Option<i64>, PhotoInfo) {
    let (stamp, width, height) = read_photo_size(path);
    if stamp.is_none() {
        return (None, PhotoInfo::default());
    }
    let mut head = Vec::new();
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(EXIF_READ_MAX_BYTES).read_to_end(&mut head));
    let mut info = if read.is_ok() {
        exif_info(&head)
    } else {
        PhotoInfo::default()
    };
    info.width = width;
    info.height = height;
    (stamp, info)
}

/// What the EXIF block in `bytes` (the start of an image file) says. Width and height are left out.
/// Anything missing or malformed is `None`.
pub fn exif_info(bytes: &[u8]) -> PhotoInfo {
    let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(bytes)) else {
        return PhotoInfo::default();
    };
    let field = |tag| exif.get_field(tag, In::PRIMARY).map(|f| &f.value);
    let text = |tag| field(tag).and_then(ascii);
    let rational = |tag| field(tag).and_then(first_rational);

    let latitude = field(Tag::GPSLatitude)
        .and_then(degrees)
        .map(|deg| signed(deg, text(Tag::GPSLatitudeRef).as_deref() == Some("S")));
    let longitude = field(Tag::GPSLongitude)
        .and_then(degrees)
        .map(|deg| signed(deg, text(Tag::GPSLongitudeRef).as_deref() == Some("W")));
    let altitude = rational(Tag::GPSAltitude).map(|alt| {
        let below = field(Tag::GPSAltitudeRef).and_then(|v| v.get_uint(0)) == Some(1);
        signed(alt, below)
    });

    PhotoInfo {
        width: None,
        height: None,
        camera_make: text(Tag::Make),
        camera_model: text(Tag::Model),
        software: text(Tag::Software),
        orientation: field(Tag::Orientation)
            .and_then(|v| v.get_uint(0))
            .and_then(orientation_name)
            .map(str::to_string),
        exposure_time: rational(Tag::ExposureTime),
        focal_length: rational(Tag::FocalLength),
        // Jellyfin reads the APEX ApertureValue, not FNumber.
        aperture: rational(Tag::ApertureValue),
        iso_speed_rating: field(Tag::PhotographicSensitivity)
            .and_then(|v| v.get_uint(0))
            .and_then(|iso| i32::try_from(iso).ok()),
        latitude,
        longitude,
        altitude,
        date_taken: text(Tag::DateTimeOriginal)
            .or_else(|| text(Tag::DateTime))
            .and_then(|s| parse_exif_date(&s)),
    }
}

/// `value` negated when `negative`.
fn signed(value: f64, negative: bool) -> f64 {
    if negative { -value } else { value }
}

/// The first string of an ASCII value, trimmed; `None` when it is empty.
fn ascii(value: &Value) -> Option<String> {
    let Value::Ascii(parts) = value else {
        return None;
    };
    let text = String::from_utf8_lossy(parts.first()?);
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!text.is_empty()).then(|| text.to_string())
}

/// The first rational of a value as a finite number.
fn first_rational(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Rational(r) => r.first()?.to_f64(),
        Value::SRational(r) => r.first()?.to_f64(),
        _ => return None,
    };
    number.is_finite().then_some(number)
}

/// Degrees from a GPS degrees, minutes, seconds triple, added up as Jellyfin's TagLib# does, so the
/// value is the same double.
fn degrees(value: &Value) -> Option<f64> {
    let Value::Rational(dms) = value else {
        return None;
    };
    let [d, m, s] = dms.get(..3)? else {
        return None;
    };
    let deg = d.to_f64() + m.to_f64() / 60.0 + s.to_f64() / 3600.0;
    deg.is_finite().then_some(deg)
}

/// An EXIF date, `YYYY:MM:DD HH:MM:SS`, read as UTC since EXIF gives no zone.
fn parse_exif_date(text: &str) -> Option<chrono::DateTime<Utc>> {
    let dt = exif::DateTime::from_ascii(text.as_bytes()).ok()?;
    let date = NaiveDate::from_ymd_opt(i32::from(dt.year), u32::from(dt.month), u32::from(dt.day))?;
    let time = date.and_hms_opt(
        u32::from(dt.hour),
        u32::from(dt.minute),
        u32::from(dt.second),
    )?;
    Some(Utc.from_utc_datetime(&time))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An APP1 EXIF segment, little-endian: IFD0 with Make, Model, Orientation and DateTime, and a
    /// GPS IFD with latitude and longitude, as the parity check's synthetic photos carry.
    fn app1(orientation: u16, lat: (u32, u32, u32, &str), lon: (u32, u32, u32, &str)) -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]) {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(value);
        }
        fn at(n: usize) -> [u8; 4] {
            (n as u32).to_le_bytes()
        }
        let make = b"Canon\0";
        let model = b"Canon EOS 5D\0";
        let date = b"2018:06:01 10:15:00\0";
        // Layout: header 8, IFD0 (2 + 6*12 + 4 = 78) at 8, GPS IFD (2 + 4*12 + 4 = 54) at 86, then data.
        let ifd0_at = 8;
        let gps_at = ifd0_at + 78;
        let mut data_at = gps_at + 54;
        let mut data = Vec::new();
        let mut put = |bytes: &[u8]| {
            let offset = data_at;
            data.extend_from_slice(bytes);
            data_at += bytes.len();
            offset
        };
        let make_at = put(make);
        let model_at = put(model);
        let date_at = put(date);
        let rationals = |(d, m, s, _): (u32, u32, u32, &str)| {
            [d, 1, m, 1, s, 100]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let lat_at = put(&rationals(lat));
        let lon_at = put(&rationals(lon));

        let mut tiff = b"II*\0".to_vec();
        tiff.extend(at(ifd0_at));
        tiff.extend(6u16.to_le_bytes());
        entry(&mut tiff, 0x010F, 2, make.len() as u32, at(make_at));
        entry(&mut tiff, 0x0110, 2, model.len() as u32, at(model_at));
        let mut short = [0u8; 4];
        short[..2].copy_from_slice(&orientation.to_le_bytes());
        entry(&mut tiff, 0x0112, 3, 1, short);
        entry(&mut tiff, 0x0132, 2, date.len() as u32, at(date_at));
        entry(&mut tiff, 0x8825, 4, 1, at(gps_at));
        // An empty Exif IFD pointer is left out; one more entry keeps the count at 6.
        entry(&mut tiff, 0x0131, 2, 4, *b"rf\0\0");
        tiff.extend(0u32.to_le_bytes());
        tiff.extend(4u16.to_le_bytes());
        entry(&mut tiff, 0x0001, 2, 2, [lat.3.as_bytes()[0], 0, 0, 0]);
        entry(&mut tiff, 0x0002, 5, 3, at(lat_at));
        entry(&mut tiff, 0x0003, 2, 2, [lon.3.as_bytes()[0], 0, 0, 0]);
        entry(&mut tiff, 0x0004, 5, 3, at(lon_at));
        tiff.extend(0u32.to_le_bytes());
        tiff.extend(data);

        let mut segment = b"Exif\0\0".to_vec();
        segment.extend(tiff);
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
        out.extend(((segment.len() + 2) as u16).to_be_bytes());
        out.extend(segment);
        out
    }

    #[test]
    fn r2photos_exif_fields_match_jellyfin() {
        // 48.8584 N, 2.2945 E as the synthetic photo writes them: 48 51' 30.24", 2 17' 40.20"
        let jpeg = app1(1, (48, 51, 3024, "N"), (2, 17, 4020, "E"));
        let info = exif_info(&jpeg);
        assert_eq!(info.camera_make.as_deref(), Some("Canon"));
        assert_eq!(info.camera_model.as_deref(), Some("Canon EOS 5D"));
        assert_eq!(info.software.as_deref(), Some("rf"));
        assert_eq!(info.orientation.as_deref(), Some("TopLeft"));
        // The same doubles Jellyfin 12.4 answers with.
        assert_eq!(info.latitude, Some(48.8584));
        assert_eq!(info.longitude, Some(2.2944999999999998));
        assert_eq!(
            info.date_taken.map(|d| d.to_rfc3339()).as_deref(),
            Some("2018-06-01T10:15:00+00:00")
        );
    }

    #[test]
    fn r2photos_south_and_west_are_negative_and_orientation_is_named() {
        // -33.8568, 151.2153: 33 51' 24.48" S, 151 12' 55.08" E
        let jpeg = app1(8, (33, 51, 2448, "S"), (151, 12, 5508, "W"));
        let info = exif_info(&jpeg);
        assert_eq!(info.latitude, Some(-33.8568));
        assert_eq!(info.longitude, Some(-151.21529999999998));
        assert_eq!(info.orientation.as_deref(), Some("LeftBottom"));
    }

    #[test]
    fn r2photos_missing_or_malformed_exif_gives_nothing() {
        assert_eq!(exif_info(b""), PhotoInfo::default());
        assert_eq!(exif_info(&[0xFF, 0xD8, 0xFF, 0xD9]), PhotoInfo::default());
        // A segment that claims more than there is, and garbage after the marker.
        let mut cut = app1(1, (1, 2, 3, "N"), (4, 5, 6, "E"));
        cut.truncate(40);
        assert_eq!(exif_info(&cut), PhotoInfo::default());
        let junk: Vec<u8> = (0..4096u32).map(|i| (i * 7919 % 251) as u8).collect();
        assert_eq!(exif_info(&junk), PhotoInfo::default());
    }

    /// A temp dir, removed when the test ends, whether it passed or not.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn r2photos_read_photo_gives_the_header_size_and_reads_a_bounded_head() {
        let temp = TempDir(std::env::temp_dir().join(format!("r2photos-{}", uuid::Uuid::new_v4())));
        let dir = &temp.0;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("p.jpg");
        image::RgbImage::from_pixel(30, 20, image::Rgb([9, 9, 9]))
            .save(&path)
            .unwrap();
        let (stamp, info) = read_photo(&path);
        assert!(stamp.is_some());
        assert_eq!((info.width, info.height), (Some(30), Some(20)));
        assert_eq!(info.camera_make, None);
        let (stamp, info) = read_photo(&dir.join("missing.jpg"));
        assert_eq!((stamp, info), (None, PhotoInfo::default()));
    }
}
