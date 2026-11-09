//! Query-string helpers that read parameters the way ASP.NET binds them for Jellyfin:
//! names match without ASCII case, values are trimmed, enum and boolean values match
//! without case, and blank values count as absent. Routes that take
//! `Query<Vec<(String, String)>>` can share these helpers.

use std::num::IntErrorKind;

use uuid::Uuid;

use crate::error::ApiError;

/// Jellyfin `BaseItemKind` names, in Jellyfin's order.
pub const ITEM_KINDS: [&str; 37] = [
    "AggregateFolder",
    "Audio",
    "AudioBook",
    "BasePluginFolder",
    "Book",
    "BoxSet",
    "Channel",
    "ChannelFolderItem",
    "CollectionFolder",
    "Episode",
    "Folder",
    "Genre",
    "ManualPlaylistsFolder",
    "Movie",
    "LiveTvChannel",
    "LiveTvProgram",
    "MusicAlbum",
    "MusicArtist",
    "MusicGenre",
    "MusicVideo",
    "Person",
    "Photo",
    "PhotoAlbum",
    "Playlist",
    "PlaylistsFolder",
    "Program",
    "Recording",
    "Season",
    "Series",
    "Studio",
    "Trailer",
    "TvChannel",
    "TvProgram",
    "UserRootFolder",
    "UserView",
    "Video",
    "Year",
];

/// Media type names that `mediaTypes` may name.
pub const MEDIA_TYPES: [&str; 5] = ["Unknown", "Video", "Audio", "Photo", "Book"];

/// The first non-blank value for `key`, trimmed.
pub fn first_value<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.trim())
        .find(|value| !value.is_empty())
}

/// Trimmed, non-blank entries for `key`; a lone value is split on `delimiter`.
pub fn list_values(params: &[(String, String)], key: &str, delimiter: char) -> Vec<String> {
    let matching: Vec<&str> = params
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
        .collect();
    let entries: Vec<&str> = match matching.as_slice() {
        [only] => only.split(delimiter).collect(),
        several => several.to_vec(),
    };
    entries
        .into_iter()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// The members of `names` that the entries for `key` name, ignoring case, each once.
pub fn enum_values(
    params: &[(String, String)],
    key: &str,
    names: &[&'static str],
) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    for entry in list_values(params, key, ',') {
        let Some(name) = names
            .iter()
            .copied()
            .find(|name| name.eq_ignore_ascii_case(&entry))
        else {
            continue;
        };
        if !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

/// The `ITEM_KINDS` that the entries for `key` name, ignoring case, each once.
pub fn item_kinds(params: &[(String, String)], key: &str) -> Vec<&'static str> {
    enum_values(params, key, &ITEM_KINDS)
}

/// The `MEDIA_TYPES` that the entries for `key` name, ignoring case, each once.
pub fn media_types(params: &[(String, String)], key: &str) -> Vec<&'static str> {
    enum_values(params, key, &MEDIA_TYPES)
}

/// The media type of a canonical `BaseItemKind` name; anything else is "Unknown".
pub fn media_type_of(kind: &str) -> &'static str {
    match kind {
        "Movie" | "Episode" | "Video" | "MusicVideo" | "Trailer" => "Video",
        "Audio" | "AudioBook" => "Audio",
        "Photo" => "Photo",
        "Book" => "Book",
        _ => "Unknown",
    }
}

/// Parses a UUID; `name` labels the 400 message.
pub fn parse_uuid(name: &str, value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value).map_err(|_| ApiError::BadRequest(format!("{name} must be a UUID")))
}

/// The UUID given for `key`, if a non-blank value is given.
pub fn uuid_param(params: &[(String, String)], key: &str) -> Result<Option<Uuid>, ApiError> {
    first_value(params, key)
        .map(|value| parse_uuid(key, value))
        .transpose()
}

/// Like `uuid_param`, but the nil UUID means no parent.
pub fn parent_param(params: &[(String, String)], key: &str) -> Result<Option<Uuid>, ApiError> {
    Ok(uuid_param(params, key)?.filter(|id| !id.is_nil()))
}

/// Parses an integer from 0 to `i32::MAX`; `name` labels the 400 message.
pub fn parse_count(name: &str, value: &str) -> Result<usize, ApiError> {
    let not_a_count = || ApiError::BadRequest(format!("{name} must be a non-negative integer"));
    let too_large = || ApiError::BadRequest(format!("{name} must be at most {}", i32::MAX));
    match value.parse::<i64>() {
        Ok(n) if n < 0 => Err(not_a_count()),
        Ok(n) if n > i64::from(i32::MAX) => Err(too_large()),
        Ok(n) => Ok(n as usize),
        Err(err) if matches!(err.kind(), IntErrorKind::PosOverflow) => Err(too_large()),
        Err(_) => Err(not_a_count()),
    }
}

/// The count given for `key`, if a non-blank value is given.
pub fn count_param(params: &[(String, String)], key: &str) -> Result<Option<usize>, ApiError> {
    first_value(params, key)
        .map(|value| parse_count(key, value))
        .transpose()
}

/// Parses `true` or `false` ignoring case; `name` labels the 400 message.
pub fn parse_bool(name: &str, value: &str) -> Result<bool, ApiError> {
    if value.eq_ignore_ascii_case("true") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("false") {
        Ok(false)
    } else {
        Err(ApiError::BadRequest(format!(
            "{name} must be true or false"
        )))
    }
}

/// The boolean given for `key`, if a non-blank value is given.
pub fn bool_param(params: &[(String, String)], key: &str) -> Result<Option<bool>, ApiError> {
    first_value(params, key)
        .map(|value| parse_bool(key, value))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    // J6: search parity

    fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// The message of a 400, or a panic for any other result.
    fn bad_request<T: std::fmt::Debug>(result: Result<T, ApiError>) -> String {
        match result {
            Err(ApiError::BadRequest(message)) => message,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    #[test]
    fn j6_names_match_without_case() {
        let sent = params(&[("SEARCHTERM", "comet")]);
        assert_eq!(first_value(&sent, "searchTerm"), Some("comet"));
        assert_eq!(first_value(&sent, "SearchTerm"), Some("comet"));
        assert_eq!(first_value(&sent, "term"), None);
    }

    #[test]
    fn j6_first_value_skips_blank_values_and_trims() {
        let sent = params(&[("limit", " "), ("Limit", " 7 "), ("LIMIT", "8")]);
        assert_eq!(first_value(&sent, "limit"), Some("7"));
        assert_eq!(first_value(&sent, "startIndex"), None);
        assert_eq!(first_value(&params(&[("limit", "")]), "limit"), None);
    }

    #[test]
    fn j6_one_list_value_splits_on_the_delimiter() {
        let sent = params(&[("includeItemTypes", " Movie ,,Series, ")]);
        assert_eq!(
            list_values(&sent, "IncludeItemTypes", ','),
            ["Movie", "Series"]
        );
        assert_eq!(
            list_values(&params(&[("tags", "a|b, c")]), "tags", '|'),
            ["a", "b, c"]
        );
        assert!(list_values(&sent, "excludeItemTypes", ',').is_empty());
        assert!(list_values(&params(&[("tags", " , ")]), "tags", ',').is_empty());
    }

    /// Jellyfin's `CommaDelimitedCollectionModelBinder` splits only a value given once.
    #[test]
    fn j6_a_repeated_list_key_is_not_split() {
        let sent = params(&[
            ("includeItemTypes", "Movie,Series"),
            ("IncludeItemTypes", " Audio "),
            ("includeitemtypes", ""),
        ]);
        assert_eq!(
            list_values(&sent, "includeItemTypes", ','),
            ["Movie,Series", "Audio"]
        );
    }

    #[test]
    fn j6_kind_names_parse_without_case_once_each() {
        let sent = params(&[(
            "includeItemTypes",
            "series,MOVIE,Movie,Bogus,1, musicalbum ,Series",
        )]);
        assert_eq!(
            item_kinds(&sent, "IncludeItemTypes"),
            ["Series", "Movie", "MusicAlbum"]
        );
        assert_eq!(
            media_types(
                &params(&[("MediaTypes", "audio,VIDEO,Movie")]),
                "mediaTypes"
            ),
            ["Audio", "Video"]
        );
    }

    #[test]
    fn j6_a_repeated_kind_key_parses_each_value_whole() {
        let sent = params(&[
            ("includeItemTypes", "Movie,Series"),
            ("includeItemTypes", "audio"),
        ]);
        assert_eq!(item_kinds(&sent, "includeItemTypes"), ["Audio"]);
    }

    #[test]
    fn j6_every_jellyfin_kind_and_media_type_parses() {
        for kind in ITEM_KINDS {
            let lower = kind.to_lowercase();
            let sent = params(&[("includeItemTypes", lower.as_str())]);
            assert_eq!(item_kinds(&sent, "includeItemTypes"), [kind]);
        }
        for kind in [
            "AggregateFolder",
            "BoxSet",
            "CollectionFolder",
            "Folder",
            "Genre",
            "LiveTvProgram",
            "MusicArtist",
            "Person",
            "Studio",
            "TvChannel",
            "UserView",
            "Year",
        ] {
            assert!(ITEM_KINDS.contains(&kind), "{kind}");
        }
        assert_eq!(MEDIA_TYPES, ["Unknown", "Video", "Audio", "Photo", "Book"]);
        for media_type in MEDIA_TYPES {
            let upper = media_type.to_uppercase();
            let sent = params(&[("mediaTypes", upper.as_str())]);
            assert_eq!(media_types(&sent, "mediaTypes"), [media_type]);
        }
    }

    #[test]
    fn j6_media_types_follow_jellyfin() {
        for (kind, media_type) in [
            ("Movie", "Video"),
            ("Episode", "Video"),
            ("Video", "Video"),
            ("MusicVideo", "Video"),
            ("Trailer", "Video"),
            ("Audio", "Audio"),
            ("AudioBook", "Audio"),
            ("Photo", "Photo"),
            ("Book", "Book"),
            ("Series", "Unknown"),
            ("Season", "Unknown"),
            ("MusicAlbum", "Unknown"),
            ("Genre", "Unknown"),
            ("Folder", "Unknown"),
            ("Person", "Unknown"),
            ("BoxSet", "Unknown"),
            ("Bogus", "Unknown"),
        ] {
            assert_eq!(media_type_of(kind), media_type, "{kind}");
        }
        for kind in ITEM_KINDS {
            assert!(MEDIA_TYPES.contains(&media_type_of(kind)), "{kind}");
        }
    }

    #[test]
    fn j6_counts_are_non_negative_i32s() {
        assert_eq!(parse_count("Limit", "0").unwrap(), 0);
        assert_eq!(parse_count("Limit", "2147483647").unwrap(), 2_147_483_647);
        for value in ["-1", "ten", "1.5", "-99999999999999999999"] {
            assert_eq!(
                bad_request(parse_count("Limit", value)),
                "Limit must be a non-negative integer",
                "{value}"
            );
        }
        for value in ["2147483648", "99999999999999999999999"] {
            assert_eq!(
                bad_request(parse_count("startIndex", value)),
                "startIndex must be at most 2147483647",
                "{value}"
            );
        }
    }

    #[test]
    fn j6_count_params_default_when_absent_or_blank() {
        assert_eq!(count_param(&params(&[]), "limit").unwrap(), None);
        assert_eq!(
            count_param(&params(&[("limit", " ")]), "limit").unwrap(),
            None
        );
        assert_eq!(
            count_param(&params(&[("LIMIT", " 5 ")]), "limit").unwrap(),
            Some(5)
        );
        assert_eq!(
            bad_request(count_param(&params(&[("StartIndex", "-1")]), "startIndex")),
            "startIndex must be a non-negative integer"
        );
    }

    #[test]
    fn j6_booleans_parse_without_case_and_blank_means_default() {
        assert!(parse_bool("isKids", "TRUE").unwrap());
        assert!(!parse_bool("isKids", "false").unwrap());
        for value in ["yes", "1", "maybe"] {
            assert_eq!(
                bad_request(parse_bool("isKids", value)),
                "isKids must be true or false",
                "{value}"
            );
        }
        assert_eq!(bool_param(&params(&[]), "isKids").unwrap(), None);
        assert_eq!(
            bool_param(&params(&[("IsKids", "")]), "isKids").unwrap(),
            None
        );
        assert_eq!(
            bool_param(&params(&[("ISKIDS", " True ")]), "isKids").unwrap(),
            Some(true)
        );
        assert_eq!(
            bad_request(bool_param(&params(&[("isKids", "no")]), "isKids")),
            "isKids must be true or false"
        );
    }

    #[test]
    fn j6_uuids_parse_and_a_nil_parent_is_none() {
        let id = Uuid::new_v4();
        // Jellyfin's clients send ids with and without dashes.
        for sent in [id.simple().to_string(), id.to_string()] {
            assert_eq!(
                uuid_param(&params(&[("UserId", sent.as_str())]), "userId").unwrap(),
                Some(id)
            );
            assert_eq!(
                parent_param(&params(&[("PARENTID", sent.as_str())]), "parentId").unwrap(),
                Some(id)
            );
        }
        assert_eq!(uuid_param(&params(&[]), "userId").unwrap(), None);
        let nil = Uuid::nil().to_string();
        assert_eq!(
            parent_param(&params(&[("parentId", nil.as_str())]), "parentId").unwrap(),
            None
        );
        assert_eq!(
            bad_request(uuid_param(&params(&[("userId", "bogus")]), "userId")),
            "userId must be a UUID"
        );
        assert_eq!(
            bad_request(parent_param(&params(&[("parentId", "bogus")]), "parentId")),
            "parentId must be a UUID"
        );
        assert_eq!(
            bad_request(parse_uuid("ParentId", "12")),
            "ParentId must be a UUID"
        );
    }
}
