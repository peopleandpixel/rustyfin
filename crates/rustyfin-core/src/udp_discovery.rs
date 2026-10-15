use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use serde::Serialize;
use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::watch;
use tracing;

/// Jellyfin client discovery UDP port
const DISCOVERY_PORT: u16 = 7359;

/// UDP discovery configuration
#[derive(Debug, Clone)]
pub struct UdpDiscoveryConfig {
    pub server_name: String,
    pub server_id: String,
    pub http_port: u16,
    pub bind_ip: Ipv4Addr,
    /// Optional published URL for remote access (e.g. "http://myserver.com:8096")
    pub endpoint_address: Option<String>,
}

/// Response sent to clients during UDP discovery
#[derive(Debug, Clone, Serialize)]
struct DiscoveryResponse {
    /// The server's local address with port, e.g. "http://192.168.1.100:8096"
    address: String,
    /// The server's unique ID (UUID)
    id: String,
    /// The server's display name
    name: String,
    /// Optional published endpoint address for remote access
    endpoint_address: Option<String>,
}

/// Start the UDP discovery responder as a background task.
/// Listens for "who is JellyfinServer?" broadcasts on port 7359 and responds with server info.
/// Returns when the shutdown signal is received.
pub async fn run_udp_discovery(
    config: Arc<UdpDiscoveryConfig>,
    mut shutdown: watch::Receiver<bool>,
) {
    let bind_addr: SocketAddr = (Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into();

    let sock = match TokioUdpSocket::bind(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("UDP discovery: failed to bind port {DISCOVERY_PORT}: {e}");
            return;
        }
    };

    // Enable broadcast on the socket
    if let Err(e) = sock.set_broadcast(true) {
        tracing::warn!("UDP discovery: failed to enable broadcast: {e}");
        return;
    }

    tracing::info!("UDP discovery responder started on port {DISCOVERY_PORT}");

    let mut buf = [0u8; 1024];

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_ok() && *shutdown.borrow() {
                    tracing::info!("UDP discovery responder shutting down");
                    break;
                }
            }
            result = sock.recv_from(&mut buf) => {
                match result {
                    Ok((len, src)) => {
                        let msg = String::from_utf8_lossy(&buf[..len]);
                        let msg_lower = msg.to_lowercase().trim().to_string();
                        // Jellyfin clients send "who is JellyfinServer?" or "Who is Jellyfin Server?"
                        if msg_lower.contains("who is jellyfin") {
                            tracing::debug!("UDP discovery request from {src}: {}", msg.trim());
                            handle_discovery_request(&sock, src, &config);
                        }
                    }
                    Err(e) => {
                        tracing::debug!("UDP discovery recv error: {e}");
                    }
                }
            }
        }
    }
}

fn handle_discovery_request(sock: &TokioUdpSocket, src: SocketAddr, config: &UdpDiscoveryConfig) {
    let response = DiscoveryResponse {
        address: format!("http://{}:{}", config.bind_ip, config.http_port),
        id: config.server_id.clone(),
        name: config.server_name.clone(),
        endpoint_address: config.endpoint_address.clone(),
    };

    let json = match serde_json::to_string(&response) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("UDP discovery: failed to serialize response: {e}");
            return;
        }
    };

    tracing::debug!("UDP discovery response to {src}: {json}");

    // Send to the requester's address
    if let Err(e) = sock.try_send_to(json.as_bytes(), src) {
        tracing::debug!("UDP discovery: failed to send response to {src}: {e}");
    }
}
