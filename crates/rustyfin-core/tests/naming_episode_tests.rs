//! `naming::parse_episode` (R1-13) on the file name stems of the G9 spec, §6.3.

use rustyfin_core::naming::{EpisodeNumbers, parse_episode};

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
