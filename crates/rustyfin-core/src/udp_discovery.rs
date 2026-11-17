use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use serde::Serialize;
use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::watch;
use tracing;

use crate::dlna::LiveConfig;

/// Jellyfin client discovery UDP port
const DISCOVERY_PORT: u16 = 7359;

/// UDP discovery configuration. The name in a reply is the configured server name, read for each
/// request (`LiveConfig`), so a rename shows at once.
#[derive(Debug, Clone)]
pub struct UdpDiscoveryConfig {
    pub server_id: String,
    pub http_port: u16,
    pub bind_ip: Ipv4Addr,
    /// Optional published URL for remote access (e.g. "http://myserver.com:8096")
    pub endpoint_address: Option<String>,
    /// `PublishedServerUriBySubnet`'s entries by subnet: for a client they match, they come
    /// before `endpoint_address` in the reply's `Address`
    pub published_subnets: crate::net_addr::PublishedUrls,
}

/// Response sent to clients during UDP discovery: Jellyfin's ServerDiscoveryInfo, with its
/// PascalCase names, which Jellyfin's client SDKs read
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
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
    settings: LiveConfig,
    shutdown: watch::Receiver<bool>,
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
    serve_udp_discovery(&sock, &config, &settings, shutdown).await;
}

/// Answers discovery requests on `sock` until the shutdown signal.
pub async fn serve_udp_discovery(
    sock: &TokioUdpSocket,
    config: &UdpDiscoveryConfig,
    settings: &LiveConfig,
    mut shutdown: watch::Receiver<bool>,
) {
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
                            let name = server_name(&settings.read().await.server_name);
                            handle_discovery_request(sock, src, config, name);
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

/// The name discovery gives: the configured server name, or "Rustyfin" when it is empty.
fn server_name(configured: &str) -> String {
    match configured.trim() {
        "" => "Rustyfin".to_string(),
        name => name.to_string(),
    }
}

fn handle_discovery_request(
    sock: &TokioUdpSocket,
    src: SocketAddr,
    config: &UdpDiscoveryConfig,
    name: String,
) {
    // Jellyfin's GetSmartApiUrl for the sender: the published URL, else the address of the
    // interface toward it (never 0.0.0.0, which no client can connect to)
    let response = DiscoveryResponse {
        address: crate::net_addr::smart_api_url(
            config
                .published_subnets
                .for_peer(src.ip(), config.endpoint_address.as_deref()),
            config.bind_ip.into(),
            config.http_port,
            src.ip(),
        ),
        id: config.server_id.clone(),
        name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyfin_model::server_info::ServerConfiguration;
    use std::time::Duration;

    /// The JSON a discovery request on `server` gets back.
    async fn ask(client: &TokioUdpSocket, server: SocketAddr) -> serde_json::Value {
        client
            .send_to(b"who is JellyfinServer?", server)
            .await
            .unwrap();
        let mut buf = [0u8; 1024];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
            .await
            .expect("a discovery reply")
            .unwrap();
        serde_json::from_slice(&buf[..len]).unwrap()
    }

    #[tokio::test]
    async fn r3parental_discovery_answers_with_the_configured_name_as_it_is_now() {
        let settings: LiveConfig = Arc::new(tokio::sync::RwLock::new(ServerConfiguration {
            server_name: "Living Room".to_string(),
            ..ServerConfiguration::default()
        }));
        let config = UdpDiscoveryConfig {
            server_id: "server-id".to_string(),
            http_port: 8096,
            bind_ip: Ipv4Addr::LOCALHOST,
            endpoint_address: None,
            published_subnets: Default::default(),
        };
        let sock = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let server = sock.local_addr().unwrap();
        let (stop, shutdown) = watch::channel(false);
        let serving = settings.clone();
        let task = tokio::spawn(async move {
            serve_udp_discovery(&sock, &config, &serving, shutdown).await;
        });
        let client = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();

        let reply = ask(&client, server).await;
        assert_eq!(reply["Name"], "Living Room", "{reply}");
        assert_eq!(reply["Id"], "server-id", "{reply}");
        assert_eq!(reply["Address"], "http://127.0.0.1:8096", "{reply}");
        // A rename shows in the next reply, without a restart; an empty name is "Rustyfin".
        settings.write().await.server_name = "Den".to_string();
        assert_eq!(ask(&client, server).await["Name"], "Den");
        settings.write().await.server_name = "  ".to_string();
        assert_eq!(ask(&client, server).await["Name"], "Rustyfin");

        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    /// Bound to every interface, the reply's Address is the interface toward the client, or the
    /// published URL when one is set; never `0.0.0.0`.
    #[tokio::test]
    async fn r3security_discovery_address_is_a_reachable_address() {
        for (published, expected) in [
            (None, "http://127.0.0.1:8096"),
            (
                Some("https://media.example/jf/"),
                "https://media.example/jf",
            ),
        ] {
            let settings: LiveConfig = Arc::default();
            let config = UdpDiscoveryConfig {
                server_id: "server-id".to_string(),
                http_port: 8096,
                bind_ip: Ipv4Addr::UNSPECIFIED,
                endpoint_address: published.map(str::to_string),
                published_subnets: Default::default(),
            };
            let sock = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let server = sock.local_addr().unwrap();
            let (stop, shutdown) = watch::channel(false);
            let task = tokio::spawn(async move {
                serve_udp_discovery(&sock, &config, &settings, shutdown).await;
            });
            let client = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let reply = ask(&client, server).await;
            assert_eq!(reply["Address"], expected, "{reply}");
            stop.send(true).unwrap();
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
