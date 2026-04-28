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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataImage {
    pub image_type: String,
    pub url: String,
}
