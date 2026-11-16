use quick_xml::de::from_str;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
struct NfoMovie {
    title: Option<String>,
    #[serde(alias = "originaltitle")]
    original_title: Option<String>,
    sorttitle: Option<String>,
    plot: Option<String>,
    year: Option<i32>,
    rating: Option<f64>,
    #[serde(alias = "mpaa")]
    mpaa: Option<String>,
    runtime: Option<i32>,
    premiered: Option<String>,
    genre: Option<Vec<String>>,
    thumb: Option<Vec<NfoThumb>>,
    fanart: Option<NfoFanart>,
    uniqueid: Option<Vec<NfoUniqueId>>,
    imdbid: Option<String>,
    tmdbid: Option<String>,
    tvdbid: Option<String>,
}

/// `<uniqueid type="tmdb">603</uniqueid>`, as Kodi and Jellyfin write them.
#[derive(Debug, Deserialize, Default)]
struct NfoUniqueId {
    #[serde(rename = "@type")]
    kind: Option<String>,
    #[serde(rename = "$text")]
    value: Option<String>,
}

impl NfoMovie {
    /// The id of `kind` ("tmdb", "imdb", "tvdb") from `<uniqueid>` or the older `<tmdbid>` tags.
    fn provider_id(&self, kind: &str) -> Option<String> {
        let unique = self.uniqueid.iter().flatten().find_map(|id| {
            id.kind
                .as_deref()
                .filter(|k| k.eq_ignore_ascii_case(kind))
                .and(id.value.as_deref())
        });
        let legacy = match kind {
            "imdb" => self.imdbid.as_deref(),
            "tmdb" => self.tmdbid.as_deref(),
            _ => self.tvdbid.as_deref(),
        };
        unique
            .or(legacy)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    }
}

#[derive(Debug, Deserialize, Default)]
struct NfoThumb {
    #[serde(rename = "@aspect")]
    aspect: Option<String>,
    #[serde(rename = "$text")]
    url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct NfoFanart {
    thumb: Option<Vec<NfoFanartThumb>>,
}

#[derive(Debug, Deserialize, Default)]
struct NfoFanartThumb {
    #[serde(rename = "$text")]
    url: Option<String>,
}

/// Parse a .nfo file and extract metadata
pub async fn parse_nfo(nfo_path: &Path) -> Option<ExternalMetadata> {
    let content = tokio::fs::read_to_string(nfo_path).await.ok()?;

    // Try parsing as movie/tvshow NFO (XML)
    if let Ok(nfo) = from_str::<NfoMovie>(&content) {
        let mut images = Vec::new();

        if let Some(thumbs) = &nfo.thumb {
            for t in thumbs {
                if let Some(url) = &t.url {
                    let img_type = match t.aspect.as_deref() {
                        Some("poster") | None => "Primary",
                        Some("banner") => "Banner",
                        Some("landscape") => "Thumb",
                        Some(other) => other,
                    };
                    images.push(MetadataImage {
                        image_type: img_type.to_string(),
                        url: url.clone(),
                    });
                }
            }
        }

        if let Some(fanart) = &nfo.fanart
            && let Some(thumbs) = &fanart.thumb
        {
            for t in thumbs {
                if let Some(url) = &t.url {
                    images.push(MetadataImage {
                        image_type: "Backdrop".to_string(),
                        url: url.clone(),
                    });
                }
            }
        }

        let runtime_ticks = nfo.runtime.map(|m| m as i64 * 600_000_000);

        let tmdb_id = nfo.provider_id("tmdb").and_then(|v| v.parse().ok());
        let tvdb_id = nfo.provider_id("tvdb").and_then(|v| v.parse().ok());
        let imdb_id = nfo.provider_id("imdb").filter(|v| v.starts_with("tt"));
        return Some(ExternalMetadata {
            name: nfo.title.or(nfo.original_title),
            sort_title: nfo.sorttitle.filter(|s| !s.trim().is_empty()),
            overview: nfo.plot,
            production_year: nfo.year,
            community_rating: nfo.rating,
            official_rating: nfo.mpaa,
            genres: nfo.genre,
            premiere_date: nfo.premiered,
            runtime_ticks,
            image_urls: images,
            tmdb_id,
            tvdb_id,
            status: None,
            original_language: None,
            tagline: None,
            production_countries: None,
            budget: None,
            revenue: None,
            homepage: None,
            end_date: None,
            vote_count: None,
            popularity: None,
            studios: None,
            directors: None,
            writers: None,
            imdb_id,
            people: None,
        });
    }

    None
}

/// Find a .nfo file next to a media file. A series reads its own with `find_series_nfo` instead.
pub fn find_nfo_for_media(media_path: &Path) -> Option<std::path::PathBuf> {
    let stem = media_path.file_stem()?;
    let parent = media_path.parent()?;

    // Try same name with .nfo extension
    let nfo = parent.join(format!("{}.nfo", stem.to_string_lossy()));
    if nfo.exists() {
        return Some(nfo);
    }

    // Try movie.nfo in same directory. A tvshow.nfo there belongs to the series, not to its seasons.
    let p = parent.join("movie.nfo");
    if p.exists() {
        return Some(p);
    }

    None
}

/// The NFO inside a music folder: `album.nfo` in an album's, `artist.nfo` in an artist's, where
/// Jellyfin reads them (`AlbumNfoProvider`, `ArtistNfoProvider`).
pub fn find_folder_nfo(dir: &Path, file_name: &str) -> Option<std::path::PathBuf> {
    let p = dir.join(file_name);
    p.exists().then_some(p)
}

/// The `tvshow.nfo` inside the series folder `series_dir`, where Jellyfin 12.4 reads it
/// (`MediaBrowser.XbmcMetadata/Providers/SeriesNfoProvider.cs:61`).
pub fn find_series_nfo(series_dir: &Path) -> Option<std::path::PathBuf> {
    let p = series_dir.join("tvshow.nfo");
    p.exists().then_some(p)
}
