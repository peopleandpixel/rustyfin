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

/// Parse a DNS name from a packet, handling compression pointers.
fn parse_dns_name(data: &[u8], offset: &mut usize) -> Result<String, ()> {
    let mut name = String::new();
    let mut jumped = false;
    let mut saved_offset = None;
    let mut current = *offset;

    loop {
        if current >= data.len() {
            return Err(());
        }

        let len = data[current] as usize;
        if len == 0 {
            current += 1;
            break;
        }

        // Check for compression pointer (top 2 bits set)
        if (len & 0xC0) == 0xC0 {
            if current + 1 >= data.len() {
                return Err(());
            }
            if !jumped {
                saved_offset = Some(current + 2);
            }
            jumped = true;
            let pointer = ((len as u16 & 0x3F) << 8) | data[current + 1] as u16;
            current = pointer as usize;
            continue;
        }

        current += 1;
        if current + len > data.len() {
            return Err(());
        }

        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(std::str::from_utf8(&data[current..current + len]).map_err(|_| ())?);
        current += len;
    }

    if let Some(saved) = saved_offset {
        *offset = saved;
    } else {
        *offset = current;
    }

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
