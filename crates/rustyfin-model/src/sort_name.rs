//! Sort names, as Jellyfin 10.11 makes them (`BaseItem.CreateSortName` and `ModifySortChunks`,
//! with the `Episode`, `Audio`, `Season` and `Person` overrides): "The Matrix" sorts as "matrix",
//! "Part 2" before "Part 10".

use std::sync::{LazyLock, RwLock};

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::media::MediaType;

/// The server configuration's `SortRemoveWords`, `SortRemoveCharacters` and
/// `SortReplaceCharacters`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortRules {
    pub remove_words: Vec<String>,
    pub remove_characters: Vec<String>,
    pub replace_characters: Vec<String>,
}

impl Default for SortRules {
    /// Jellyfin's defaults.
    fn default() -> Self {
        let list = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        Self {
            remove_words: list(&["the", "a", "an"]),
            remove_characters: list(&[",", "&", "-", "{", "}", "'"]),
            replace_characters: list(&[".", "+", "%"]),
        }
    }
}

impl SortRules {
    /// The rules in a server configuration's JSON fields; a missing or malformed list keeps
    /// Jellyfin's default.
    pub fn from_config(fields: &serde_json::Map<String, serde_json::Value>) -> Self {
        let defaults = Self::default();
        let list = |key: &str, default: Vec<String>| match fields.get(key) {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect(),
            _ => default,
        };
        Self {
            remove_words: list("SortRemoveWords", defaults.remove_words),
            remove_characters: list("SortRemoveCharacters", defaults.remove_characters),
            replace_characters: list("SortReplaceCharacters", defaults.replace_characters),
        }
    }

    /// A string that changes whenever the rules do, to tell whether stored sort names were made
    /// with other rules.
    pub fn fingerprint(&self) -> String {
        serde_json::json!([
            self.remove_words,
            self.remove_characters,
            self.replace_characters
        ])
        .to_string()
    }
}

static RULES: LazyLock<RwLock<SortRules>> = LazyLock::new(|| RwLock::new(SortRules::default()));

/// The rules in use: the server configuration's, once the server has set them.
pub fn current_rules() -> SortRules {
    RULES.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Sets the rules in use; the server does this at startup and when its configuration changes.
pub fn set_rules(rules: SortRules) {
    *RULES.write().unwrap_or_else(|e| e.into_inner()) = rules;
}

/// Jellyfin's `CreateSortName` for a name: lowercased; the remove words dropped at the start or
/// end, or between spaces; the remove characters dropped and the replace characters made spaces;
/// then the number runs padded and the accents dropped.
pub fn create_sort_name(name: &str, rules: &SortRules) -> String {
    let mut sortable = name.trim().to_lowercase();
    for word in rules.remove_words.iter().filter(|w| !w.is_empty()) {
        if let Some(rest) = sortable.strip_prefix(&format!("{word} ")) {
            sortable = rest.to_string();
        }
        sortable = sortable.replace(&format!(" {word} "), " ");
        if let Some(rest) = sortable.strip_suffix(&format!(" {word}")) {
            sortable = rest.to_string();
        }
    }
    for c in rules.remove_characters.iter().filter(|c| !c.is_empty()) {
        sortable = sortable.replace(c.as_str(), "");
    }
    for c in rules.replace_characters.iter().filter(|c| !c.is_empty()) {
        sortable = sortable.replace(c.as_str(), " ");
    }
    modify_sort_chunks(&sortable)
}

/// The sort name of a forced sort name, as Jellyfin keeps it: number runs padded, lowercased.
pub fn forced_sort_name(forced: &str) -> String {
    modify_sort_chunks(forced).to_lowercase()
}

/// Jellyfin's `ModifySortChunks`: each run of digits padded with zeros to 10, then accents dropped.
pub fn modify_sort_chunks(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 16);
    let mut digits = String::new();
    let flush = |out: &mut String, digits: &mut String| {
        for _ in digits.chars().count()..10 {
            out.push('0');
        }
        out.push_str(digits);
        digits.clear();
    };
    for c in name.chars() {
        // .NET's char.IsDigit is Unicode's decimal digits; is_numeric is the closest std test
        if c.is_numeric() {
            digits.push(c);
        } else {
            if !digits.is_empty() {
                flush(&mut out, &mut digits);
            }
            out.push(c);
        }
    }
    if !digits.is_empty() {
        flush(&mut out, &mut digits);
    }
    remove_diacritics(&out)
}

/// Decomposes, drops the combining marks and recomposes: "dìdi" becomes "didi". Letters that do
/// not decompose take their base letters, as the Diacritics library Jellyfin uses maps them:
/// "Søren" becomes "Soren", "Łódź" "Lodz", "Straße" "Strasse".
fn remove_diacritics(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.nfd().filter(|c| !is_combining_mark(*c)).nfc() {
        match c {
            'ø' => out.push('o'),
            'Ø' => out.push('O'),
            'ł' => out.push('l'),
            'Ł' => out.push('L'),
            'đ' | 'ð' => out.push('d'),
            'Đ' | 'Ð' => out.push('D'),
            'ħ' => out.push('h'),
            'Ħ' => out.push('H'),
            'ŧ' => out.push('t'),
            'Ŧ' => out.push('T'),
            'ı' => out.push('i'),
            'æ' => out.push_str("ae"),
            'Æ' => out.push_str("AE"),
            'œ' => out.push_str("oe"),
            'Œ' => out.push_str("OE"),
            'ß' => out.push_str("ss"),
            'þ' => out.push_str("th"),
            'Þ' => out.push_str("TH"),
            c => out.push(c),
        }
    }
    out
}

/// The sort name Jellyfin makes for an item of `media_type` named `name`: an episode sorts by
/// season and episode number ("001 - 0002 - Name"), a track by disc and track number, a season by
/// its number, and everything else by [`create_sort_name`].
pub fn item_sort_name(
    media_type: MediaType,
    name: &str,
    parent_index_number: Option<i32>,
    index_number: Option<i32>,
    rules: &SortRules,
) -> String {
    let number =
        |n: Option<i32>, width: usize| n.map(|n| format!("{n:0width$} - ")).unwrap_or_default();
    match media_type {
        MediaType::Episode => format!(
            "{}{}{name}",
            number(parent_index_number, 3),
            number(index_number, 4)
        ),
        MediaType::Audio => format!(
            "{}{}{name}",
            number(parent_index_number, 4),
            number(index_number, 4)
        ),
        MediaType::Season => season_sort_name(index_number),
        _ => create_sort_name(name, rules),
    }
}

/// A person's sort name: the name as written, since Jellyfin turns its alphanumeric sorting off
/// for people.
pub fn person_sort_name(name: &str) -> String {
    name.trim_start().to_string()
}

/// The sort name of a season numbered `number`. Rustyfin's own ("season 0001"), so seasons sort
/// by number; Jellyfin's is the bare number.
pub fn season_sort_name(number: Option<i32>) -> String {
    format!("season {:04}", number.unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort(name: &str) -> String {
        create_sort_name(name, &SortRules::default())
    }

    #[test]
    fn r3sortname_a_leading_article_is_dropped() {
        assert_eq!(sort("The Matrix"), "matrix");
        assert_eq!(sort("A Most Violent Year"), "most violent year");
        assert_eq!(
            sort("An American Werewolf in London"),
            "american werewolf in london"
        );
        assert_eq!(sort("  The Matrix  "), "matrix");
    }

    #[test]
    fn r3sortname_an_article_without_a_following_space_stays() {
        assert_eq!(sort("Theory of Everything"), "theory of everything");
        assert_eq!(sort("Alien"), "alien");
        assert_eq!(sort("The"), "the");
        assert_eq!(sort("A-Team"), "ateam");
    }

    #[test]
    fn r3sortname_articles_inside_and_at_the_end_go_as_in_jellyfin() {
        assert_eq!(sort("I Saw the Devil"), "i saw devil");
        assert_eq!(sort("Ichi the Killer"), "ichi killer");
        assert_eq!(sort("Raising a Child"), "raising child");
        assert_eq!(sort("Plan A"), "plan");
        // Only whole words: "a" inside a word stays
        assert_eq!(sort("Banana Split"), "banana split");
    }

    #[test]
    fn r3sortname_numbers_are_padded() {
        assert_eq!(sort("28 Years Later"), "0000000028 years later");
        assert!(sort("Part 2") < sort("Part 10"));
        assert_eq!(sort("Bravo Story (2005)"), "bravo story (0000002005)");
        assert_eq!(sort("12345678901"), "12345678901");
    }

    #[test]
    fn r3sortname_punctuation_is_removed_or_spaced() {
        assert_eq!(sort("The Queen's Gambit"), "queens gambit");
        assert_eq!(sort("Mushi-Shi"), "mushishi");
        assert_eq!(sort("Compartment No. 6"), "compartment no  0000000006");
        assert_eq!(
            sort("Mission: Impossible - Dead Reckoning Part One"),
            "mission: impossible  dead reckoning part one"
        );
        assert_eq!(
            sort("American Entertainment Partners L.P."),
            "american entertainment partners l p "
        );
    }

    #[test]
    fn r3sortname_non_ascii_loses_accents_only() {
        assert_eq!(sort("Dìdi"), "didi");
        assert_eq!(sort("Ålgen"), "algen");
        assert_eq!(sort("Ça Ira"), "ca ira");
        assert_eq!(sort("Die Hölle"), "die holle");
        assert_eq!(sort("弟弟"), "弟弟");
        assert_eq!(sort("Søren"), "soren");
        assert_eq!(sort("Łódź"), "lodz");
        assert_eq!(sort("Đorđe"), "dorde");
        assert_eq!(sort("Straße"), "strasse");
        assert_eq!(sort("Ærø"), "aero");
        // Other scripts' decimal digits are padded too, as .NET's char.IsDigit takes them
        assert_eq!(sort("Part ٣"), "part 000000000٣");
    }

    #[test]
    fn r3sortname_rules_come_from_the_configuration() {
        let fields = serde_json::json!({ "SortRemoveWords": ["der", "die", "das"] });
        let rules = SortRules::from_config(fields.as_object().unwrap());
        assert_eq!(
            rules.remove_characters,
            SortRules::default().remove_characters
        );
        assert_eq!(create_sort_name("Die Hard", &rules), "hard");
        assert_eq!(create_sort_name("The Matrix", &rules), "the matrix");
        assert_ne!(rules.fingerprint(), SortRules::default().fingerprint());
    }

    #[test]
    fn r3sortname_types_sort_as_in_jellyfin() {
        let rules = SortRules::default();
        let ep = item_sort_name(MediaType::Episode, "The Pilot", Some(1), Some(2), &rules);
        assert_eq!(ep, "001 - 0002 - The Pilot");
        let track = item_sort_name(MediaType::Audio, "Dusk", Some(1), Some(1), &rules);
        assert_eq!(track, "0001 - 0001 - Dusk");
        assert_eq!(
            item_sort_name(MediaType::Audio, "Dusk", None, None, &rules),
            "Dusk"
        );
        assert_eq!(
            person_sort_name(" Aaron Taylor-Johnson"),
            "Aaron Taylor-Johnson"
        );
        assert_eq!(
            item_sort_name(MediaType::Season, "Season 2", Some(1), Some(2), &rules),
            "season 0002"
        );
        assert_eq!(
            item_sort_name(MediaType::Movie, "The Matrix", None, None, &rules),
            "matrix"
        );
        assert_eq!(forced_sort_name("Matrix, The 2"), "matrix, the 0000000002");
    }
}
