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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_type_as_str_intro() {
        assert_eq!(SegmentType::Intro.as_str(), "Intro");
    }

    #[test]
    fn test_segment_type_as_str_outro() {
        assert_eq!(SegmentType::Outro.as_str(), "Outro");
    }

    #[test]
    fn test_segment_type_as_str_credits() {
        assert_eq!(SegmentType::Credits.as_str(), "Credits");
    }

    #[test]
    fn test_segment_type_as_str_recap() {
        assert_eq!(SegmentType::Recap.as_str(), "Recap");
    }
}
