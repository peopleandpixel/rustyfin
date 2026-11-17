use serde::{Deserialize, Serialize};

/// Metadata fetched from external providers or local NFO files
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalMetadata {
    pub name: Option<String>,
    /// An NFO's `<sorttitle>`: a forced sort name, as Jellyfin's NFO parsers make it.
    #[serde(default)]
    pub sort_title: Option<String>,
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

/// A series' current season list, as the provider's series details give it: what the missing
/// episode task asks for once a run, to learn of new seasons and episodes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeriesSeasons {
    /// The series' status now ("Returning Series", "Ended", ...)
    pub status: Option<String>,
    pub seasons: Vec<SeasonSummary>,
}

/// One season in a series' season list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeasonSummary {
    pub number: i32,
    pub name: Option<String>,
    /// How many episodes the provider lists in it now
    pub episode_count: Option<i32>,
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
    /// A series' season list, asked of the provider even when an answer is cached (the new
    /// answer replaces it): the missing-episode task asks it once a run for each series that has
    /// not ended. A provider without one answers with none.
    fn series_seasons_fresh(&self, id: i64) -> LookupFuture<'_, SeriesSeasons> {
        let _ = id;
        Box::pin(async { Ok(None) })
    }
    /// `season`, asked of the provider even when an answer is cached: for a season whose episode
    /// count has changed since the cached answer.
    fn season_fresh(&self, series_id: i64, season_number: i32) -> LookupFuture<'_, SeasonMetadata> {
        self.season(series_id, season_number)
    }
    /// Every match of a name search, for an admin's Identify (`series` picks the TV search).
    /// A provider without a list search answers with none.
    fn search_all(
        &self,
        series: bool,
        name: &str,
        year: Option<i32>,
    ) -> LookupFuture<'_, Vec<RemoteSearchHit>> {
        let _ = (series, name, year);
        Box::pin(async { Ok(Some(Vec::new())) })
    }
    /// Every image the provider has for a movie's or series' id, for an admin's image editor.
    fn images(&self, series: bool, id: i64) -> LookupFuture<'_, Vec<RemoteImage>> {
        let _ = (series, id);
        Box::pin(async { Ok(Some(Vec::new())) })
    }
    /// Forgets the cached answers about a movie's or series' id (a series' seasons included), so
    /// that a "replace all metadata" refresh asks the provider again.
    fn forget(&self, series: bool, id: i64) -> Forgetting<'_> {
        let _ = (series, id);
        Box::pin(async {})
    }
    /// Forgets the cached searches for `title`, so that a "replace all metadata" refresh of an
    /// item with no provider id searches again.
    fn forget_search(&self, series: bool, title: &str) -> Forgetting<'_> {
        let _ = (series, title);
        Box::pin(async {})
    }
}

/// A cache entry being forgotten.
pub type Forgetting<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

/// Lookups by IMDb id (OMDb's), asked after the provider that identified the item: a movie's or
/// series' details, and a series' season with its episodes. Their answers fill only what is
/// still missing, as a later provider does in Jellyfin.
pub trait ImdbLookups: Send + Sync {
    fn by_imdb_id(&self, imdb_id: &str) -> LookupFuture<'_, ExternalMetadata>;
    fn season(&self, series_imdb_id: &str, season_number: i32) -> LookupFuture<'_, SeasonMetadata>;
    /// Forgets the cached answers about an IMDb id (a series' seasons included).
    fn forget(&self, imdb_id: &str) -> Forgetting<'_> {
        let _ = imdb_id;
        Box::pin(async {})
    }
}

/// One match of a provider search: Jellyfin's RemoteSearchResult, with the provider's own id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteSearchHit {
    pub id: i64,
    pub name: String,
    pub production_year: Option<i32>,
    /// `YYYY-MM-DD`
    pub premiere_date: Option<String>,
    pub overview: Option<String>,
    pub image_url: Option<String>,
}

/// One image a provider offers: Jellyfin's RemoteImageInfo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteImage {
    /// Jellyfin's ImageType: "Primary", "Backdrop", "Logo"
    pub image_type: String,
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// ISO 639-1, or none for an image without text
    pub language: Option<String>,
    pub community_rating: Option<f64>,
    pub vote_count: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataImage {
    pub image_type: String,
    pub url: String,
}
