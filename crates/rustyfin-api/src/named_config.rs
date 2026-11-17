//! Jellyfin's named configurations `network` (`NetworkConfiguration`) and `branding`
//! (`BrandingOptions`), each stored as a JSON file in the data directory: Jellyfin 10.11's defaults
//! under what an administrator saved. Fields Rustyfin doesn't know are kept as they came.
//!
//! Of the network settings, the server applies at its next start, when the matching environment
//! variable is unset: `LocalNetworkAddresses` and `InternalHttpPort` (the bind address,
//! `RUSTYFIN_BIND`), `KnownProxies` (`RUSTYFIN_KNOWN_PROXIES`) and the `all=` entry of
//! `PublishedServerUriBySubnet` (`RUSTYFIN_PUBLISHED_URL`). The others are stored only. The
//! branding settings apply at once.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub const NETWORK_KEY: &str = "network";
pub const BRANDING_KEY: &str = "branding";

/// The largest named configuration stored, as JSON.
const MAX_BYTES: usize = 256 * 1024;

/// The fields of the network configuration that only administrators read.
const NETWORK_PRIVATE_FIELDS: [&str; 1] = ["CertificatePassword"];

fn file_name(key: &str) -> Option<&'static str> {
    if key.eq_ignore_ascii_case(NETWORK_KEY) {
        Some("network.json")
    } else if key.eq_ignore_ascii_case(BRANDING_KEY) {
        Some("branding.json")
    } else {
        None
    }
}

/// Whether `key` names one of the configurations kept here.
pub fn handles(key: &str) -> bool {
    file_name(key).is_some()
}

fn path(data_dir: &Path, key: &str) -> Option<PathBuf> {
    file_name(key).map(|f| data_dir.join(f))
}

/// Jellyfin 10.11's `NetworkConfiguration` defaults.
fn network_defaults() -> Value {
    json!({
        "BaseUrl": "",
        "EnableHttps": false,
        "RequireHttps": false,
        "CertificatePath": "",
        "CertificatePassword": "",
        "InternalHttpPort": 8096,
        "InternalHttpsPort": 8920,
        "PublicHttpPort": 8096,
        "PublicHttpsPort": 8920,
        "AutoDiscovery": true,
        "EnableUPnP": false,
        "EnableIPv4": true,
        "EnableIPv6": false,
        "EnableRemoteAccess": true,
        "LocalNetworkSubnets": [],
        "LocalNetworkAddresses": [],
        "KnownProxies": [],
        "IgnoreVirtualInterfaces": true,
        "VirtualInterfaceNames": ["veth"],
        "EnablePublishedServerUriByRequest": false,
        "PublishedServerUriBySubnet": [],
        "RemoteIPFilter": [],
        "IsRemoteIPFilterBlacklist": false,
    })
}

/// Jellyfin's `BrandingOptions` defaults.
fn branding_defaults() -> Value {
    json!({
        "LoginDisclaimer": "",
        "CustomCss": "",
        "SplashscreenEnabled": false,
    })
}

fn defaults(key: &str) -> Map<String, Value> {
    let value = if key.eq_ignore_ascii_case(NETWORK_KEY) {
        network_defaults()
    } else {
        branding_defaults()
    };
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The defaults under the saved text; an unreadable file reads as the defaults.
fn merge(key: &str, saved: Option<&str>) -> Map<String, Value> {
    let mut options = defaults(key);
    if let Some(text) = saved {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(saved)) => options.extend(saved),
            Ok(_) => tracing::warn!("ignoring {key} settings that are not a JSON object"),
            Err(e) => tracing::warn!("ignoring unreadable {key} settings: {e}"),
        }
    }
    options
}

/// The stored configuration `key`, read now. Blocking; for the server's start.
pub fn load_sync(data_dir: &Path, key: &str) -> Map<String, Value> {
    let saved = path(data_dir, key).and_then(|p| std::fs::read_to_string(p).ok());
    merge(key, saved.as_deref())
}

/// The stored configuration `key`. Callers who aren't administrators get it without its secrets.
pub async fn load(data_dir: &Path, key: &str, is_admin: bool) -> Map<String, Value> {
    let saved = match path(data_dir, key) {
        Some(p) => tokio::fs::read_to_string(p).await.ok(),
        None => None,
    };
    let mut options = merge(key, saved.as_deref());
    if !is_admin && key.eq_ignore_ascii_case(NETWORK_KEY) {
        for field in NETWORK_PRIVATE_FIELDS {
            options.insert(field.to_string(), json!(""));
        }
    }
    options
}

fn strings<'a>(options: &'a Map<String, Value>, field: &str) -> Vec<&'a str> {
    options
        .get(field)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Checks a network configuration a client posts; the sentence for the first bad field.
fn validate_network(options: &Map<String, Value>) -> Result<(), String> {
    for field in [
        "InternalHttpPort",
        "InternalHttpsPort",
        "PublicHttpPort",
        "PublicHttpsPort",
    ] {
        if let Some(value) = options.get(field) {
            let ok = value.as_u64().is_some_and(|p| (1..=65535).contains(&p));
            if !ok {
                return Err(format!("{field}: a port from 1 to 65535"));
            }
        }
    }
    for field in [
        "LocalNetworkSubnets",
        "LocalNetworkAddresses",
        "KnownProxies",
        "PublishedServerUriBySubnet",
        "RemoteIPFilter",
        "VirtualInterfaceNames",
    ] {
        if let Some(value) = options.get(field) {
            let ok = value
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string));
            if !ok {
                return Err(format!("{field}: a list of strings"));
            }
        }
    }
    for address in strings(options, "LocalNetworkAddresses") {
        if address.trim().parse::<IpAddr>().is_err() {
            return Err(format!(
                "LocalNetworkAddresses: {address:?} is not an IP address"
            ));
        }
    }
    for proxy in strings(options, "KnownProxies") {
        known_proxy(proxy).map_err(|e| format!("KnownProxies: {e}"))?;
    }
    for entry in strings(options, "PublishedServerUriBySubnet") {
        if !entry.contains('=') {
            return Err(format!(
                "PublishedServerUriBySubnet: {entry:?} is not \"subnet=url\""
            ));
        }
    }
    Ok(())
}

/// Checks one known proxy entry: exactly one IP address or CIDR range, never every address (a
/// `/0` range would let any client set its own address through `X-Forwarded-For`).
fn known_proxy(entry: &str) -> Result<(), String> {
    let nets = crate::middleware::parse_known_proxies(entry);
    if entry.contains(',') || nets.len() != 1 {
        return Err(format!(
            "{entry:?} is not one IP address or CIDR range (one per entry)"
        ));
    }
    if nets[0].prefix_len() == 0 {
        return Err(format!(
            "{entry:?} trusts every address; list the proxy's own address or network"
        ));
    }
    Ok(())
}

/// Saves the configuration `key` a client posted. `Err` is the sentence for a bad request.
pub async fn save(data_dir: &Path, key: &str, body: Value) -> Result<(), SaveError> {
    let Some(path) = path(data_dir, key) else {
        return Err(SaveError::Invalid(format!("{key}: unknown configuration")));
    };
    let Value::Object(options) = body else {
        return Err(SaveError::Invalid(format!(
            "{key}: the body must be a JSON object"
        )));
    };
    if key.eq_ignore_ascii_case(NETWORK_KEY) {
        validate_network(&options).map_err(|e| SaveError::Invalid(format!("{key}: {e}")))?;
    }
    let text = serde_json::to_string_pretty(&options).map_err(|e| SaveError::Io(e.to_string()))?;
    if text.len() > MAX_BYTES {
        return Err(SaveError::TooLarge(format!(
            "{key}: {} bytes is over the {MAX_BYTES}-byte limit",
            text.len()
        )));
    }
    tokio::task::spawn_blocking(move || {
        // Each save writes its own temp file; the last rename wins
        let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    })
    .await
    .map_err(|e| SaveError::Io(e.to_string()))?
    .map_err(|e| SaveError::Io(e.to_string()))
}

#[derive(Debug)]
pub enum SaveError {
    Invalid(String),
    TooLarge(String),
    Io(String),
}

/// What the stored network configuration sets for the server's start; `None` where it sets
/// nothing, or where an environment variable is set (the caller checks those).
#[derive(Debug, Default, PartialEq)]
pub struct NetworkStartup {
    /// The bind address: the first `LocalNetworkAddresses` entry (else every address) and
    /// `InternalHttpPort`, when the file sets either
    pub bind: Option<String>,
    /// `KnownProxies`, comma-separated
    pub known_proxies: Option<String>,
    /// The URL of `PublishedServerUriBySubnet`'s `all=` entry
    pub published_url: Option<String>,
}

/// Reads the network settings the server applies at its start. Only fields the file itself holds
/// count, so a server that never saved them starts as before.
pub fn network_startup(data_dir: &Path) -> NetworkStartup {
    let saved = path(data_dir, NETWORK_KEY).and_then(|p| std::fs::read_to_string(p).ok());
    let Some(Value::Object(saved)) = saved.and_then(|t| serde_json::from_str(&t).ok()) else {
        return NetworkStartup::default();
    };
    let address = strings(&saved, "LocalNetworkAddresses")
        .into_iter()
        .find_map(|a| a.trim().parse::<IpAddr>().ok());
    let port = saved
        .get("InternalHttpPort")
        .and_then(Value::as_u64)
        .filter(|p| (1..=65535).contains(p));
    let bind = (address.is_some() || port.is_some()).then(|| {
        let port = port.unwrap_or(8096);
        match address {
            Some(IpAddr::V6(ip)) => format!("[{ip}]:{port}"),
            Some(IpAddr::V4(ip)) => format!("{ip}:{port}"),
            None => format!("0.0.0.0:{port}"),
        }
    });
    let proxies = strings(&saved, "KnownProxies");
    let known_proxies = (!proxies.is_empty()).then(|| proxies.join(","));
    let published_url = strings(&saved, "PublishedServerUriBySubnet")
        .into_iter()
        .filter_map(|e| e.split_once('='))
        .find(|(subnet, _)| subnet.trim().eq_ignore_ascii_case("all"))
        .map(|(_, url)| url.trim().to_string())
        .filter(|url| !url.is_empty());
    NetworkStartup {
        bind,
        known_proxies,
        published_url,
    }
}

/// The address the server listens on: `RUSTYFIN_BIND` (`env`) when set, else the saved one when
/// it can be bound now, else every address on port 8096. A saved address that can't be bound (a
/// typo, an address the machine no longer has, a port in use) is logged, so a bad save never
/// locks the administrator out.
pub fn startup_bind(env: Option<String>, saved: Option<String>) -> String {
    const DEFAULT: &str = "0.0.0.0:8096";
    if let Some(env) = env {
        return env;
    }
    let Some(saved) = saved else {
        return DEFAULT.to_string();
    };
    match std::net::TcpListener::bind(&saved) {
        Ok(probe) => {
            drop(probe);
            saved
        }
        Err(e) => {
            tracing::error!(
                "can't listen on the saved network address {saved} ({e}); listening on {DEFAULT} instead"
            );
            DEFAULT.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r3dash3_network_startup_reads_only_saved_fields() {
        let dir = std::env::temp_dir().join(format!("r3dash3-net-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(network_startup(&dir), NetworkStartup::default());
        std::fs::write(
            dir.join("network.json"),
            r#"{"InternalHttpPort":8300,"LocalNetworkAddresses":["::1"],
                "KnownProxies":["10.0.0.0/8","192.168.1.2"],
                "PublishedServerUriBySubnet":["10.0.0.0/8=http://lan","all=https://media.example"]}"#,
        )
        .unwrap();
        let s = network_startup(&dir);
        assert_eq!(s.bind.as_deref(), Some("[::1]:8300"));
        assert_eq!(s.known_proxies.as_deref(), Some("10.0.0.0/8,192.168.1.2"));
        assert_eq!(s.published_url.as_deref(), Some("https://media.example"));
        std::fs::write(dir.join("network.json"), r#"{"InternalHttpPort":9000}"#).unwrap();
        assert_eq!(network_startup(&dir).bind.as_deref(), Some("0.0.0.0:9000"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn r3dash3_startup_bind_falls_back_from_an_unusable_saved_address() {
        let env = Some("127.0.0.1:1".to_string());
        assert_eq!(startup_bind(env, Some("bad".into())), "127.0.0.1:1");
        assert_eq!(startup_bind(None, None), "0.0.0.0:8096");
        assert_eq!(
            startup_bind(None, Some("127.0.0.1:0".into())),
            "127.0.0.1:0"
        );
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = busy.local_addr().unwrap().to_string();
        assert_eq!(startup_bind(None, Some(taken)), "0.0.0.0:8096");
        assert_eq!(
            startup_bind(None, Some("999.1.1.1:8096".into())),
            "0.0.0.0:8096"
        );
    }

    #[test]
    fn r3dash3_network_validation() {
        let check = |v: Value| validate_network(v.as_object().unwrap());
        assert!(check(json!({"InternalHttpPort": 8096, "KnownProxies": ["10.0.0.0/8"]})).is_ok());
        assert!(check(json!({"InternalHttpPort": 0})).is_err());
        assert!(check(json!({"InternalHttpPort": "8096"})).is_err());
        assert!(check(json!({"KnownProxies": ["proxy.lan"]})).is_err());
        for bad in [
            "10.0.0.1,10.0.0.2",
            "10.0.0.1, nope",
            "0.0.0.0/0",
            "::/0",
            "",
        ] {
            assert!(check(json!({ "KnownProxies": [bad] })).is_err(), "{bad}");
        }
        assert!(check(json!({"KnownProxies": ["::1", "fd00::/8", "192.168.1.2"]})).is_ok());
        let e = check(json!({"KnownProxies": ["0.0.0.0/0"]})).unwrap_err();
        assert!(e.contains("every address"), "{e}");
        assert!(check(json!({"LocalNetworkAddresses": ["nope"]})).is_err());
        assert!(check(json!({"PublishedServerUriBySubnet": ["https://x"]})).is_err());
    }
}
