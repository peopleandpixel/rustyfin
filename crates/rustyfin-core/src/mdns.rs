use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::watch;
use tracing;

/// mDNS multicast address and port
const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

/// mDNS configuration
#[derive(Debug, Clone)]
pub struct MdnsConfig {
    pub server_name: String,
    pub server_id: String,
    pub http_port: u16,
    pub bind_ip: Ipv4Addr,
}

/// Start the mDNS responder as a background task.
/// Responds to mDNS queries for _jellyfin._tcp.local. on port 5353.
/// Returns when the shutdown signal is received.
pub async fn run_mdns_responder(config: Arc<MdnsConfig>, mut shutdown: watch::Receiver<bool>) {
    let bind_addr: SocketAddr = (Ipv4Addr::UNSPECIFIED, MDNS_PORT).into();

    // Use socket2 for SO_REUSEADDR/SO_REUSEPORT support
    let socket = match socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("mDNS: failed to create socket: {e}");
            return;
        }
    };

    // Allow port reuse for mDNS (multiple services may use the same port)
    socket.set_only_v6(false).ok();
    socket.set_reuse_address(true).ok();
    #[cfg(unix)]
    socket.set_reuse_port(true).ok();
    socket.set_nonblocking(true).ok();

    if let Err(e) = socket.bind(&bind_addr.into()) {
        tracing::warn!("mDNS: failed to bind UDP port {MDNS_PORT}: {e}");
        return;
    }

    let std_sock: std::net::UdpSocket = socket.into();

    // Join the mDNS multicast group
    if let Err(e) = std_sock.join_multicast_v4(&MDNS_ADDR, &config.bind_ip) {
        tracing::warn!("mDNS: failed to join multicast group: {e}");
        return;
    }

    let sock = match TokioUdpSocket::from_std(std_sock) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("mDNS: failed to create async socket: {e}");
            return;
        }
    };

    tracing::info!("mDNS responder started on port {MDNS_PORT}");

    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_ok() && *shutdown.borrow() {
                    tracing::info!("mDNS responder shutting down");
                    break;
                }
            }
            result = sock.recv_from(&mut buf) => {
                match result {
                    Ok((len, src)) => {
                        // Check if this is an mDNS query for our service
                        if is_mdns_query_for_jellyfin(&buf[..len]) {
                            tracing::debug!("mDNS query from {src} for _jellyfin._tcp.local.");
                            send_mdns_response(&sock, src, &config);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("mDNS recv error: {e}");
                    }
                }
            }
        }
    }
}

/// Check if a DNS packet is a query for _jellyfin._tcp.local.
fn is_mdns_query_for_jellyfin(data: &[u8]) -> bool {
    if data.len() < 12 {
        return false;
    }

    // DNS header:
    // Bytes 0-1: Transaction ID
    // Bytes 2-3: Flags (0x0000 = query, 0x8000 = response)
    let flags = u16::from_be_bytes([data[2], data[3]]);
    let is_query = (flags & 0x8000) == 0;
    if !is_query {
        return false;
    }

    // Bytes 4-5: Question count
    let qd_count = u16::from_be_bytes([data[4], data[5]]);
    if qd_count == 0 {
        return false;
    }

    // Parse question section to look for _jellyfin._tcp.local.
    let mut offset = 12;
    for _ in 0..qd_count {
        if offset >= data.len() {
            return false;
        }

        match parse_dns_name(data, &mut offset) {
            Ok(name) => {
                if name.eq_ignore_ascii_case("_jellyfin._tcp.local.") {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }

    false
}

/// The longest DNS name, in wire bytes: its labels with their length bytes and the root label
/// (RFC 1035 section 3.1).
const MAX_NAME_WIRE_LEN: usize = 255;
/// The most compression pointers one name may follow. A real name needs a handful.
const MAX_POINTER_JUMPS: usize = 64;

/// Parse a DNS name from a packet, handling compression pointers. `offset` moves past the name
/// as it is written at `offset` (past its first pointer, if it has one).
///
/// The packet comes from anyone on the network, so the walk is bounded: every pointer must point
/// before the place the previous jump landed on (before `offset` for the first), so a chain of
/// pointers can't loop; a name follows at most `MAX_POINTER_JUMPS` of them; and it is at most
/// `MAX_NAME_WIRE_LEN` bytes long. Reserved label types (`0x40`, `0x80`) are refused.
fn parse_dns_name(data: &[u8], offset: &mut usize) -> Result<String, ()> {
    let mut name = String::new();
    let mut wire_len = 1; // the root label
    let mut jumps = 0;
    let mut resume_at = None;
    // Every pointer must point before this: where the current run of labels started
    let mut run_start = *offset;
    let mut current = *offset;

    loop {
        let len = usize::from(*data.get(current).ok_or(())?);
        if len == 0 {
            current += 1;
            break;
        }
        match len & 0xC0 {
            // A compression pointer: the top 2 bits set, then a 14-bit offset
            0xC0 => {
                let low = usize::from(*data.get(current + 1).ok_or(())?);
                let target = ((len & 0x3F) << 8) | low;
                jumps += 1;
                if target >= run_start || jumps > MAX_POINTER_JUMPS {
                    return Err(());
                }
                resume_at.get_or_insert(current + 2);
                run_start = target;
                current = target;
            }
            0x00 => {
                wire_len += 1 + len;
                if wire_len > MAX_NAME_WIRE_LEN {
                    return Err(());
                }
                let label = data.get(current + 1..current + 1 + len).ok_or(())?;
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(std::str::from_utf8(label).map_err(|_| ())?);
                current += 1 + len;
            }
            _ => return Err(()),
        }
    }

    *offset = resume_at.unwrap_or(current);
    Ok(name)
}

/// Send an mDNS response for _jellyfin._tcp.local.
fn send_mdns_response(sock: &TokioUdpSocket, src: SocketAddr, config: &MdnsConfig) {
    // Build a minimal DNS response for _jellyfin._tcp.local.
    // We create a simple response with a PTR record pointing to the server
    let mut packet = Vec::new();

    // DNS Header
    // Transaction ID (echo from query, but we use 0 for simplicity)
    packet.extend_from_slice(&[0x00, 0x00]);
    // Flags: Standard response, recursion available, no error
    packet.extend_from_slice(&[0x84, 0x00]);
    // Questions: 1
    packet.extend_from_slice(&[0x00, 0x01]);
    // Answer RRs: 1
    packet.extend_from_slice(&[0x00, 0x01]);
    // Authority RRs: 0
    packet.extend_from_slice(&[0x00, 0x00]);
    // Additional RRs: 0
    packet.extend_from_slice(&[0x00, 0x00]);

    // Question: _jellyfin._tcp.local.
    encode_dns_name(&mut packet, "_jellyfin._tcp.local.");
    // QTYPE: PTR (12)
    packet.extend_from_slice(&[0x00, 0x0C]);
    // QCLASS: IN (1)
    packet.extend_from_slice(&[0x00, 0x01]);

    // Answer RDATA: PTR to server name
    // Name: pointer to question (0xC00C = offset 12)
    packet.extend_from_slice(&[0xC0, 0x0C]);
    // TYPE: PTR (12)
    packet.extend_from_slice(&[0x00, 0x0C]);
    // CLASS: IN (1)
    packet.extend_from_slice(&[0x00, 0x01]);
    // TTL: 120 seconds
    packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x78]);

    // Build the target name: {server_id}._jellyfin._tcp.local.
    let target_name = format!("{}._jellyfin._tcp.local.", config.server_id);
    let rdata_len: u16 = target_name.len() as u16 + 1; // +1 for null terminator in encoding
    packet.extend_from_slice(&rdata_len.to_be_bytes());

    // Encode the target name
    let parts: Vec<&str> = target_name.split('.').collect();
    for part in &parts {
        if !part.is_empty() {
            packet.push(part.len() as u8);
            packet.extend_from_slice(part.as_bytes());
        }
    }
    packet.push(0); // Null terminator

    // Multicast response to the mDNS group
    let multicast_addr: SocketAddr = (MDNS_ADDR, MDNS_PORT).into();

    // Also send unicast back to the requester
    if let Err(e) = sock.try_send_to(&packet, src) {
        tracing::debug!("mDNS: failed to send unicast response to {src}: {e}");
    }

    // Send multicast response
    if let Err(e) = sock.try_send_to(&packet, multicast_addr) {
        tracing::debug!("mDNS: failed to send multicast response: {e}");
    }

    tracing::debug!("mDNS response sent for {}", config.server_id);
}

/// Encode a DNS name into a packet buffer.
fn encode_dns_name(packet: &mut Vec<u8>, name: &str) {
    let parts: Vec<&str> = name.split('.').collect();
    for part in &parts {
        if !part.is_empty() {
            packet.push(part.len() as u8);
            packet.extend_from_slice(part.as_bytes());
        }
    }
    packet.push(0); // Null terminator
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Runs `f` on its own thread and fails the test when it doesn't return within 5 s: an
    /// unbounded walk would otherwise hang the test run.
    fn within_deadline<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the DNS parser did not return within 5 s")
    }

    fn parse(packet: &[u8], offset: usize) -> Result<(String, usize), ()> {
        let packet = packet.to_vec();
        within_deadline(move || {
            let mut at = offset;
            parse_dns_name(&packet, &mut at).map(|name| (name, at))
        })
    }

    /// A query header with one question, then `question`.
    fn query(question: &[u8]) -> Vec<u8> {
        let mut packet = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        packet.extend_from_slice(question);
        packet
    }

    #[test]
    fn r3security_dns_name_reads_labels_and_backward_pointers() {
        // "a.b" at 12, then "c" and a pointer back to 12 at 17
        let packet = query(&[1, b'a', 1, b'b', 0, 1, b'c', 0xC0, 12]);
        assert_eq!(parse(&packet, 12), Ok(("a.b".to_string(), 17)));
        assert_eq!(parse(&packet, 17), Ok(("c.a.b".to_string(), 21)));
    }

    #[test]
    fn r3security_dns_name_refuses_pointer_loops() {
        // A pointer to itself
        assert_eq!(parse(&query(&[0xC0, 12]), 12), Err(()));
        // Two pointers that point at each other
        assert_eq!(parse(&query(&[0xC0, 14, 0xC0, 12]), 14), Err(()));
        // A label, then a pointer back to that label: backwards, and still a loop
        assert_eq!(parse(&query(&[1, b'x', 0xC0, 12]), 12), Err(()));
        // A forward pointer
        assert_eq!(
            parse(&query(&[0xC0, 20, 0, 0, 0, 0, 0, 0, 1, b'a', 0]), 12),
            Err(())
        );
    }

    #[test]
    fn r3security_dns_name_bounds_jumps_and_length() {
        // A root label at 12, then 100 pointers from 13 on, each to the one before it
        let mut packet = query(&[0]);
        for i in 0..100u16 {
            let target = if i == 0 { 12 } else { 11 + 2 * i };
            packet.extend_from_slice(&[0xC0 | (target >> 8) as u8, target as u8]);
        }
        let last = packet.len() - 2;
        assert_eq!(parse(&packet, last), Err(()), "more than 64 jumps");
        assert_eq!(parse(&packet, 13), Ok((String::new(), 15)), "one jump");
        assert_eq!(
            parse(&packet, 13 + 2 * 63),
            Ok((String::new(), 141)),
            "64 jumps"
        );

        // 5 labels of 63 bytes: 321 wire bytes, over 255
        let mut long = Vec::new();
        for _ in 0..5 {
            long.push(63);
            long.extend_from_slice(&[b'a'; 63]);
        }
        long.push(0);
        assert_eq!(parse(&query(&long), 12), Err(()));
        // Reserved label types
        assert_eq!(parse(&query(&[0x40, 0]), 12), Err(()));
        assert_eq!(parse(&query(&[0x80, 0]), 12), Err(()));
    }

    /// Crafted packets: random bytes, and random bytes salted with compression pointers. Every
    /// one must be answered promptly, whatever it holds.
    #[test]
    fn r3security_mdns_query_check_survives_crafted_packets() {
        within_deadline(|| {
            let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
            let mut next = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed
            };
            for round in 0..20_000 {
                let len = 13 + (next() % 500) as usize;
                let mut packet: Vec<u8> = (0..len).map(|_| next() as u8).collect();
                // A query with the most questions
                packet[2] = 0;
                packet[4] = 0xFF;
                packet[5] = 0xFF;
                if round % 2 == 0 {
                    for _ in 0..(next() % 40) {
                        let at = 12 + (next() as usize % (len - 12));
                        packet[at] = 0xC0 | (next() as u8 & 0x01);
                    }
                }
                let _ = is_mdns_query_for_jellyfin(&packet);
                for start in [12, len / 2, len - 1] {
                    let mut at = start;
                    if let Ok(name) = parse_dns_name(&packet, &mut at) {
                        assert!(name.len() < MAX_NAME_WIRE_LEN);
                        assert!(at <= packet.len());
                    }
                }
            }
        });
    }
}
