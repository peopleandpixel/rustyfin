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
    /// Cast and crew, in the provider's billing order
    #[serde(default)]
    pub people: Option<Vec<MetadataPerson>>,
}

/// A cast or crew member from a metadata provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataPerson {
    pub name: String,
    /// Jellyfin's PersonKind: "Actor", "GuestStar", "Director", "Writer", "Producer"
    pub kind: String,
    /// The character for actors
    pub role: Option<String>,
    /// The person's photo
    pub image_url: Option<String>,
}

/// One season from a metadata provider, with the episodes it lists. One provider call fills a
/// season and all of its episodes.
#[derive(Debug, Clone, Default)]
pub struct SeasonMetadata {
    pub meta: ExternalMetadata,
    /// By episode number
    pub episodes: Vec<(i32, ExternalMetadata)>,
}

/// A lookup that got no final answer (a timeout, a 5xx, a rate limit, no API key), so it is worth
/// asking again later. `Ok(None)` is a final "not there" (a 404).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupFailed {
    /// A timeout, a 5xx, a rate limit, a network error: worth asking again
    Transient,
    /// No API key, or the provider refused it (401, 403): every other lookup will fail too
    Unauthorized,
}

/// The answer of a lookup: `Ok(Some(_))` found, `Ok(None)` not there, `Err` failed.
pub type LookupResult<T> = Result<Option<T>, LookupFailed>;

type LookupFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = LookupResult<T>> + Send + 'a>>;

/// Lookups a provider offers beyond the name searches: by provider id, a TV search with the
/// first-air year, and a season with its episodes. Ids are the provider's own (TMDb for TMDb).
pub trait MetadataLookups: Send + Sync {
    fn movie_by_id(&self, id: i64) -> LookupFuture<'_, ExternalMetadata>;
    fn series_by_id(&self, id: i64) -> LookupFuture<'_, ExternalMetadata>;
    fn search_series(&self, name: &str, year: Option<i32>) -> LookupFuture<'_, ExternalMetadata>;
    fn season(&self, series_id: i64, season_number: i32) -> LookupFuture<'_, SeasonMetadata>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataImage {
    pub image_type: String,
    pub url: String,
}
