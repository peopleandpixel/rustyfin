// B21: the pure OIDC helpers (spec §5).
use super::*;
use serde_json::json;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn callback_url_appends_the_callback_path() {
    let expected = "http://rf.test:8121/Auth/Oidc/Callback";
    assert_eq!(callback_url("http://rf.test:8121"), expected);
    assert_eq!(callback_url("http://rf.test:8121/"), expected);
    assert_eq!(
        callback_url("https://media.example/jf/"),
        "https://media.example/jf/Auth/Oidc/Callback"
    );
    assert_eq!(callback_url("http://h//"), "http://h/Auth/Oidc/Callback");
    assert_eq!(callback_url(""), "", "no public URL, no callback URL");
}

#[test]
fn cookie_path_follows_the_public_url() {
    assert_eq!(cookie_path("http://rf.test:8121"), "/Auth/Oidc");
    assert_eq!(cookie_path("http://rf.test:8121/"), "/Auth/Oidc");
    assert_eq!(cookie_path("https://media.example/jf"), "/jf/Auth/Oidc");
    assert_eq!(cookie_path("https://media.example/jf/"), "/jf/Auth/Oidc");
    assert_eq!(cookie_path(""), "/Auth/Oidc");
    assert_eq!(cookie_path("not a url"), "/Auth/Oidc");
}

#[test]
fn effective_scopes_default_and_typed() {
    assert_eq!(effective_scopes("", false), ["openid", "profile", "email"]);
    let with_groups = ["openid", "profile", "email", "groups"];
    assert_eq!(effective_scopes("", true), with_groups);
    assert_eq!(effective_scopes("  ", true), with_groups);
    assert_eq!(
        effective_scopes("profile email", false),
        ["openid", "profile", "email"]
    );
    assert_eq!(
        effective_scopes("profile email", true),
        ["openid", "profile", "email"],
        "a typed list is used as typed"
    );
    assert_eq!(
        effective_scopes("email,openid  groups email", false),
        ["openid", "email", "groups"]
    );
    assert_eq!(effective_scopes("openid", false), ["openid"]);
}

#[test]
fn accepted_algs_drop_symmetric_and_unknown_algorithms() {
    assert_eq!(accepted_algs(&[]), ["RS256"], "the OIDC default");
    assert_eq!(accepted_algs(&strings(&["ES256"])), ["ES256"]);
    let listed = strings(&["HS256", "none", "RS256", "ES384", "PS256", "EdDSA", "HS512"]);
    assert_eq!(accepted_algs(&listed), ["RS256", "ES384", "PS256", "EdDSA"]);
    assert!(accepted_algs(&strings(&["HS256", "none"])).is_empty());
    assert_eq!(accepted_algs(&strings(&["ES512", "ES256"])), ["ES256"]);
    assert_eq!(accepted_algs(&strings(&["XY999", "RS512"])), ["RS512"]);
}

#[test]
fn in_group_matches_exactly() {
    let claims = json!({ "groups": ["family", "Admins"], "role": "tv", "n": 5 });
    assert!(in_group(&claims, "groups", "family"));
    assert!(in_group(&claims, "groups", "Admins"));
    assert!(!in_group(&claims, "groups", "admins"), "case matters");
    assert!(!in_group(&claims, "groups", "fam"));
    assert!(
        in_group(&claims, "role", "tv"),
        "a string claim is one group"
    );
    assert!(!in_group(&claims, "missing", "family"));
    assert!(!in_group(&claims, "n", "5"), "only text counts");
    assert!(!in_group(&claims, "groups", ""));
}

#[test]
fn claim_username_trims_and_needs_text() {
    let claims = json!({
        "preferred_username": "  alice ",
        "email": "alice@x.test",
        "blank": "  ",
        "n": 42,
    });
    assert_eq!(
        claim_username(&claims, "preferred_username").as_deref(),
        Some("alice")
    );
    assert_eq!(
        claim_username(&claims, "email").as_deref(),
        Some("alice@x.test")
    );
    assert_eq!(claim_username(&claims, "blank"), None);
    assert_eq!(claim_username(&claims, "n"), None);
    assert_eq!(claim_username(&claims, "missing"), None);
}

#[test]
fn names_match_folds_case_and_compatibility_forms() {
    let same = [
        ("Alice", "alice"),
        ("Änne", "änne"),
        ("Änne", "A\u{0308}nne"),
        ("ÄNNE", "änne"),
        ("kelvin", "KELVIN"),
        ("kelvin", "\u{212A}elvin"),
        ("kelvin", "ｋｅｌｖｉｎ"),
        ("strauss", "Strauß"),
        ("strauss", "STRAUß"),
        ("ΟΔΟ\u{03A3}", "οδο\u{03C2}"),
        ("\u{FB01}ona", "FIONA"),
        ("\u{3392}", "mhz"),
    ];
    for (a, b) in same {
        assert!(names_match(a, b), "{a:?} and {b:?} are one name");
        assert!(names_match(b, a), "{b:?} and {a:?} are one name");
    }
    for (a, b) in [
        ("alice", "alice2"),
        ("alice", " alice"),
        ("alice", "alíce"),
        ("anne", "Änne"),
    ] {
        assert!(!names_match(a, b), "{a:?} and {b:?} are two names");
    }
}
