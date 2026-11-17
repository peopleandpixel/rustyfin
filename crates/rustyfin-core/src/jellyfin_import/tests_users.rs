//! H18, H19: the import matches user names as sign-in does, and a failed run keeps the codes of
//! the users it created.

use std::path::PathBuf;

use chrono::Utc;
use rustyfin_model::user::{User, names_match};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::job::{ImportJob, JobState};
use super::plan::{UserMatch, match_user_name, plan_users};
use super::source::JfUser;
use super::{SkipReason, UserAction, create_users};

/// A temp folder removed when the test ends.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("rustyfin_h19_unit_{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh Rustyfin database in `tmp`.
async fn pool_in(tmp: &TempDir) -> SqlitePool {
    rustyfin_db::init_db(tmp.0.join("rustyfin.db").to_str().unwrap())
        .await
        .unwrap()
}

/// The Rustyfin accounts as (id, name).
async fn accounts(pool: &SqlitePool) -> Vec<(Uuid, String)> {
    rustyfin_db::repo_user::get_all_users(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|user| (user.id, user.name))
        .collect()
}

/// Jellyfin users with these names, ids 100, 101, ...
fn jf_users(names: &[&str]) -> Vec<JfUser> {
    (0..)
        .zip(names)
        .map(|(n, name)| JfUser {
            id: Uuid::from_u128(100 + n as u128),
            name: name.to_string(),
            internal_id: n,
            is_admin: false,
            parental: None,
            access: None,
        })
        .collect()
}

fn user(name: &str) -> User {
    User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: "x".to_string(),
        is_administrator: false,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::Value::Object(Default::default()),
    }
}

// H18: the import matches a name exactly when sign-in would (`names_match`), non-ASCII included.
#[test]
fn h18_match_user_name_folds_like_sign_in() {
    let pairs = [
        ("ẞ", "ss"),
        ("Straße", "STRASSE"),
        ("straße", "Strasse"),
        ("İlker", "i\u{307}lker"),
        ("Ilker", "ılker"),
        ("İlker", "ilker"),
        ("ΟΔΟΣ", "οδος"),
        ("Anna", "ANNA"),
        ("Anna", "Ana"),
        ("e\u{301}lise", "Élise"),
    ];
    let id = Uuid::from_u128(1);
    for (a, b) in pairs {
        let found = match_user_name(a, &[(id, b.to_string())]);
        assert_eq!(
            found == UserMatch::Matched(id),
            names_match(a, b),
            "{a:?} vs {b:?}: the import and sign-in agree"
        );
    }
    assert_eq!(
        match_user_name("ẞ", &[(id, "ss".to_string())]),
        UserMatch::Matched(id),
        "ẞ folds to ss"
    );
    assert_eq!(
        match_user_name("İlker", &[(id, "i\u{307}lker".to_string())]),
        UserMatch::Matched(id),
        "dotted İ folds to i with a combining dot"
    );
    assert_eq!(
        match_user_name("Ilker", &[(id, "ılker".to_string())]),
        UserMatch::NoMatch,
        "dotless ı is its own letter, as at sign-in"
    );
}

// H18: two Jellyfin users sign-in can't tell apart: the preview creates the first only, and a name
// that folds to an existing account's is a suggested match, not a new account.
#[test]
fn h18_plan_users_treats_fold_equal_names_as_one() {
    let jf = jf_users(&["Straße", "STRASSE", "carol"]);
    let rows = plan_users(&jf, None, &[], &[]);
    let actions: Vec<(&str, UserAction, Option<SkipReason>)> = rows
        .iter()
        .map(|row| (row.name.as_str(), row.action, row.reason))
        .collect();
    assert_eq!(
        actions,
        [
            ("carol", UserAction::Create, None),
            // Sorted by lowercase name: "strasse" comes before "straße".
            ("STRASSE", UserAction::Create, None),
            ("Straße", UserAction::Skip, Some(SkipReason::Ambiguous)),
        ]
    );

    let existing = Uuid::from_u128(9);
    let rows = plan_users(&jf[..1], None, &[], &[(existing, "strasse".to_string())]);
    assert_eq!(rows[0].action, UserAction::Skip);
    assert_eq!(rows[0].reason, Some(SkipReason::NotConfirmed));
    assert_eq!(
        rows[0].suggested_match.as_ref().map(|m| m.user_id),
        Some(existing)
    );
}

// H18 and the run-time mediums: a row to create whose name now folds to an account is skipped as
// not confirmed (no merge, no second account); a blank name is skipped, never created.
#[tokio::test]
async fn h18_run_skips_fold_equal_and_invalid_names() {
    let tmp = TempDir::new();
    let pool = pool_in(&tmp).await;
    let jf = jf_users(&["Straße", "  ", "dave"]);
    let mut rows = plan_users(&jf, None, &[], &[]);
    // An account made after the preview, whose name sign-in can't tell from "Straße".
    rustyfin_db::repo_user::create_user(&pool, &user("STRASSE"))
        .await
        .unwrap();
    let job = ImportJob::default();
    let handle = job.try_start().unwrap();
    create_users(&pool, &mut rows, accounts(&pool).await, &handle)
        .await
        .unwrap();
    let by_name = |name: &str| rows.iter().find(|row| row.name == name).unwrap();
    assert_eq!(by_name("Straße").action, UserAction::Skip);
    assert_eq!(by_name("Straße").reason, Some(SkipReason::NotConfirmed));
    assert_eq!(by_name("Straße").user_id, None);
    assert_eq!(by_name("  ").reason, Some(SkipReason::InvalidName));
    assert!(by_name("dave").code.is_some());
    assert_eq!(accounts(&pool).await.len(), 2, "STRASSE and dave only");

    // The repo refuses a fold-equal name too, whatever the caller read before.
    let variant = user("strasse");
    let created = rustyfin_db::repo_setup_code::create_user_with_code(&pool, &variant, "h");
    assert!(!created.await.unwrap());
}

// H19: a run that fails after creating its first user still shows that user's code, once; a re-run
// creates the rest, and both users can redeem their codes.
#[tokio::test]
async fn h19_failed_run_keeps_created_codes() {
    let tmp = TempDir::new();
    let pool = pool_in(&tmp).await;
    let jf = jf_users(&["alice", "bob"]);
    sqlx::raw_sql(
        "CREATE TRIGGER h19_fail BEFORE INSERT ON users WHEN NEW.name = 'bob' \
         BEGIN SELECT RAISE(ABORT, 'disk full'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    let job = ImportJob::default();
    let handle = job.try_start().unwrap();
    let mut rows = plan_users(&jf, None, &[], &[]);
    let failed = create_users(&pool, &mut rows, Vec::new(), &handle).await;
    let error = failed.expect_err("bob's insert fails");
    assert!(
        job.status().created_users.is_empty(),
        "no codes while running"
    );
    handle.fail(error.to_string());

    let status = job.status();
    assert_eq!(status.state, JobState::Failed);
    let names: Vec<&str> = status
        .created_users
        .iter()
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(names, ["alice"]);
    let alice_code = status.created_users[0].code.clone().expect("alice's code");
    assert_eq!(
        job.status().created_users[0].code,
        None,
        "the code is shown once"
    );

    sqlx::raw_sql("DROP TRIGGER h19_fail")
        .execute(&pool)
        .await
        .unwrap();
    let handle = job.try_start().unwrap();
    assert!(
        job.status().created_users.is_empty(),
        "a new run starts empty"
    );
    let known = accounts(&pool).await;
    let mut rows = plan_users(&jf, None, &[], &known);
    create_users(&pool, &mut rows, known, &handle)
        .await
        .unwrap();
    let bob = rows.iter().find(|row| row.name == "bob").unwrap();
    let bob_code = bob.code.clone().expect("bob is created with a code");

    for (name, code) in [("alice", &alice_code), ("bob", &bob_code)] {
        assert!(
            rustyfin_auth::setup_code_pending(&pool, name, code)
                .await
                .unwrap(),
            "{name}'s code is usable"
        );
    }
}
