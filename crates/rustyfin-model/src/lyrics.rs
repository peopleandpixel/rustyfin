//! Jellyfin's lyric DTOs (`MediaBrowser.Model/Lyrics`), which `/Audio/{id}/Lyrics` answers with.

use serde::{Deserialize, Serialize};

/// The lyrics of a track: the file's header fields and its lines in time order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricDto {
    pub metadata: LyricMetadata,
    pub lyrics: Vec<LyricLine>,
}

/// The header fields of a lyric file. Jellyfin leaves out a null field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The length of the song in ticks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The offset of the timestamps in ticks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_synced: Option<bool>,
}

/// One line of lyrics. `Start` is in ticks (100 ns), and is left out of an unsynced line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricLine {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
}
