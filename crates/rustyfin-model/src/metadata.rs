use serde::{Deserialize, Serialize};

/// Metadata fetched from external providers or local NFO files
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalMetadata {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub production_year: Option<i32>,
    pub community_rating: Option<f64>,
    pub official_rating: Option<String>,
    pub genres: Option<Vec<String>>,
    pub premiere_date: Option<String>,
    pub runtime_ticks: Option<i64>,
    /// URLs for images (poster, backdrop, etc.)
    pub image_urls: Vec<MetadataImage>,
    /// TMDb ID for the item, used to fetch cast/crew photos
    pub tmdb_id: Option<i64>,
    /// TheTVDB ID for the item, used to fetch fanart.tv images for TV shows
    pub tvdb_id: Option<i64>,
    /// Series/Show status (e.g. "Returning Series", "Ended", "Released", "Canceled")
    pub status: Option<String>,
    /// Original language code (e.g. "en", "de", "fr")
    pub original_language: Option<String>,
    /// Tagline / short promotional text
    pub tagline: Option<String>,
    /// Production countries (ISO 3166-1 alpha-2 codes, stored as JSON array string)
    pub production_countries: Option<Vec<String>>,
    /// Budget in USD (movies)
    pub budget: Option<i64>,
    /// Revenue in USD (movies)
    pub revenue: Option<i64>,
    /// Official homepage URL
    pub homepage: Option<String>,
    /// End date for TV series
    pub end_date: Option<String>,
    /// Number of votes for the rating
    pub vote_count: Option<i32>,
    /// Popularity score from TMDb
    pub popularity: Option<f64>,
    /// Studios (production companies)
    pub studios: Option<Vec<String>>,
    /// Directors
    pub directors: Option<Vec<String>>,
    /// Writers
    pub writers: Option<Vec<String>>,
    /// IMDb ID
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataImage {
    pub image_type: String,
    pub url: String,
}
