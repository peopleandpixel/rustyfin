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
        ("Arvel (2012) Bluray-2160p", "Arvel", Some(2012)),
        ("Arvel", "Arvel", None),
        (
            "A Most Patient Harbor (2014) Remux-1080p",
            "A Most Patient Harbor",
            Some(2014),
        ),
        (
            "Hazel.Mirrors.2012.BRRip.XviD.7qGG",
            "Hazel Mirrors",
            Some(2012),
        ),
        (
            "Fickle Gardens U S 2007 LiMiTED DVDRip XviD QRTZLAB",
            "Fickle Gardens U S",
            Some(2007),
        ),
        (
            "Fickle.Gardens.U.S.2007.LiMiTED.DVDRip.XviD-QRTZLAB.CD1",
            "Fickle Gardens U S",
            Some(2007),
        ),
        (
            "the.mangle.2013.720p.bluray.x264.dts-quillgrp",
            "the mangle",
            Some(2013),
        ),
        (
            "Sky Tiders (1977) WEBDL-2160p Proper",
            "Sky Tiders",
            Some(1977),
        ),
        ("Alderlune³ (1992) Bluray-1080p", "Alderlune³", Some(1992)),
        ("1917 (2019)", "1917", Some(2019)),
        ("1917", "1917", None),
        ("Se7en (1995)", "Se7en", Some(1995)),
        ("Movie - (2010)", "Movie", Some(2010)),
        ("Clover Vane cd1", "Clover Vane", None),
        ("Arvel Bluray-2160p", "Arvel", None),
        ("Movie [Director's Cut]", "Movie", None),
        ("[YTS] Movie Name", "Movie Name", None),
        ("Movie Name - 2", "Movie Name", None),
        ("Some_Movie_Name", "Some Movie Name", None),
        ("News.2010.10.12", "News 2010 10 12", None),
        ("Brindlemoor ()", "Brindlemoor", None),
        ("Hollow Pockets 2 (0)", "Hollow Pockets 2", None),
        (
            "The Wandering Clerk () Bluray-1080p",
            "The Wandering Clerk",
            None,
        ),
        (
            "Mild Ferns TV Series 2007 - IMDb ()",
            "Mild Ferns TV Series",
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
            ("Alderlune³", "alderlune³"),
            ("The Mangle", "themangle"),
            ("Se7en", "se7en"),
            ("Amélie", "amélie"),
            ("Marble: Harbor – Undertow", "marbleharborundertow"),
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
        ("Arvel (2012) Bluray-2160p", Some(2160)),
        ("The.Mangle.2013.720p.bluray.x264", Some(720)),
        ("7qGG-hmir480p", Some(480)),
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
fn group_arvel() {
    assert_eq!(
        movie_groups(
            "Arvel (2012)",
            &["Arvel.mkv", "Arvel (2012) Bluray-2160p.mkv"]
        ),
        [group(
            "Arvel",
            Some(2012),
            &["Arvel (2012) Bluray-2160p.mkv", "Arvel.mkv"]
        )]
    );
}

#[test]
fn group_clover_vane_parts() {
    assert_eq!(
        movie_groups(
            "Clover Vane (1941)",
            &[
                "Clover Vane cd1.avi",
                "Clover Vane (1941) Bluray-2160p.mkv",
                "Clover Vane cd2.avi",
            ]
        ),
        [group(
            "Clover Vane",
            Some(1941),
            &[
                "Clover Vane (1941) Bluray-2160p.mkv",
                "Clover Vane cd1.avi",
                "Clover Vane cd2.avi",
            ]
        )]
    );
}

#[test]
fn group_alderlune3_two_films() {
    // Two titles are two films. "Alderlunes" sorts first, as 's' is below the
    // first byte of '³'.
    assert_eq!(
        movie_groups(
            "Alderlune³ (1992)",
            &[
                "Alderlune³ (1992) Bluray-1080p.mkv",
                "Alderlunes (1986) Bluray-1080p.mkv",
            ]
        ),
        [
            group(
                "Alderlunes",
                Some(1986),
                &["Alderlunes (1986) Bluray-1080p.mkv"]
            ),
            group(
                "Alderlune³",
                Some(1992),
                &["Alderlune³ (1992) Bluray-1080p.mkv"]
            ),
        ]
    );
}

#[test]
fn group_odd_pudding_folder_name_first() {
    // The file named like its folder is the primary, ahead of a higher
    // resolution.
    assert_eq!(
        movie_groups(
            "Odd Pudding (2003)",
            &[
                "Odd Pudding.mkv",
                "Odd Pudding (2003) Bluray-1080p.mkv",
                "Odd Pudding (2003).mkv",
            ]
        ),
        [group(
            "Odd Pudding",
            Some(2003),
            &[
                "Odd Pudding (2003).mkv",
                "Odd Pudding (2003) Bluray-1080p.mkv",
                "Odd Pudding.mkv",
            ]
        )]
    );
}

#[test]
fn group_fickle_gardens_folder_year() {
    // The folder's year wins over the year in a file name.
    assert_eq!(
        movie_groups(
            "Fickle Gardens (2007)",
            &[
                "Fickle Gardens.mkv",
                "Fickle Gardens (2008) Bluray-1080p.mkv"
            ]
        ),
        [group(
            "Fickle Gardens",
            Some(2007),
            &[
                "Fickle Gardens (2008) Bluray-1080p.mkv",
                "Fickle Gardens.mkv"
            ]
        )]
    );
}

#[test]
fn group_the_mangle() {
    assert_eq!(
        movie_groups(
            "The Mangle (2013)",
            &[
                "the.mangle.2013.720p.bluray.x264.dts-quillgrp.mkv",
                "The Mangle (2013) Bluray-1080p.mkv",
            ]
        ),
        [group(
            "The Mangle",
            Some(2013),
            &[
                "The Mangle (2013) Bluray-1080p.mkv",
                "the.mangle.2013.720p.bluray.x264.dts-quillgrp.mkv",
            ]
        )]
    );
}

#[test]
fn group_resolution_order() {
    assert_eq!(
        movie_groups(
            "A Most Patient Harbor (2014)",
            &[
                "A Most Patient Harbor (2014) Bluray-720p.mkv",
                "A Most Patient Harbor (2014) Remux-1080p.mkv",
                "A Most Patient Harbor (2014) Bluray-1080p.mkv",
            ]
        ),
        [group(
            "A Most Patient Harbor",
            Some(2014),
            &[
                "A Most Patient Harbor (2014) Bluray-1080p.mkv",
                "A Most Patient Harbor (2014) Remux-1080p.mkv",
                "A Most Patient Harbor (2014) Bluray-720p.mkv",
            ]
        )]
    );
}

#[test]
fn group_hazel_mirrors_scene_file() {
    // The file name has no title; the folder has one.
    assert_eq!(
        movie_groups("Hazel.Mirrors.2012.BRRip.XviD.7qGG", &["7qGG-hmir480p.avi"]),
        [group("Hazel Mirrors", Some(2012), &["7qGG-hmir480p.avi"])]
    );
}

#[test]
fn group_tin_ladle_empty_year_folder() {
    assert_eq!(
        movie_groups(
            "The Tin Ladle ()",
            &["The Tin Ladle (2023) WEBDL-2160p.mkv"]
        ),
        [group(
            "The Tin Ladle",
            Some(2023),
            &["The Tin Ladle (2023) WEBDL-2160p.mkv"]
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
fn group_sky_tiders_saga() {
    assert_eq!(
        movie_groups(
            "Sky Tiders Complete Saga",
            &[
                "Sky Tiders Episode IV - Marble Orchard (1977).mkv",
                "Sky Tiders Episode I - Pale Cartographer (1999).mkv",
            ]
        ),
        [
            group(
                "Sky Tiders Episode I - Pale Cartographer",
                Some(1999),
                &["Sky Tiders Episode I - Pale Cartographer (1999).mkv"]
            ),
            group(
                "Sky Tiders Episode IV - Marble Orchard",
                Some(1977),
                &["Sky Tiders Episode IV - Marble Orchard (1977).mkv"]
            ),
        ]
    );
}

#[test]
fn group_root_files() {
    // A scan that read the root's name would call this film "Films" (2024).
    assert_eq!(
        groups_in(Path::new("/media/Films 2024"), &["The Attic.avi"], true),
        [group("The Attic", None, &["The Attic.avi"])]
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
    // By whole name, "Tin Kites (" would sort before "Tin Kites.", as a space is below a dot.
    assert_eq!(
        movie_groups(
            "Tin Kites (2014)",
            &["Tin Kites (2014) BR-DISK.m2ts", "Tin Kites.mkv"]
        ),
        [group(
            "Tin Kites",
            Some(2014),
            &["Tin Kites.mkv", "Tin Kites (2014) BR-DISK.m2ts"]
        )]
    );
}

#[test]
fn b32_series_title_and_year() {
    // Jellyfin's TitleWithYearRegex: the text before the first "(dddd)", trimmed, and that year.
    let table: &[(&str, Option<(&str, i32)>)] = &[
        ("Owl Hollow (2023)", Some(("Owl Hollow", 2023))),
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
