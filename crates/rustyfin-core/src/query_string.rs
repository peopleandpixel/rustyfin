//! Query strings for the URLs the server hands to clients (transcode and subtitle URLs). Every
//! name and value is percent-encoded, so a `+`, `&`, `=` or `#` in a value (a device id, a token)
//! reaches the server as sent instead of changing the query.

use std::fmt::{Display, Write};

/// Percent-encodes `value` for a query string: UTF-8 bytes other than RFC 3986's unreserved
/// characters (`A-Z a-z 0-9 - . _ ~`) and `,` become `%XX`. The comma stays, as it is legal in a
/// query and separates Jellyfin's list values (`VideoCodec=h264,hevc`).
pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~,".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// A query string built pair by pair, with each name and value percent-encoded
/// (`encode_component`).
#[derive(Debug, Default, Clone)]
pub struct QueryBuilder {
    query: String,
}

impl QueryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `name=value`.
    pub fn pair(&mut self, name: &str, value: impl Display) -> &mut Self {
        if !self.query.is_empty() {
            self.query.push('&');
        }
        self.query.push_str(&encode_component(name));
        self.query.push('=');
        self.query.push_str(&encode_component(&value.to_string()));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    /// `path`, then `?` and the pairs when there are any.
    pub fn url(&self, path: &str) -> String {
        if self.query.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{}", self.query)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3security_query_values_are_percent_encoded() {
        let mut query = QueryBuilder::new();
        query
            .pair("DeviceId", "a+b&c=d")
            .pair("ApiKey", "t#1 2/3?")
            .pair("Name", "Größe 名")
            .pair("VideoCodec", "h264,hevc")
            .pair("odd&name", 7);
        assert_eq!(
            query.url("/Videos/x/master.m3u8"),
            "/Videos/x/master.m3u8?DeviceId=a%2Bb%26c%3Dd&ApiKey=t%231%202%2F3%3F\
             &Name=Gr%C3%B6%C3%9Fe%20%E5%90%8D&VideoCodec=h264,hevc&odd%26name=7"
        );
        assert_eq!(QueryBuilder::new().url("/p"), "/p");
    }
}
