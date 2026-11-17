//! The address this server gives a client on the local network: in DLNA stream URLs, the SSDP
//! `LOCATION` and the UDP discovery reply. A server bound to `0.0.0.0` must never advertise
//! that, as no client can connect to it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

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
}
