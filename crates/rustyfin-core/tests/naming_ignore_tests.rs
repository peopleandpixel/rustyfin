//! Folder and file names the scanner skips (R1-12), after Jellyfin's IgnorePatterns.

use rustyfin_core::naming::{is_ignored_dir, is_ignored_file};

#[test]
fn ignored_dir_names() {
    for name in [
        ".@__thumb",
        ".hidden",
        ".actors",
        "@eaDir",
        "@Recycle",
        "@__thumb",
        "#recycle",
        "#Recycle",
        "$RECYCLE.BIN",
        "lost+found",
        "System Volume Information",
        "metadata",
        "Metadata",
        "extrafanart",
        "extrathumbs",
        "subs",
        "Subs",
        "TempRec",
        "TempSBE",
        "eaDir",
        "ps3_update",
        "ps3_vprm",
        "sample",
        "Sample",
        "MINTA",
        "Arvel (2012).trickplay",
    ] {
        assert!(is_ignored_dir(name), "{name:?} should be ignored");
    }
    for name in [
        "Season 1",
        "Arvel (2012)",
        "Featurettes",
        "Extras",
        "Samples",
        "Sample Movie (2010)",
        "metadata2",
        "#1 Cheerleader Camp (2010)",
        "The .hack Series",
        "Subs and Dubs",
        "trickplay",
    ] {
        assert!(!is_ignored_dir(name), "{name:?} should not be ignored");
    }
}

#[test]
fn ignored_file_names() {
    for name in [
        ".DS_Store",
        "._Arvel (2012).mkv",
        ".hidden.mkv",
        "small.jpg",
        "Small.JPG",
        "albumart.jpg",
        "Thumbs.db",
        "thumbs.db",
        "Arvel.bts",
        "Arvel.mkv.sync",
        "Arvel.trickplay",
        "sample.mkv",
        "Sample.avi",
        "sample.m2ts",
        "Arvel.sample.mkv",
        "Arvel (2012).Sample.MKV",
        "minta.mp4",
        "Arvel.minta.m2ts",
    ] {
        assert!(is_ignored_file(name), "{name:?} should be ignored");
    }
    for name in [
        "Arvel (2012).mkv",
        "Sample Movie (2010).mkv",
        "Arvel-sample.mkv",
        "sample.toolong",
        "Velloran (1984) - Sampler.mkv",
        "poster.jpg",
        "albumart.png",
        "Show - S01E01.mkv",
    ] {
        assert!(!is_ignored_file(name), "{name:?} should not be ignored");
    }
}

#[test]
fn b32_disc_folders_are_ignored() {
    // The streams of a Blu-ray or DVD folder are no titles of their own (BaseVideoResolver.cs:261-286).
    for name in ["BDMV", "bdmv", "VIDEO_TS", "Video_TS"] {
        assert!(is_ignored_dir(name), "{name:?} should be ignored");
    }
    for name in ["BDMV Rips", "VIDEO_TS2", "STREAM", "AUDIO_TS"] {
        assert!(!is_ignored_dir(name), "{name:?} should not be ignored");
    }
}
