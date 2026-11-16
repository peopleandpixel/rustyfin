use serde::{Deserialize, Serialize};

/// One trickplay tile set of an item, as Jellyfin's `TrickplayInfoDto`: thumbnails `width` by
/// `height` pixels, one every `interval` milliseconds, laid out `tile_width` by `tile_height` in
/// each tile image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrickplayInfo {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub thumbnail_count: u32,
    /// Milliseconds between thumbnails
    pub interval: u32,
    /// The highest bits per second a tile image takes to load while it plays
    pub bandwidth: u32,
}

impl TrickplayInfo {
    /// Thumbnails in each full tile image.
    pub fn thumbnails_per_tile(&self) -> u32 {
        (self.tile_width * self.tile_height).max(1)
    }

    /// The number of tile images.
    pub fn tile_count(&self) -> u32 {
        self.thumbnail_count.div_ceil(self.thumbnails_per_tile())
    }

    /// The tile set's HLS image playlist, as Jellyfin writes it (`#EXT-X-IMAGES-ONLY`, one
    /// `#EXT-X-TILES` entry per tile image). Each tile's URL is `{index}.jpg` followed by `query`
    /// (empty, or starting with `?`). Empty when the set has no thumbnails.
    pub fn hls_playlist(&self, query: &str) -> String {
        if self.thumbnail_count == 0 {
            return String::new();
        }
        let per_tile = self.thumbnails_per_tile();
        let thumb_seconds = f64::from(self.interval) / 1000.0;
        let tiles = self.tile_count();
        let mut out = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:{tiles}\n#EXT-X-VERSION:7\n#EXT-X-MEDIA-SEQUENCE:1\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-IMAGES-ONLY\n"
        );
        for i in 0..tiles {
            let in_tile = if i == tiles - 1 {
                self.thumbnail_count - i * per_tile
            } else {
                per_tile
            };
            out.push_str(&format!(
                "#EXT-X-TILES:RESOLUTION={}x{},LAYOUT={}x{},DURATION={}\n#EXTINF:{},\n{i}.jpg{query}\n",
                self.width,
                self.height,
                self.tile_width,
                self.tile_height,
                short_decimal(thumb_seconds),
                short_decimal(thumb_seconds * f64::from(in_tile)),
            ));
        }
        out.push_str("#EXT-X-ENDLIST\n");
        out
    }
}

/// A number with at most three decimals and no trailing zeros, like .NET's `{0:0.###}`.
fn short_decimal(n: f64) -> String {
    let s = format!("{n:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(count: u32) -> TrickplayInfo {
        TrickplayInfo {
            width: 320,
            height: 180,
            tile_width: 10,
            tile_height: 10,
            thumbnail_count: count,
            interval: 10_000,
            bandwidth: 1000,
        }
    }

    #[test]
    fn r2chapters_trickplay_playlist_matches_jellyfin() {
        let got = info(150).hls_playlist("?MediaSourceId=abc");
        assert_eq!(
            got,
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-VERSION:7\n#EXT-X-MEDIA-SEQUENCE:1\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-IMAGES-ONLY\n\
             #EXT-X-TILES:RESOLUTION=320x180,LAYOUT=10x10,DURATION=10\n#EXTINF:1000,\n\
             0.jpg?MediaSourceId=abc\n\
             #EXT-X-TILES:RESOLUTION=320x180,LAYOUT=10x10,DURATION=10\n#EXTINF:500,\n\
             1.jpg?MediaSourceId=abc\n#EXT-X-ENDLIST\n"
        );
        assert_eq!(info(0).hls_playlist(""), "");
        assert_eq!(info(100).tile_count(), 1);
        assert_eq!(info(101).tile_count(), 2);
    }

    #[test]
    fn r2chapters_short_decimal() {
        assert_eq!(short_decimal(10.0), "10");
        assert_eq!(short_decimal(2.5), "2.5");
        assert_eq!(short_decimal(0.3333333), "0.333");
    }

    #[test]
    fn r2chapters_trickplay_json_is_jellyfins() {
        let v = serde_json::to_value(info(5)).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "Bandwidth",
                "Height",
                "Interval",
                "ThumbnailCount",
                "TileHeight",
                "TileWidth",
                "Width"
            ]
        );
    }
}
