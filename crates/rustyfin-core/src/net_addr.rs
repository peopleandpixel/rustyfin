//! The address this server gives a client on the local network: in DLNA stream URLs, the SSDP
//! `LOCATION` and the UDP discovery reply. A server bound to `0.0.0.0` must never advertise
//! that, as no client can connect to it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use ipnet::IpNet;

/// Which clients one `PublishedServerUriBySubnet` entry is for.
#[derive(Debug, Clone, PartialEq)]
enum Scope {
    /// Clients in this subnet (an address alone is a one-address subnet)
    Subnet(IpNet),
    /// Clients on the local network (`internal`)
    Internal,
    /// Clients outside it (`external`)
    External,
}

/// The per-client entries of Jellyfin's `PublishedServerUriBySubnet` (`subnet=url`): a CIDR
/// subnet or one address, `internal` or `external`. The `all=` entry is kept apart, as the
/// server's published URL; interface names and entries that don't parse are left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublishedUrls {
    entries: Vec<(Scope, String)>,
}

impl PublishedUrls {
    /// The entries of `subnet=url` strings that name a subnet, an address, `internal` or
    /// `external`, with a non-empty URL.
    pub fn parse<'a>(entries: impl IntoIterator<Item = &'a str>) -> Self {
        let entries = entries
            .into_iter()
            .filter_map(|entry| {
                let (key, url) = entry.split_once('=')?;
                let (key, url) = (key.trim(), url.trim());
                if url.is_empty() {
                    return None;
                }
                let scope = if key.eq_ignore_ascii_case("internal") {
                    Scope::Internal
                } else if key.eq_ignore_ascii_case("external") {
                    Scope::External
                } else if let Ok(net) = key.parse::<IpNet>() {
                    Scope::Subnet(net.trunc())
                } else {
                    let ip = key.parse::<IpAddr>().ok()?.to_canonical();
                    Scope::Subnet(IpNet::new(ip, if ip.is_ipv4() { 32 } else { 128 }).ok()?)
                };
                Some((scope, url.to_string()))
            })
            .collect();
        Self { entries }
    }

    /// The published URL for a client at `peer`, as Jellyfin's `GetSmartApiUrl` picks it: the
    /// entry of the most specific subnet that holds `peer`; else the `internal` or `external`
    /// entry, as `peer` is on the local network or not; else `all` (the `all=` entry or the
    /// configured published URL).
    pub fn for_peer<'a>(&'a self, peer: IpAddr, all: Option<&'a str>) -> Option<&'a str> {
        let peer = peer.to_canonical();
        let subnet = self
            .entries
            .iter()
            .filter_map(|(scope, url)| match scope {
                Scope::Subnet(net) if net.contains(&peer) => Some((net.prefix_len(), url)),
                _ => None,
            })
            .max_by_key(|(prefix, _)| *prefix)
            .map(|(_, url)| url.as_str());
        let wanted = if is_local_network(peer) {
            Scope::Internal
        } else {
            Scope::External
        };
        subnet
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|(scope, _)| *scope == wanted)
                    .map(|(_, url)| url.as_str())
            })
            .or(all)
    }
}

/// Whether `ip` is on the local network: loopback, private (RFC 1918), link-local, or an IPv6
/// unique local or link-local address.
fn is_local_network(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let o = v6.octets();
            v6.is_loopback() || (o[0] & 0xfe) == 0xfc || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
        }
    }
}

/// The local address this host uses to reach `peer`. With the usual symmetric routes, that is the
/// address of the interface `peer`'s packets arrive on. The kernel's routing table answers:
/// connecting a UDP socket only picks the route and sends nothing. `None` when no route leads to
/// `peer`.
pub fn local_ip_toward(peer: IpAddr) -> Option<IpAddr> {
    let peer = peer.to_canonical();
    let any: SocketAddr = match peer {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let sock = UdpSocket::bind(any).ok()?;
    sock.connect((peer, 9)).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The base URL a client on the local network should use, as Jellyfin's `GetSmartApiUrl`
/// gives it, without a trailing `/`: the published server URL when one is set; else `http://`
/// and the bound address when the server listens on one address; else the local address of the
/// interface toward `peer` (`local_ip_toward`); else loopback. Never `0.0.0.0` or `::`.
pub fn smart_api_url(published: Option<&str>, bind_ip: IpAddr, port: u16, peer: IpAddr) -> String {
    if let Some(url) = published.map(str::trim).filter(|url| !url.is_empty()) {
        return url.trim_end_matches('/').to_string();
    }
    let ip = if bind_ip.is_unspecified() {
        local_ip_toward(peer).unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
    } else {
        bind_ip
    };
    format!("http://{}", SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    #[test]
    fn r3security_smart_url_prefers_the_published_url() {
        let url = smart_api_url(Some(" https://media.example/jf/ "), ANY, 8096, LOOPBACK);
        assert_eq!(url, "https://media.example/jf");
        // A blank one counts as unset
        assert_eq!(
            smart_api_url(Some("  "), ANY, 8096, LOOPBACK),
            "http://127.0.0.1:8096"
        );
    }

    #[test]
    fn r3security_smart_url_never_gives_the_unspecified_address() {
        // Bound to every interface: the interface toward the peer
        assert_eq!(
            smart_api_url(None, ANY, 8096, LOOPBACK),
            "http://127.0.0.1:8096"
        );
        // An IPv4-mapped IPv6 peer, as a dual-stack listener reports it
        let mapped = IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped());
        assert_eq!(
            smart_api_url(None, ANY, 8096, mapped),
            "http://127.0.0.1:8096"
        );
        // Bound to one address: that address
        let bound = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7));
        assert_eq!(
            smart_api_url(None, bound, 8920, LOOPBACK),
            "http://192.0.2.7:8920"
        );
        // An unspecified IPv6 bind never shows either
        let url = smart_api_url(None, IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8096, LOOPBACK);
        assert!(!url.contains("0.0.0.0") && !url.contains("[::]"), "{url}");
        assert_eq!(local_ip_toward(LOOPBACK), Some(LOOPBACK));
    }

    #[test]
    fn r3security_published_url_follows_the_peers_subnet() {
        let urls = PublishedUrls::parse([
            "10.0.0.0/8=http://lan",
            "10.1.2.0/24=http://lab",
            "192.0.2.9=http://one",
            "external=https://outside",
            "eth0=http://interface-names-are-left-out",
            "all=https://all-is-kept-apart",
            "bad",
        ]);
        let all = Some("https://public");
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(urls.for_peer(ip("10.9.8.7"), all), Some("http://lan"));
        assert_eq!(
            urls.for_peer(ip("10.1.2.3"), all),
            Some("http://lab"),
            "most specific"
        );
        let mapped = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 5).to_ipv6_mapped());
        assert_eq!(urls.for_peer(mapped, all), Some("http://lan"));
        assert_eq!(urls.for_peer(ip("192.0.2.9"), all), Some("http://one"));
        assert_eq!(
            urls.for_peer(ip("198.51.100.1"), all),
            Some("https://outside")
        );
        // A local client in no listed subnet, with no `internal` entry: `all`
        assert_eq!(
            urls.for_peer(ip("192.168.1.4"), all),
            Some("https://public")
        );
        assert_eq!(urls.for_peer(ip("192.168.1.4"), None), None);
        let internal = PublishedUrls::parse(["internal=http://home"]);
        assert_eq!(
            internal.for_peer(ip("192.168.1.4"), all),
            Some("http://home")
        );
        assert_eq!(
            internal.for_peer(ip("198.51.100.1"), all),
            Some("https://public")
        );
        // The address a 10.x client gets, before the local interface
        let url = smart_api_url(urls.for_peer(ip("10.9.8.7"), all), ANY, 8096, LOOPBACK);
        assert_eq!(url, "http://lan");
    }
}
