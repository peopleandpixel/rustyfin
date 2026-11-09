// B21 fix round (spec §10, AC39): codes are unique among live requests.
use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};

use super::*;

/// Stored requests, one per `(code, age in seconds)`.
fn stored(requests: &[(&str, i64)], now: DateTime<Utc>) -> HashMap<String, QuickConnectResult> {
    requests
        .iter()
        .enumerate()
        .map(|(n, &(code, age))| {
            let request = QuickConnectResult {
                authenticated: false,
                secret: format!("{n:032x}"),
                code: code.to_string(),
                device_id: String::new(),
                device_name: String::new(),
                app_name: String::new(),
                app_version: String::new(),
                date_added: now - Duration::seconds(age),
                user_id: None,
                client_key: String::new(),
            };
            (request.secret.clone(), request)
        })
        .collect()
}

#[test]
fn free_code_skips_live_codes_and_wraps() {
    let now = Utc::now();
    let empty = HashMap::new();
    assert_eq!(free_code(&empty, 123, now).as_deref(), Some("000123"));
    assert_eq!(
        free_code(&empty, 1_000_123, now).as_deref(),
        Some("000123"),
        "the start is taken modulo a million"
    );

    let live = stored(&[("000123", 0), ("000124", 299)], now);
    assert_eq!(
        free_code(&live, 123, now).as_deref(),
        Some("000125"),
        "live codes are taken"
    );
    let expired = stored(&[("000123", 300)], now);
    assert_eq!(
        free_code(&expired, 123, now).as_deref(),
        Some("000123"),
        "an expired request's code is free again"
    );
    let end = stored(&[("999999", 0), ("000000", 0)], now);
    assert_eq!(
        free_code(&end, 999_999, now).as_deref(),
        Some("000001"),
        "after 999999 comes 000000"
    );
}
