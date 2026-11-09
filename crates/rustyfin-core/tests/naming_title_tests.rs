//! Movie titles and version groups (R1-27, R1-20) and episode version groups in
//! `naming`, on the cases of the G9 spec, §6.4 and §6.5.

use std::fmt::Debug;
use std::path::{Path, PathBuf};

use rustyfin_core::naming::{
    CleanName, VersionGroup, clean_name, group_episode_files, group_movie_files, resolution,
    series_title_and_year, title_key,
};

/// Runs `f` on every input of `table` and reports all wrong rows at once.
fn check<W: Debug, G: Debug + PartialEq<W>>(table: &[(&str, W)], f: impl Fn(&str) -> G) {
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|&(input, ref want)| {
            let got = f(input);
            (got != *want).then(|| format!("{input:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} of {} rows wrong:\n{}",
        wrong.len(),
        table.len(),
        wrong.join("\n")
    );
}

#[test]
fn clean_name_table() {
    let rows = [
        ("Argo (2012) Bluray-2160p", "Argo", Some(2012)),
        ("Argo", "Argo", None),
        (
            "A Most Violent Year (2014) Remux-1080p",
            "A Most Violent Year",
            Some(2014),
        ),
        (
            "Holy.Motors.2012.BRRip.XviD.5rFF",
            "Holy Motors",
            Some(2012),
        ),
        (
            "Funny Games U S 2007 LiMiTED DVDRip XviD NODLABS",
            "Funny Games U S",
            Some(2007),
        ),
        (
            "Funny.Games.U.S.2007.LiMiTED.DVDRip.XviD-NODLABS.CD1",
            "Funny Games U S",
            Some(2007),
        ),
        (
            "the.machine.2013.720p.bluray.x264.dts-nohate",
            "the machine",
            Some(2013),
        ),
        (
            "Star Wars (1977) WEBDL-2160p Proper",
            "Star Wars",
            Some(1977),
        ),
        ("Alien³ (1992) Bluray-1080p", "Alien³", Some(1992)),
        ("1917 (2019)", "1917", Some(2019)),
        ("1917", "1917", None),
        ("Se7en (1995)", "Se7en", Some(1995)),
        ("Movie - (2010)", "Movie", Some(2010)),
        ("Citizen Kane cd1", "Citizen Kane", None),
        ("Argo Bluray-2160p", "Argo", None),
        ("Movie [Director's Cut]", "Movie", None),
        ("[YTS] Movie Name", "Movie Name", None),
        ("Movie Name - 2", "Movie Name", None),
        ("Some_Movie_Name", "Some Movie Name", None),
        ("News.2010.10.12", "News 2010 10 12", None),
        ("Arcadian ()", "Arcadian", None),
        ("Hocus Pocus 2 (0)", "Hocus Pocus 2", None),
        ("The Running Man () Bluray-1080p", "The Running Man", None),
        (
            "Mad Men TV Series 2007 - IMDb ()",
            "Mad Men TV Series",
            Some(2007),
        ),
    ];
    let table: Vec<(&str, CleanName)> = rows
        .into_iter()
        .map(|(stem, title, year)| {
            let title = title.to_string();
            (stem, CleanName { title, year })
        })
        .collect();
    check(&table, clean_name);
}

#[test]
fn title_key_table() {
    check(
        &[
            ("Alien³", "alien³"),
            ("The Machine", "themachine"),
            ("Se7en", "se7en"),
            ("Amélie", "amélie"),
            ("Mission: Impossible – Fallout", "missionimpossiblefallout"),
            ("WALL·E", "walle"),
        ],
        title_key,
    );
}

#[test]
fn resolution_table() {
    let table: &[(&str, Option<u64>)] = &[
        ("x 2160p", Some(2160)),
        ("720P", Some(720)),
        ("576i", Some(576)),
        ("x264", None),
        ("4k", None),
        ("S01E02", None),
        ("Argo (2012) Bluray-2160p", Some(2160)),
        ("The.Machine.2013.720p.bluray.x264", Some(720)),
        ("5rFF-hmot480p", Some(480)),
    ];
    check(table, resolution);
}

/// A group as the tests compare it: title, year, and file names, primary first.
type Group = (String, Option<i32>, Vec<String>);

fn group(title: &str, year: Option<i32>, files: &[&str]) -> Group {
    let files = files.iter().map(|file| file.to_string()).collect();
    (title.to_string(), year, files)
}

/// The names of `files`, which must all be directly in `folder`.
fn names_in(folder: &Path, files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            assert_eq!(file.parent(), Some(folder), "{}", file.display());
            file.file_name().unwrap().to_string_lossy().into_owned()
        })
        .collect()
}

/// Groups the files `names` of `folder`, which are passed in this order.
fn groups_in(folder: &Path, names: &[&str], is_library_root: bool) -> Vec<Group> {
    let files: Vec<PathBuf> = names.iter().map(|name| folder.join(name)).collect();
    let groups: Vec<VersionGroup> = group_movie_files(folder, &files, is_library_root);
    groups
        .into_iter()
        .map(|g| (g.title, g.year, names_in(folder, &g.files)))
        .collect()
}

/// Groups the files `names` of the movie folder `/media/film/<folder>`.
fn movie_groups(folder: &str, names: &[&str]) -> Vec<Group> {
    groups_in(&Path::new("/media/film").join(folder), names, false)
}

#[test]
fn group_argo() {
    assert_eq!(
        movie_groups("Argo (2012)", &["Argo.mkv", "Argo (2012) Bluray-2160p.mkv"]),
        [group(
            "Argo",
            Some(2012),
            &["Argo (2012) Bluray-2160p.mkv", "Argo.mkv"]
        )]
    );
}

#[test]
fn group_citizen_kane_parts() {
    assert_eq!(
        movie_groups(
            "Citizen Kane (1941)",
            &[
                "Citizen Kane cd1.avi",
                "Citizen Kane (1941) Bluray-2160p.mkv",
                "Citizen Kane cd2.avi",
            ]
        ),
        [group(
            "Citizen Kane",
            Some(1941),
            &[
                "Citizen Kane (1941) Bluray-2160p.mkv",
                "Citizen Kane cd1.avi",
                "Citizen Kane cd2.avi",
            ]
        )]
    );
}

#[test]
fn group_alien3_two_films() {
    // Two titles are two films. "Aliens" sorts first, as 's' is below the
    // first byte of '³'.
    assert_eq!(
        movie_groups(
            "Alien³ (1992)",
            &[
                "Alien³ (1992) Bluray-1080p.mkv",
                "Aliens (1986) Bluray-1080p.mkv",
            ]
        ),
        [
            group("Aliens", Some(1986), &["Aliens (1986) Bluray-1080p.mkv"]),
            group("Alien³", Some(1992), &["Alien³ (1992) Bluray-1080p.mkv"]),
        ]
    );
}

#[test]
fn group_bad_santa_folder_name_first() {
    // The file named like its folder is the primary, ahead of a higher
    // resolution.
    assert_eq!(
        movie_groups(
            "Bad Santa (2003)",
            &[
                "Bad Santa.mkv",
                "Bad Santa (2003) Bluray-1080p.mkv",
                "Bad Santa (2003).mkv",
            ]
        ),
        [group(
            "Bad Santa",
            Some(2003),
            &[
                "Bad Santa (2003).mkv",
                "Bad Santa (2003) Bluray-1080p.mkv",
                "Bad Santa.mkv",
            ]
        )]
    );
}

#[test]
fn group_funny_games_folder_year() {
    // The folder's year wins over the year in a file name.
    assert_eq!(
        movie_groups(
            "Funny Games (2007)",
            &["Funny Games.mkv", "Funny Games (2008) Bluray-1080p.mkv"]
        ),
        [group(
            "Funny Games",
            Some(2007),
            &["Funny Games (2008) Bluray-1080p.mkv", "Funny Games.mkv"]
        )]
    );
}

#[test]
fn group_the_machine() {
    assert_eq!(
        movie_groups(
            "The Machine (2013)",
            &[
                "the.machine.2013.720p.bluray.x264.dts-nohate.mkv",
                "The Machine (2013) Bluray-1080p.mkv",
            ]
        ),
        [group(
            "The Machine",
            Some(2013),
            &[
                "The Machine (2013) Bluray-1080p.mkv",
                "the.machine.2013.720p.bluray.x264.dts-nohate.mkv",
            ]
        )]
    );
}

#[test]
fn group_resolution_order() {
    assert_eq!(
        movie_groups(
            "A Most Violent Year (2014)",
            &[
                "A Most Violent Year (2014) Bluray-720p.mkv",
                "A Most Violent Year (2014) Remux-1080p.mkv",
                "A Most Violent Year (2014) Bluray-1080p.mkv",
            ]
        ),
        [group(
            "A Most Violent Year",
            Some(2014),
            &[
                "A Most Violent Year (2014) Bluray-1080p.mkv",
                "A Most Violent Year (2014) Remux-1080p.mkv",
                "A Most Violent Year (2014) Bluray-720p.mkv",
            ]
        )]
    );
}

#[test]
fn group_holy_motors_scene_file() {
    // The file name has no title; the folder has one.
    assert_eq!(
        movie_groups("Holy.Motors.2012.BRRip.XviD.5rFF", &["5rFF-hmot480p.avi"]),
        [group("Holy Motors", Some(2012), &["5rFF-hmot480p.avi"])]
    );
}

#[test]
fn group_iron_claw_empty_year_folder() {
    assert_eq!(
        movie_groups(
            "The Iron Claw ()",
            &["The Iron Claw (2023) WEBDL-2160p.mkv"]
        ),
        [group(
            "The Iron Claw",
            Some(2023),
            &["The Iron Claw (2023) WEBDL-2160p.mkv"]
        )]
    );
}

#[test]
fn group_split_by_year() {
    // The file without a year joins the group of the folder's year.
    assert_eq!(
        movie_groups(
            "Movie (2010)",
            &["Movie.mkv", "Movie (2019).mkv", "Movie (2010).mkv"]
        ),
        [
            group("Movie", Some(2010), &["Movie (2010).mkv", "Movie.mkv"]),
            group("Movie", Some(2019), &["Movie (2019).mkv"]),
        ]
    );
}

#[test]
fn group_star_wars_saga() {
    assert_eq!(
        movie_groups(
            "Star Wars Complete Saga",
            &[
                "Star Wars Episode IV - A New Hope (1977).mkv",
                "Star Wars Episode I - The Phantom Menace (1999).mkv",
            ]
        ),
        [
            group(
                "Star Wars Episode I - The Phantom Menace",
                Some(1999),
                &["Star Wars Episode I - The Phantom Menace (1999).mkv"]
            ),
            group(
                "Star Wars Episode IV - A New Hope",
                Some(1977),
                &["Star Wars Episode IV - A New Hope (1977).mkv"]
            ),
        ]
    );
}

#[test]
fn group_root_files() {
    // A scan that read the root's name would call this film "Films" (2024).
    assert_eq!(
        groups_in(Path::new("/media/Films 2024"), &["The Room.avi"], true),
        [group("The Room", None, &["The Room.avi"])]
    );
}

/// Groups the files `names` of the season folder `/media/tv/Show/Season 1`.
fn episode_groups(names: &[&str], folder_season: Option<i32>) -> Vec<Vec<String>> {
    let folder = Path::new("/media/tv/Show/Season 1");
    let files: Vec<PathBuf> = names.iter().map(|name| folder.join(name)).collect();
    group_episode_files(&files, folder_season)
        .iter()
        .map(|group| names_in(folder, group))
        .collect()
}

#[test]
fn group_episode_versions() {
    assert_eq!(
        episode_groups(
            &[
                "Show S01E02.mkv",
                "Show S01E01 720p.mkv",
                "Show S01E01 1080p.mkv",
            ],
            Some(1)
        ),
        [
            vec!["Show S01E01 1080p.mkv", "Show S01E01 720p.mkv"],
            vec!["Show S01E02.mkv"],
        ]
    );
}

#[test]
fn group_episode_unparsed_alone() {
    assert_eq!(
        episode_groups(&["Pilot.mkv", "Extras.mkv"], Some(1)),
        [vec!["Extras.mkv"], vec!["Pilot.mkv"]]
    );
}

#[test]
fn group_episode_file_season_in_key() {
    // A season in the file name wins; without one, the folder's counts.
    assert_eq!(
        episode_groups(
            &["Show S02E05.mkv", "Show S01E05.mkv", "Show - Episode 5.mkv",],
            Some(1)
        ),
        [
            vec!["Show - Episode 5.mkv", "Show S01E05.mkv"],
            vec!["Show S02E05.mkv"],
        ]
    );
}

#[test]
fn b32_f2_versions_without_a_resolution_sort_by_stem() {
    // Jellyfin orders them by FileNameWithoutExtension (12.4 `VideoListResolver.cs:282`, `:296-300`).
    // By whole name, "Big Eyes (" would sort before "Big Eyes.", as a space is below a dot.
    assert_eq!(
        movie_groups(
            "Big Eyes (2014)",
            &["Big Eyes (2014) BR-DISK.m2ts", "Big Eyes.mkv"]
        ),
        [group(
            "Big Eyes",
            Some(2014),
            &["Big Eyes.mkv", "Big Eyes (2014) BR-DISK.m2ts"]
        )]
    );
}

#[test]
fn b32_series_title_and_year() {
    // Jellyfin's TitleWithYearRegex: the text before the first "(dddd)", trimmed, and that year.
    let table: &[(&str, Option<(&str, i32)>)] = &[
        ("One Night (2023)", Some(("One Night", 2023))),
        ("Hotel Series (2015)", Some(("Hotel Series", 2015))),
        ("1923 (2022)", Some(("1923", 2022))),
        ("Show(2010)", Some(("Show", 2010))),
        ("Show  (2010)", Some(("Show", 2010))),
        ("x(2010)", Some(("x", 2010))),
        ("Show (US) (2010)", Some(("Show (US)", 2010))),
        ("Show (2010) [1080p]", Some(("Show", 2010))),
        ("Show (2010) (2011)", Some(("Show", 2010))),
        ("Show - (2010)", Some(("Show -", 2010))),
        ("Émission (2010)", Some(("Émission", 2010))),
        ("Show", None),
        ("Show 2010", None),
        ("Show [2010]", None),
        ("Show (201)", None),
        ("Show (20100)", None),
        ("Show (２０１０)", None),
        ("(2010)", None),
        ("(2010) Show", None),
        // Jellyfin would give an empty title; the folder's name is kept instead.
        (" (2010)", None),
    ];
    let wrong: Vec<String> = table
        .iter()
        .filter_map(|&(folder, want)| {
            let got = series_title_and_year(folder);
            (got != want).then(|| format!("{folder:?}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
