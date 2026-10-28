use rustyfin_model::metadata::MetadataImage;
use serde::Deserialize;

const FANART_BASE: &str = "https://webservice.fanart.tv/v3";

/// Single fanart.tv image entry
#[derive(Debug, Deserialize)]
struct FanartImage {
    #[allow(dead_code)]
    id: String,
    url: String,
    #[allow(dead_code)]
    lang: Option<String>,
    #[allow(dead_code)]
    likes: Option<String>,
}

/// Fanart.tv movie response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FanartMovie {
    hdmovielogo: Option<Vec<FanartImage>>,
    movieposter: Option<Vec<FanartImage>>,
    moviebackground: Option<Vec<FanartImage>>,
    movie4kbackground: Option<Vec<FanartImage>>,
    hdmovieclearart: Option<Vec<FanartImage>>,
    moviebanner: Option<Vec<FanartImage>>,
    moviethumb: Option<Vec<FanartImage>>,
    moviedisc: Option<Vec<FanartImage>>,
    movielogo: Option<Vec<FanartImage>>,
    movieart: Option<Vec<FanartImage>>,
}

/// Fanart.tv TV show response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FanartShow {
    hdtvlogo: Option<Vec<FanartImage>>,
    clearlogo: Option<Vec<FanartImage>>,
    hdclearart: Option<Vec<FanartImage>>,
    showbackground: Option<Vec<FanartImage>>,
    show4kbackground: Option<Vec<FanartImage>>,
    tvthumb: Option<Vec<FanartImage>>,
    seasonposter: Option<Vec<FanartImage>>,
    tvbanner: Option<Vec<FanartImage>>,
    characterart: Option<Vec<FanartImage>>,
}

/// Fetch fanart.tv images for a movie by TMDB ID
pub async fn get_movie_images(api_key: &str, tmdb_movie_id: i64) -> Vec<MetadataImage> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/movie/{}?api_key={}",
        FANART_BASE, tmdb_movie_id, api_key
    );

    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(movie) = resp.json::<FanartMovie>().await
    {
        // Posters -> Primary
        if let Some(posters) = movie.movieposter {
            for (i, p) in posters.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Primary", i),
                    url: p.url,
                });
            }
        }

        // Backgrounds -> Backdrop (prefer 4K, fall back to 1080p)
        let backdrops: Vec<_> = movie
            .movie4kbackground
            .unwrap_or_default()
            .into_iter()
            .chain(movie.moviebackground.into_iter().flatten())
            .collect();
        for (i, b) in backdrops.into_iter().enumerate() {
            result.push(MetadataImage {
                image_type: image_type_str("Backdrop", i),
                url: b.url,
            });
        }

        // HD Logos -> Logo
        if let Some(logos) = movie.hdmovielogo.or(movie.movielogo)
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: l.url,
            });
        }

        // ClearArt -> Art
        if let Some(arts) = movie.hdmovieclearart
            && let Some(a) = arts.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Art".to_string(),
                url: a.url,
            });
        }

        // Banners -> Banner
        if let Some(banners) = movie.moviebanner
            && let Some(b) = banners.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Banner".to_string(),
                url: b.url,
            });
        }

        // Disc art -> Disc
        if let Some(discs) = movie.moviedisc
            && let Some(d) = discs.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Disc".to_string(),
                url: d.url,
            });
        }

        // Thumbnails -> Thumb
        if let Some(thumbs) = movie.moviethumb {
            for (i, t) in thumbs.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Thumb", i),
                    url: t.url,
                });
            }
        }
    }

    result
}

/// Fetch fanart.tv images for a TV show by TheTVDB ID
pub async fn get_tv_show_images(api_key: &str, tvdb_id: i64) -> Vec<MetadataImage> {
    let client = reqwest::Client::new();
    let url = format!("{}/tv/{}?api_key={}", FANART_BASE, tvdb_id, api_key);

    let mut result = Vec::new();

    if let Ok(resp) = client.get(&url).send().await
        && let Ok(show) = resp.json::<FanartShow>().await
    {
        // Season posters -> Primary
        if let Some(posters) = show.seasonposter {
            for (i, p) in posters.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Primary", i),
                    url: p.url,
                });
            }
        }

        // Backgrounds -> Backdrop (prefer 4K)
        let backdrops: Vec<_> = show
            .show4kbackground
            .unwrap_or_default()
            .into_iter()
            .chain(show.showbackground.into_iter().flatten())
            .collect();
        for (i, b) in backdrops.into_iter().enumerate() {
            result.push(MetadataImage {
                image_type: image_type_str("Backdrop", i),
                url: b.url,
            });
        }

        // Logos -> Logo (prefer HD)
        if let Some(logos) = show.hdtvlogo.or(show.clearlogo)
            && let Some(l) = logos.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Logo".to_string(),
                url: l.url,
            });
        }

        // ClearArt -> Art
        if let Some(arts) = show.hdclearart
            && let Some(a) = arts.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Art".to_string(),
                url: a.url,
            });
        }

        // Banners -> Banner
        if let Some(banners) = show.tvbanner
            && let Some(b) = banners.into_iter().next()
        {
            result.push(MetadataImage {
                image_type: "Banner".to_string(),
                url: b.url,
            });
        }

        // Thumbnails -> Thumb
        if let Some(thumbs) = show.tvthumb {
            for (i, t) in thumbs.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Thumb", i),
                    url: t.url,
                });
            }
        }

        // Character art -> Screenshot (closest match)
        if let Some(chars) = show.characterart {
            for (i, c) in chars.into_iter().enumerate() {
                result.push(MetadataImage {
                    image_type: image_type_str("Screenshot", i),
                    url: c.url,
                });
            }
        }
    }

    result
}

/// Build an image_type string: "Type" for index 0, "Type_N" for others.
fn image_type_str(base: &str, index: usize) -> String {
    if index == 0 {
        base.to_string()
    } else {
        format!("{}_{}", base, index)
    }
}
