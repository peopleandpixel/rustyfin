//! `naming::parse_episode` (R1-13) on the file name stems of the G9 spec, §6.3.

use rustyfin_core::naming::{EpisodeNumbers, is_specials_folder, parse_episode};

#[test]
fn parse_episode_table() {
    // (stem, Some((season, episode, ending))), or None when no rule matches.
    type Want = Option<(Option<i32>, i32, Option<i32>)>;
    let table: &[(&str, Want)] = &[
        ("Show - S01E02 - Title", Some((Some(1), 2, None))),
        ("show.s1e2", Some((Some(1), 2, None))),
        ("Show S01.E02", Some((Some(1), 2, None))),
        ("Show S01 E02", Some((Some(1), 2, None))),
        ("Show S01_E02", Some((Some(1), 2, None))),
        ("Show 1x02", Some((Some(1), 2, None))),
        ("Show S01E02E03", Some((Some(1), 2, Some(3)))),
        ("Show S01E02-E03", Some((Some(1), 2, Some(3)))),
        ("Show S01E02-03", Some((Some(1), 2, Some(3)))),
        ("Show S01E01E02E03", Some((Some(1), 1, Some(3)))),
        ("Show 1x02x03", Some((Some(1), 2, Some(3)))),
        ("Show S02E05-E04", Some((Some(2), 5, None))),
        ("Show S01E02-1080p", Some((Some(1), 2, None))),
        ("Show S01E02 1080p", Some((Some(1), 2, None))),
        ("Show S00E01", Some((Some(0), 1, None))),
        ("Show.ep05", Some((None, 5, None))),
        ("Show - Episode 7", Some((None, 7, None))),
        (
            "Severance - S02E01 - Hello, Ms. Cobel",
            Some((Some(2), 1, None)),
        ),
        (
            "The Queen's Gambit - S01E01 - Openings",
            Some((Some(1), 1, None)),
        ),
        ("McMillion$ - S01E01 - Episode 1", Some((Some(1), 1, None))),
        ("This Is England '86 - S01E01", Some((Some(1), 1, None))),
        ("Movie 1920x1080", None),
        ("Pilot", None),
    ];

    let mut wrong = Vec::new();
    for &(stem, want) in table {
        let want = want.map(|(season, episode, ending)| EpisodeNumbers {
            season,
            episode,
            ending,
        });
        let got = parse_episode(stem);
        if got != want {
            wrong.push(format!("{stem:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} stems wrong:\n{}",
        wrong.len(),
        table.len(),
        wrong.join("\n")
    );
}

#[test]
fn b32_parse_episode_e_dot() {
    // Jellyfin's third rule, `foo.E01.` (NamingOptions.cs:330-331): `e`, digits and a dot, which may be the
    // extension's, so the digits may end the stem. It comes after `S01E02` and `ep05`, before `1x02`.
    let table: &[(&str, Option<i32>)] = &[
        (
            "Dekalog.E02.1989.720p.BluRay.AAC.1.0.x264-ZQ-xpost",
            Some(2),
        ),
        ("Dekalog.E08.1080p.BluRay.x264-DEPTH", Some(8)),
        ("Show.e05", Some(5)),
        ("Show E05", Some(5)),
        ("ShowE05", Some(5)),
        ("Show.E05.1x02", Some(5)),
        ("Episode 7.E05", Some(5)),
        ("Show.ep03.E05", Some(3)),
        ("Show.E05 720p", None),
        ("Show.E1080p", None),
    ];
    for &(stem, episode) in table {
        let want = episode.map(|episode| EpisodeNumbers {
            season: None,
            episode,
            ending: None,
        });
        assert_eq!(parse_episode(stem), want, "{stem:?}");
    }
    assert_eq!(
        parse_episode("Show.S01E02.E05").map(|n| (n.season, n.episode)),
        Some((Some(1), 2)),
        "S01E02 comes first"
    );
}

#[test]
fn b32_specials_folders() {
    // Jellyfin's specials aliases (SeasonPathParser.cs:81-94): the folder name without ` ._-[]` and without the
    // series folder's name, in any case, is `specials` or `extras`.
    let table: &[(&str, &str, bool)] = &[
        ("Specials", "Show", true),
        ("SPECIALS", "Show", true),
        ("specials", "Show", true),
        ("[Specials]", "Show", true),
        ("Show - Specials", "Show", true),
        ("Show.Specials", "Show", true),
        ("show_specials", "Show", true),
        ("The Show Specials", "The Show", true),
        ("Show (2010) Specials", "Show (2010)", true),
        ("Extras", "Show", true),
        ("Show Extras", "Show", true),
        ("Specials", "", true),
        ("Specials 2", "Show", false),
        ("Special", "Show", false),
        ("Season Specials", "Show", false),
        ("Showcase Specials", "Show", false),
        ("Specials (2010)", "Show", false),
        ("Season 1", "Show", false),
    ];
    for &(name, series, want) in table {
        assert_eq!(
            is_specials_folder(name, series),
            want,
            "{name:?} in {series:?}"
        );
    }
}
