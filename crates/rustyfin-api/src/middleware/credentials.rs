use axum::http::{HeaderMap, Uri};

/// The fields of a `MediaBrowser` or `Emby` authorization header, as Jellyfin clients send them.
///
/// `Debug` prints every field except the token, which it shows only as present or absent. The
/// token as sent is kept only to compare with the configured API key, and `Debug` never prints it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MediaBrowserAuth {
    pub client: Option<String>,
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub token: Option<String>,
    /// The `Token` value as sent: quotes trimmed, not URL-decoded. Set with `token`.
    token_as_sent: Option<String>,
}

impl std::fmt::Debug for MediaBrowserAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaBrowserAuth")
            .field("client", &self.client)
            .field("device", &self.device)
            .field("device_id", &self.device_id)
            .field("version", &self.version)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl MediaBrowserAuth {
    /// Stores one decoded value under its key. Only the five exact Jellyfin keys count, and a
    /// later pair overwrites an earlier one. A `Token` also keeps its value as sent.
    fn store(&mut self, key: &str, value: &str) {
        let raw = value.trim_matches('"');
        let value = Some(url_decode(raw));
        match key {
            "Client" => self.client = value,
            "Device" => self.device = value,
            "DeviceId" => self.device_id = value,
            "Version" => self.version = value,
            "Token" => {
                self.token = value;
                self.token_as_sent = Some(raw.to_string());
            }
            _ => {}
        }
    }
}

/// Parses a `MediaBrowser` or `Emby` authorization header, such as
/// `MediaBrowser Client="Jellyfin Web", Token="..."`, the way Jellyfin's `GetParts` does. The
/// scheme is matched ignoring ASCII case. Returns `None` for any other scheme, a value with no
/// space, or unbalanced quotes.
pub fn parse_media_browser(value: &str) -> Option<MediaBrowserAuth> {
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("MediaBrowser") && !scheme.eq_ignore_ascii_case("Emby") {
        return None;
    }
    if rest.bytes().filter(|&b| b == b'"').count() % 2 == 1 {
        return None;
    }

    let mut auth = MediaBrowserAuth::default();
    let mut start = 0;
    let mut key = "";
    let mut in_quotes = false;
    for (i, byte) in rest.bytes().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            b',' if !in_quotes => {
                if start < i {
                    auth.store(key, &rest[start..i]);
                    key = "";
                }
                start = i + 1;
            }
            b'=' if !in_quotes => {
                key = rest[start..i].trim();
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < rest.len() {
        auth.store(key, &rest[start..]);
    }
    Some(auth)
}

/// Decodes `value` like .NET's `WebUtility.UrlDecode`. `+` becomes a space, and `%` followed by two
/// hex digits becomes the byte they name. Every other byte is copied, and invalid UTF-8 becomes
/// U+FFFD.
fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let high = bytes.get(i + 1).copied().and_then(hex_digit);
                let low = bytes.get(i + 2).copied().and_then(hex_digit);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high << 4) | low);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The first query parameter named `name`, compared ignoring ASCII case: its value URL-decoded,
/// and its value as sent. Names are URL-decoded for the comparison, and a pair without `=` has an
/// empty value.
fn query_value(uri: &Uri, name: &str) -> Option<(String, String)> {
    uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        url_decode(key)
            .eq_ignore_ascii_case(name)
            .then(|| (url_decode(value), value.to_string()))
    })
}

/// The `MediaBrowser` token of a header value, decoded, with its value as sent.
fn media_browser_token(value: &str) -> Option<PresentedToken> {
    let auth = parse_media_browser(value)?;
    Some(PresentedToken {
        token: auth.token?,
        as_sent: auth.token_as_sent,
    })
}

/// A token from a source that keeps no value as sent: `Bearer`, `X-Emby-Token`,
/// `X-MediaBrowser-Token` or `ApiKey`.
fn without_as_sent(token: Option<String>) -> Option<PresentedToken> {
    token.map(|token| PresentedToken {
        token,
        as_sent: None,
    })
}

/// A token as a request presents it.
pub(crate) struct PresentedToken {
    /// The token. It is URL-decoded when it comes from a `MediaBrowser` header or a query
    /// parameter, and taken as sent otherwise.
    pub token: String,
    /// The token as sent, before decoding, when it comes from the `api_key` query parameter or
    /// from a `MediaBrowser` header's `Token` (in `Authorization` or `X-Emby-Authorization`).
    /// `None` for `Bearer`, `X-Emby-Token`, `X-MediaBrowser-Token` and `ApiKey`.
    pub as_sent: Option<String>,
}

/// The token a request presents, or `None`. A blank token counts as absent, and the first other
/// token wins. The sources, in order: the `Authorization` header (a `Bearer` token as sent,
/// otherwise its `MediaBrowser` token), the `MediaBrowser` token of `X-Emby-Authorization`, the
/// `X-Emby-Token` header, the `X-MediaBrowser-Token` header, then the `ApiKey` and `api_key` query
/// parameters. A header that is absent or not visible ASCII gives nothing. A `MediaBrowser`
/// token and the `api_key` parameter also give their value as sent, in `as_sent`. The other
/// sources give `None` there.
pub(crate) fn request_token(headers: &HeaderMap, uri: &Uri) -> Option<PresentedToken> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let authorization =
        header("authorization").and_then(|value| match value.strip_prefix("Bearer ") {
            Some(token) => without_as_sent(Some(token.to_string())),
            None => media_browser_token(value),
        });
    let api_key = query_value(uri, "api_key").map(|(token, raw)| PresentedToken {
        token,
        as_sent: Some(raw),
    });
    [
        authorization,
        header("x-emby-authorization").and_then(media_browser_token),
        without_as_sent(header("x-emby-token").map(str::to_string)),
        without_as_sent(header("x-mediabrowser-token").map(str::to_string)),
        without_as_sent(query_value(uri, "ApiKey").map(|(token, _)| token)),
        api_key,
    ]
    .into_iter()
    .flatten()
    .find(|presented| !presented.token.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{MediaBrowserAuth, PresentedToken, parse_media_browser, request_token};
    use axum::http::{HeaderMap, HeaderValue, Uri};

    const WEB: &str = r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="TW96aWxsYS81LjA", Version="10.10.0", Token="0123456789abcdef0123456789abcdef""#;

    fn token_of(value: &str) -> Option<String> {
        parse_media_browser(value).and_then(|auth| auth.token)
    }

    /// What `request_token` picks from `headers` and `uri`.
    fn presented(headers: &[(&'static str, &str)], uri: &str) -> Option<PresentedToken> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        request_token(&map, &uri.parse::<Uri>().unwrap())
    }

    /// The token `request_token` picks from `headers` and `uri`.
    fn token(headers: &[(&'static str, &str)], uri: &str) -> Option<String> {
        presented(headers, uri).map(|presented| presented.token)
    }

    /// The token as sent, before decoding, when `request_token` picks the `api_key` parameter or a
    /// `MediaBrowser` token.
    fn as_sent(headers: &[(&'static str, &str)], uri: &str) -> Option<String> {
        presented(headers, uri).and_then(|presented| presented.as_sent)
    }

    fn mb(token: &str) -> String {
        format!(r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", Token="{token}""#)
    }

    #[test]
    fn j1_parses_every_field_of_a_jellyfin_web_header() {
        assert_eq!(
            parse_media_browser(WEB),
            Some(MediaBrowserAuth {
                client: Some("Jellyfin Web".to_string()),
                device: Some("Firefox".to_string()),
                device_id: Some("TW96aWxsYS81LjA".to_string()),
                version: Some("10.10.0".to_string()),
                token: Some("0123456789abcdef0123456789abcdef".to_string()),
                token_as_sent: Some("0123456789abcdef0123456789abcdef".to_string()),
            })
        );
    }

    #[test]
    fn j1_accepts_the_mediabrowser_and_emby_schemes_in_any_case() {
        for scheme in [
            "MediaBrowser",
            "mediabrowser",
            "MEDIABROWSER",
            "Emby",
            "emby",
            "EMBY",
        ] {
            let value = format!(r#"{scheme} Client="Jellyfin Web", Token="t1""#);
            assert_eq!(token_of(&value).as_deref(), Some("t1"), "{value}");
        }
    }

    #[test]
    fn j1_other_schemes_and_a_missing_space_give_no_fields() {
        for value in [
            "Basic dDE6cHc=",
            r#"Bearer Token="t1""#,
            r#"Digest Token="t1""#,
            r#"MediaBrowserX Token="t1""#,
            r#"MediaBrowserToken="t1""#,
            r#"Token="t1""#,
            "MediaBrowser",
            "",
        ] {
            assert_eq!(parse_media_browser(value), None, "{value:?}");
        }
    }

    #[test]
    fn j1_a_comma_inside_quotes_does_not_split() {
        let auth = parse_media_browser(
            r#"MediaBrowser Client="Jellyfin Web", Device="Phone, 2", DeviceId="d1", Version="10.10.0", Token="t1""#,
        )
        .unwrap();
        assert_eq!(auth.device.as_deref(), Some("Phone, 2"));
        assert_eq!(auth.device_id.as_deref(), Some("d1"));
        assert_eq!(auth.version.as_deref(), Some("10.10.0"));
        assert_eq!(auth.token.as_deref(), Some("t1"));
    }

    #[test]
    fn j1_unquoted_values_and_spacing_around_keys() {
        let auth = parse_media_browser("MediaBrowser Client=Kodi,Device=Box,  Token=t1").unwrap();
        assert_eq!(auth.client.as_deref(), Some("Kodi"));
        assert_eq!(auth.device.as_deref(), Some("Box"));
        assert_eq!(auth.token.as_deref(), Some("t1"));
    }

    /// Values are decoded like .NET's `WebUtility.UrlDecode`, which Jellyfin applies to each value.
    #[test]
    fn j1_values_are_url_decoded_like_webutility() {
        let cases = [
            ("Jellyfin%20Web", "Jellyfin Web"),
            ("Jellyfin+Web", "Jellyfin Web"),
            ("a%2Bb", "a+b"),
            ("a%2cb", "a,b"),
            ("%22quoted%22", "\"quoted\""),
            ("K%C3%A9vin", "K\u{e9}vin"),
            ("bad%FFbyte", "bad\u{FFFD}byte"),
            ("100%", "100%"),
            ("%4", "%4"),
            ("%zz", "%zz"),
            ("%%41", "%A"),
            ("%", "%"),
        ];
        for (raw, decoded) in cases {
            let value = format!(r#"MediaBrowser Device="{raw}", Token="t1""#);
            let auth = parse_media_browser(&value).unwrap();
            assert_eq!(auth.device.as_deref(), Some(decoded), "{raw}");
            assert_eq!(auth.token.as_deref(), Some("t1"), "{raw}");
        }
    }

    #[test]
    fn j1_an_empty_or_missing_token_reads_as_such() {
        assert_eq!(
            token_of(r#"MediaBrowser Client="Jellyfin Web", Token="""#),
            Some(String::new())
        );
        let before_sign_in = r#"MediaBrowser Client="Jellyfin Web", Device="Firefox", DeviceId="d1", Version="10.10.0""#;
        let auth = parse_media_browser(before_sign_in).unwrap();
        assert_eq!(auth.client.as_deref(), Some("Jellyfin Web"));
        assert_eq!(auth.token, None);
        // An empty unquoted value adds nothing, as in Jellyfin.
        assert_eq!(token_of(r#"MediaBrowser Client="a", Token="#), None);
    }

    /// Jellyfin stores each value with `result[key] = …` (AuthorizationContext.cs:296 and 313 at
    /// v12.4), so a repeated key keeps its last value.
    #[test]
    fn j1_a_repeated_key_takes_its_last_value_like_jellyfin() {
        let auth =
            parse_media_browser(r#"MediaBrowser Client="a", Token="t1", Client="b", Token="t2""#)
                .unwrap();
        assert_eq!(auth.client.as_deref(), Some("b"));
        assert_eq!(auth.token.as_deref(), Some("t2"));
        assert_eq!(
            token_of(r#"MediaBrowser Token="t1", Token="""#),
            Some(String::new())
        );
    }

    #[test]
    fn j1_unbalanced_quotes_give_no_fields() {
        for value in [
            r#"MediaBrowser Token="t1"#,
            r#"MediaBrowser Token=t1""#,
            r#"MediaBrowser Client="a, Token="t1""#,
            r#"MediaBrowser Client="Jellyfin Web", Device="Fire"fox", Token="t1""#,
        ] {
            assert_eq!(parse_media_browser(value), None, "{value}");
        }
    }

    #[test]
    fn j1_keys_are_case_sensitive_like_jellyfin() {
        assert_eq!(token_of(r#"MediaBrowser token="t1""#), None);
        assert_eq!(token_of(r#"MediaBrowser TOKEN="t1""#), None);
    }

    #[test]
    fn j1_debug_output_hides_the_token() {
        let shown = format!("{:?}", parse_media_browser(WEB).unwrap());
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
        assert!(shown.contains("Jellyfin Web"), "{shown}");
    }

    #[test]
    fn j1_each_source_alone_gives_its_token() {
        let header = |name: &'static str, value: &str| token(&[(name, value)], "/System/Info");
        assert_eq!(header("authorization", "Bearer t1").as_deref(), Some("t1"));
        assert_eq!(header("authorization", &mb("t1")).as_deref(), Some("t1"));
        assert_eq!(
            header("x-emby-authorization", &mb("t1")).as_deref(),
            Some("t1")
        );
        assert_eq!(header("x-emby-token", "t1").as_deref(), Some("t1"));
        assert_eq!(header("x-mediabrowser-token", "t1").as_deref(), Some("t1"));
        for query in [
            "ApiKey=t1",
            "apikey=t1",
            "APIKEY=t1",
            "api_key=t1",
            "API_KEY=t1",
            "static=true&ApiKey=t1&DeviceId=d1",
        ] {
            let uri = format!("/Videos/v1/stream?{query}");
            assert_eq!(token(&[], &uri).as_deref(), Some("t1"), "{query}");
        }
        assert_eq!(token(&[], "/System/Info"), None);
    }

    #[test]
    fn j1_query_names_and_values_are_decoded_but_token_headers_are_not() {
        assert_eq!(token(&[], "/x?ApiKey=a%2Bb").as_deref(), Some("a+b"));
        assert_eq!(token(&[], "/x?ApiKey=a+b").as_deref(), Some("a b"));
        assert_eq!(token(&[], "/x?api_key=a%20b").as_deref(), Some("a b"));
        assert_eq!(token(&[], "/x?Api%4Bey=t1").as_deref(), Some("t1"));
        assert_eq!(token(&[], "/x?api_key=a=b").as_deref(), Some("a=b"));
        assert_eq!(token(&[], "/x?ApiKey=t1&ApiKey=t2").as_deref(), Some("t1"));
        assert_eq!(
            token(&[("x-emby-token", "a%2Bb")], "/x").as_deref(),
            Some("a%2Bb")
        );
        assert_eq!(
            token(&[("x-mediabrowser-token", "a+b")], "/x").as_deref(),
            Some("a+b")
        );
        assert_eq!(
            token(&[("authorization", "Bearer a%2Bb")], "/x").as_deref(),
            Some("a%2Bb")
        );
        assert_eq!(
            token(&[("authorization", &mb("a%2Bb+c"))], "/x").as_deref(),
            Some("a+b c")
        );
    }

    #[test]
    fn j1_the_first_source_with_a_token_wins() {
        let sources = [
            ("authorization", mb("t1")),
            ("x-emby-authorization", mb("t2")),
            ("x-emby-token", "t3".to_string()),
            ("x-mediabrowser-token", "t4".to_string()),
        ];
        // Without the first n sources, source n + 1 decides.
        for n in 0..sources.len() {
            let headers: Vec<(&'static str, &str)> = sources[n..]
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            let expected = format!("t{}", n + 1);
            assert_eq!(
                token(&headers, "/x?ApiKey=t5&api_key=t6").as_deref(),
                Some(expected.as_str())
            );
        }
        assert_eq!(token(&[], "/x?ApiKey=t5&api_key=t6").as_deref(), Some("t5"));
        assert_eq!(token(&[], "/x?api_key=t6&ApiKey=t5").as_deref(), Some("t5"));
        assert_eq!(
            token(
                &[
                    ("authorization", "Bearer t1"),
                    ("x-emby-authorization", &mb("t2"))
                ],
                "/x"
            )
            .as_deref(),
            Some("t1")
        );
        assert_eq!(
            token(&[("x-emby-token", "t3"), ("x-emby-token", "t9")], "/x").as_deref(),
            Some("t3")
        );
    }

    #[test]
    fn j1_a_blank_token_counts_as_absent() {
        let before_sign_in = r#"MediaBrowser Client="Jellyfin Web", Device="Firefox""#;
        for first in [mb(""), mb("   "), mb("%20"), before_sign_in.to_string()] {
            for name in ["authorization", "x-emby-authorization"] {
                assert_eq!(
                    token(&[(name, &first), ("x-emby-token", "t3")], "/x").as_deref(),
                    Some("t3"),
                    "{name}: {first}"
                );
            }
        }
        assert_eq!(
            token(
                &[("authorization", "Bearer "), ("x-emby-token", "t3")],
                "/x"
            )
            .as_deref(),
            Some("t3")
        );
        assert_eq!(
            token(
                &[("authorization", "Bearer   "), ("x-emby-token", "t3")],
                "/x"
            )
            .as_deref(),
            Some("t3")
        );
        assert_eq!(
            token(
                &[("x-emby-token", ""), ("x-mediabrowser-token", "t4")],
                "/x"
            )
            .as_deref(),
            Some("t4")
        );
        assert_eq!(
            token(&[("x-mediabrowser-token", " ")], "/x?ApiKey=t5").as_deref(),
            Some("t5")
        );
        for query in [
            "ApiKey=&api_key=t6",
            "ApiKey=%20&api_key=t6",
            "ApiKey=+&api_key=t6",
            "ApiKey&api_key=t6",
        ] {
            assert_eq!(
                token(&[], &format!("/x?{query}")).as_deref(),
                Some("t6"),
                "{query}"
            );
        }
        assert_eq!(token(&[("authorization", &mb(""))], "/x?api_key="), None);
    }

    #[test]
    fn j1_malformed_or_foreign_authorization_gives_no_token() {
        for value in [
            "Basic dDE6cHc=",
            "Basic t1",
            r#"MediaBrowserToken="t1""#,
            r#"MediaBrowser Token="t1"#,
            r#"MediaBrowser Client="a, Token="t1""#,
            r#"MediaBrowser token="t1""#,
            r#"MediaBrowser ,,=,"",=="""#,
            "Emby",
            "",
        ] {
            assert_eq!(token(&[("authorization", value)], "/x"), None, "{value:?}");
            // The next source still counts.
            assert_eq!(
                token(&[("authorization", value), ("x-emby-token", "t3")], "/x").as_deref(),
                Some("t3"),
                "{value:?}"
            );
        }
    }

    #[test]
    fn j1_a_header_that_is_not_visible_ascii_gives_nothing() {
        let uri: Uri = "/x".parse().unwrap();
        let mut map = HeaderMap::new();
        map.insert(
            "authorization",
            HeaderValue::from_bytes(b"MediaBrowser Device=\"K\xc3\xa9vin\", Token=\"t1\"").unwrap(),
        );
        assert!(request_token(&map, &uri).is_none());
        map.insert("x-emby-token", HeaderValue::from_bytes(b"t3\xff").unwrap());
        assert!(request_token(&map, &uri).is_none());
        map.insert("x-mediabrowser-token", HeaderValue::from_static("t4"));
        let token = request_token(&map, &uri).map(|presented| presented.token);
        assert_eq!(token.as_deref(), Some("t4"));
    }

    // J1 review F1: URLs such as an *arr Webhook's carry the configured key as typed, and master
    // compared `api_key` as sent, so that parameter also gives its raw value. `ApiKey` is decoded
    // only, as in Jellyfin.
    #[test]
    fn j1_the_api_key_parameter_also_gives_its_value_as_sent() {
        for (query, decoded, raw) in [
            ("api_key=a+b%2Bc", "a b+c", "a+b%2Bc"),
            ("API_KEY=a%41b", "aAb", "a%41b"),
            ("static=true&api_key=a+b&api_key=c", "a b", "a+b"),
            ("ApiKey=&api_key=a+b", "a b", "a+b"),
            ("ApiKey=%20&api_key=a=b", "a=b", "a=b"),
        ] {
            let uri = format!("/x?{query}");
            assert_eq!(token(&[], &uri).as_deref(), Some(decoded), "{query}");
            assert_eq!(as_sent(&[], &uri).as_deref(), Some(raw), "{query}");
        }
        // *arr sends Basic next to `api_key`, and Basic gives no token.
        let basic = [("authorization", "Basic YXJyOnNlY3JldA==")];
        assert_eq!(token(&basic, "/x?api_key=a+b").as_deref(), Some("a b"));
        assert_eq!(as_sent(&basic, "/x?api_key=a+b").as_deref(), Some("a+b"));
        // An earlier source decides, with its own value as sent or none.
        assert_eq!(token(&[], "/x?ApiKey=a+b").as_deref(), Some("a b"));
        assert_eq!(as_sent(&[], "/x?ApiKey=a+b"), None);
        assert_eq!(as_sent(&[], "/x?ApiKey=t1&api_key=a+b"), None);
        assert_eq!(as_sent(&[("x-emby-token", "t3")], "/x?api_key=a+b"), None);
        assert_eq!(
            as_sent(&[("authorization", &mb("t%31"))], "/x?api_key=a+b").as_deref(),
            Some("t%31")
        );
        assert_eq!(
            as_sent(&[("authorization", "Bearer t1")], "/x?api_key=a+b"),
            None
        );
        // A value that is blank once decoded counts as absent, as in every source.
        assert!(presented(&[], "/x?api_key=+").is_none());
        assert!(presented(&[], "/x?api_key=%20").is_none());
    }

    // J1 re-check R1: Sonarr and Radarr send the configured key as typed in `X-MediaBrowser-Token`
    // and in `Authorization: MediaBrowser Token="…"`, and the decoded `MediaBrowser` token decides.
    // So that token also gives its value as sent: inside the quotes, before decoding.
    #[test]
    fn j1_a_media_browser_token_also_gives_its_value_as_sent() {
        for name in ["authorization", "x-emby-authorization"] {
            for (value, decoded, sent) in [
                (r#"MediaBrowser Token="a+b%2Bc""#, "a b+c", "a+b%2Bc"),
                (
                    r#"Emby Client="c+d", Token="j1%41key""#,
                    "j1Akey",
                    "j1%41key",
                ),
                ("mediabrowser Token=a+b", "a b", "a+b"),
                // A repeated key keeps its last value, in both spellings.
                (r#"MediaBrowser Token="t+1", Token="c%41""#, "cA", "c%41"),
            ] {
                let headers = [(name, value)];
                assert_eq!(
                    token(&headers, "/x").as_deref(),
                    Some(decoded),
                    "{name}: {value}"
                );
                assert_eq!(
                    as_sent(&headers, "/x").as_deref(),
                    Some(sent),
                    "{name}: {value}"
                );
            }
        }
        // *arr's two headers: the `MediaBrowser` token decides, and keeps its spelling as sent.
        let arr = [
            ("x-mediabrowser-token", "k+y%41"),
            ("authorization", r#"MediaBrowser Token="k+y%41""#),
        ];
        assert_eq!(token(&arr, "/x").as_deref(), Some("k yA"));
        assert_eq!(as_sent(&arr, "/x").as_deref(), Some("k+y%41"));
        let emby = [
            ("x-emby-token", "k+y"),
            ("x-emby-authorization", r#"Emby Token="k+y""#),
        ];
        assert_eq!(token(&emby, "/x").as_deref(), Some("k y"));
        assert_eq!(as_sent(&emby, "/x").as_deref(), Some("k+y"));
        // `Bearer` and the token headers are taken as sent already; `ApiKey` is decoded only.
        for (headers, uri, taken) in [
            (vec![("authorization", "Bearer a+b")], "/x", "a+b"),
            (vec![("x-emby-token", "a+b")], "/x", "a+b"),
            (vec![("x-mediabrowser-token", "a+b")], "/x", "a+b"),
            (vec![], "/x?ApiKey=a+b", "a b"),
        ] {
            assert_eq!(
                token(&headers, uri).as_deref(),
                Some(taken),
                "{headers:?} {uri}"
            );
            assert_eq!(as_sent(&headers, uri), None, "{headers:?} {uri}");
        }
        // A `MediaBrowser` token that is blank once decoded still counts as absent.
        let blank = [
            ("authorization", r#"MediaBrowser Token="+""#),
            ("x-mediabrowser-token", "k1"),
        ];
        assert_eq!(token(&blank, "/x").as_deref(), Some("k1"));
        assert_eq!(as_sent(&blank, "/x"), None);
        // `Debug` shows neither spelling.
        let auth = parse_media_browser(r#"MediaBrowser Client="c", Token="j1%41key""#).unwrap();
        let shown = format!("{auth:?}");
        assert!(!shown.contains("j1%41key"), "{shown}");
        assert!(!shown.contains("j1Akey"), "{shown}");
    }
}
