use quick_xml::de::from_str;
use rustyfin_model::metadata::{ExternalMetadata, MetadataImage};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
struct NfoMovie {
    title: Option<String>,
    #[serde(alias = "originaltitle")]
    original_title: Option<String>,
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

        return Some(ExternalMetadata {
            name: nfo.title.or(nfo.original_title),
            overview: nfo.plot,
            production_year: nfo.year,
            community_rating: nfo.rating,
            official_rating: nfo.mpaa,
            genres: nfo.genre,
            premiere_date: nfo.premiered,
            runtime_ticks,
            image_urls: images,
            tmdb_id: None,
        });
    }

    None
}

/// Find a .nfo file next to a media file
pub fn find_nfo_for_media(media_path: &Path) -> Option<std::path::PathBuf> {
    let stem = media_path.file_stem()?;
    let parent = media_path.parent()?;

    // Try same name with .nfo extension
    let nfo = parent.join(format!("{}.nfo", stem.to_string_lossy()));
    if nfo.exists() {
        return Some(nfo);
    }

    // Try movie.nfo or tvshow.nfo in same directory
    for name in &["movie.nfo", "tvshow.nfo"] {
        let p = parent.join(name);
        if p.exists() {
            return Some(p);
        }
    }

    None
}
