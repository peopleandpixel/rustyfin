use serde::{Deserialize, Serialize};

/// Segment type for media segments (intro, outro, credits, recap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SegmentType {
    Intro,
    Outro,
    Credits,
    Recap,
}

impl SegmentType {
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentType::Intro => "Intro",
            SegmentType::Outro => "Outro",
            SegmentType::Credits => "Credits",
            SegmentType::Recap => "Recap",
        }
    }
}

/// A detected media segment (intro, outro, credits, or recap).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegment {
    /// Item ID this segment belongs to
    pub item_id: String,
    /// Type of segment
    #[serde(rename = "Type")]
    pub segment_type: String,
    /// Start time in ticks (100-nanosecond units)
    pub segment_start_ticks: i64,
    /// End time in ticks (100-nanosecond units)
    pub segment_end_ticks: i64,
}
