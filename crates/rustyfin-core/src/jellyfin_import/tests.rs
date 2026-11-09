//! B20: unit tests for the Jellyfin importer's pure parts (written with the spec, before the code).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use rustyfin_db::repo_jellyfin_import::{SourceStreams, StreamRow, UserDataRow};
use rustyfin_model::image::ImageType;
use rustyfin_model::user::User;
use uuid::Uuid;

use super::images::{resolve, signature};
use super::job::{ImportJob, ImportStage, ImportStatus, JobState};
use super::merge::{MappedTracks, convert_rating, harmonize, merge};
use super::metadata::planned_credits;
use super::paths::{PathMapping, suggest_metadata, translate_path};
use super::plan::{UserMatch, match_user_name, plan_users};
use super::source::{
    FreeSpace, IN_MEMORY_LIMIT, JELLYFIN_TABLES, JfCredit, JfImage, JfStream, JfUser, JfUserData,
    ProviderIds, StreamKind, copy_at_most, copy_fits, free_space, guid_from_blob, guid_from_text,
    locate, open_snapshot, open_snapshot_with_free, parse_jellyfin_date, parse_old_images,
    parse_provider_ids, read_library_folders, read_snapshot, reads_in_memory,
};
use super::tracks::{choose_source, map_track, normalize_codec};
use super::{
    ConfirmedMatch, ImportError, ImportReport, ItemReport, Layout, LayoutChoice, SkipReason,
    TrackReport, UserAction, UserRow, WatchDataReport, fill_suggested_facts,
};

/// A temp folder removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("rustyfin_b20_unit_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// Creates an empty `jellyfin.db` in `rel` (and its folders) and returns that folder.
    fn db_in(&self, rel: &str) -> PathBuf {
        let dir = self.0.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("jellyfin.db"), b"").unwrap();
        dir
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pair(from: &str, to: &str) -> PathMapping {
    PathMapping {
        from: from.to_string(),
        to: to.to_string(),
    }
}

// 2.8: component-wise prefixes, longest From wins, trailing slashes ignored.
#[test]
fn b20_translate_path_by_longest_component_prefix() {
    let pairs = vec![
        pair("/data/movies", "/srv/films"),
        pair("/data/tv/", "/srv/tv"),
        pair("/data/movies/kids/", "/srv/kids/"),
        pair("%MetadataPath%", "/cfg/metadata"),
    ];
    let t = |p: &str| translate_path(p, &pairs);
    assert_eq!(
        t("/data/movies/A (2001)/A.mkv").as_deref(),
        Some("/srv/films/A (2001)/A.mkv")
    );
    assert_eq!(
        t("/data/movies/kids/B/B.mkv").as_deref(),
        Some("/srv/kids/B/B.mkv")
    );
    assert_eq!(t("/data/movies").as_deref(), Some("/srv/films"));
    assert_eq!(
        t("/data/tv/Show/Season 01").as_deref(),
        Some("/srv/tv/Show/Season 01")
    );
    assert_eq!(
        t("%MetadataPath%/library/ab/abcd/poster.jpg").as_deref(),
        Some("/cfg/metadata/library/ab/abcd/poster.jpg")
    );
    assert_eq!(t("/data/movies2/C.mkv"), None, "not a component prefix");
    assert_eq!(
        t("/Data/Movies/A.mkv"),
        None,
        "paths compare case-sensitively"
    );
    assert_eq!(t("/elsewhere/x.mkv"), None);
    assert_eq!(translate_path("/data/movies/A.mkv", &[]), None);
}

// 2.13: Jellyfin's date formats, all UTC; anything else is no date.
#[test]
fn b20_parse_jellyfin_dates() {
    let base = Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap();
    let frac = base + chrono::Duration::nanoseconds(123_456_700);
    assert_eq!(
        parse_jellyfin_date("2025-01-02 03:04:05.1234567"),
        Some(frac)
    );
    assert_eq!(
        parse_jellyfin_date("2025-01-02 03:04:05.1234567Z"),
        Some(frac)
    );
    assert_eq!(parse_jellyfin_date("2025-01-02 03:04:05"), Some(base));
    assert_eq!(parse_jellyfin_date("2025-01-02 03:04:05Z"), Some(base));
    assert_eq!(
        parse_jellyfin_date("2025-01-02 03:04:05.5"),
        Some(base + chrono::Duration::milliseconds(500))
    );
    assert_eq!(
        parse_jellyfin_date("2025-01-02T03:04:05.1234567Z"),
        Some(frac)
    );
    assert_eq!(parse_jellyfin_date("2025-01-02T05:04:05+02:00"), Some(base));
    assert_eq!(parse_jellyfin_date(""), None);
    assert_eq!(parse_jellyfin_date("never"), None);
    assert_eq!(parse_jellyfin_date("2025-02-30 00:00:00"), None);
}

// 2.3a: GUIDs as TEXT in any case, or as 16-byte BLOBs in .NET byte order.
#[test]
fn b20_guids_from_text_and_blobs() {
    let id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap();
    assert_eq!(
        guid_from_text("00112233-4455-6677-8899-AABBCCDDEEFF"),
        Some(id)
    );
    assert_eq!(
        guid_from_text("00112233-4455-6677-8899-aabbccddeeff"),
        Some(id)
    );
    assert_eq!(guid_from_text("not a guid"), None);
    assert_eq!(guid_from_text(""), None);
    let dotnet = [
        0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    assert_eq!(guid_from_blob(&dotnet), Some(id));
    assert_eq!(guid_from_blob(&dotnet[..15]), None);
    assert_eq!(guid_from_blob(&[]), None);
}

// 2.3a: provider ids parse leniently; bad values are ignored, not errors.
#[test]
fn b20_provider_ids_parse_leniently() {
    assert_eq!(
        parse_provider_ids("Tmdb=1|Imdb=tt0079540|Tvdb=3"),
        ProviderIds {
            tmdb: Some(1),
            tvdb: Some(3),
            imdb: Some("tt0079540".to_string()),
        }
    );
    assert_eq!(
        parse_provider_ids("Tmdb=abc|Imdb=nm0000123|Tvdb=-5|Zap2It=EP1|garbage|=7"),
        ProviderIds::default()
    );
    assert_eq!(parse_provider_ids("Imdb=tt|Tmdb=12a").imdb, None);
    assert_eq!(parse_provider_ids("tmdb=7").tmdb, Some(7));
    assert_eq!(parse_provider_ids(""), ProviderIds::default());

    let mut ids = ProviderIds::default();
    ids.set("Tvdb", "333");
    ids.set("Tmdb", "x1");
    ids.set("Imdb", "tt42");
    ids.set("MusicBrainzAlbum", "123");
    assert_eq!(
        ids,
        ProviderIds {
            tmdb: None,
            tvdb: Some(333),
            imdb: Some("tt42".to_string()),
        }
    );
}

// 2.1: jellyfin.db at <s>, <s>/data or <s>/data/data; D holds it, P is D's parent.
#[test]
fn b20_locate_finds_the_database_in_three_places() {
    let tmp = TempDir::new();
    let a = tmp.db_in("a");
    let found = locate(&a).unwrap();
    assert_eq!(found.data_path, a);
    assert_eq!(found.program_data_path, tmp.0);

    let b = tmp.db_in("b/data");
    let found = locate(&tmp.0.join("b")).unwrap();
    assert_eq!(found.data_path, b);
    assert_eq!(found.program_data_path, tmp.0.join("b"));

    let c = tmp.db_in("c/data/data");
    let found = locate(&tmp.0.join("c")).unwrap();
    assert_eq!(found.data_path, c);
    assert_eq!(found.program_data_path, tmp.0.join("c/data"));

    // The first hit wins.
    tmp.db_in("b");
    assert_eq!(locate(&tmp.0.join("b")).unwrap().data_path, tmp.0.join("b"));
}

#[test]
fn b20_locate_rejects_bad_sources() {
    let tmp = TempDir::new();
    let empty = tmp.0.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    match locate(&empty) {
        Err(ImportError::BadSource(msg)) => {
            for candidate in ["jellyfin.db", "data/jellyfin.db", "data/data/jellyfin.db"] {
                let path = empty.join(candidate);
                assert!(
                    msg.contains(path.to_str().unwrap()),
                    "{msg} names {candidate}"
                );
            }
        }
        other => panic!("expected BadSource, got {other:?}"),
    }
    for bad in ["", "relative/config"] {
        assert!(
            matches!(locate(Path::new(bad)), Err(ImportError::BadSource(_))),
            "{bad:?}"
        );
    }
    assert!(matches!(
        locate(&tmp.0.join("missing")),
        Err(ImportError::BadSource(_))
    ));
}

// 2.5: case-insensitive names; an exact-case match wins; otherwise two hits are ambiguous.
#[test]
fn b20_match_user_names() {
    let (alice, bob_title, bob_lower, unal) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let users = vec![
        (alice, "alice".to_string()),
        (bob_title, "Bob".to_string()),
        (bob_lower, "bob".to_string()),
        (unal, "Ünal".to_string()),
    ];
    assert_eq!(match_user_name("Alice", &users), UserMatch::Matched(alice));
    assert_eq!(
        match_user_name("bob", &users),
        UserMatch::Matched(bob_lower)
    );
    assert_eq!(
        match_user_name("Bob", &users),
        UserMatch::Matched(bob_title)
    );
    assert_eq!(match_user_name("BOB", &users), UserMatch::Ambiguous);
    assert_eq!(match_user_name("ünal", &users), UserMatch::Matched(unal));
    assert_eq!(match_user_name("carol", &users), UserMatch::NoMatch);
}

// R2: a picked user's name match is a suggestion. Its data goes into that account only when the
// request confirms the pairing by the account's id; any other confirmation is refused.
#[test]
fn b20_plan_users_needs_confirmed_matches() {
    let [alice, bob_title, bob_lower, erin] = [1, 2, 3, 4].map(Uuid::from_u128);
    let rustyfin = vec![
        (alice, "alice".to_string()),
        (bob_title, "Bob".to_string()),
        (bob_lower, "bob".to_string()),
        (erin, "erin".to_string()),
    ];
    let names = [
        "Alice", "erin", "BOB", "carol", "dave", "Bob", "bob", "ERIN",
    ];
    let jf_users: Vec<JfUser> = (0..)
        .zip(names)
        .map(|(n, name)| JfUser {
            id: Uuid::from_u128(100 + n as u128),
            name: name.to_string(),
            internal_id: n,
            is_admin: false,
        })
        .collect();
    let jf = |name: &str| jf_users.iter().find(|user| user.name == name).unwrap().id;
    let pair = |name: &str, user_id| ConfirmedMatch {
        jellyfin_id: jf(name),
        user_id,
    };
    let confirmed = [
        pair("Alice", alice),
        pair("Alice", alice),
        pair("BOB", bob_lower),
        pair("dave", erin),
        pair("Bob", bob_lower),
        pair("bob", bob_lower),
        pair("bob", alice),
        pair("ERIN", erin),
        ConfirmedMatch {
            jellyfin_id: Uuid::from_u128(999),
            user_id: alice,
        },
    ];
    let picked: Vec<Uuid> = jf_users
        .iter()
        .filter(|user| user.name != "ERIN")
        .map(|user| user.id)
        .collect();

    let rows = plan_users(&jf_users, Some(&picked), &confirmed, &rustyfin);
    let got: Vec<_> = rows
        .iter()
        .map(|row| {
            let suggested = row.suggested_match.as_ref();
            let suggested = suggested.map(|account| (account.user_id, account.name.as_str()));
            (
                row.name.as_str(),
                row.action,
                row.reason,
                row.user_id,
                suggested,
            )
        })
        .collect();
    let (matched, create, skip) = (UserAction::Match, UserAction::Create, UserAction::Skip);
    let ambiguous = Some(SkipReason::Ambiguous);
    let not_selected = Some(SkipReason::NotSelected);
    let not_confirmed = Some(SkipReason::NotConfirmed);
    let other = Some(SkipReason::ConfirmedOtherAccount);
    let expected = [
        ("Alice", matched, None, Some(alice), Some((alice, "alice"))),
        ("BOB", skip, ambiguous, None, None),
        ("Bob", skip, other, None, Some((bob_title, "Bob"))),
        ("bob", skip, other, None, Some((bob_lower, "bob"))),
        ("carol", create, None, None, None),
        ("dave", skip, other, None, None),
        ("erin", skip, not_confirmed, None, Some((erin, "erin"))),
        ("ERIN", skip, not_selected, None, None),
    ];
    assert_eq!(got, expected);

    // Without confirmations every name match is skipped, and the rest is as before.
    let rows = plan_users(&jf_users, None, &[], &rustyfin);
    let got: Vec<_> = rows
        .iter()
        .map(|row| (row.name.as_str(), row.action, row.reason))
        .collect();
    let expected = [
        ("Alice", skip, not_confirmed),
        ("BOB", skip, ambiguous),
        ("Bob", skip, not_confirmed),
        ("bob", skip, not_confirmed),
        ("carol", create, None),
        ("dave", create, None),
        ("erin", skip, not_confirmed),
        ("ERIN", skip, not_confirmed),
    ];
    assert_eq!(got, expected);
    assert!(rows.iter().all(|row| row.user_id.is_none()));
}

// B36: import polish

/// Jellyfin users with these names, ids 100, 101, ...
fn b36_jf_users(names: &[&str]) -> Vec<JfUser> {
    (0..)
        .zip(names)
        .map(|(n, name)| JfUser {
            id: Uuid::from_u128(100 + n as u128),
            name: name.to_string(),
            internal_id: n,
            is_admin: false,
        })
        .collect()
}

// F3: the warning flags every name that sign-in treats as the same (`names_match`: full case
// folding, composed or decomposed accents, compatibility forms), not only `to_lowercase` variants.
// Which account is suggested doesn't change: each Jellyfin name has an exact-case match.
#[test]
fn b36_case_variant_names_fold_like_sign_in() {
    let [
        strauss,
        strauss_upper,
        jose_nfc,
        jose_nfd,
        bob,
        bob_wide,
        odos,
        odos_lower,
        strausz,
    ] = [1, 2, 3, 4, 5, 6, 7, 8, 9].map(Uuid::from_u128);
    let rustyfin = vec![
        (strauss, "Strauß".to_string()),
        (strauss_upper, "STRAUSS".to_string()),
        (jose_nfc, "Jos\u{e9}".to_string()),
        (jose_nfd, "Jose\u{301}".to_string()),
        (bob, "bob".to_string()),
        (bob_wide, "\u{ff42}\u{ff4f}\u{ff42}".to_string()),
        (odos, "ΟΔΟΣ".to_string()),
        (odos_lower, "οδοσ".to_string()),
        (strausz, "Strausz".to_string()),
    ];
    let jf_users = b36_jf_users(&["Strauß", "STRAUSS", "Jose\u{301}", "bob", "ΟΔΟΣ"]);
    let rows = plan_users(&jf_users, None, &[], &rustyfin);
    let got: Vec<_> = rows
        .iter()
        .map(|row| {
            let suggested = row.suggested_match.as_ref().expect("an exact-case match");
            (
                row.name.as_str(),
                suggested.user_id,
                suggested.other_case_names.clone(),
            )
        })
        .collect();
    let names = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    let expected = [
        ("bob", bob, names(&["\u{ff42}\u{ff4f}\u{ff42}"])),
        ("Jose\u{301}", jose_nfd, names(&["Jos\u{e9}"])),
        ("STRAUSS", strauss_upper, names(&["Strauß"])),
        ("Strauß", strauss, names(&["STRAUSS"])),
        ("ΟΔΟΣ", odos, names(&["οδοσ"])),
    ];
    assert_eq!(got, expected);
}

// F4: the facts come from the users the plan read, once. A suggestion whose account isn't among
// them is dropped, never sent with placeholder facts.
#[test]
fn b36_suggested_facts_come_from_the_planning_read() {
    let [kept, gone, plain] = [1, 2, 3].map(Uuid::from_u128);
    let created = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
    let login = Utc.with_ymd_and_hms(2026, 5, 6, 7, 8, 9).unwrap();
    let account = |id, name: &str, is_administrator, last_login_date| User {
        id,
        name: name.to_string(),
        password_hash: String::new(),
        is_administrator,
        created_at: created,
        last_login_date,
        configuration: serde_json::Value::Null,
    };
    let rustyfin = vec![
        (kept, "kept".to_string()),
        (gone, "gone".to_string()),
        (plain, "plain".to_string()),
    ];
    let mut rows = plan_users(
        &b36_jf_users(&["kept", "gone", "plain"]),
        None,
        &[],
        &rustyfin,
    );
    assert!(rows.iter().all(|row| row.suggested_match.is_some()));

    let accounts = [
        account(kept, "kept", true, Some(login)),
        account(plain, "plain", false, None),
    ];
    let pending = HashSet::from([kept, gone]);
    fill_suggested_facts(&accounts, &pending, &mut rows);
    let got: Vec<_> = rows
        .iter()
        .map(|row| {
            let facts = row.suggested_match.as_ref().map(|s| {
                (
                    s.user_id,
                    s.is_administrator,
                    s.created_at,
                    s.last_login_date,
                    s.setup_code_pending,
                )
            });
            (row.name.as_str(), facts)
        })
        .collect();
    let expected = [
        ("gone", None),
        ("kept", Some((kept, true, created, Some(login), true))),
        ("plain", Some((plain, false, created, None, false))),
    ];
    assert_eq!(got, expected);
}

// ── 2b: the import job (spec 2.19, 2.21, 2.25) ──

/// A small report whose one created user holds `code`.
fn b20_report_with_code(code: &str) -> ImportReport {
    ImportReport {
        layout: Layout::New,
        data_path: "/config/data".to_string(),
        path_mappings: Vec::new(),
        suggested_path_mappings: Vec::new(),
        unpaired_library_paths: Vec::new(),
        users: vec![UserRow {
            jellyfin_id: Uuid::from_u128(7),
            name: "bob".to_string(),
            was_administrator: false,
            action: UserAction::Create,
            reason: None,
            user_id: Some(Uuid::from_u128(8)),
            suggested_match: None,
            code: Some(code.to_string()),
        }],
        items: ItemReport {
            total: 0,
            matched_by_path: 0,
            matched_by_id: 0,
            unmatched: 0,
            unmatched_samples: Vec::new(),
        },
        watch_data: WatchDataReport::default(),
        tracks: TrackReport::default(),
        metadata: Default::default(),
    }
}

#[test]
fn b20_job_is_idle_at_first() {
    let status = ImportJob::default().status();
    assert_eq!((status.state, status.stage), (JobState::Idle, None));
    assert_eq!((status.done, status.total), (0, 0));
    assert!(status.result.is_none() && status.error.is_none());
}

#[test]
fn b20_job_runs_one_at_a_time() {
    let job = ImportJob::default();
    let clone = job.clone();
    let handle = job.try_start().expect("an idle job starts");
    assert!(clone.try_start().is_none(), "a second run while one runs");
    let status = clone.status();
    assert_eq!(
        (status.state, status.stage),
        (JobState::Running, Some(ImportStage::Reading))
    );
    handle.stage(ImportStage::Users, 3);
    handle.progress(2);
    let status = job.status();
    assert_eq!(status.stage, Some(ImportStage::Users));
    assert_eq!((status.done, status.total), (2, 3));
    handle.stage(ImportStage::Matching, 10);
    let status = job.status();
    assert_eq!(
        (status.done, status.total),
        (0, 10),
        "a new stage starts at 0"
    );
    handle.complete(b20_report_with_code("ABCD-EFGH-JKMN"));
    let status = job.status();
    assert_eq!((status.state, status.stage), (JobState::Completed, None));
    assert!(status.error.is_none());

    let again = job.try_start().expect("a finished job starts again");
    let status = job.status();
    assert_eq!(status.state, JobState::Running);
    assert!(status.result.is_none(), "a new run drops the old report");
    again.fail("stopped".to_string());
}

#[test]
fn b20_job_shows_codes_once() {
    let job = ImportJob::default();
    let handle = job.try_start().unwrap();
    handle.complete(b20_report_with_code("ABCD-EFGH-JKMN"));
    let code = |status: &ImportStatus| status.result.as_ref().unwrap().users[0].code.clone();
    let first = job.status();
    assert_eq!(code(&first), Some("ABCD-EFGH-JKMN".to_string()));
    let second = job.status();
    assert_eq!(second.state, JobState::Completed);
    assert_eq!(code(&second), None, "the code is shown once");
    let user_id = second.result.unwrap().users[0].user_id;
    assert_eq!(
        user_id,
        Some(Uuid::from_u128(8)),
        "the rest of the report stays"
    );
}

#[test]
fn b20_job_reports_failures() {
    let job = ImportJob::default();
    job.try_start().unwrap().fail("cannot read x".to_string());
    let status = job.status();
    assert_eq!((status.state, status.stage), (JobState::Failed, None));
    assert_eq!(status.error.as_deref(), Some("cannot read x"));
    assert!(status.result.is_none());

    // A handle dropped without an ending fails the run; the job starts again.
    drop(job.try_start().unwrap());
    let status = job.status();
    assert_eq!(status.state, JobState::Failed);
    assert_eq!(
        status.error.as_deref(),
        Some("the import stopped before it finished")
    );
    assert!(job.try_start().is_some());
}

// ── 2c: watch data and track choices (spec 2.12-2.16, 2.28-2.30) ──

/// Noon of January `day`, 2025.
fn at(day: u32) -> Option<DateTime<Utc>> {
    Some(Utc.with_ymd_and_hms(2025, 1, day, 12, 0, 0).unwrap())
}

/// A Jellyfin row of `user` for `item` with nothing set.
fn jf_row(user: u128, item: u128) -> JfUserData {
    JfUserData {
        item: Uuid::from_u128(item),
        user: Uuid::from_u128(user),
        played: false,
        play_count: 0,
        position_ticks: 0,
        last_played: None,
        is_favorite: false,
        rating: None,
        likes: None,
        audio: None,
        subtitle: None,
    }
}

/// User 1's row of item 10: played twice, last on January 5, stopped at 600 ticks.
fn jf_watched() -> JfUserData {
    JfUserData {
        played: true,
        play_count: 2,
        position_ticks: 600,
        last_played: at(5),
        is_favorite: true,
        rating: Some(7.6),
        likes: Some(true),
        audio: Some(1),
        subtitle: Some(3),
        ..jf_row(1, 10)
    }
}

/// The Rustyfin indexes of `jf_watched`'s track choices.
const MAPPED: MappedTracks = MappedTracks {
    audio: Some(2),
    audio_key: None,
    subtitle: Some(4),
    subtitle_key: None,
};

#[test]
fn b20_normalize_codecs() {
    for (codec, normal) in [
        ("PGSSUB", "hdmv_pgs_subtitle"),
        ("hdmv_pgs_subtitle", "hdmv_pgs_subtitle"),
        ("dvdsub", "dvd_subtitle"),
        ("DVBSUB", "dvb_subtitle"),
        ("srt", "subrip"),
        ("SubRip", "subrip"),
        ("AAC", "aac"),
        ("eac3", "eac3"),
    ] {
        assert_eq!(normalize_codec(codec), normal, "{codec}");
    }
}

#[test]
fn b20_convert_ratings() {
    for (rating, converted) in [
        (7.6, Some(8)),
        (7.5, Some(8)),
        (7.4, Some(7)),
        (9.5, Some(10)),
        (0.0, Some(0)),
        (10.0, Some(10)),
        (-1.0, Some(0)),
        (11.2, Some(10)),
        (f64::INFINITY, Some(10)),
        (f64::NEG_INFINITY, Some(0)),
        (f64::NAN, None),
    ] {
        assert_eq!(convert_rating(rating), converted, "{rating}");
    }
}

#[test]
fn b20_harmonize_keeps_the_newest_row() {
    // The latest date wins over a higher count; no date is the oldest.
    let older = JfUserData {
        last_played: at(1),
        play_count: 9,
        position_ticks: 900,
        ..jf_row(1, 10)
    };
    let newer = JfUserData {
        last_played: at(2),
        play_count: 1,
        ..jf_row(1, 10)
    };
    let undated = JfUserData {
        play_count: 50,
        position_ticks: 5000,
        ..jf_row(1, 10)
    };
    let other_user = JfUserData {
        last_played: at(1),
        ..jf_row(2, 10)
    };
    let other_item = JfUserData {
        played: true,
        ..jf_row(1, 5)
    };
    let rows = harmonize(vec![
        older,
        undated.clone(),
        other_user.clone(),
        newer.clone(),
        other_item.clone(),
    ]);
    assert_eq!(
        rows,
        vec![other_item, newer, other_user],
        "one row per user and item, sorted by user, then item"
    );
    assert_eq!(harmonize(vec![undated.clone()]), vec![undated]);
    assert!(harmonize(Vec::new()).is_empty());
}

#[test]
fn b20_harmonize_breaks_ties_by_count_then_position() {
    let base = JfUserData {
        last_played: at(3),
        play_count: 2,
        position_ticks: 10,
        ..jf_row(1, 10)
    };
    let more_plays = JfUserData {
        play_count: 3,
        position_ticks: 5,
        ..base.clone()
    };
    let further = JfUserData {
        position_ticks: 20,
        ..base.clone()
    };
    let favorite = JfUserData {
        is_favorite: true,
        ..base.clone()
    };
    assert_eq!(
        harmonize(vec![base.clone(), more_plays.clone()]),
        vec![more_plays]
    );
    assert_eq!(
        harmonize(vec![further.clone(), base.clone()]),
        vec![further]
    );
    assert_eq!(
        harmonize(vec![base.clone(), favorite.clone()]),
        vec![base.clone()],
        "a full tie keeps the first"
    );
    assert_eq!(harmonize(vec![favorite.clone(), base]), vec![favorite]);
}

#[test]
fn b20_merge_without_a_rustyfin_row() {
    let expected = UserDataRow {
        played: true,
        play_count: 2,
        position_ticks: 600,
        last_played: at(5),
        is_favorite: true,
        user_rating: Some(8),
        likes: Some(true),
        audio: Some(2),
        audio_key: None,
        subtitle: Some(4),
        subtitle_key: None,
    };
    assert_eq!(
        merge(None, &jf_watched(), MAPPED),
        expected,
        "tracks are the mapped indexes"
    );
    // Without a Rustyfin row even an undated Jellyfin row is newer; unmapped tracks stay unset.
    let undated = JfUserData {
        position_ticks: 900,
        audio: Some(1),
        ..jf_row(1, 10)
    };
    let expected = UserDataRow {
        position_ticks: 900,
        ..UserDataRow::default()
    };
    assert_eq!(merge(None, &undated, MappedTracks::default()), expected);
}

#[test]
fn b20_merge_keeps_newer_rustyfin_progress() {
    let stored = UserDataRow {
        played: false,
        play_count: 5,
        position_ticks: 100,
        last_played: at(9),
        is_favorite: false,
        user_rating: Some(3),
        likes: Some(false),
        audio: Some(7),
        audio_key: None,
        subtitle: None,
        subtitle_key: None,
    };
    // Played and favorite if either; the larger count, the later date, Rustyfin's rating,
    // likes, position and tracks.
    let expected = UserDataRow {
        played: true,
        is_favorite: true,
        ..stored.clone()
    };
    assert_eq!(merge(Some(&stored), &jf_watched(), MAPPED), expected);
    let undated = JfUserData {
        last_played: None,
        ..jf_watched()
    };
    assert_eq!(
        merge(Some(&stored), &undated, MAPPED),
        expected,
        "an undated Jellyfin row is older than a dated Rustyfin row"
    );
}

#[test]
fn b20_merge_takes_newer_jellyfin_progress() {
    let stored = UserDataRow {
        play_count: 1,
        position_ticks: 50,
        last_played: at(1),
        audio: Some(7),
        subtitle: Some(0),
        ..UserDataRow::default()
    };
    // A track without a mapped value keeps Rustyfin's; subtitle -1 (off) is a value.
    let tracks = MappedTracks {
        audio: None,
        audio_key: None,
        subtitle: Some(-1),
        subtitle_key: None,
    };
    let expected = UserDataRow {
        played: true,
        play_count: 2,
        position_ticks: 600,
        last_played: at(5),
        is_favorite: true,
        user_rating: Some(8),
        likes: Some(true),
        audio: Some(7),
        audio_key: None,
        subtitle: Some(-1),
        subtitle_key: None,
    };
    assert_eq!(merge(Some(&stored), &jf_watched(), tracks), expected);

    // A Rustyfin row without a date is older than a dated Jellyfin row.
    let undated = UserDataRow {
        last_played: None,
        play_count: 4,
        ..stored
    };
    let merged = merge(Some(&undated), &jf_watched(), MAPPED);
    assert_eq!(
        (merged.position_ticks, merged.last_played, merged.play_count),
        (600, at(5), 4)
    );
    assert_eq!((merged.audio, merged.subtitle), (Some(2), Some(4)));
}

#[test]
fn b20_merge_on_a_tie_keeps_rustyfin_progress() {
    let stored = UserDataRow {
        position_ticks: 70,
        last_played: at(5),
        audio: Some(1),
        ..UserDataRow::default()
    };
    let expected = UserDataRow {
        played: true,
        play_count: 2,
        is_favorite: true,
        user_rating: Some(8),
        likes: Some(true),
        ..stored.clone()
    };
    assert_eq!(
        merge(Some(&stored), &jf_watched(), MAPPED),
        expected,
        "the same date"
    );

    let stored = UserDataRow {
        position_ticks: 70,
        ..UserDataRow::default()
    };
    let undated = JfUserData {
        position_ticks: 900,
        audio: Some(1),
        ..jf_row(1, 10)
    };
    let tracks = MappedTracks {
        audio: Some(2),
        audio_key: None,
        subtitle: None,
        subtitle_key: None,
    };
    assert_eq!(
        merge(Some(&stored), &undated, tracks),
        stored,
        "neither side has a date"
    );
}

#[test]
fn b20_merge_moves_keys_with_indexes() {
    let key = |name: &str| Some(name.to_string());
    let stored = UserDataRow {
        last_played: at(1),
        audio: Some(7),
        audio_key: key("rf audio"),
        subtitle: Some(0),
        subtitle_key: key("rf subtitle"),
        ..UserDataRow::default()
    };
    // A newer Jellyfin row: a mapped index brings its key, an unmapped one keeps Rustyfin's pair.
    let tracks = MappedTracks {
        audio: Some(2),
        audio_key: key("jf audio"),
        subtitle: None,
        subtitle_key: None,
    };
    let merged = merge(Some(&stored), &jf_watched(), tracks.clone());
    assert_eq!(
        (
            merged.audio,
            merged.audio_key,
            merged.subtitle,
            merged.subtitle_key
        ),
        (Some(2), key("jf audio"), Some(0), key("rf subtitle"))
    );
    // Subtitles off name no stream.
    let off = MappedTracks {
        subtitle: Some(-1),
        ..MappedTracks::default()
    };
    let merged = merge(Some(&stored), &jf_watched(), off);
    assert_eq!((merged.subtitle, merged.subtitle_key), (Some(-1), None));
    // An older Jellyfin row keeps both of Rustyfin's pairs.
    let newer = UserDataRow {
        last_played: at(9),
        ..stored.clone()
    };
    let merged = merge(Some(&newer), &jf_watched(), tracks);
    assert_eq!(
        (
            merged.audio,
            merged.audio_key,
            merged.subtitle,
            merged.subtitle_key
        ),
        (Some(7), key("rf audio"), Some(0), key("rf subtitle"))
    );
}

#[test]
fn b20_merge_ratings_and_likes() {
    let unrated = UserDataRow {
        last_played: at(9),
        ..UserDataRow::default()
    };
    let merged = merge(Some(&unrated), &jf_watched(), MAPPED);
    assert_eq!(
        (merged.user_rating, merged.likes),
        (Some(8), Some(true)),
        "Jellyfin's when Rustyfin has none"
    );
    let rated = UserDataRow {
        user_rating: Some(0),
        likes: Some(false),
        ..unrated.clone()
    };
    let merged = merge(Some(&rated), &jf_watched(), MAPPED);
    assert_eq!(
        (merged.user_rating, merged.likes),
        (Some(0), Some(false)),
        "Rustyfin's own when set"
    );
    let not_a_number = JfUserData {
        rating: Some(f64::NAN),
        likes: None,
        ..jf_watched()
    };
    let merged = merge(Some(&unrated), &not_a_number, MAPPED);
    assert_eq!((merged.user_rating, merged.likes), (None, None));
    let high = JfUserData {
        rating: Some(9.5),
        likes: Some(false),
        ..jf_watched()
    };
    let merged = merge(None, &high, MAPPED);
    assert_eq!((merged.user_rating, merged.likes), (Some(10), Some(false)));
}

#[test]
fn b20_merge_is_idempotent() {
    let stored = [
        None,
        Some(UserDataRow {
            play_count: 5,
            position_ticks: 100,
            last_played: at(9),
            user_rating: Some(3),
            audio: Some(7),
            ..UserDataRow::default()
        }),
        Some(UserDataRow {
            position_ticks: 50,
            last_played: at(1),
            subtitle: Some(0),
            ..UserDataRow::default()
        }),
        Some(UserDataRow {
            position_ticks: 70,
            last_played: at(5),
            ..UserDataRow::default()
        }),
        Some(UserDataRow {
            position_ticks: 70,
            ..UserDataRow::default()
        }),
    ];
    let undated = JfUserData {
        last_played: None,
        rating: None,
        ..jf_watched()
    };
    let off = MappedTracks {
        audio: None,
        audio_key: None,
        subtitle: Some(-1),
        subtitle_key: None,
    };
    for existing in &stored {
        for jf in [jf_watched(), undated.clone()] {
            for tracks in [MAPPED, MappedTracks::default(), off.clone()] {
                let once = merge(existing.as_ref(), &jf, tracks.clone());
                assert_eq!(
                    merge(Some(&once), &jf, tracks.clone()),
                    once,
                    "{existing:?} {jf:?} {tracks:?}"
                );
            }
        }
    }
}

/// A Rustyfin stream as the probe stores it: no language, title or flags.
fn rf_stream(index: i64, codec_type: &str, codec: &str) -> StreamRow {
    StreamRow {
        index,
        codec_type: codec_type.to_string(),
        codec: Some(codec.to_string()),
        language: None,
        title: None,
        is_external: false,
        path: None,
        is_default: false,
        is_forced: false,
    }
}

/// An embedded Jellyfin stream of item 10 without language, title or flags.
fn jf_stream(index: i64, kind: StreamKind, codec: &str) -> JfStream {
    JfStream {
        item: Uuid::from_u128(10),
        index,
        kind,
        codec: Some(codec.to_string()),
        language: None,
        title: None,
        is_external: false,
        path: None,
        is_default: false,
        is_forced: false,
    }
}

fn text(value: &str) -> Option<String> {
    Some(value.to_string())
}

#[test]
fn b20_choose_source() {
    let first = SourceStreams {
        path: "/m/A/a.mkv".to_string(),
        streams: vec![rf_stream(0, "video", "h264")],
    };
    let second = SourceStreams {
        path: "/m/A/b.mkv".to_string(),
        streams: Vec::new(),
    };
    let sources = [first.clone(), second.clone()];
    assert_eq!(choose_source(&sources, Some("/m/A/b.mkv")), Some(&second));
    assert_eq!(choose_source(&sources, Some("/m/A/a.mkv")), Some(&first));
    assert_eq!(
        choose_source(&sources, Some("/m/A/c.mkv")),
        None,
        "no source has the path: the choices were made on another file"
    );
    assert_eq!(choose_source(&sources, None), None, "no path: no source");
    assert_eq!(choose_source(&[], Some("/m/A/a.mkv")), None);
}

#[test]
fn b20_map_unique_tracks() {
    use StreamKind::{Audio, Subtitle, Video};
    let jf = [
        jf_stream(0, Video, "h264"),
        JfStream {
            language: text("eng"),
            ..jf_stream(1, Audio, "aac")
        },
        JfStream {
            language: text("ger"),
            ..jf_stream(2, Audio, "ac3")
        },
        JfStream {
            language: text("eng"),
            ..jf_stream(3, Subtitle, "srt")
        },
        JfStream {
            language: text("ger"),
            ..jf_stream(4, Subtitle, "PGSSUB")
        },
    ];
    // Rustyfin numbers the streams its own way and knows no languages.
    let rf = [
        rf_stream(0, "video", "h264"),
        rf_stream(1, "subtitle", "hdmv_pgs_subtitle"),
        rf_stream(2, "audio", "ac3"),
        rf_stream(3, "audio", "aac"),
        rf_stream(4, "subtitle", "subrip"),
    ];
    assert_eq!(map_track(1, Audio, &jf, &rf), Some(3));
    assert_eq!(map_track(2, Audio, &jf, &rf), Some(2));
    assert_eq!(map_track(3, Subtitle, &jf, &rf), Some(4), "srt = subrip");
    assert_eq!(map_track(4, Subtitle, &jf, &rf), Some(1), "pgssub");
    assert_eq!(map_track(9, Audio, &jf, &rf), None, "no Jellyfin stream 9");
    assert_eq!(map_track(0, Audio, &jf, &rf), None, "stream 0 is video");
    assert_eq!(map_track(1, Subtitle, &jf, &rf), None, "stream 1 is audio");
    assert_eq!(map_track(1, Audio, &jf, &[]), None, "no Rustyfin streams");
    let ac3_only = [rf_stream(2, "audio", "ac3")];
    assert_eq!(map_track(1, Audio, &jf, &ac3_only), None, "no aac stream");
    let upper = [rf_stream(5, "Audio", "AAC")];
    assert_eq!(
        map_track(1, Audio, &jf, &upper),
        Some(5),
        "codec types and codecs compare case-insensitively"
    );
}

#[test]
fn b20_map_tracks_by_ordinal() {
    use StreamKind::{Audio, Video};
    // Three AAC tracks Rustyfin can't tell apart: the ordinal among embedded audio decides.
    let jf = [
        jf_stream(0, Video, "h264"),
        JfStream {
            language: text("eng"),
            ..jf_stream(1, Audio, "aac")
        },
        JfStream {
            language: text("ger"),
            ..jf_stream(2, Audio, "aac")
        },
        JfStream {
            language: text("fre"),
            ..jf_stream(3, Audio, "aac")
        },
    ];
    let rf = [
        rf_stream(0, "video", "h264"),
        rf_stream(1, "audio", "aac"),
        rf_stream(2, "audio", "aac"),
        rf_stream(3, "audio", "aac"),
    ];
    assert_eq!(map_track(1, Audio, &jf, &rf), Some(1));
    assert_eq!(map_track(2, Audio, &jf, &rf), Some(2));
    assert_eq!(map_track(3, Audio, &jf, &rf), Some(3));

    let jf = [
        jf_stream(1, Audio, "aac"),
        jf_stream(2, Audio, "ac3"),
        jf_stream(3, Audio, "aac"),
    ];
    let rf = [
        rf_stream(1, "audio", "aac"),
        rf_stream(2, "audio", "aac"),
        rf_stream(3, "audio", "ac3"),
    ];
    assert_eq!(map_track(1, Audio, &jf, &rf), Some(1));
    assert_eq!(
        map_track(3, Audio, &jf, &rf),
        None,
        "the stream at the ordinal is no candidate"
    );
    assert_eq!(map_track(2, Audio, &jf, &rf), Some(3), "one candidate");
    let two = [rf_stream(1, "audio", "aac"), rf_stream(2, "audio", "aac")];
    assert_eq!(
        map_track(3, Audio, &jf, &two),
        None,
        "no stream at the ordinal"
    );
}

#[test]
fn b20_map_tracks_by_flags_language_and_title() {
    use StreamKind::{Audio, Subtitle};
    let jf = [
        JfStream {
            is_forced: true,
            ..jf_stream(3, Subtitle, "subrip")
        },
        JfStream {
            is_default: true,
            ..jf_stream(4, Subtitle, "subrip")
        },
        jf_stream(5, Subtitle, "subrip"),
    ];
    let rf = [
        rf_stream(3, "subtitle", "subrip"),
        StreamRow {
            is_default: true,
            ..rf_stream(4, "subtitle", "subrip")
        },
        StreamRow {
            is_forced: true,
            ..rf_stream(5, "subtitle", "subrip")
        },
    ];
    assert_eq!(map_track(3, Subtitle, &jf, &rf), Some(5), "forced");
    assert_eq!(map_track(4, Subtitle, &jf, &rf), Some(4), "default");
    assert_eq!(map_track(5, Subtitle, &jf, &rf), Some(3), "neither");
    let unflagged = [rf_stream(3, "subtitle", "subrip")];
    assert_eq!(
        map_track(3, Subtitle, &jf, &unflagged),
        None,
        "no forced stream"
    );

    // Languages and titles narrow the candidates when both sides know them.
    let jf = [
        JfStream {
            language: text("ENG"),
            ..jf_stream(1, Audio, "aac")
        },
        JfStream {
            language: text("ger"),
            title: text("Commentary"),
            ..jf_stream(2, Audio, "aac")
        },
        JfStream {
            language: text("ger"),
            title: text("Stereo"),
            ..jf_stream(3, Audio, "aac")
        },
    ];
    let rf = [
        StreamRow {
            language: text("ger"),
            title: text("stereo"),
            ..rf_stream(1, "audio", "aac")
        },
        StreamRow {
            language: text("eng"),
            ..rf_stream(2, "audio", "aac")
        },
        StreamRow {
            language: text("ger"),
            title: text("commentary"),
            ..rf_stream(3, "audio", "aac")
        },
    ];
    assert_eq!(map_track(1, Audio, &jf, &rf), Some(2), "language");
    assert_eq!(map_track(2, Audio, &jf, &rf), Some(3), "title");
    assert_eq!(map_track(3, Audio, &jf, &rf), Some(1), "language and title");

    // An unknown language (none, empty or `und`) rules nothing out.
    let jf = [
        JfStream {
            language: text("und"),
            ..jf_stream(1, Audio, "aac")
        },
        JfStream {
            language: text("eng"),
            ..jf_stream(2, Audio, "aac")
        },
    ];
    let rf = [
        StreamRow {
            language: text("ger"),
            ..rf_stream(1, "audio", "aac")
        },
        StreamRow {
            language: text(""),
            ..rf_stream(2, "audio", "aac")
        },
    ];
    assert_eq!(
        map_track(1, Audio, &jf, &rf),
        Some(1),
        "two candidates, ordinal 0"
    );
    assert_eq!(
        map_track(2, Audio, &jf, &rf),
        Some(2),
        "eng rules out ger only"
    );
}

#[test]
fn b20_map_external_tracks() {
    use StreamKind::{Audio, Subtitle, Video};
    let jf_ext = |index: i64, path: &str| JfStream {
        is_external: true,
        path: text(path),
        ..jf_stream(index, Subtitle, "srt")
    };
    // Jellyfin numbers external streams first; the slice need not be sorted.
    let jf = [
        jf_stream(2, Video, "h264"),
        jf_stream(3, Audio, "aac"),
        JfStream {
            language: text("ger"),
            ..jf_stream(5, Subtitle, "subrip")
        },
        JfStream {
            language: text("eng"),
            ..jf_stream(4, Subtitle, "subrip")
        },
        jf_ext(0, "/data/movies/Alpha (2001)/Alpha.en.srt"),
        jf_ext(1, "/data/movies/Alpha (2001)/Alpha.de.srt"),
    ];
    let rf_ext = |index: i64, path: &str| StreamRow {
        is_external: true,
        path: text(path),
        ..rf_stream(index, "subtitle", "subrip")
    };
    let rf = [
        rf_stream(0, "video", "h264"),
        rf_stream(1, "audio", "aac"),
        rf_stream(2, "subtitle", "subrip"),
        rf_stream(3, "subtitle", "subrip"),
        rf_ext(4, "/srv/movies/Alpha (2001)/Alpha.de.srt"),
    ];
    assert_eq!(
        map_track(4, Subtitle, &jf, &rf),
        Some(2),
        "embedded ordinal 0"
    );
    assert_eq!(
        map_track(5, Subtitle, &jf, &rf),
        Some(3),
        "embedded ordinal 1"
    );
    assert_eq!(map_track(3, Audio, &jf, &rf), Some(1));
    assert_eq!(map_track(1, Subtitle, &jf, &rf), Some(4), "by file name");
    assert_eq!(map_track(0, Subtitle, &jf, &rf), None, "no Alpha.en.srt");
    let twice = [
        rf_ext(4, "/srv/a/Alpha.de.srt"),
        rf_ext(5, "/srv/b/Alpha.de.srt"),
    ];
    assert_eq!(
        map_track(1, Subtitle, &jf, &twice),
        None,
        "several external candidates"
    );
    let other_case = [rf_ext(4, "/srv/movies/Alpha (2001)/ALPHA.de.srt")];
    assert_eq!(
        map_track(1, Subtitle, &jf, &other_case),
        None,
        "file names compare exactly"
    );
}

// 5.8: the old layout's `Images` text: `Path*Ticks*Type[*Width*Height[*Blurhash]]` entries joined
// by `|`, with `/` and `\` standing for `*` and `|` in a blurhash. Sizes of 0 or less and an empty
// blurhash are none; malformed entries and ignored or unknown types are skipped.
#[test]
fn b20_old_layout_image_entries() {
    let text = "/m/poster.jpg*637000000000000000*Primary*1000*1500*LEHV/nWB\\yk8\
                |%MetadataPath%/b.jpg*1*Backdrop\
                |/m/box.jpg*1*Box*10*10*x\
                |/m/s.png*1*Screenshot*0*-1*\
                |garbage\
                |/m/odd.jpg*1*NoSuchType*1*1*h";
    let image =
        |path: &str, image_type, size: Option<(i32, i32)>, blurhash: Option<&str>| JfImage {
            path: path.to_string(),
            image_type,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            blurhash: blurhash.map(str::to_string),
        };
    assert_eq!(
        parse_old_images(text),
        vec![
            image(
                "/m/poster.jpg",
                ImageType::Primary,
                Some((1000, 1500)),
                Some("LEHV*nWB|yk8")
            ),
            image("%MetadataPath%/b.jpg", ImageType::Backdrop, None, None),
            image("/m/s.png", ImageType::Screenshot, None, None),
        ]
    );
    assert_eq!(parse_old_images(""), Vec::new());
}

// 5.9: a copy's type comes from the file's first bytes; anything else is unusable.
#[test]
fn b20_image_signatures() {
    assert_eq!(
        signature(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]),
        Some("jpg")
    );
    assert_eq!(signature(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), Some("png"));
    assert_eq!(signature(b"RIFF\x24\0\0\0WEBPVP8 "), Some("webp"));
    assert_eq!(signature(b"RIFF\0\0\0\0WEBP"), Some("webp"));
    assert_eq!(signature(b"GIF87a\x01\0"), Some("gif"));
    assert_eq!(signature(b"GIF89a\x01\0"), Some("gif"));
    let others: [&[u8]; 7] = [
        b"SQLite format 3\0",
        b"RIFF\x24\0\0\0WAVEfmt ",
        b"RIFF\0\0\0\0WEB",
        b"GIF88a",
        &[0xFF, 0xD8],
        b"<html>",
        b"",
    ];
    for other in others {
        assert_eq!(signature(other), None, "{other:?}");
    }
}

// 5.9: image paths resolve through the pairs (root: the pair's local side), else the placeholders
// under Jellyfin's program data; `..`, relative paths and URLs are missing.
#[test]
fn b20_resolve_image_paths() {
    let program_data = Path::new("/srv/jf");
    let pairs = vec![
        pair("/config/metadata", "/srv/jf/metadata"),
        pair("/data/movies", "/media/movies"),
    ];
    let resolved = |path: &str| resolve(path, program_data, &pairs);
    let found = |file: &str, root: &str| Some((PathBuf::from(file), PathBuf::from(root)));
    assert_eq!(
        resolved("%MetadataPath%/library/ab/x/poster.jpg"),
        found(
            "/srv/jf/metadata/library/ab/x/poster.jpg",
            "/srv/jf/metadata"
        )
    );
    assert_eq!(
        resolved("%AppDataPath%/collections/c/poster.jpg"),
        found("/srv/jf/data/collections/c/poster.jpg", "/srv/jf/data")
    );
    assert_eq!(
        resolved("/config/metadata/People/A/Ann/folder.jpg"),
        found(
            "/srv/jf/metadata/People/A/Ann/folder.jpg",
            "/srv/jf/metadata"
        )
    );
    assert_eq!(
        resolved("/data/movies/Alpha (2001)/logo.png"),
        found("/media/movies/Alpha (2001)/logo.png", "/media/movies")
    );
    for missing in [
        "/elsewhere/x.jpg",
        "%MetadataPath%/../data/jellyfin.db",
        "/config/metadata/../outside.jpg",
        "/data/movies/../../etc/passwd",
        "https://image.example/a.jpg",
        "metadata/library/x.jpg",
        "",
    ] {
        assert_eq!(resolved(missing), None, "{missing}");
    }

    // A pair for a placeholder wins over program data.
    let moved = vec![pair("%MetadataPath%", "/mnt/jfmeta")];
    assert_eq!(
        resolve("%MetadataPath%/library/x.jpg", program_data, &moved),
        found("/mnt/jfmeta/library/x.jpg", "/mnt/jfmeta")
    );
}

// 5.9: the suggested pair for Jellyfin's metadata folder: the prefix up to a `metadata` component
// that most paths share (ties: shortest, then smallest), only when `<program data>/metadata`
// exists and no pair covers the prefix.
#[test]
fn b20_suggest_metadata_pair() {
    let tmp = TempDir::new();
    let paths = [
        "/config/metadata/library/ab/x/poster.jpg",
        "/config/metadata/People/A/Ann/folder.jpg",
        "/other/metadata/library/x.jpg",
        "%MetadataPath%/library/y.jpg",
        "/data/movies/A/poster.jpg",
    ];
    assert_eq!(
        suggest_metadata(&paths, &tmp.0, &[]),
        None,
        "no local metadata folder"
    );
    let local = tmp.0.join("metadata");
    std::fs::create_dir_all(&local).unwrap();
    let local = local.display().to_string();
    assert_eq!(
        suggest_metadata(&paths, &tmp.0, &[]),
        Some(pair("/config/metadata", &local))
    );
    let covered = [pair("/config", "/srv/config")];
    assert_eq!(
        suggest_metadata(&paths, &tmp.0, &covered),
        None,
        "a pair covers the prefix"
    );
    let tie = [
        "/b/metadata/x.jpg",
        "/a/metadata/y.jpg",
        "/long/er/metadata/z.jpg",
    ];
    assert_eq!(
        suggest_metadata(&tie, &tmp.0, &[]),
        Some(pair("/a/metadata", &local))
    );
    assert_eq!(suggest_metadata(&[], &tmp.0, &[]), None);
}

// Fix 7, 8: a source database is read into memory with its `-wal` applied as SQLite's recovery
// would: committed frames only, up to the first torn or damaged frame; a damaged header means no
// frames. Reading changes nothing in the folder.

/// A WAL-mode `jellyfin.db` in `dir` whose file holds table `t` with row 1, and a writer that
/// never checkpoints, so what it adds stays in the `-wal`.
async fn wal_writer(dir: &Path) -> sqlx::SqliteConnection {
    use sqlx::ConnectOptions;
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(dir.join("jellyfin.db"))
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .connect()
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE t (n INTEGER, pad TEXT); INSERT INTO t VALUES (1, ''); \
         PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint = 0;",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn
}

/// The `n` of every row of `t` in the snapshot of `file`, read with `limit` as the in-memory limit
/// (0 always copies). The data folder it gets is empty again once the snapshot is dropped.
async fn snapshot_rows_with(file: &Path, limit: u64) -> Vec<i64> {
    let data = TempDir::new();
    let mut conn = open_snapshot_with_free(file, &data.0, &[], limit, |_| Ok(u64::MAX))
        .await
        .unwrap_or_else(|e| panic!("{}: {e:?}", file.display()));
    let rows: Vec<i64> = sqlx::query_scalar("SELECT n FROM t ORDER BY n")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let left: Vec<PathBuf> = std::fs::read_dir(&data.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(left.is_empty(), "left in the data folder: {left:?}");
    rows
}

/// The `n` of every row of `t` in the snapshot of `file`, read as the importer reads it.
async fn snapshot_rows(file: &Path) -> Vec<i64> {
    snapshot_rows_with(file, IN_MEMORY_LIMIT).await
}

/// Each entry of `dir` with its size and modification time.
fn folder_listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            (entry.path(), meta.len(), meta.modified().unwrap())
        })
        .collect();
    entries.sort();
    entries
}

/// A copy of `from`'s `jellyfin.db` with `wal` as its `-wal`, in the new folder `to`.
fn source_with_wal(from: &Path, to: &Path, wal: &[u8]) -> PathBuf {
    std::fs::create_dir_all(to).unwrap();
    let file = to.join("jellyfin.db");
    std::fs::copy(from.join("jellyfin.db"), &file).unwrap();
    std::fs::write(to.join("jellyfin.db-wal"), wal).unwrap();
    file
}

#[tokio::test]
async fn b20_snapshot_applies_committed_wal_frames() {
    let tmp = TempDir::new();
    let mut writer = wal_writer(&tmp.0).await;
    sqlx::query("INSERT INTO t VALUES (2, '')")
        .execute(&mut writer)
        .await
        .unwrap();
    let file = tmp.0.join("jellyfin.db");
    let before = folder_listing(&tmp.0);
    assert_eq!(snapshot_rows(&file).await, [1, 2]);
    assert_eq!(folder_listing(&tmp.0), before, "reading changed the folder");
    let no_wal = source_with_wal(&tmp.0, &tmp.0.join("no_wal"), b"");
    assert_eq!(
        snapshot_rows(&no_wal).await,
        [1],
        "row 2 is only in the -wal"
    );
}

#[tokio::test]
async fn b20_snapshot_skips_torn_and_uncommitted_frames() {
    let tmp = TempDir::new();
    let mut writer = wal_writer(&tmp.0).await;
    sqlx::query("INSERT INTO t VALUES (2, '')")
        .execute(&mut writer)
        .await
        .unwrap();
    let wal_path = tmp.0.join("jellyfin.db-wal");
    let first = std::fs::metadata(&wal_path).unwrap().len() as usize;
    // Twenty rows of 4000 characters: one transaction over many pages, so many frames.
    sqlx::query(
        "WITH RECURSIVE k(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM k WHERE i < 20) \
         INSERT INTO t SELECT 3, hex(randomblob(2000)) FROM k",
    )
    .execute(&mut writer)
    .await
    .unwrap();
    let wal = std::fs::read(&wal_path).unwrap();
    let page_size = u32::from_be_bytes(wal[8..12].try_into().unwrap()) as usize;
    let frame = 24 + page_size;
    assert!(
        wal.len() >= first + 3 * frame,
        "the second transaction spans frames"
    );
    for (case, cut) in [
        ("whole first transaction", first),
        ("frames without their commit", first + 2 * frame + 100),
        ("torn commit frame", wal.len() - 1),
    ] {
        let dir = tmp.0.join(case.replace(' ', "_"));
        let file = source_with_wal(&tmp.0, &dir, &wal[..cut]);
        assert_eq!(snapshot_rows(&file).await, [1, 2], "{case}");
    }
    let all = snapshot_rows(&tmp.0.join("jellyfin.db")).await;
    assert_eq!(all.len(), 22, "the whole -wal");
    assert_eq!(all.iter().filter(|&&n| n == 3).count(), 20);
}

#[tokio::test]
async fn b20_snapshot_stops_at_a_damaged_frame_and_ignores_a_damaged_header() {
    let tmp = TempDir::new();
    let mut writer = wal_writer(&tmp.0).await;
    let wal_path = tmp.0.join("jellyfin.db-wal");
    let mut ends = Vec::new();
    for n in 2i64..=4 {
        sqlx::query("INSERT INTO t VALUES (?, '')")
            .bind(n)
            .execute(&mut writer)
            .await
            .unwrap();
        ends.push(std::fs::metadata(&wal_path).unwrap().len() as usize);
    }
    let wal = std::fs::read(&wal_path).unwrap();
    // A byte of the page in the first frame that adds 3: 3 and 4 are gone.
    let mut bad_page = wal.clone();
    bad_page[ends[0] + 24 + 100] ^= 0xff;
    // A salt of the first frame that adds 4: only 4 is gone.
    let mut bad_salt = wal.clone();
    bad_salt[ends[1] + 8] ^= 0xff;
    // A salt in the header: its checksum fails, so no frame counts.
    let mut bad_header = wal.clone();
    bad_header[16] ^= 0xff;
    for (case, bytes, rows) in [
        ("damaged page", bad_page, vec![1, 2]),
        ("damaged frame salt", bad_salt, vec![1, 2, 3]),
        ("damaged header", bad_header, vec![1]),
    ] {
        let dir = tmp.0.join(case.replace(' ', "_"));
        let file = source_with_wal(&tmp.0, &dir, &bytes);
        assert_eq!(snapshot_rows(&file).await, rows, "{case}");
    }
    assert_eq!(
        snapshot_rows(&tmp.0.join("jellyfin.db")).await,
        [1, 2, 3, 4]
    );
}

// Fix round 2, 1: a database or `-wal` that is not a regular file is a bad source, found before any
// open. A FIFO's open waits for a writer forever; a link to /dev/zero would be read until memory runs
// out (/dev/null stands in for it here).

/// What `open_snapshot` gives for `file`, or None while it is still busy after 5 s. It runs on its
/// own thread and runtime, so an open that never returns can't hold up the test.
fn snapshot_within_5s(file: &Path) -> Option<Result<(), ImportError>> {
    let file = file.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let data = TempDir::new();
        let _ = sender.send(
            runtime
                .block_on(open_snapshot(&file, &data.0, &[]))
                .map(drop),
        );
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .ok()
}

#[cfg(unix)]
#[tokio::test]
async fn b20_snapshot_refuses_files_that_are_not_regular_files() {
    let tmp = TempDir::new();
    let mut writer = wal_writer(&tmp.0).await;
    sqlx::query("INSERT INTO t VALUES (2, '')")
        .execute(&mut writer)
        .await
        .unwrap();
    let wal = std::fs::read(tmp.0.join("jellyfin.db-wal")).unwrap();
    let mkfifo = |path: &Path| {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo {}", path.display());
    };
    // A folder `name` with a copy of the database (row 1; row 2 is only in the -wal).
    let folder = |name: &str| {
        let dir = tmp.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(tmp.0.join("jellyfin.db"), dir.join("jellyfin.db")).unwrap();
        dir
    };
    let fifo_wal = folder("fifo_wal");
    mkfifo(&fifo_wal.join("jellyfin.db-wal"));
    let null_wal = folder("null_wal");
    std::os::unix::fs::symlink("/dev/null", null_wal.join("jellyfin.db-wal")).unwrap();
    let fifo_db = tmp.0.join("fifo_db");
    std::fs::create_dir_all(&fifo_db).unwrap();
    mkfifo(&fifo_db.join("jellyfin.db"));
    let null_db = tmp.0.join("null_db");
    std::fs::create_dir_all(&null_db).unwrap();
    std::os::unix::fs::symlink("/dev/null", null_db.join("jellyfin.db")).unwrap();
    for (case, dir, odd) in [
        ("FIFO -wal", &fifo_wal, "jellyfin.db-wal"),
        ("-wal linked to /dev/null", &null_wal, "jellyfin.db-wal"),
        ("FIFO database", &fifo_db, "jellyfin.db"),
        ("database linked to /dev/null", &null_db, "jellyfin.db"),
    ] {
        match snapshot_within_5s(&dir.join("jellyfin.db")) {
            Some(Err(ImportError::BadSource(text))) => assert!(
                text.contains(&format!("{odd} is not a regular file")),
                "{case}: {text}"
            ),
            None => panic!("{case}: open_snapshot still busy after 5 s"),
            Some(other) => panic!("{case}: {other:?}, want BadSource naming {odd}"),
        }
    }
    // A link to a regular file is fine: a -wal linked to a real one is read.
    let real_wal = tmp.0.join("real.wal");
    std::fs::write(&real_wal, &wal).unwrap();
    let linked = folder("linked_wal");
    std::os::unix::fs::symlink(&real_wal, linked.join("jellyfin.db-wal")).unwrap();
    assert_eq!(snapshot_rows(&linked.join("jellyfin.db")).await, [1, 2]);
}

// Fix round 2, 2: SQLite's one buffer for an image refuses more than 2,147,483,391 bytes, so a source
// above the in-memory limit is copied to a private folder in Rustyfin's data folder and opened there
// read-only; SQLite's own recovery applies the copied -wal.

#[test]
fn b20_big_sources_are_not_read_into_memory() {
    const SQLITE_MAX_ALLOCATION: u64 = 2_147_483_391;
    assert!(
        reads_in_memory(IN_MEMORY_LIMIT, 0, SQLITE_MAX_ALLOCATION),
        "an image at the limit must fit SQLite's buffer"
    );
    assert!(reads_in_memory(IN_MEMORY_LIMIT, 0, IN_MEMORY_LIMIT));
    assert!(reads_in_memory(
        IN_MEMORY_LIMIT - 4096,
        4096,
        IN_MEMORY_LIMIT
    ));
    assert!(!reads_in_memory(IN_MEMORY_LIMIT, 1, IN_MEMORY_LIMIT));
    assert!(!reads_in_memory(
        SQLITE_MAX_ALLOCATION + 1,
        0,
        IN_MEMORY_LIMIT
    ));
    assert!(!reads_in_memory(
        0,
        SQLITE_MAX_ALLOCATION + 1,
        IN_MEMORY_LIMIT
    ));
    assert!(!reads_in_memory(u64::MAX, u64::MAX, IN_MEMORY_LIMIT));
    assert!(reads_in_memory(0, 0, 0));
    assert!(!reads_in_memory(1, 0, 0));
}

#[tokio::test]
async fn b20_snapshot_copies_a_big_source_to_the_data_folder() {
    let tmp = TempDir::new();
    let mut writer = wal_writer(&tmp.0).await;
    sqlx::query("INSERT INTO t VALUES (2, '')")
        .execute(&mut writer)
        .await
        .unwrap();
    let wal_path = tmp.0.join("jellyfin.db-wal");
    let first = std::fs::metadata(&wal_path).unwrap().len() as usize;
    sqlx::query(
        "WITH RECURSIVE k(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM k WHERE i < 20) \
         INSERT INTO t SELECT 3, hex(randomblob(2000)) FROM k",
    )
    .execute(&mut writer)
    .await
    .unwrap();
    let wal = std::fs::read(&wal_path).unwrap();
    let page_size = u32::from_be_bytes(wal[8..12].try_into().unwrap()) as usize;
    let frame = 24 + page_size;
    // A limit of 0 copies: committed frames only, and the source folder stays as it was. Without a
    // -wal (Jellyfin stopped cleanly) the copy is a WAL-mode database alone.
    let mut all = vec![1, 2];
    all.extend([3; 20]);
    for (case, cut, rows) in [
        ("no wal", None, &all[..1]),
        (
            "frames without their commit",
            Some(first + 2 * frame + 100),
            &all[..2],
        ),
        ("whole wal", Some(wal.len()), &all[..]),
    ] {
        let dir = tmp.0.join(case.replace(' ', "_"));
        let file = source_with_wal(&tmp.0, &dir, &wal[..cut.unwrap_or(0)]);
        if cut.is_none() {
            std::fs::remove_file(dir.join("jellyfin.db-wal")).unwrap();
        }
        let before = folder_listing(&dir);
        assert_eq!(snapshot_rows_with(&file, 0).await, rows, "{case}");
        assert_eq!(
            folder_listing(&dir),
            before,
            "{case}: the source folder changed"
        );
    }
    // Database and -wal at the limit are read in memory; one byte more is copied, into a folder of
    // the data folder that only the server can read, and the copy goes with the connection.
    let file = tmp.0.join("whole_wal").join("jellyfin.db");
    let total = std::fs::metadata(&file).unwrap().len() + wal.len() as u64;
    for (limit, copied) in [(total, false), (total - 1, true)] {
        let data = TempDir::new();
        let mut conn = open_snapshot_with_free(&file, &data.0, &[], limit, |_| Ok(u64::MAX))
            .await
            .unwrap_or_else(|e| panic!("limit {limit}: {e:?}"));
        let main: String =
            sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        let main = PathBuf::from(main);
        let in_data = main.starts_with(&data.0) || main.starts_with(data.0.canonicalize().unwrap());
        assert_eq!(
            in_data,
            copied,
            "limit {limit} of {total}: main database {}",
            main.display()
        );
        #[cfg(unix)]
        if copied {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(main.parent().unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o077,
                0,
                "the copy's folder is open to others: {mode:o}"
            );
        }
        drop(conn);
        assert_eq!(
            std::fs::read_dir(&data.0).unwrap().count(),
            0,
            "limit {limit}: the copy outlived its connection"
        );
    }
}

// Fix round 2, 3: the reader reads the stored rows of plain tables only. A view, a virtual table or a
// generated column in place of a table it reads is a bad source, found before any row is read: a
// recursive view never ends, and a view or a generated column can make values of any size. Credits
// come from two plain scans, so a repeated person id can't multiply them.

/// The user tables of `jellyfin.db`, with the columns the reader reads.
const USERS_DDL: &str = "\
    CREATE TABLE Users (Id TEXT NOT NULL PRIMARY KEY, InternalId INTEGER NOT NULL, \
        Username TEXT NOT NULL); \
    CREATE TABLE Permissions (Id INTEGER PRIMARY KEY, UserId TEXT, Kind INTEGER NOT NULL, \
        Value INTEGER NOT NULL);";

/// The new layout's library tables in `jellyfin.db`, with the columns the reader reads. Unlike
/// Jellyfin's, `Peoples` may repeat an id and `PeopleBaseItemMap` may lack a person.
const NEW_LIBRARY_DDL: &str = "\
    CREATE TABLE BaseItems (Id TEXT NOT NULL PRIMARY KEY, Type TEXT NOT NULL, Name TEXT, Path TEXT, \
        ParentId TEXT, SeriesId TEXT, SeasonId TEXT, IndexNumber INTEGER, \
        ParentIndexNumber INTEGER, ForcedSortName TEXT, OriginalTitle TEXT, Overview TEXT, \
        Tagline TEXT, OfficialRating TEXT, CommunityRating REAL, CriticRating REAL, \
        PremiereDate TEXT, EndDate TEXT, ProductionYear INTEGER, Genres TEXT, Studios TEXT, \
        Tags TEXT); \
    CREATE TABLE BaseItemProviders (ItemId TEXT NOT NULL, ProviderId TEXT NOT NULL, \
        ProviderValue TEXT NOT NULL); \
    CREATE TABLE UserData (ItemId TEXT NOT NULL, UserId TEXT NOT NULL, Played INTEGER NOT NULL, \
        PlayCount INTEGER NOT NULL, PlaybackPositionTicks INTEGER NOT NULL, LastPlayedDate TEXT, \
        IsFavorite INTEGER NOT NULL, Rating REAL, Likes INTEGER, AudioStreamIndex INTEGER, \
        SubtitleStreamIndex INTEGER); \
    CREATE TABLE MediaStreamInfos (ItemId TEXT NOT NULL, StreamIndex INTEGER NOT NULL, \
        StreamType INTEGER NOT NULL, Codec TEXT, Language TEXT, Title TEXT, \
        IsExternal INTEGER NOT NULL, Path TEXT, IsDefault INTEGER NOT NULL, \
        IsForced INTEGER NOT NULL); \
    CREATE TABLE BaseItemImageInfos (Id TEXT, Path TEXT NOT NULL, ImageType INTEGER NOT NULL, \
        Width INTEGER NOT NULL, Height INTEGER NOT NULL, Blurhash BLOB, ItemId TEXT NOT NULL); \
    CREATE TABLE Peoples (Id TEXT, Name TEXT NOT NULL, PersonType TEXT); \
    CREATE TABLE PeopleBaseItemMap (SortOrder INTEGER, ListOrder INTEGER, Role TEXT, \
        ItemId TEXT NOT NULL, PeopleId TEXT);";

/// The old layout's `library.db`, with the columns the reader reads.
const OLD_LIBRARY_DDL: &str = "\
    CREATE TABLE TypedBaseItems (guid GUID PRIMARY KEY NOT NULL, type TEXT NOT NULL, Path TEXT, \
        ParentId GUID, Name TEXT, IndexNumber INT, ParentIndexNumber INT, UserDataKey TEXT, \
        SeasonId GUID, SeriesId GUID, ProviderIds TEXT, ForcedSortName TEXT, OriginalTitle TEXT, \
        Overview TEXT, Tagline TEXT, OfficialRating TEXT, CommunityRating FLOAT, \
        CriticRating FLOAT, PremiereDate DATETIME, EndDate DATETIME, ProductionYear INT, \
        Genres TEXT, Studios TEXT, Tags TEXT, Images TEXT); \
    CREATE TABLE UserDatas (key TEXT NOT NULL, userId INT NOT NULL, rating FLOAT, \
        played BIT NOT NULL, playCount INT NOT NULL, isFavorite BIT NOT NULL, \
        playbackPositionTicks BIGINT NOT NULL, lastPlayedDate DATETIME, AudioStreamIndex INT, \
        SubtitleStreamIndex INT); \
    CREATE TABLE mediastreams (ItemId GUID, StreamIndex INT, StreamType TEXT, Codec TEXT, \
        Language TEXT, Title TEXT, IsExternal BIT, Path TEXT, IsDefault BIT, IsForced BIT); \
    CREATE TABLE People (ItemId GUID, Name TEXT NOT NULL, Role TEXT, PersonType TEXT, \
        SortOrder INT, ListOrder INT);";

/// One movie with one credit, for `NEW_LIBRARY_DDL`.
const ONE_CREDIT: &str = "\
    INSERT INTO BaseItems (Id, Type, Name, Path) VALUES ('00000000-0000-0000-0000-0000000000F1', \
        'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', '/media/film.mkv'); \
    INSERT INTO Peoples (Id, Name, PersonType) VALUES ('p1', 'Ann', 'Actor'); \
    INSERT INTO PeopleBaseItemMap (ItemId, PeopleId, Role, ListOrder, SortOrder) \
        VALUES ('00000000-0000-0000-0000-0000000000F1', 'p1', 'Lead', 0, 0);";

/// A new SQLite file at `file`, made by running `scripts` in turn.
async fn write_db(file: &Path, scripts: &[&'static str]) {
    use sqlx::{ConnectOptions, Connection};
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .create_if_missing(true)
        .connect()
        .await
        .unwrap();
    for script in scripts {
        sqlx::raw_sql(*script)
            .execute(&mut conn)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    }
    conn.close().await.unwrap();
}

/// What `read_snapshot` gives for the source folder `dir` (layout Auto), or None while it is still
/// busy after 5 s. Like `snapshot_within_5s`, it runs on its own thread and runtime.
fn read_within_5s(dir: &Path) -> Option<Result<(), ImportError>> {
    let dir = dir.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let data = TempDir::new();
        let read = locate(&dir).and_then(|location| {
            runtime
                .block_on(read_snapshot(&location, LayoutChoice::Auto, &data.0))
                .map(drop)
        });
        let _ = sender.send(read);
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .ok()
}

#[tokio::test]
async fn b20_snapshot_reads_only_tables() {
    // Endless views: one that makes a row now and then, the review's, and one that makes none.
    const ENDLESS_USERS: &str = "DROP TABLE Users; CREATE VIEW Users AS WITH RECURSIVE c(x) AS \
        (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT randomblob(16) AS Id, 'u' AS Username, \
        x AS InternalId FROM c WHERE x % 1000000 = 0";
    const REVIEW_USERS: &str = "DROP TABLE Users; CREATE VIEW Users AS WITH RECURSIVE c(x) AS \
        (SELECT 1 UNION ALL SELECT x+1 FROM c) SELECT x,'u',x FROM c";
    const ENDLESS_PEOPLES: &str = "DROP TABLE Peoples; CREATE VIEW Peoples AS WITH RECURSIVE \
        c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT 'p1' AS Id, 'n' AS Name, \
        NULL AS PersonType FROM c WHERE x < 0";
    const VIRTUAL_COLUMN: &str = "DROP TABLE Users; CREATE TABLE Users (Id TEXT, \
        InternalId INTEGER, Username TEXT GENERATED ALWAYS AS ('u' || InternalId) VIRTUAL)";
    const STORED_COLUMN: &str = "DROP TABLE Peoples; CREATE TABLE Peoples (Id TEXT, Name TEXT, \
        PersonType TEXT GENERATED ALWAYS AS ('Actor') STORED)";
    const VIRTUAL_TABLE: &str =
        "DROP TABLE Peoples; CREATE VIRTUAL TABLE Peoples USING fts5(Id, Name, PersonType)";
    const ENDLESS_USER_DATAS: &str = "DROP TABLE UserDatas; CREATE VIEW UserDatas AS WITH \
        RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT 'k' AS key, \
        1 AS userId, 0 AS played, 0 AS playCount, 0 AS playbackPositionTicks, \
        NULL AS lastPlayedDate, 0 AS isFavorite, NULL AS rating, NULL AS AudioStreamIndex, \
        NULL AS SubtitleStreamIndex FROM c WHERE x < 0";
    let tmp = TempDir::new();
    let mut cases = Vec::new();
    for (case, change, want) in [
        (
            "endless view",
            ENDLESS_USERS,
            "Users is a view, not a plain table",
        ),
        (
            "review view",
            REVIEW_USERS,
            "Users is a view, not a plain table",
        ),
        (
            "view in place of a later table",
            ENDLESS_PEOPLES,
            "Peoples is a view, not a plain table",
        ),
        (
            "virtual generated column",
            VIRTUAL_COLUMN,
            "Users has generated columns",
        ),
        (
            "stored generated column",
            STORED_COLUMN,
            "Peoples has generated columns",
        ),
        (
            "virtual table",
            VIRTUAL_TABLE,
            "Peoples is a virtual table, not a plain table",
        ),
    ] {
        let dir = tmp.0.join(case.replace(' ', "_"));
        std::fs::create_dir_all(&dir).unwrap();
        write_db(
            &dir.join("jellyfin.db"),
            &[USERS_DDL, NEW_LIBRARY_DDL, ONE_CREDIT, change],
        )
        .await;
        cases.push((case, dir, "jellyfin.db", want));
    }
    // The old layout's tables are checked in `library.db`.
    let old = tmp.0.join("old_layout");
    std::fs::create_dir_all(&old).unwrap();
    write_db(&old.join("jellyfin.db"), &[USERS_DDL]).await;
    write_db(
        &old.join("library.db"),
        &[OLD_LIBRARY_DDL, ENDLESS_USER_DATAS],
    )
    .await;
    cases.push((
        "endless view in library.db",
        old,
        "library.db",
        "UserDatas is a view, not a plain table",
    ));

    let mut wrong = Vec::new();
    for (case, dir, file, want) in &cases {
        match read_within_5s(dir) {
            Some(Err(ImportError::BadSource(text)))
                if text.contains(file) && text.contains(want) => {}
            None => wrong.push(format!("{case}: read_snapshot still busy after 5 s")),
            Some(other) => wrong.push(format!(
                "{case}: {other:?}, want a BadSource naming {file} that says \"{want}\""
            )),
        }
    }

    // Fix round 8 (re-check 7, H3 and H5), on both paths: rows that load but that the kind query
    // misread. Loading the schema compares a row's type without regard to case, so a `VIEW` row
    // loads as a view and a `TABLE` row as a table; at 48f90f4 the query matched only lower case, so
    // the name had no kind and went unchecked (H3). A virtual table whose row has a root page and SQL
    // that doesn't start `CREATE VIRTUAL` passed as a table, so the checks that followed connected it
    // through its module, which compiles its content view (H5).
    const UPPER_CASE_VIEW: &str = "DROP TABLE Users; CREATE VIEW Users AS SELECT 'u1' AS Id, \
        1 AS InternalId, 'alice' AS Username; PRAGMA writable_schema = ON; \
        UPDATE sqlite_schema SET type = 'VIEW' WHERE name = 'Users'; PRAGMA writable_schema = OFF";
    const UPPER_CASE_TABLE: &str = "DROP TABLE Peoples; CREATE TABLE Peoples (Id TEXT, \
        Name TEXT NOT NULL, PersonType TEXT AS ('Actor') VIRTUAL); PRAGMA writable_schema = ON; \
        UPDATE sqlite_schema SET type = 'TABLE' WHERE name = 'Peoples'; \
        PRAGMA writable_schema = OFF";
    const ROOTED_VIRTUAL_TABLE: &str = "DROP TABLE Peoples; CREATE VIEW cv AS SELECT 'p1' AS Id, \
        'Ann' AS Name, 'Actor' AS PersonType; CREATE VIRTUAL TABLE Peoples USING fts4(content=cv); \
        PRAGMA writable_schema = ON; UPDATE sqlite_schema SET rootpage = 2, \
        sql = 'CREATE/**/VIRTUAL TABLE Peoples USING fts4(content=cv)' WHERE name = 'Peoples'; \
        PRAGMA writable_schema = OFF";
    let data = TempDir::new();
    for (case, change, want) in [
        (
            "upper-case view",
            UPPER_CASE_VIEW,
            "Users is a view, not a plain table",
        ),
        (
            "upper-case table",
            UPPER_CASE_TABLE,
            "Peoples has generated columns",
        ),
        (
            "virtual table with a root page",
            ROOTED_VIRTUAL_TABLE,
            "Peoples is a virtual table, not a plain table",
        ),
    ] {
        let db = tmp.0.join(case.replace(' ', "_")).join("jellyfin.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        write_db(&db, &[USERS_DDL, NEW_LIBRARY_DDL, ONE_CREDIT, change]).await;
        for (path, limit) in [("in memory", u64::MAX), ("copy", 0)] {
            let open =
                open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX));
            match tokio::time::timeout(std::time::Duration::from_secs(5), open).await {
                Ok(Err(ImportError::BadSource(text)))
                    if text.contains("jellyfin.db") && text.contains(want) => {}
                Err(_) => wrong.push(format!("{case} ({path}): still busy after 5 s")),
                Ok(other) => wrong.push(format!(
                    "{case} ({path}): {:?}, want a BadSource naming jellyfin.db that says \
                     \"{want}\"",
                    other.map(drop)
                )),
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn b20_credits_take_the_first_person_of_an_id() {
    // Ann's id is repeated, one credit has no person and one an unknown person.
    const CREDITS: &str = "\
        INSERT INTO BaseItems (Id, Type, Name, Path) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', '/media/film.mkv'); \
        INSERT INTO Peoples (Id, Name, PersonType) VALUES ('p1', 'Ann', 'Actor'), \
            ('p2', 'Bob', 'Director'), ('p1', 'Ann again', 'Actor'), \
            ('p1', 'Ann once more', 'GuestStar'); \
        INSERT INTO PeopleBaseItemMap (ItemId, PeopleId, Role, ListOrder, SortOrder) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', 'p1', 'Lead', 0, 0), \
            ('00000000-0000-0000-0000-0000000000F1', 'p2', NULL, 1, 1), \
            ('00000000-0000-0000-0000-0000000000F1', NULL, 'Nobody', 2, 2), \
            ('00000000-0000-0000-0000-0000000000F1', 'p9', 'Unknown', 3, 3);";
    let tmp = TempDir::new();
    write_db(
        &tmp.0.join("jellyfin.db"),
        &[USERS_DDL, NEW_LIBRARY_DDL, CREDITS],
    )
    .await;
    let data = TempDir::new();
    let location = locate(&tmp.0).unwrap();
    let snapshot = read_snapshot(&location, LayoutChoice::Auto, &data.0)
        .await
        .unwrap();
    let credits: Vec<_> = snapshot.metadata.credits[&Uuid::from_u128(0xF1)]
        .iter()
        .map(|credit| {
            (
                &*credit.name,
                credit.kind.as_deref(),
                credit.role.as_deref(),
                credit.list_order,
                credit.sort_order,
            )
        })
        .collect();
    assert_eq!(
        credits,
        [
            ("Ann", Some("Actor"), Some("Lead"), Some(0), Some(0)),
            ("Bob", Some("Director"), None, Some(1), Some(1)),
        ]
    );
}

/// Writes a JPEG of `len` bytes to `path`: the signature, then `fill`.
fn write_jpeg(path: &Path, len: usize, fill: u8) {
    let mut bytes = vec![fill; len];
    bytes[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    std::fs::write(path, bytes).unwrap();
}

#[cfg(unix)]
#[test]
fn b20_image_copy_takes_only_the_checked_file() {
    use super::images::{check, copy};
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new();
    let root = tmp.0.join("metadata");
    let outside = tmp.0.join("outside");
    let out = tmp.0.join("images");
    for dir in [root.join("aa"), outside.join("aa")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // A file outside the root that looks like the poster: a JPEG of the same length and time.
    let poster = root.join("aa/poster.jpg");
    let secret = outside.join("aa/poster.jpg");
    write_jpeg(&poster, 4096, b'p');
    write_jpeg(&secret, 4096, b's');
    let modified = std::fs::metadata(&poster).unwrap().modified().unwrap();
    let file = std::fs::File::options().write(true).open(&secret).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);

    // Unchanged, the checked file is copied.
    let image = check(&poster, &root).unwrap();
    copy(&image, &out.join("kept.jpg")).unwrap();
    assert_eq!(
        std::fs::read(out.join("kept.jpg")).unwrap(),
        std::fs::read(&poster).unwrap()
    );

    // After its check, the file becomes a link to the other one.
    let image = check(&poster, &root).unwrap();
    let moved = root.join("aa/poster.moved");
    std::fs::rename(&poster, &moved).unwrap();
    symlink(&secret, &poster).unwrap();
    let copied = copy(&image, &out.join("link.jpg"));
    assert!(copied.is_err(), "copied the file a link put in its place");
    std::fs::remove_file(&poster).unwrap();
    std::fs::rename(&moved, &poster).unwrap();

    // After its check, its folder becomes a link to a folder outside the root.
    let image = check(&poster, &root).unwrap();
    std::fs::rename(root.join("aa"), root.join("aa.moved")).unwrap();
    symlink(outside.join("aa"), root.join("aa")).unwrap();
    let copied = copy(&image, &out.join("folder.jpg"));
    assert!(copied.is_err(), "copied through a folder link");
    std::fs::remove_file(root.join("aa")).unwrap();
    std::fs::rename(root.join("aa.moved"), root.join("aa")).unwrap();

    // After its check, the file grows.
    let image = check(&poster, &root).unwrap();
    let mut file = std::fs::File::options().append(true).open(&poster).unwrap();
    std::io::Write::write_all(&mut file, &[b'p'; 100]).unwrap();
    drop(file);
    let copied = copy(&image, &out.join("grown.jpg"));
    assert!(copied.is_err(), "copied a file that changed");

    // Only the first copy is there, and no temporary file.
    let names: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["kept.jpg"]);
}

// Fix round 3, 1: a person's name and type are held once, however many credits name them, and both
// have caps, so a long name can't be multiplied by its links.

#[tokio::test]
async fn b20_credits_of_a_person_share_one_name() {
    // Ann, padded, is credited on three films.
    const CREDITS: &str = "\
        INSERT INTO BaseItems (Id, Type, Name, Path) \
            SELECT printf('00000000-0000-0000-0000-0000000000F%d', x), \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', printf('/media/film%d.mkv', x) \
            FROM (SELECT 1 AS x UNION ALL SELECT 2 UNION ALL SELECT 3); \
        INSERT INTO Peoples (Id, Name, PersonType) VALUES ('p1', '  Ann  ', ' Actor '); \
        INSERT INTO PeopleBaseItemMap (ItemId, PeopleId, Role, ListOrder, SortOrder) \
            SELECT Id, 'p1', 'Lead', 0, 0 FROM BaseItems;";
    let tmp = TempDir::new();
    write_db(
        &tmp.0.join("jellyfin.db"),
        &[USERS_DDL, NEW_LIBRARY_DDL, CREDITS],
    )
    .await;
    let data = TempDir::new();
    let location = locate(&tmp.0).unwrap();
    let snapshot = read_snapshot(&location, LayoutChoice::Auto, &data.0)
        .await
        .unwrap();
    let credits: Vec<_> = snapshot.metadata.credits.values().flatten().collect();
    assert_eq!(credits.len(), 3);
    let names: std::collections::HashSet<_> =
        credits.iter().map(|credit| credit.name.as_ptr()).collect();
    let kinds: std::collections::HashSet<_> = credits
        .iter()
        .map(|credit| credit.kind.as_deref().map(str::as_ptr))
        .collect();
    assert_eq!(names.len(), 1, "each credit holds its own copy of the name");
    assert_eq!(kinds.len(), 1, "each credit holds its own copy of the type");
    for credit in credits {
        assert_eq!(
            (&*credit.name, credit.kind.as_deref()),
            ("Ann", Some("Actor"))
        );
    }
}

#[tokio::test]
async fn b20_person_names_and_types_have_caps() {
    // In both layouts: a name of 1024 bytes and a type of 64 are kept, a name of 1025 bytes drops
    // its credit, and a type of 65 bytes is none.
    const NEW: &str = "\
        INSERT INTO BaseItems (Id, Type, Name, Path) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', '/media/film.mkv'); \
        INSERT INTO Peoples (Id, Name, PersonType) VALUES \
            ('p1', replace(hex(zeroblob(512)), '0', 'n'), replace(hex(zeroblob(32)), '0', 'k')), \
            ('p2', replace(hex(zeroblob(512)), '0', 'n') || 'n', 'Actor'), \
            ('p3', 'Bob', replace(hex(zeroblob(32)), '0', 'k') || 'k'); \
        INSERT INTO PeopleBaseItemMap (ItemId, PeopleId, Role, ListOrder, SortOrder) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', 'p1', NULL, 0, 0), \
            ('00000000-0000-0000-0000-0000000000F1', 'p2', NULL, 1, 1), \
            ('00000000-0000-0000-0000-0000000000F1', 'p3', NULL, 2, 2);";
    const OLD: &str = "\
        INSERT INTO TypedBaseItems (guid, type, Name, Path) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', '/media/film.mkv'); \
        INSERT INTO People (ItemId, Name, PersonType, ListOrder, SortOrder) VALUES \
            ('00000000-0000-0000-0000-0000000000F1', replace(hex(zeroblob(512)), '0', 'n'), \
                replace(hex(zeroblob(32)), '0', 'k'), 0, 0), \
            ('00000000-0000-0000-0000-0000000000F1', \
                replace(hex(zeroblob(512)), '0', 'n') || 'n', 'Actor', 1, 1), \
            ('00000000-0000-0000-0000-0000000000F1', 'Bob', \
                replace(hex(zeroblob(32)), '0', 'k') || 'k', 2, 2);";
    let tmp = TempDir::new();
    let (new, old) = (tmp.0.join("new_layout"), tmp.0.join("old_layout"));
    for dir in [&new, &old] {
        std::fs::create_dir_all(dir).unwrap();
    }
    write_db(&new.join("jellyfin.db"), &[USERS_DDL, NEW_LIBRARY_DDL, NEW]).await;
    write_db(&old.join("jellyfin.db"), &[USERS_DDL]).await;
    write_db(&old.join("library.db"), &[OLD_LIBRARY_DDL, OLD]).await;
    let (name, kind) = ("n".repeat(1024), "k".repeat(64));
    let want = [(name.as_str(), Some(kind.as_str())), ("Bob", None)];
    let data = TempDir::new();
    for dir in [&new, &old] {
        let location = locate(dir).unwrap();
        let snapshot = read_snapshot(&location, LayoutChoice::Auto, &data.0)
            .await
            .unwrap();
        let credits: Vec<_> = snapshot.metadata.credits[&Uuid::from_u128(0xF1)]
            .iter()
            .map(|credit| (&*credit.name, credit.kind.as_deref()))
            .collect();
        assert_eq!(credits, want, "{}", dir.display());
    }
}

// Fix round 3, 2: an old-layout `UserDatas` row reaches every item with its key, so the watch data
// is held to the items plus the rows: only the first row of a key and user counts, and a file that
// still gives more is a bad source.

#[tokio::test]
async fn b20_old_watch_data_is_at_most_items_and_rows() {
    // 30 users. One source has 200 movies on one key with 200 rows of the first user, play counts
    // 1 to 200 in rowid order; the other has 30 movies on one key with a row of each user.
    const USERS: &str = "\
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30) \
        INSERT INTO Users (Id, InternalId, Username) \
            SELECT printf('00000000-0000-0000-0000-%012d', x), x, printf('user%d', x) FROM n;";
    const ONE_USER: &str = "\
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 200) \
        INSERT INTO TypedBaseItems (guid, type, Name, Path, UserDataKey) \
            SELECT printf('00000000-0000-0000-0001-%012d', x), \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', \
            printf('/media/film%d.mkv', x), 'k1' FROM n; \
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 200) \
        INSERT INTO UserDatas (key, userId, played, playCount, isFavorite, playbackPositionTicks) \
            SELECT 'k1', 1, 1, x, 0, 0 FROM n;";
    const MANY_USERS: &str = "\
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30) \
        INSERT INTO TypedBaseItems (guid, type, Name, Path, UserDataKey) \
            SELECT printf('00000000-0000-0000-0002-%012d', x), \
            'MediaBrowser.Controller.Entities.Movies.Movie', 'Film', \
            printf('/media/film%d.mkv', x), 'k2' FROM n; \
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30) \
        INSERT INTO UserDatas (key, userId, played, playCount, isFavorite, playbackPositionTicks) \
            SELECT 'k2', x, 1, 1, 0, 0 FROM n;";
    let tmp = TempDir::new();
    let (one, many) = (tmp.0.join("one_user"), tmp.0.join("many_users"));
    for (dir, rows) in [(&one, ONE_USER), (&many, MANY_USERS)] {
        std::fs::create_dir_all(dir).unwrap();
        write_db(&dir.join("jellyfin.db"), &[USERS_DDL, USERS]).await;
        write_db(&dir.join("library.db"), &[OLD_LIBRARY_DDL, rows]).await;
    }
    let data = TempDir::new();
    let one_user = read_snapshot(&locate(&one).unwrap(), LayoutChoice::Auto, &data.0)
        .await
        .unwrap();
    let many_users = read_snapshot(&locate(&many).unwrap(), LayoutChoice::Auto, &data.0)
        .await
        .map(|snapshot| snapshot.user_data.len());
    let first_rows = one_user
        .user_data
        .iter()
        .filter(|row| row.user == Uuid::from_u128(1) && row.play_count == 1)
        .count();
    let items: std::collections::HashSet<_> =
        one_user.user_data.iter().map(|row| row.item).collect();
    let refused = matches!(&many_users, Err(ImportError::BadSource(text))
        if text.contains("library.db") && text.contains("UserDatas"));
    // One assert for both sources, so a failing run shows both.
    assert_eq!(
        (one_user.user_data.len(), first_rows, items.len(), refused),
        (200, 200, 200, true),
        "200 rows of one user on 200 items, each from its first row; 30 users on 30 items \
         refused, not {many_users:?}"
    );
}

// Fix round 3, 4: a `.mblink` holds one path, so one over 64 KiB, such as a link to a film, is a bad
// source and is never read whole.

#[cfg(unix)]
#[test]
fn b20_mblinks_are_at_most_64_kib() {
    // Each case is a program data folder of its own, as a bad source ends the read.
    const CASES: [&str; 4] = ["at_limit", "over_limit", "to_a_film", "not_utf8"];
    let tmp = TempDir::new();
    let link_in = |case: &str| tmp.0.join(case).join("root/default/Films/films.mblink");
    for case in CASES {
        std::fs::create_dir_all(link_in(case).parent().unwrap()).unwrap();
    }
    let path_of = |len: usize| format!("/media/{}", "a".repeat(len - "/media/".len()));
    std::fs::write(link_in("at_limit"), path_of(64 << 10)).unwrap();
    std::fs::write(link_in("over_limit"), path_of((64 << 10) + 1)).unwrap();
    // A sparse 8 MiB film, linked in as a `.mblink`.
    let film = tmp.0.join("film.mkv");
    std::fs::File::create(&film)
        .unwrap()
        .set_len(8 << 20)
        .unwrap();
    std::os::unix::fs::symlink(&film, link_in("to_a_film")).unwrap();
    std::fs::write(link_in("not_utf8"), b"/media/\xff").unwrap();
    let outcomes: Vec<String> = CASES
        .into_iter()
        .map(|case| match read_library_folders(&tmp.0.join(case)) {
            Ok(folders) => {
                let bytes: usize = folders
                    .iter()
                    .flat_map(|folder| &folder.mblinks)
                    .map(String::len)
                    .sum();
                format!("read {bytes} bytes")
            }
            Err(ImportError::BadSource(text))
                if text.contains(&link_in(case).display().to_string()) =>
            {
                "bad source naming the file".to_string()
            }
            Err(other) => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            "read 65536 bytes",
            "bad source naming the file",
            "bad source naming the file",
            "bad source naming the file",
        ]
    );
}

// Fix round 3, 5: a damaged table is a bad source before any row is read. A b-tree whose interior
// pages point at one child page more than once makes every scan repeat that child's rows, so a
// small file could be read without end.

/// Damages `table` in the SQLite file `file`, which has no `-wal`: its root page and the root's
/// first `levels - 1` children become interior pages whose three child pointers all point at the
/// next of these pages, and the last one's at the root's `levels`th child, a leaf. A scan of the
/// table then reads each row of that leaf 3^`levels` times. The root must be an interior page with
/// at least `levels` children.
async fn share_child_pages(file: &Path, table: &str, levels: usize) {
    use sqlx::{ConnectOptions, Connection};
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .connect()
        .await
        .unwrap();
    let (root,): (i64,) = sqlx::query_as("SELECT rootpage FROM sqlite_schema WHERE name = ?1")
        .bind(table)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(
        !file.with_extension("db-wal").exists(),
        "{}",
        file.display()
    );
    let mut bytes = std::fs::read(file).unwrap();
    let u16_at =
        |bytes: &[u8], at: usize| usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]));
    let u32_at =
        |bytes: &[u8], at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    // The header holds the page size at offset 16; 1 means 65536.
    let size = match u16_at(&bytes, 16) {
        1 => 65536,
        size => size,
    };
    let start = |page: u32| (page as usize - 1) * size;
    let root = u32::try_from(root).unwrap();
    let at = start(root);
    assert_eq!(bytes[at], 0x05, "{table}'s root is not an interior page");
    let mut children: Vec<u32> = (0..u16_at(&bytes, at + 3))
        .map(|cell| u32_at(&bytes, at + u16_at(&bytes, at + 12 + 2 * cell)))
        .collect();
    children.push(u32_at(&bytes, at + 8));
    assert!(
        children.len() >= levels,
        "{table} has {} leaves",
        children.len()
    );
    let chain: Vec<u32> = std::iter::once(root)
        .chain(children[..levels - 1].iter().copied())
        .collect();
    for (level, &page) in chain.iter().enumerate() {
        let child = chain
            .get(level + 1)
            .copied()
            .unwrap_or(children[levels - 1])
            .to_be_bytes();
        // An interior table page: two cells at the page's end, with the keys 1 and 2, and the
        // right-most pointer, all pointing at `child`.
        let mut new = vec![0; size];
        new[0] = 0x05;
        new[3..5].copy_from_slice(&2u16.to_be_bytes());
        new[5..7].copy_from_slice(&u16::try_from(size - 10).unwrap().to_be_bytes());
        new[8..12].copy_from_slice(&child);
        new[12..14].copy_from_slice(&u16::try_from(size - 10).unwrap().to_be_bytes());
        new[14..16].copy_from_slice(&u16::try_from(size - 5).unwrap().to_be_bytes());
        new[size - 10..size - 6].copy_from_slice(&child);
        new[size - 6] = 1;
        new[size - 5..size - 1].copy_from_slice(&child);
        new[size - 1] = 2;
        bytes[start(page)..start(page) + size].copy_from_slice(&new);
    }
    std::fs::write(file, bytes).unwrap();
}

#[tokio::test]
async fn b20_snapshot_refuses_damaged_tables() {
    // Permissions that make no one an administrator, so the reader's query keeps none of them,
    // however often it reads them.
    const PERMISSIONS: &str = "\
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 2000) \
        INSERT INTO Permissions (UserId, Kind, Value) \
            SELECT printf('00000000-0000-0000-0000-%012d', x), 9, 0 FROM n;";
    const PEOPLE: &str = "\
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 1000) \
        INSERT INTO People (ItemId, Name, PersonType, SortOrder, ListOrder) \
            SELECT '00000000-0000-0000-0000-0000000000F1', printf('Person %d', x), 'Actor', x, x \
            FROM n;";
    // A `Peoples` table whose CHECK fails every row, filled with checks off.
    const CHECKED_PEOPLES: &str = "\
        DROP TABLE Peoples; \
        CREATE TABLE Peoples (Id TEXT, Name TEXT NOT NULL, PersonType TEXT, CHECK (0)); \
        PRAGMA ignore_check_constraints = ON;";
    let tmp = TempDir::new();
    let (deep, old, checked) = (
        tmp.0.join("permissions_18_levels"),
        tmp.0.join("people_3_levels"),
        tmp.0.join("check_fails"),
    );
    for dir in [&deep, &old, &checked] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let deep_db = deep.join("jellyfin.db");
    write_db(&deep_db, &[USERS_DDL, NEW_LIBRARY_DDL, PERMISSIONS]).await;
    share_child_pages(&deep_db, "Permissions", 18).await;
    write_db(&old.join("jellyfin.db"), &[USERS_DDL]).await;
    write_db(&old.join("library.db"), &[OLD_LIBRARY_DDL, PEOPLE]).await;
    share_child_pages(&old.join("library.db"), "People", 3).await;
    write_db(
        &checked.join("jellyfin.db"),
        &[USERS_DDL, NEW_LIBRARY_DDL, CHECKED_PEOPLES, ONE_CREDIT],
    )
    .await;
    let outcome = |dir: &Path, file: &str, table: &str| match read_within_5s(dir) {
        None => "still reading after 5 s".to_string(),
        Some(Ok(())) => "read".to_string(),
        Some(Err(ImportError::BadSource(text)))
            if text.contains(&dir.join(file).display().to_string())
                && text.contains(&format!("{table} is damaged")) =>
        {
            format!("{file}: {table} is damaged")
        }
        Some(Err(other)) => format!("{other:?}"),
    };
    assert_eq!(
        [
            outcome(&deep, "jellyfin.db", "Permissions"),
            outcome(&old, "library.db", "People"),
            outcome(&checked, "jellyfin.db", "Peoples"),
        ],
        [
            "jellyfin.db: Permissions is damaged",
            "library.db: People is damaged",
            "read",
        ]
    );
}

/// Zeroes the root page of `table` in the SQLite file `file`, as a torn write can leave a page: no
/// b-tree page at all. The file must have no -wal.
async fn zero_root_page(file: &Path, table: &str) {
    use sqlx::{ConnectOptions, Connection};
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .connect()
        .await
        .unwrap();
    let (root,): (i64,) = sqlx::query_as("SELECT rootpage FROM sqlite_schema WHERE name = ?1")
        .bind(table)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    let (size,): (i64,) = sqlx::query_as("PRAGMA page_size")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(
        !file.with_extension("db-wal").exists(),
        "{}",
        file.display()
    );
    let (root, size) = (
        usize::try_from(root).unwrap(),
        usize::try_from(size).unwrap(),
    );
    let mut bytes = std::fs::read(file).unwrap();
    bytes[(root - 1) * size..root * size].fill(0);
    std::fs::write(file, bytes).unwrap();
}

// Fix round 5, R3: the walk covers page 1 and the trees `quick_check` reads, so a bad page in a
// table the import never reads leaves the read alone, and the same page in a checked table, or in
// one of its indexes, is refused, on both paths.
#[tokio::test]
async fn b20_snapshot_walks_only_checked_tables() {
    use super::source::integrity_roots;
    const ACTIVITY_LOGS: &str = "\
        CREATE TABLE ActivityLogs (Id INTEGER PRIMARY KEY, Name TEXT NOT NULL); \
        INSERT INTO ActivityLogs (Name) VALUES ('Started');";
    // The schema row of Users' automatic index (for its TEXT primary key) names another table.
    // SQLite checks a row's `tbl_name` against the row's SQL, but this row has none: SQLite binds
    // it to the index by name alone, so `quick_check(Users)` reads it all the same.
    const MISNAMED_INDEX: &str = "\
        PRAGMA writable_schema = ON; \
        UPDATE sqlite_schema SET tbl_name = 'ActivityLogs' \
            WHERE name = 'sqlite_autoindex_Users_1'; \
        PRAGMA writable_schema = OFF;";
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let mut outcomes = Vec::new();
    for damaged in ["ActivityLogs", "Permissions", "sqlite_autoindex_Users_1"] {
        let dir = tmp.0.join(damaged);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("jellyfin.db");
        write_db(
            &db,
            &[USERS_DDL, NEW_LIBRARY_DDL, ACTIVITY_LOGS, MISNAMED_INDEX],
        )
        .await;
        zero_root_page(&db, damaged).await;
        let shown = |outcome: Result<(), ImportError>| match outcome {
            Ok(()) => "read".to_string(),
            Err(ImportError::BadSource(text)) => text.replace(&dir.display().to_string(), "<dir>"),
            Err(other) => format!("{other:?}"),
        };
        let read = read_within_5s(&dir).map_or_else(|| "still reading after 5 s".into(), shown);
        // The in-memory path, then the copy path.
        let mut opened = Vec::new();
        for limit in [u64::MAX, 0] {
            let open =
                open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX))
                    .await;
            opened.push(shown(open.map(drop)));
        }
        outcomes.push((damaged, read, opened));
    }
    let read = "read".to_string();
    let damaged = |table: &str| format!("<dir>/jellyfin.db: {table} is damaged");
    let (permissions, users) = (damaged("Permissions"), damaged("Users"));
    assert_eq!(
        outcomes,
        [
            ("ActivityLogs", read.clone(), vec![read.clone(), read]),
            (
                "Permissions",
                permissions.clone(),
                vec![permissions.clone(), permissions]
            ),
            (
                "sqlite_autoindex_Users_1",
                users.clone(),
                vec![users.clone(), users]
            ),
        ]
    );
    // A root list SQLite doesn't give as expected is refused, never walked in part.
    assert_eq!(integrity_roots(3, "[0,2,5]"), Some(vec![2, 5]));
    assert_eq!(integrity_roots(4, "[0,2,5]"), None);
    assert_eq!(integrity_roots(3, "[0,2,5"), None);
}

// Fix round 3, 3: a copy of a big source holds at most the measured bytes, and starts only when
// the data folder has room for it and 1 GiB to spare. The free space is injected, so no test fills
// the disk, and the one copy made is of a small database.

#[tokio::test]
async fn b20_copies_fit_the_disk() {
    const SPARE: u64 = 1 << 30;
    // At, under and past the bound; the -wal counts; sums that would overflow saturate.
    assert_eq!(
        [
            copy_fits(1, 2, 3 + SPARE),
            copy_fits(1, 2, 4 + SPARE),
            copy_fits(1, 2, 2 + SPARE),
            copy_fits(0, 2, 1 + SPARE),
            copy_fits(u64::MAX, 1, u64::MAX - 1),
            copy_fits(u64::MAX, u64::MAX, SPARE),
        ],
        [true, true, false, false, false, false]
    );
    // Fewer bytes than measured are fine; a byte past them means the file grew.
    let at_most = |bytes: usize, len: u64| {
        let source = vec![7u8; bytes];
        let mut into = Vec::new();
        let whole = copy_at_most(&mut source.as_slice(), &mut into, len).unwrap();
        (whole, into.len())
    };
    assert_eq!(
        [
            at_most(3, 5),
            at_most(5, 5),
            at_most(6, 5),
            at_most(70_000, 65_536),
        ],
        [(true, 3), (true, 5), (false, 5), (false, 65_536)]
    );
    let tmp = TempDir::new();
    assert!(free_space(&tmp.0).unwrap() > 0, "{}", tmp.0.display());
    // A limit of 0 always copies. A copy without room, or whose free space can't be taken, is
    // refused before a byte is copied, and no case leaves a copy folder behind.
    let file = tmp.0.join("jellyfin.db");
    write_db(&file, &[USERS_DDL]).await;
    let data = TempDir::new();
    let folder = data.0.join("jellyfin-import-copy");
    let cases: [(&str, FreeSpace); 4] = [
        ("room", |_| Ok(u64::MAX)),
        ("none", |_| Ok(0)),
        ("2 MiB less a byte", |_| Ok((2 << 20) - 1)),
        ("an error", |_| {
            Err(std::io::Error::other("no statvfs here"))
        }),
    ];
    let mut outcomes = Vec::new();
    for (case, free) in cases {
        let outcome = match open_snapshot_with_free(&file, &data.0, &[], 0, free).await {
            Ok(conn) => {
                let copied = folder.is_dir();
                drop(conn);
                format!("copied: {copied}")
            }
            Err(ImportError::BadSource(message)) => message,
            Err(other) => format!("{other:?}"),
        };
        outcomes.push((case, outcome, std::fs::read_dir(&data.0).unwrap().count()));
    }
    let (file, folder) = (file.display(), folder.display());
    let no_room = |free: &str| {
        format!(
            "not enough free space to copy {file} to {folder}: it needs 1 MiB and 1 GiB to spare, \
             and {free} MiB are free"
        )
    };
    assert_eq!(
        outcomes,
        [
            ("room", "copied: true".to_string(), 0),
            ("none", no_room("0"), 0),
            ("2 MiB less a byte", no_room("1"), 0),
            (
                "an error",
                format!("cannot copy {file} to {folder}: no statvfs here"),
                0
            ),
        ]
    );
}

// F5 (round 4): planned_credits drops repeated names in linear time. 80,000 distinct names are the
// worst case for the old O(n^2) scan (each new name compared with every earlier one): on 8758ca9
// the debug run takes tens of seconds, while a HashSet of the planned names is milliseconds. The
// count is correct either way; the time bound is what fails on 8758ca9. Debug profile (opt-level 0).
#[test]
fn b20_credits_are_deduplicated() {
    const N: usize = 80_000;
    let mut credits: Vec<JfCredit> = (0..N)
        .map(|i| JfCredit {
            name: std::sync::Arc::from(i.to_string().as_str()),
            kind: None,
            role: None,
            list_order: Some(i as i64),
            sort_order: Some(i as i64),
        })
        .collect();
    // A block of repeats of one existing name: they must all collapse to the single planned credit.
    for _ in 0..1_000 {
        credits.push(JfCredit {
            name: std::sync::Arc::from("0"),
            kind: None,
            role: None,
            list_order: Some(0),
            sort_order: Some(0),
        });
    }
    let start = std::time::Instant::now();
    let planned = planned_credits(&credits);
    let elapsed = start.elapsed();
    assert_eq!(
        planned.len(),
        N,
        "each distinct name is planned once, repeats dropped"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "planned_credits of {N} distinct names took {elapsed:?}; it must be linear, not quadratic"
    );
}

// F2 (round 4): metadata_image_paths adds each credited person's images once, not once per credit.
// One person with 1,000 images credited 1,000 times on one matched item makes K x L = 10^6 entries
// on 8758ca9 (~16 MB); the fix (a set of the people already added) makes it K + L. The item has no
// images of its own, so the result is exactly the 1,000 person image paths.
#[test]
fn b20_person_images_are_added_once() {
    use super::source::{JfImage, JfItemMeta, JfMetadata, Snapshot};
    use rustyfin_model::image::ImageType;
    let person = "Ann";
    let images: Vec<JfImage> = (0..1_000)
        .map(|i| JfImage {
            path: format!("/config/metadata/People/a/ann/{i}.jpg"),
            image_type: ImageType::Primary,
            width: None,
            height: None,
            blurhash: None,
        })
        .collect();
    let item = Uuid::from_u128(0xF1);
    let credits: Vec<JfCredit> = (0..1_000)
        .map(|_| JfCredit {
            name: std::sync::Arc::from(person),
            kind: None,
            role: None,
            list_order: None,
            sort_order: None,
        })
        .collect();
    let mut person_images = std::collections::HashMap::new();
    person_images.insert(person.to_string(), images);
    let mut meta_items = std::collections::HashMap::new();
    meta_items.insert(item, JfItemMeta::default());
    let mut meta_credits = std::collections::HashMap::new();
    meta_credits.insert(item, credits);
    let snapshot = Snapshot {
        layout: Layout::New,
        users: Vec::new(),
        items: Vec::new(),
        user_data: Vec::new(),
        streams: Vec::new(),
        metadata: JfMetadata {
            items: meta_items,
            credits: meta_credits,
            person_images,
        },
    };
    let mut matched = std::collections::HashMap::new();
    matched.insert(item, Uuid::from_u128(0xA1));
    let paths = super::metadata_image_paths(&snapshot, &matched);
    assert_eq!(
        paths.len(),
        1_000,
        "a person's images are added once across all credits, not once per credit"
    );
}

// F3 (round 4): the total .mblink text across a source is capped at 1 MiB. 32 links of 32 KiB each
// (exactly 1 MiB) are read; a 33rd that crosses 1 MiB is a BadSource naming the folder. On 8758ca9
// only the per-file 64 KiB cap exists, so the "over" folder reads without error. The links are
// small files, not large or sparse ones.
#[test]
fn b20_mblink_text_is_capped() {
    const S: usize = 32 << 10; // 32 KiB per link, well under the 64 KiB per-file cap.
    let tmp = TempDir::new();
    let films = |case: &str| tmp.0.join(case).join("root/default/Films");
    let path_of = |len: usize| format!("/media/{}", "a".repeat(len - "/media/".len()));
    for (case, n) in [("under", 32usize), ("over", 33usize)] {
        let dir = films(case);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            std::fs::write(dir.join(format!("link{i:03}.mblink")), path_of(S)).unwrap();
        }
    }
    // 32 x 32 KiB = exactly 1 MiB: all read.
    let under = read_library_folders(&tmp.0.join("under")).unwrap();
    let count: usize = under.iter().map(|folder| folder.mblinks.len()).sum();
    assert_eq!(count, 32, "32 links of 32 KiB (exactly 1 MiB) are all read");
    // The 33rd link crosses 1 MiB: a bad source naming the folder.
    let folder = films("over");
    match read_library_folders(&tmp.0.join("over")) {
        Err(ImportError::BadSource(text)) => assert!(
            text.contains(&folder.display().to_string()),
            "the bad source must name the folder, got: {text}"
        ),
        other => panic!("expected a bad source over the 1 MiB total, got {other:?}"),
    }
}

// L6 (round 4): a damaged-table line is escaped (each control character -> \uXXXX) and cut to 200
// bytes on a char boundary with a trailing … , so a crafted quick_check result can't flood the log
// with control bytes or ANSI escapes. On 8758ca9 clip_check_line returns the line unchanged.
#[test]
fn b20_damaged_line_is_clipped_and_escaped() {
    use super::source::clip_check_line;
    // A control character becomes \uXXXX; printable text is unchanged.
    assert_eq!(
        clip_check_line("page 3\u{1b}[0m bad"),
        "page 3\\u001b[0m bad"
    );
    // A clean short line is returned as-is.
    assert_eq!(clip_check_line("page 3 is corrupt"), "page 3 is corrupt");
    // A long line is cut to 200 bytes on a char boundary and ends with a … .
    let long = "x".repeat(500);
    let out = clip_check_line(&long);
    assert!(
        out.ends_with('…'),
        "a cut line ends with an ellipsis: {out}"
    );
    assert!(
        out.len() <= 200 + '…'.len_utf8(),
        "a cut line is at most 200 bytes plus the ellipsis, got {}",
        out.len()
    );
    assert_eq!(&out[..200], &long[..200], "the kept prefix is the input's");
    // A line of multi-byte characters is cut on a char boundary (no panic) and stays bounded.
    let wide = "\u{2713}".repeat(100); // 300 bytes of U+2713
    let out = clip_check_line(&wide);
    assert!(
        out.ends_with('…') && out.len() <= 200 + '…'.len_utf8(),
        "a multi-byte line is cut on a boundary and bounded, got {} bytes",
        out.len()
    );
}

// L7 (round 4): open_regular opens with O_NONBLOCK, so a FIFO (or device) swapped in for a regular
// file between the stat and the open can't block the open forever; the handle's regular-file check
// still rejects it. A static non-regular file is already caught at the stat, so the guarantee is
// the open flag itself. On 8758ca9 the open has no O_NONBLOCK.
#[cfg(unix)]
#[test]
fn b20_regular_source_is_opened_nonblocking() {
    use std::os::unix::io::AsRawFd;
    let tmp = TempDir::new();
    let db = tmp.0.join("library.db");
    std::fs::write(&db, b"hello").unwrap();
    let handle = super::source::open_regular(&db)
        .unwrap()
        .expect("a regular file opens");
    let flags = unsafe { libc::fcntl(handle.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0, "F_GETFL failed");
    assert!(
        flags & libc::O_NONBLOCK != 0,
        "open_regular opens with O_NONBLOCK so a FIFO swapped in after the stat can't block the open"
    );
}

// F4 (round 4): a progress handler bounds the work a snapshot's schema load and checks may do.
// `ProgressGuard::tick`, called every PROGRESS_OPS steps, returns false to interrupt once the budget
// of ticks is spent or the cancel flag is set, so a crafted schema that spins and a request dropped
// mid-read both stop the worker. `progress_budget` scales the budget with the page count, and
// `page_count` reads the page size from the header (bytes 16-17, where 1 means 65536). On 8758ca9
// none of this exists and a schema load that spins never returns.
#[test]
fn b20_progress_budget_and_cancel() {
    use super::source::{
        PROGRESS_FLOOR, PROGRESS_PER_PAGE, ProgressGuard, page_count, progress_budget,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // A budget of N ticks lets N calls continue, then interrupts, and stays interrupted.
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = ProgressGuard::new(2, Arc::clone(&cancel));
    assert!(guard.tick(), "tick 1 of budget 2 continues");
    assert!(guard.tick(), "tick 2 of budget 2 continues");
    assert!(!guard.tick(), "tick 3 of budget 2 interrupts");
    assert!(!guard.tick(), "and stays interrupted");

    // The cancel flag interrupts at once, whatever the budget left.
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = ProgressGuard::new(1_000_000, Arc::clone(&cancel));
    assert!(guard.tick(), "a fresh guard with budget to spare continues");
    cancel.store(true, Ordering::Relaxed);
    assert!(
        !guard.tick(),
        "a set cancel flag interrupts before the budget is spent"
    );

    // The budget is the floor for a small source and grows per page for a big one; it never wraps.
    assert_eq!(
        progress_budget(0),
        PROGRESS_FLOOR,
        "no pages gets the floor"
    );
    assert_eq!(
        progress_budget(u64::MAX),
        u64::MAX.saturating_mul(PROGRESS_PER_PAGE),
        "a huge page count saturates, it does not wrap"
    );
    let big = PROGRESS_FLOOR * 10;
    assert_eq!(
        progress_budget(big),
        big.saturating_mul(PROGRESS_PER_PAGE),
        "beyond the floor the budget is per page"
    );

    // page_count divides the byte length by the header's page size; a field of 1 means 65536; a
    // short or unreadable header takes the largest page, hence the fewest pages (tightest budget).
    let mut header = [0u8; 100];
    header[16] = 0x10; // page size 4096, big-endian
    header[17] = 0x00;
    assert_eq!(
        page_count(&header, 4096 * 7),
        7,
        "byte length over page size"
    );
    header[16] = 0;
    header[17] = 1; // the page-size field 1 means 65536
    assert_eq!(
        page_count(&header, 65536 * 3),
        3,
        "a field of 1 means 65536"
    );
    assert_eq!(
        page_count(&[], 1_000_000),
        1_000_000 / 65536,
        "a short header takes the largest page"
    );
}

// F4 (round 4): once the guard is installed, a query that runs past its budget is interrupted, and
// an interrupted read is reported as a damaged schema, not a generic read error, because a schema
// that makes the read spin is what trips the budget. A benign recursive query stands in for a
// crafted spinning schema here; the live check exercises the real open_snapshot path. On 8758ca9
// there is no guard and the query runs to completion.
#[tokio::test]
async fn b20_progress_interrupt_is_a_damaged_schema() {
    use super::source::{guard_connection, unreadable};
    use sqlx::ConnectOptions;

    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .connect()
        .await
        .unwrap();
    // Budget 0: the first time the handler fires (after PROGRESS_OPS steps) it interrupts.
    let _cancel = guard_connection(Path::new("jellyfin.db"), &mut conn, 0)
        .await
        .unwrap();
    // A recursive CTE counting toward 100000 does far more than PROGRESS_OPS steps before it ends.
    let err = sqlx::query(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 100000) \
         SELECT count(*) FROM c",
    )
    .fetch_one(&mut conn)
    .await
    .expect_err("the query is interrupted before it counts to 100000");
    let ImportError::BadSource(message) = unreadable(Path::new("jellyfin.db"), &err) else {
        panic!("an interrupted read is a BadSource");
    };
    assert!(
        message.contains("its schema is damaged"),
        "an interrupted read is a damaged schema, got: {message}"
    );
}

// Fix round 5, R5: guard_connection shares the guard's tick counter, so a snapshot can log how much
// of the budget an import spent (SnapshotConn's drop). The counter starts at zero and grows as the
// guarded connection steps, so the margin (budget / ticks) the log reports reflects real work.
#[tokio::test]
async fn b20_guard_connection_counts_ticks() {
    use super::source::guard_connection;
    use sqlx::ConnectOptions;
    use std::sync::atomic::Ordering;

    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .connect()
        .await
        .unwrap();
    // A budget beyond any real work, so the tick counter, not an interrupt, is what is exercised.
    let guarded = guard_connection(Path::new("jellyfin.db"), &mut conn, u64::MAX)
        .await
        .unwrap();
    assert_eq!(
        guarded.ticks.load(Ordering::Relaxed),
        0,
        "no ticks before any query runs"
    );
    sqlx::query(
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 100000) \
         SELECT count(*) FROM c",
    )
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(
        guarded.ticks.load(Ordering::Relaxed) > 0,
        "the shared counter grows as the guarded connection steps"
    );
}

// F1 (round 4): check_tree_depth walks the schema b-tree (page 1) and every table/index root over
// raw pages before quick_check runs, refusing a tree that is too deep, shares or revisits a page, or
// holds a bad page number, type or cell offset — so quick_check can't recurse into it and overflow
// the sqlx worker thread's stack. These feed the walk synthetic pages through a Vec-backed `Pages`,
// and exercise `wal_commit_frames`/`CopyPages` (the copy path's WAL-aware page source) on a hand-made
// WAL. On d5425c6 none of `Pages`, `check_tree_depth`, `wal_commit_frames` or `CopyPages` exist.

/// A `Pages` over a Vec of equal-length pages. Page `n` (1-indexed) is `pages[n - 1]`; `reserved`
/// trims the usable area as a real database header's byte 20 would.
struct VecPages {
    pages: Vec<Vec<u8>>,
    reserved: usize,
}

impl super::source::Pages for VecPages {
    fn page_count(&self) -> u64 {
        self.pages.len() as u64
    }
    fn page_size(&self) -> usize {
        self.pages.first().map_or(0, Vec::len)
    }
    fn reserved(&self) -> usize {
        self.reserved
    }
    fn read_page(&self, pgno: u32, buf: &mut [u8]) -> bool {
        match self.pages.get(pgno as usize - 1) {
            Some(page) if page.len() == buf.len() => {
                buf.copy_from_slice(page);
                true
            }
            _ => false,
        }
    }
}

/// An interior b-tree page (type 5) whose header sits at `h` (0, or 100 for page 1), with one cell
/// per `children` entry (the cell is just its 4-byte left child, laid near the page's end) and
/// `right` as the right child.
fn interior_at(ps: usize, h: usize, children: &[u32], right: u32) -> Vec<u8> {
    let mut page = vec![0u8; ps];
    page[h] = 5;
    page[h + 3..h + 5].copy_from_slice(&(children.len() as u16).to_be_bytes());
    page[h + 8..h + 12].copy_from_slice(&right.to_be_bytes());
    let mut cell_at = ps - 4;
    for (i, &child) in children.iter().enumerate() {
        let ptr = h + 12 + 2 * i;
        page[ptr..ptr + 2].copy_from_slice(&(cell_at as u16).to_be_bytes());
        page[cell_at..cell_at + 4].copy_from_slice(&child.to_be_bytes());
        cell_at -= 4;
    }
    page
}

/// A leaf b-tree page (type 13) whose header sits at `h`.
fn leaf_at(ps: usize, h: usize) -> Vec<u8> {
    let mut page = vec![0u8; ps];
    page[h] = 13;
    page
}

#[test]
fn b20_check_tree_depth_walks_and_refuses() {
    use super::source::check_tree_depth;
    let ps = 512;
    let file = Path::new("jellyfin.db");

    // A healthy shallow tree: page 1 is the schema leaf, the root at page 2 is an interior with two
    // leaf children (pages 3 and 4). It passes.
    let good = VecPages {
        pages: vec![
            leaf_at(ps, 100),
            interior_at(ps, 0, &[3], 4),
            leaf_at(ps, 0),
            leaf_at(ps, 0),
        ],
        reserved: 0,
    };
    check_tree_depth(&good, file, &[("Permissions".into(), 2)]).expect("a shallow tree passes");

    // A chain one level past the limit: pages 2..=23 each point to the next, so the walk reaches a
    // depth of 21 and refuses the named root.
    let mut pages = vec![leaf_at(ps, 100)];
    for next in 3..=24u32 {
        pages.push(interior_at(ps, 0, &[], next));
    }
    pages.push(leaf_at(ps, 0));
    let deep = VecPages { pages, reserved: 0 };
    let err = check_tree_depth(&deep, file, &[("Permissions".into(), 2)])
        .expect_err("a chain past the depth limit is refused");
    assert!(
        matches!(&err, ImportError::BadSource(m) if m.contains("Permissions is damaged")),
        "got: {err:?}"
    );

    // A page shared by two roots: the second visit to page 3 is a revisit, refused with the second
    // root's name.
    let shared = VecPages {
        pages: vec![
            leaf_at(ps, 100),
            interior_at(ps, 0, &[], 3),
            leaf_at(ps, 0),
            interior_at(ps, 0, &[], 3),
        ],
        reserved: 0,
    };
    let err = check_tree_depth(&shared, file, &[("A".into(), 2), ("B".into(), 4)])
        .expect_err("a page shared between two roots is refused");
    assert!(
        matches!(&err, ImportError::BadSource(m) if m.contains("B is damaged")),
        "got: {err:?}"
    );

    // A violation reached from page 1 (the schema) reports "its schema is damaged", not a table name.
    let bad_schema = VecPages {
        pages: vec![interior_at(ps, 100, &[], 99), leaf_at(ps, 0)],
        reserved: 0,
    };
    let err = check_tree_depth(&bad_schema, file, &[])
        .expect_err("an out-of-range schema child is refused");
    assert!(
        matches!(&err, ImportError::BadSource(m) if m.contains("its schema is damaged")),
        "got: {err:?}"
    );

    // An unknown page-type byte is refused.
    let mut odd = leaf_at(ps, 0);
    odd[0] = 7;
    let bad_type = VecPages {
        pages: vec![leaf_at(ps, 100), odd],
        reserved: 0,
    };
    let err = check_tree_depth(&bad_type, file, &[("T".into(), 2)])
        .expect_err("an unknown page type is refused");
    assert!(
        matches!(&err, ImportError::BadSource(m) if m.contains("T is damaged")),
        "got: {err:?}"
    );
}

/// Builds a one-commit `-wal` (big-endian magic) over `page_size` carrying `frames` as
/// (page number, page bytes); the last frame commits and sets the database size to `db_pages`.
fn build_wal(page_size: usize, db_pages: u32, frames: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let last = frames.len().saturating_sub(1);
    let frames: Vec<(u32, u32, Vec<u8>)> = frames
        .iter()
        .enumerate()
        .map(|(i, (pgno, page))| (*pgno, if i == last { db_pages } else { 0 }, page.clone()))
        .collect();
    build_wal_commits(page_size, &frames)
}

/// Builds a `-wal` (big-endian magic) over `page_size` carrying `frames` as (page number, the
/// database size a commit frame sets or 0 for none, page bytes).
fn build_wal_commits(page_size: usize, frames: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    use super::source::checksum;
    let mut wal = vec![0u8; 32];
    wal[0..4].copy_from_slice(&0x377f_0683u32.to_be_bytes());
    wal[4..8].copy_from_slice(&3_007_000u32.to_be_bytes());
    wal[8..12].copy_from_slice(&(page_size as u32).to_be_bytes());
    wal[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let (h0, h1) = checksum(&wal[0..24], true, (0, 0));
    wal[24..28].copy_from_slice(&h0.to_be_bytes());
    wal[28..32].copy_from_slice(&h1.to_be_bytes());
    let mut running = (h0, h1);
    for (pgno, commit, page) in frames {
        let mut frame = vec![0u8; 24];
        frame[0..4].copy_from_slice(&pgno.to_be_bytes());
        frame[4..8].copy_from_slice(&commit.to_be_bytes());
        frame[8..16].copy_from_slice(&wal[16..24]);
        let sum = checksum(&frame[0..8], true, running);
        let sum = checksum(page, true, sum);
        frame[16..20].copy_from_slice(&sum.0.to_be_bytes());
        frame[20..24].copy_from_slice(&sum.1.to_be_bytes());
        running = sum;
        wal.extend_from_slice(&frame);
        wal.extend_from_slice(page);
    }
    wal
}

#[test]
fn b20_wal_commit_frames_maps_the_latest_frame() {
    use super::source::{CopyPages, Pages, read_slice, wal_commit_frames};
    let ps = 512;
    let file = Path::new("jellyfin.db");

    // Two frames for page 2: the first carries X, the last carries Y and commits two pages.
    let x = vec![0xAAu8; ps];
    let y = vec![0xBBu8; ps];
    let wal = build_wal(ps, 2, &[(2, x.clone()), (2, y.clone())]);
    let commit = wal_commit_frames(file, read_slice(&wal))
        .unwrap()
        .expect("a valid commit");
    assert_eq!(commit.page_size, ps);
    assert_eq!(commit.pages, 2);
    assert_eq!(commit.committed, 2, "the commit covers both frames");
    assert_eq!(
        commit.frames.len(),
        1,
        "page 2 maps to one frame, the later"
    );
    let last_off = commit.frames[&2] as usize;
    assert_eq!(
        &wal[last_off..last_off + ps],
        y.as_slice(),
        "the later frame holds Y"
    );

    // CopyPages over a db file (page 2 = X on disk) and this WAL reads Y for page 2: the committed
    // frame overrides the page on disk.
    let dir = TempDir::new();
    let mut db = vec![0u8; 2 * ps];
    db[16..18].copy_from_slice(&(ps as u16).to_be_bytes());
    db[ps..].copy_from_slice(&x);
    let db_path = dir.0.join("source.db");
    let wal_path = dir.0.join("source.db-wal");
    std::fs::write(&db_path, &db).unwrap();
    std::fs::write(&wal_path, &wal).unwrap();
    let pages = CopyPages::open(file, &db_path, &wal_path).expect("open the copy");
    assert_eq!(pages.page_count(), 2);
    let mut buf = vec![0u8; ps];
    assert!(pages.read_page(2, &mut buf));
    assert_eq!(
        buf, y,
        "page 2 reads the WAL frame (Y), overriding the db page (X)"
    );
}

// Fix round 5, R1: the copy path streams the copied -wal a frame at a time (at 3a2d9da `read_whole`
// read all of it into one buffer) and maps each page to its latest committed frame. Two commits,
// page 2 rewritten across them, and an uncommitted tail that rewrites page 3 and adds page 5.
#[test]
fn b20_wal_streams_only_committed_frames() {
    use super::source::{CopyPages, Pages, read_slice, wal_commit_frames};
    use std::collections::HashMap;
    let ps = 512;
    let file = Path::new("jellyfin.db");
    let page = |byte: u8| vec![byte; ps];
    let wal = build_wal_commits(
        ps,
        &[
            (2, 0, page(0xA1)),
            (3, 3, page(0xB1)), // commit 1: three pages
            (2, 0, page(0xA2)), // page 2 rewritten
            (4, 4, page(0xC1)), // commit 2: four pages
            (3, 0, page(0xB2)), // the uncommitted tail
            (5, 0, page(0xD1)),
        ],
    );
    // Frame i (from 0) holds its page at 32 + i * (24 + ps) + 24.
    let data = |i: u64| 32 + i * (24 + ps as u64) + 24;
    let expected = HashMap::from([(2, data(2)), (3, data(1)), (4, data(3))]);

    // The scan asks for one frame at a time.
    let mut largest = 0;
    let mut slice = read_slice(&wal);
    let commit = wal_commit_frames(file, |offset, buf| {
        largest = largest.max(buf.len());
        slice(offset, buf)
    })
    .unwrap()
    .expect("a valid commit");
    assert!(
        largest <= 24 + ps,
        "the largest read is {largest} bytes, more than one frame"
    );
    assert_eq!((commit.pages, commit.committed), (4, 4));
    assert_eq!(commit.frames, expected);

    // The copy path streams the copied file to the same map and reads pages through it. The db
    // file holds three pages, 2 and 3 filled with 0xEE.
    let dir = TempDir::new();
    let mut db = vec![0u8; 3 * ps];
    db[16..18].copy_from_slice(&(ps as u16).to_be_bytes());
    db[ps..].fill(0xEE);
    let db_path = dir.0.join("source.db");
    let wal_path = dir.0.join("source.db-wal");
    std::fs::write(&db_path, &db).unwrap();
    std::fs::write(&wal_path, &wal).unwrap();
    let pages = CopyPages::open(file, &db_path, &wal_path).expect("open the copy");
    assert_eq!(pages.frames, expected);
    assert_eq!(pages.page_count(), 4);
    let mut buf = vec![0u8; ps];
    for (pgno, byte) in [(2, 0xA2), (3, 0xB1), (4, 0xC1)] {
        assert!(pages.read_page(pgno, &mut buf), "page {pgno} reads");
        assert_eq!(buf, page(byte), "page {pgno}");
    }
}

// Fix round 5, N1: the copy path reads the copy by the page size SQLite uses, page 1's as read
// through the -wal. A copy whose database header, -wal header and page 1 disagree is refused with
// apply_wal's text, and so is a commit naming more pages than the file and frames hold. Before, the
// copy path took the size from the database header alone and the page count from the commit as is.
#[tokio::test]
async fn b20_copy_path_reads_sqlites_page_size() {
    use sqlx::ConnectOptions;
    let tmp = TempDir::new();
    let data = TempDir::new();
    // A WAL-mode database with 1024-byte pages whose tables were made after the last checkpoint,
    // so its -wal commits page 1. The writer stays open: closing it would checkpoint the -wal.
    let mut writer = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(tmp.0.join("jellyfin.db"))
        .create_if_missing(true)
        .page_size(1024)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .connect()
        .await
        .unwrap();
    sqlx::raw_sql("PRAGMA wal_autocheckpoint = 0;")
        .execute(&mut writer)
        .await
        .unwrap();
    sqlx::raw_sql(USERS_DDL).execute(&mut writer).await.unwrap();
    let live_wal = std::fs::read(tmp.0.join("jellyfin.db-wal")).unwrap();
    assert_eq!(
        live_wal[8..12],
        1024u32.to_be_bytes(),
        "the -wal's page size"
    );

    // Re-check 4, point 4 (c): the database header says 512, the -wal and its page 1 say 1024.
    let small_header = source_with_wal(&tmp.0, &tmp.0.join("small_header"), &live_wal);
    let mut bytes = std::fs::read(&small_header).unwrap();
    bytes[16..18].copy_from_slice(&512u16.to_be_bytes());
    std::fs::write(&small_header, bytes).unwrap();

    // The tables checkpointed into the file, then hand-made -wals over it that carry page 1: one
    // whose page 1 says 512 while both headers say 1024, and one whose commit names two pages
    // more than the file's and its one frame.
    sqlx::raw_sql("PRAGMA wal_checkpoint(TRUNCATE);")
        .execute(&mut writer)
        .await
        .unwrap();
    let db = std::fs::read(tmp.0.join("jellyfin.db")).unwrap();
    let db_pages = u32::try_from(db.len() / 1024).unwrap();
    let page1 = db[..1024].to_vec();
    let mut small_page1 = page1.clone();
    small_page1[16..18].copy_from_slice(&512u16.to_be_bytes());
    let small_first = source_with_wal(
        &tmp.0,
        &tmp.0.join("small_page1"),
        &build_wal(1024, db_pages, &[(1, small_page1)]),
    );
    let past_end = source_with_wal(
        &tmp.0,
        &tmp.0.join("past_end"),
        &build_wal(1024, db_pages + 2, &[(1, page1)]),
    );

    let shown = |file: &Path, outcome: Result<(), ImportError>| match outcome {
        Ok(()) => "read".to_string(),
        Err(ImportError::BadSource(text)) => text.replace(&file.display().to_string(), "<file>"),
        Err(other) => format!("{other:?}"),
    };
    let (mut copied, mut in_memory) = (Vec::new(), Vec::new());
    for file in [&small_header, &small_first, &past_end] {
        for (limit, outcomes) in [(0, &mut copied), (u64::MAX, &mut in_memory)] {
            let outcome =
                open_snapshot_with_free(file, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX))
                    .await;
            outcomes.push(shown(file, outcome.map(drop)));
        }
    }
    drop(writer);
    let mismatch = "<file>: its database file does not match its -wal";
    let past = "<file>: its -wal commits more pages than its database holds";
    assert_eq!(copied, [mismatch, mismatch, past], "the copy path");
    // The in-memory path refused the first and the last already. Its image is read by its page 1's
    // size, whatever that says, so the second is checked on the copy path only.
    assert_eq!(
        [&in_memory[0], &in_memory[2]],
        [mismatch, past],
        "the in-memory path"
    );
}

/// Writes `file`, a database of 512-byte pages whose schema b-tree spins (re-check 3, F4): page 1
/// and the next `levels - 1` pages are interior pages whose `fan_out` child pointers all point at
/// the next of them, and the last one's at a leaf of `rows` schema rows holding only NULLs. Loading
/// the schema reads that leaf `fan_out`^`levels` times, and SQLite refuses each row (it has no root
/// page) without ending the load, so 6 levels of 20 (3.5 KB) load for minutes. No tree is deeper
/// than `levels`, so no cursor or check can overflow a stack.
fn spinning_schema(file: &Path, levels: u32, fan_out: u16, rows: u8) {
    let ps = 512;
    let pages = levels + 1;
    let mut bytes = vec![0u8; ps * pages as usize];
    // The database header (file format section 1.3): a rollback-journal database in UTF-8 whose
    // size in pages holds (its change counter matches its version-valid-for number).
    bytes[..16].copy_from_slice(b"SQLite format 3\0");
    bytes[16..24].copy_from_slice(&[2, 0, 1, 1, 0, 64, 32, 32]);
    bytes[24..28].copy_from_slice(&1u32.to_be_bytes());
    bytes[28..32].copy_from_slice(&pages.to_be_bytes());
    bytes[44..48].copy_from_slice(&4u32.to_be_bytes());
    bytes[56..60].copy_from_slice(&1u32.to_be_bytes());
    bytes[92..96].copy_from_slice(&1u32.to_be_bytes());
    bytes[96..100].copy_from_slice(&3_046_000u32.to_be_bytes());
    let cells = fan_out - 1;
    for level in 0..levels {
        let page = &mut bytes[level as usize * ps..][..ps];
        let h = if level == 0 { 100 } else { 0 };
        let child = (level + 2).to_be_bytes();
        // An interior table page: `cells` cells of a child pointer and a one-byte key, and the
        // right-most pointer, all at the next page.
        page[h] = 0x05;
        page[h + 3..h + 5].copy_from_slice(&cells.to_be_bytes());
        page[h + 5..h + 7].copy_from_slice(&(512 - 5 * cells).to_be_bytes());
        page[h + 8..h + 12].copy_from_slice(&child);
        for cell in 0..usize::from(cells) {
            let at = ps - 5 * (cell + 1);
            page[h + 12 + 2 * cell..h + 14 + 2 * cell].copy_from_slice(&(at as u16).to_be_bytes());
            page[at..at + 4].copy_from_slice(&child);
            page[at + 4] = cell as u8 + 1;
        }
    }
    // The leaf: `rows` cells of 8 bytes, the payload size, the rowid and a record of five NULLs.
    let leaf = &mut bytes[levels as usize * ps..];
    leaf[0] = 0x0D;
    leaf[3..5].copy_from_slice(&u16::from(rows).to_be_bytes());
    leaf[5..7].copy_from_slice(&(512 - 8 * u16::from(rows)).to_be_bytes());
    for row in 0..rows {
        let at = ps - 8 * (usize::from(row) + 1);
        let pointer = 8 + 2 * usize::from(row);
        leaf[pointer..pointer + 2].copy_from_slice(&(at as u16).to_be_bytes());
        leaf[at..at + 8].copy_from_slice(&[6, row + 1, 6, 0, 0, 0, 0, 0]);
    }
    std::fs::write(file, bytes).unwrap();
}

// Fix round 5, R4 (re-check 4, point 4 (d)): a request dropped while a snapshot's first statement
// loads a spinning schema stops the worker, on both paths. The probe lifts the budget, so only the
// cancel can end the load, and counts the handler's ticks. Before, `guard_connection` returned a
// bare flag that was wrapped in `CancelOnDrop` only after `read_roots`, so dropping the request
// there dropped the flag unset and the worker spun on until the load ended.
#[tokio::test]
async fn b20_dropped_snapshot_stops_its_worker() {
    use super::source::{PROGRESS_PROBE, ProgressProbe};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    let tmp = TempDir::new();
    let data = TempDir::new();
    let file = tmp.0.join("jellyfin.db");
    spinning_schema(&file, 6, 20, 10);
    let mut outcomes = Vec::new();
    for (path, limit) in [("in memory", u64::MAX), ("copy", 0)] {
        let probe = Arc::new(ProgressProbe {
            budget: u64::MAX,
            ticks: AtomicU64::new(0),
        });
        let open =
            open_snapshot_with_free(&file, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX));
        let ticked = async {
            while probe.ticks.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        // Run the request until the worker's first tick, then drop it: the worker is then inside
        // the spinning load, which only the drop's cancel can end. A loaded machine may not
        // schedule the sqlx worker for seconds, so the wait allows 10 s. Dropping the request at a
        // fixed 200 ms flaked (re-check 5, L2), and could pass without the worker ever running
        // (re-check 6, L1).
        let started = tokio::select! {
            _ = PROGRESS_PROBE.scope(Arc::clone(&probe), open) => "opened before its first tick",
            () = ticked => "ticked",
            () = tokio::time::sleep(Duration::from_secs(10)) => "no tick within 10 s",
        };
        // The probe counts a tick before the handler checks the cancel, so a working cancel lets
        // the ticks climb in at most one 100 ms sample after the drop; a broken one lets them climb
        // in every sample. Two climbs in a row within 2 s mean the drop did not stop the worker.
        let outcome = if started == "ticked" {
            let dropped = Instant::now();
            let mut last = probe.ticks.load(Ordering::Relaxed);
            let mut climbed = false;
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let now = probe.ticks.load(Ordering::Relaxed);
                if now != last && climbed {
                    break "still spinning after the drop";
                }
                climbed = now != last;
                last = now;
                if dropped.elapsed() > Duration::from_secs(2) {
                    break "stopped";
                }
            }
        } else {
            started
        };
        outcomes.push(format!("{path}: {outcome}"));
    }
    assert_eq!(outcomes, ["in memory: stopped", "copy: stopped"]);
}

// Fix round 5, R6: SQLite's error text can carry text from the file, such as the object name in
// "malformed database schema (<name>)", so `unreadable` passes it through `clip_check_line` as it
// does a quick_check line: control characters escaped, at most 200 bytes. Before, a crafted name
// reached the message, and the log, whole.
#[tokio::test]
async fn b20_unreadable_text_is_clipped() {
    // A schema row with no root page, named by an ANSI escape, 1,000 x's and a line break.
    const CRAFTED_NAME: &str = "\
        PRAGMA writable_schema = ON; \
        INSERT INTO sqlite_schema (type, name, tbl_name, rootpage, sql) \
            VALUES ('table', char(27) || '[31m' || replace(hex(zeroblob(500)), '0', 'x') \
                || char(10) || 'end', 'Crafted', NULL, NULL); \
        PRAGMA writable_schema = OFF;";
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let db = tmp.0.join("jellyfin.db");
    write_db(&db, &[USERS_DDL, CRAFTED_NAME]).await;
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let open =
            open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX)).await;
        texts.push(match open.map(drop) {
            Ok(()) => "read".to_string(),
            Err(ImportError::BadSource(text)) => text.replace(&db.display().to_string(), "<file>"),
            Err(other) => format!("{other:?}"),
        });
    }
    // 68 bytes of sqlx's and SQLite's text, the escape's 6 and "[31m", then 122 x's make 200.
    let clipped = format!(
        "cannot read <file>: error returned from database: (code: 11) malformed database schema \
         (\\u001b[31m{}…",
        "x".repeat(122)
    );
    assert_eq!(
        texts,
        [clipped.clone(), clipped],
        "the in-memory path, then the copy path"
    );
}

/// Rewrites `table`'s root in the SQLite file `file` (which has no `-wal`) into the head of a
/// single-child interior chain `levels` pages deep, appended to the file: the root and the first
/// `levels - 1` appended pages become interior pages carrying only a right-most pointer to the next,
/// and the last appended page a leaf. The walk reaches a depth past `MAX_TREE_DEPTH` and refuses the
/// tree; a `quick_check` would instead recurse the chain past a cursor's `BTCURSOR_MAX_DEPTH`.
/// `levels` must exceed `MAX_TREE_DEPTH` and the table's own tree must be shallower.
async fn deepen_tree(file: &Path, table: &str, levels: u32) {
    use sqlx::{ConnectOptions, Connection};
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(file)
        .connect()
        .await
        .unwrap();
    let (root,): (i64,) = sqlx::query_as("SELECT rootpage FROM sqlite_schema WHERE name = ?1")
        .bind(table)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(
        !file.with_extension("db-wal").exists(),
        "{}",
        file.display()
    );
    let mut bytes = std::fs::read(file).unwrap();
    // The header holds the page size at offset 16 (1 means 65536) and the size in pages at 28.
    let size = match u16::from_be_bytes([bytes[16], bytes[17]]) {
        1 => 65536,
        s => usize::from(s),
    };
    let count = u32::try_from(bytes.len() / size).unwrap();
    let root = u32::try_from(root).unwrap();
    let start = |page: u32| (page as usize - 1) * size;
    // Append `levels` pages and chain root -> count+1 -> ... -> count+levels (a leaf). A table root
    // is never page 1, so every page's b-tree header sits at offset 0.
    bytes.resize(bytes.len() + levels as usize * size, 0);
    let chain: Vec<u32> = std::iter::once(root)
        .chain(count + 1..=count + levels)
        .collect();
    for pair in chain.windows(2) {
        let (page, next) = (pair[0], pair[1]);
        let slot = &mut bytes[start(page)..start(page) + size];
        slot.fill(0);
        slot[0] = 0x05; // interior table page, zero cells
        slot[8..12].copy_from_slice(&next.to_be_bytes()); // right-most pointer
    }
    let leaf = start(count + levels);
    bytes[leaf..leaf + size].fill(0);
    bytes[leaf] = 0x0D; // leaf table page
    bytes[28..32].copy_from_slice(&(count + levels).to_be_bytes());
    std::fs::write(file, bytes).unwrap();
}

// Fix round 5, T (re-check 4, point 4 (a)): the depth walk is wired into `open_snapshot` on both
// paths. A single-child chain 23 pages deep under `Permissions` passes `read_roots`' EXPLAIN (which
// compiles the check without reading the tree) and is refused by `check_tree_depth` at its depth
// limit — "<file>: Permissions is damaged" with no ": <line>" suffix. With the walk removed,
// `check_tables`' `quick_check` would recurse the chain and add a suffix, or fail to read it, so the
// text would differ: that is the red. 23 levels are far below a stack overflow, so a regression
// fails the assertion without crashing the test.
#[tokio::test]
async fn b20_snapshot_refuses_a_deep_tree() {
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let db = tmp.0.join("jellyfin.db");
    write_db(&db, &[USERS_DDL]).await;
    deepen_tree(&db, "Permissions", 23).await;
    let expected = format!("{}: Permissions is damaged", db.display());
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let open =
            open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX)).await;
        texts.push(match open.map(drop) {
            Ok(()) => "read".to_string(),
            Err(ImportError::BadSource(text)) => text,
            Err(other) => format!("{other:?}"),
        });
    }
    assert_eq!(
        texts,
        [expected.clone(), expected],
        "the in-memory path (u64::MAX), then the copy path (0)"
    );
}

// Fix round 5, T (re-check 4, point 4 (b)): the progress handler is wired into `open_snapshot` on
// both paths. A 6x20 shared-page schema DAG (`spinning_schema`) makes the schema load re-walk its
// leaf for about 90 s; the handler `guard_connection` installs before `read_roots` spends the budget
// and interrupts the load (SQLITE_INTERRUPT -> "its schema is damaged") well within 5 s. With the
// handler removed, the load spins past the timeout: that is the red.
#[tokio::test]
async fn b20_snapshot_refuses_a_spinning_schema() {
    use std::time::Duration;
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let file = tmp.0.join("jellyfin.db");
    spinning_schema(&file, 6, 20, 10);
    let expected = format!("{}: its schema is damaged", file.display());
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let open =
            open_snapshot_with_free(&file, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX));
        texts.push(
            match tokio::time::timeout(Duration::from_secs(5), open).await {
                Err(_) => "still spinning after 5 s".to_string(),
                Ok(open) => match open.map(drop) {
                    Ok(()) => "read".to_string(),
                    Err(ImportError::BadSource(text)) => text,
                    Err(other) => format!("{other:?}"),
                },
            },
        );
    }
    assert_eq!(
        texts,
        [expected.clone(), expected],
        "the in-memory path (u64::MAX), then the copy path (0)"
    );
}

// Fix round 6 (re-check 5, H1): `read_roots` compiles `EXPLAIN PRAGMA quick_check(<table>)` to find
// a table's roots, and at 480c971 it did so with no guard against a generated-column chain. A
// crafted `Users` carrying a deep chain of *typed* (INTEGER) virtual generated columns (each the sum
// of the one below) makes `quick_check` double its op array per level as it compiles each column's
// affinity, exhausting memory at *compile* time — the progress guard fires on execution, so it can't
// stop a compile — on a few-hundred-byte source (SQLITE_NOMEM, or an OOM kill on a capped host).
// Typing drives the blow-up through affinity, not a CHECK, so `PRAGMA ignore_check_constraints = ON`
// does not flatten it: the fix refuses any generated column before the first `EXPLAIN` (as
// `check_tables` already does once the snapshot is open), returning "<file>: Users has generated
// columns" on both paths. On 480c971 the `EXPLAIN` instead compiles the chain and the open fails with
// "out of memory" (seen in ~2 s under `timeout`+`prlimit`): that is the red. A red run needs a
// prlimit, such as `--as=1GiB`: without one the `EXPLAIN` grows its op array to about 2 GiB a path
// before SQLite refuses (re-check 6, L2). The depth can't shrink to fit: below about 25 levels the
// parent's `EXPLAIN` finishes and `check_tables` refuses with the same text, so the test would pass
// on its parent. Green runs cost nothing, as the generated columns are refused first.
//
// The schema is written straight into `sqlite_schema` with `writable_schema`, leaving the real root
// page intact, as a hand-built source would store it — so building the fixture never compiles the
// chain, only the buggy `EXPLAIN` does.
#[tokio::test]
async fn b20_read_roots_refuses_a_generated_column_chain_before_the_explain() {
    use sqlx::{ConnectOptions, Connection};
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let db = tmp.0.join("jellyfin.db");
    write_db(&db, &[USERS_DDL]).await;
    // 30 virtual generated columns, each `c{k} = c{k-1} + c{k-1}`, typed INTEGER: the op array
    // `quick_check` compiles doubles per level as it resolves each column's affinity, past SQLite's
    // 0x7fffff00 realloc cap well before this depth. Typing drives it, not a CHECK, so the pragma the
    // fix sets cannot flatten it — only refusing the generated columns keeps the compile from running.
    const DEPTH: usize = 30;
    let mut cols = String::from("c0 INTEGER");
    for k in 1..=DEPTH {
        let prev = k - 1;
        cols.push_str(&format!(", c{k} INTEGER AS (c{prev} + c{prev})"));
    }
    let malicious = format!(
        "CREATE TABLE Users (Id TEXT NOT NULL PRIMARY KEY, InternalId INTEGER NOT NULL, \
         Username TEXT NOT NULL, {cols})"
    );
    {
        let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .connect()
            .await
            .unwrap();
        // Swap Users' stored schema text for the malicious one without SQLite validating or
        // expanding it, so the chain is compiled only when `read_roots` reaches its `EXPLAIN`.
        sqlx::query("PRAGMA writable_schema = ON")
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("UPDATE sqlite_schema SET sql = ?1 WHERE name = 'Users'")
            .bind(&malicious)
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::query("PRAGMA writable_schema = OFF")
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
    }
    let expected = format!("{}: Users has generated columns", db.display());
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let open =
            open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX)).await;
        texts.push(match open.map(drop) {
            Ok(()) => "read".to_string(),
            Err(ImportError::BadSource(text)) => text,
            Err(other) => format!("{other:?}"),
        });
    }
    assert_eq!(
        texts,
        [expected.clone(), expected],
        "the in-memory path (u64::MAX), then the copy path (0)"
    );
}

// Fix round 7 (re-check 6, H2): at 76680f2 `read_roots` and `check_tables` found each named table's
// kind in `pragma_table_list`, which first prepares `SELECT * FROM "<view>"` for every view in the
// file, named or not. That prepare expands the view and generates its code at compile time, where
// the progress guard never fires, so neither the F4 budget nor the R4 cancel can stop it, and it runs
// before either guard. Here an unnamed view filters an unnamed table through a 1,000-value IN list
// and each level above it joins the one below to itself, so resolving the top view copies the list
// 1,024 times. The fix reads the kind from `sqlite_schema`, which costs only the schema parse, so no
// view is resolved: both paths open and read `Users` within 5 s. On 76680f2 both are still busy after
// 5 s (seen under `timeout`+`prlimit`): that is the red, and it needs a debug build, which builds
// the bundled SQLite at `-O0`. Python's optimised SQLite 3.46.1 takes 5.8 s and about 120 MiB a
// connection on these ten levels, only 0.8 s over the bound, so a `--release` run or a faster host
// may pass on the parent. The first path's prepare goes on after its timeout while the second path
// runs, so a red run holds about 240 MiB at once. From eight levels up, each level more about
// doubles the time and the memory.
#[tokio::test]
async fn b20_snapshot_never_resolves_an_unnamed_view() {
    use sqlx::{ConnectOptions, Connection};
    use std::time::Duration;
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let db = tmp.0.join("jellyfin.db");
    let alice = "INSERT INTO Users VALUES ('u1', 1, 'alice')";
    write_db(&db, &[USERS_DDL, alice]).await;
    // Each level doubles the copies of `v0` the view expander makes: 2^DEPTH in all.
    const DEPTH: usize = 10;
    let list: Vec<String> = (0..1000).map(|n| n.to_string()).collect();
    let mut views = format!(
        "CREATE TABLE x (c0 INTEGER); CREATE VIEW v0 AS SELECT c0 FROM x WHERE c0 IN ({});",
        list.join(", ")
    );
    for j in 1..=DEPTH {
        let prev = j - 1;
        views.push_str(&format!(
            " CREATE VIEW v{j} AS SELECT a.c0 FROM v{prev} a, v{prev} b;"
        ));
    }
    {
        // Creating a view doesn't resolve it, so building the fixture is quick.
        let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(views))
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
    }
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let read = async {
            let mut snapshot =
                open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX))
                    .await?;
            let (name,): (String,) = sqlx::query_as("SELECT Username FROM Users")
                .fetch_one(&mut *snapshot)
                .await
                .unwrap();
            Ok::<_, ImportError>(name)
        };
        texts.push(
            match tokio::time::timeout(Duration::from_secs(5), read).await {
                Err(_) => "still busy after 5 s".to_string(),
                Ok(Ok(name)) => format!("read {name}"),
                Ok(Err(ImportError::BadSource(text))) => text,
                Ok(Err(other)) => format!("{other:?}"),
            },
        );
    }
    assert_eq!(
        texts,
        ["read alice", "read alice"],
        "the in-memory path (u64::MAX), then the copy path (0)"
    );
}

// Fix round 8 (re-check 7, H4): every schema load ends by compiling `SELECT tbl,idx,stat FROM
// main.sqlite_stat1` against the file's own definition of that table, before any guard has run, and
// no progress handler can stop a compile. Here `tbl` tops a chain of virtual generated columns, each
// the sum of the one below, so the program doubles per level: 20 levels make 3,145,736 ops
// (3 * 2^20 + 8, counted with Python's SQLite 3.46.1). The fix caps a program at 1,000,000 ops
// (`SQLITE_LIMIT_VDBE_OP`) before the first statement loads the schema, so the load fails and both
// paths refuse the file. Twenty levels are the fewest that clear the cap twice over: 19 make
// 1,572,872 ops, and 18 make 786,440, which can fit under it. On 48f90f4 the compile finishes, the
// empty table is read and both paths open the file: that is the red.
//
// The table is a plain one renamed in `sqlite_schema` with `writable_schema`, as its name is
// reserved. A CREATE TABLE of the chain would itself compile a `SELECT *` over it, which SQLite runs
// to look for generated-column loops, at about twice the cost of the load under test.
#[tokio::test]
async fn b20_snapshot_refuses_a_crafted_sqlite_stat1() {
    use sqlx::{ConnectOptions, Connection};
    let (tmp, data) = (TempDir::new(), TempDir::new());
    let db = tmp.0.join("jellyfin.db");
    write_db(&db, &[USERS_DDL]).await;
    const DEPTH: usize = 20;
    let mut cols = String::from("c0");
    for k in 1..DEPTH {
        let prev = k - 1;
        cols.push_str(&format!(", c{k} AS (c{prev} + c{prev})"));
    }
    let top = DEPTH - 1;
    let stat1 = format!(
        "CREATE TABLE stand_in (a); \
         PRAGMA writable_schema = ON; \
         UPDATE sqlite_schema SET name = 'sqlite_stat1', tbl_name = 'sqlite_stat1', \
         sql = 'CREATE TABLE sqlite_stat1 ({cols}, tbl AS (c{top} + c{top}), idx, stat)' \
         WHERE name = 'stand_in'; \
         PRAGMA writable_schema = OFF"
    );
    {
        let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&db)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(stat1))
            .execute(&mut conn)
            .await
            .unwrap();
        conn.close().await.unwrap();
    }
    let expected = format!("{}: its schema is damaged or too big to read", db.display());
    let mut texts = Vec::new();
    for limit in [u64::MAX, 0] {
        let open =
            open_snapshot_with_free(&db, &data.0, JELLYFIN_TABLES, limit, |_| Ok(u64::MAX)).await;
        texts.push(match open.map(drop) {
            Ok(()) => "read".to_string(),
            Err(ImportError::BadSource(text)) => text,
            Err(other) => format!("{other:?}"),
        });
    }
    assert_eq!(
        texts,
        [expected.clone(), expected],
        "the in-memory path (u64::MAX), then the copy path (0)"
    );
}
