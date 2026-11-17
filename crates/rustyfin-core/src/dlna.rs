use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use rustyfin_model::server_info::ServerConfiguration;
use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::{RwLock, watch};
use tracing;

/// SSDP multicast address and port
const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
/// SERVER header of the SSDP messages: Rustyfin's version, as a UPnP product token.
const SSDP_SERVER: &str = concat!(
    "Rustyfin/",
    env!("CARGO_PKG_VERSION"),
    " DLNADOC/1.50 UPnP/1.0"
);

/// DLNA server configuration. `server_name` is used by the device description, which is built per
/// request from the configured name.
#[derive(Debug, Clone)]
pub struct DlnaConfig {
    pub server_name: String,
    pub server_id: String,
    pub http_port: u16,
    pub bind_ip: Ipv4Addr,
    /// The published server URL, which the SSDP `LOCATION` uses when it is set
    pub published_url: Option<String>,
    /// `PublishedServerUriBySubnet`'s entries by subnet: for a client they match, they come
    /// before `published_url`
    pub published_subnets: crate::net_addr::PublishedUrls,
}

impl DlnaConfig {
    /// The device description's URL for a client at `peer`: under the published server URL, else
    /// at the address of the interface toward `peer` (`net_addr::smart_api_url`).
    fn location(&self, peer: IpAddr) -> String {
        let base = crate::net_addr::smart_api_url(
            self.published_subnets
                .for_peer(peer, self.published_url.as_deref()),
            self.bind_ip.into(),
            self.http_port,
            peer,
        );
        format!("{base}/dlna/description.xml")
    }

    /// The ssdp:alive announcement for clients reached through `peer`.
    fn alive(&self, peer: IpAddr) -> String {
        let location = self.location(peer);
        let usn = format!("uuid:{}", self.server_id);
        format!(
            "NOTIFY * HTTP/1.1\r\n\
             HOST: 239.255.255.250:1900\r\n\
             CACHE-CONTROL: max-age=1800\r\n\
             LOCATION: {location}\r\n\
             NT: upnp:rootdevice\r\n\
             NTS: ssdp:alive\r\n\
             SERVER: {SSDP_SERVER}\r\n\
             USN: {usn}::upnp:rootdevice\r\n\
             \r\n"
        )
    }
}

/// The server configuration as it is now: SSDP reads `EnableDlnaServer` from it on every request
/// and announcement, so the dashboard's switch takes effect without a restart.
pub type LiveConfig = Arc<RwLock<ServerConfiguration>>;

/// How often the SSDP tasks look at `EnableDlnaServer` while nothing else happens.
const DLNA_POLL: Duration = Duration::from_secs(5);
/// How often an alive announcement is repeated while the DLNA server is on.
const ALIVE_EVERY: Duration = Duration::from_secs(600);

async fn dlna_on(settings: &LiveConfig) -> bool {
    settings.read().await.enable_dlna_server
}

/// Waits `delay`; true when the shutdown signal came first.
async fn shut_down_within(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        result = shutdown.changed() => result.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(delay) => false,
    }
}

/// The SSDP socket on port 1900, in the multicast group.
fn bind_ssdp(config: &DlnaConfig) -> std::io::Result<TokioUdpSocket> {
    let std_sock = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, SSDP_PORT)))?;
    std_sock.set_nonblocking(true)?;
    std_sock.join_multicast_v4(&SSDP_ADDR, &config.bind_ip)?;
    TokioUdpSocket::from_std(std_sock)
}

/// The SSDP discovery responder, until the shutdown signal. It holds port 1900 only while the DLNA
/// server is on, and lets it go when it is turned off.
pub async fn run_ssdp_responder(
    config: Arc<DlnaConfig>,
    settings: LiveConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut warned = false;
    loop {
        if !dlna_on(&settings).await {
            if shut_down_within(&mut shutdown, DLNA_POLL).await {
                return;
            }
            continue;
        }
        let sock = match bind_ssdp(&config) {
            Ok(sock) => sock,
            Err(e) => {
                if !warned {
                    tracing::warn!("SSDP: failed to listen on UDP port {SSDP_PORT}: {e}");
                    warned = true;
                }
                if shut_down_within(&mut shutdown, DLNA_POLL * 12).await {
                    return;
                }
                continue;
            }
        };
        tracing::info!("SSDP responder started on port {SSDP_PORT}");
        if serve_ssdp(&sock, &config, &settings, &mut shutdown).await {
            tracing::info!("SSDP responder shutting down");
            return;
        }
        tracing::info!("SSDP responder stopped: the DLNA server is off");
    }
}

/// Answers M-SEARCH requests on `sock` while the DLNA server is on. Returns true on shutdown, false
/// once the DLNA server is turned off (a request that arrives while it is off gets no answer).
pub async fn serve_ssdp(
    sock: &TokioUdpSocket,
    config: &DlnaConfig,
    settings: &LiveConfig,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let mut buf = [0u8; 4096];
    let mut poll = tokio::time::interval(DLNA_POLL);
    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return true;
                }
            }
            _ = poll.tick() => {
                if !dlna_on(settings).await {
                    return false;
                }
            }
            result = sock.recv_from(&mut buf) => {
                match result {
                    Ok((len, src)) => {
                        let msg = String::from_utf8_lossy(&buf[..len]);
                        if msg.contains("M-SEARCH") && msg.contains("ssdp:discover") {
                            if !dlna_on(settings).await {
                                return false;
                            }
                            handle_msearch(sock, src, config).await;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("SSDP recv error: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }
}

async fn handle_msearch(sock: &TokioUdpSocket, src: SocketAddr, config: &DlnaConfig) {
    let location = config.location(src.ip());
    let usn = format!("uuid:{}", config.server_id);

    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         CACHE-CONTROL: max-age=1800\r\n\
         LOCATION: {location}\r\n\
         SERVER: {SSDP_SERVER}\r\n\
         ST: upnp:rootdevice\r\n\
         USN: {usn}::upnp:rootdevice\r\n\
         EXT:\r\n\
         \r\n"
    );

    if let Err(e) = sock.send_to(response.as_bytes(), src).await {
        tracing::debug!("SSDP: failed to send response to {src}: {e}");
    }
}

/// The SSDP announcements to the multicast group, until the shutdown signal
/// (`run_ssdp_advertiser_to`).
pub async fn run_ssdp_advertiser(
    config: Arc<DlnaConfig>,
    settings: LiveConfig,
    shutdown: watch::Receiver<bool>,
) {
    let target: SocketAddr = (SSDP_ADDR, SSDP_PORT).into();
    run_ssdp_advertiser_to(config, settings, target, DLNA_POLL, shutdown).await;
}

/// SSDP announcements to `target`, following `EnableDlnaServer` as it is now (checked every
/// `poll`): ssdp:alive when the DLNA server is turned on and every ten minutes while it stays on,
/// ssdp:byebye when it is turned off and at shutdown while it is on.
pub async fn run_ssdp_advertiser_to(
    config: Arc<DlnaConfig>,
    settings: LiveConfig,
    target: SocketAddr,
    poll: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let sock = match TokioUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0u16)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("SSDP advertiser: failed to bind: {e}");
            return;
        }
    };

    let usn = format!("uuid:{}", config.server_id);
    let byebye = format!(
        "NOTIFY * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         NT: upnp:rootdevice\r\n\
         NTS: ssdp:byebye\r\n\
         USN: {usn}::upnp:rootdevice\r\n\
         \r\n"
    );

    let mut interval = tokio::time::interval(poll);
    // When the last alive went out, while the DLNA server is on
    let mut alive_at: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    if alive_at.is_some() {
                        sock.send_to(byebye.as_bytes(), target).await.ok();
                    }
                    tracing::info!("SSDP advertiser shutting down");
                    break;
                }
            }
            _ = interval.tick() => {
                let on = dlna_on(&settings).await;
                // The alive's LOCATION is the address of the interface toward the multicast
                // group, looked up for each announcement as interfaces come and go
                let message = match alive_at {
                    Some(at) if on && at.elapsed() < ALIVE_EVERY => None,
                    _ if on => Some(config.alive(target.ip())),
                    Some(_) => Some(byebye.clone()),
                    None => None,
                };
                alive_at = match (on, alive_at) {
                    (false, _) => None,
                    (true, Some(at)) if at.elapsed() < ALIVE_EVERY => Some(at),
                    (true, _) => Some(tokio::time::Instant::now()),
                };
                if let Some(message) = message
                    && let Err(e) = sock.send_to(message.as_bytes(), target).await
                {
                    tracing::debug!("SSDP advertise error: {e}");
                }
            }
        }
    }
}

/// Generate UPnP device description XML.
pub fn device_description_xml(config: &DlnaConfig) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"
      xmlns:dlna="urn:schemas-dlna-org:device-1-0">
  <specVersion>
    <major>1</major>
    <minor>0</minor>
  </specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>{name}</friendlyName>
    <manufacturer>Rustyfin</manufacturer>
    <manufacturerURL>https://github.com/rustyfin</manufacturerURL>
    <modelDescription>Rustyfin DLNA Media Server</modelDescription>
    <modelName>Rustyfin</modelName>
    <modelNumber>{version}</modelNumber>
    <UDN>uuid:{id}</UDN>
    <dlna:X_DLNADOC>DMS-1.50</dlna:X_DLNADOC>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <SCPDURL>/dlna/ContentDirectory.xml</SCPDURL>
        <controlURL>/dlna/control/ContentDirectory</controlURL>
        <eventSubURL>/dlna/event/ContentDirectory</eventSubURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
        <SCPDURL>/dlna/ConnectionManager.xml</SCPDURL>
        <controlURL>/dlna/control/ConnectionManager</controlURL>
        <eventSubURL>/dlna/event/ConnectionManager</eventSubURL>
      </service>
    </serviceList>
  </device>
</root>"#,
        name = config
            .server_name
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;"),
        id = config.server_id,
        version = env!("CARGO_PKG_VERSION"),
    )
}

/// ContentDirectory service description (SCPD) XML.
pub fn content_directory_scpd() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action>
      <name>Browse</name>
      <argumentList>
        <argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
        <argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>
        <argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
        <argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
        <argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
        <argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
        <argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSystemUpdateID</name>
      <argumentList>
        <argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSearchCapabilities</name>
      <argumentList>
        <argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action>
      <name>GetSortCapabilities</name>
      <argumentList>
        <argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument>
      </argumentList>
    </action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType><allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#
}

/// ConnectionManager service description (SCPD) XML.
pub fn connection_manager_scpd() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action>
      <name>GetProtocolInfo</name>
      <argumentList>
        <argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>
        <argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>
      </argumentList>
    </action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r3parental_config() -> DlnaConfig {
        DlnaConfig {
            server_name: "Test".to_string(),
            server_id: "test-id".to_string(),
            http_port: 8096,
            bind_ip: Ipv4Addr::LOCALHOST,
            published_url: None,
            published_subnets: Default::default(),
        }
    }

    fn r3parental_settings(on: bool) -> LiveConfig {
        Arc::new(RwLock::new(ServerConfiguration {
            enable_dlna_server: on,
            ..ServerConfiguration::default()
        }))
    }

    /// The next datagram on `sock` within `wait`, as text.
    async fn next_message(sock: &TokioUdpSocket, wait: Duration) -> Option<String> {
        let mut buf = [0u8; 4096];
        let (len, _) = tokio::time::timeout(wait, sock.recv_from(&mut buf))
            .await
            .ok()?
            .ok()?;
        Some(String::from_utf8_lossy(&buf[..len]).to_string())
    }

    #[tokio::test]
    async fn r3parental_ssdp_announcements_follow_the_dlna_switch_without_a_restart() {
        let settings = r3parental_settings(false);
        let listener = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let target = listener.local_addr().unwrap();
        let (stop, shutdown) = watch::channel(false);
        let task = tokio::spawn(run_ssdp_advertiser_to(
            Arc::new(r3parental_config()),
            settings.clone(),
            target,
            Duration::from_millis(20),
            shutdown,
        ));
        let short = Duration::from_millis(300);
        let long = Duration::from_secs(5);
        assert_eq!(next_message(&listener, short).await, None, "off: silent");

        settings.write().await.enable_dlna_server = true;
        let alive = next_message(&listener, long).await.expect("alive once on");
        assert!(alive.contains("NTS: ssdp:alive"), "{alive}");
        assert!(
            alive.contains("USN: uuid:test-id::upnp:rootdevice"),
            "{alive}"
        );
        assert_eq!(next_message(&listener, short).await, None, "alive once");

        settings.write().await.enable_dlna_server = false;
        let bye = next_message(&listener, long)
            .await
            .expect("byebye once off");
        assert!(bye.contains("NTS: ssdp:byebye"), "{bye}");
        assert_eq!(next_message(&listener, short).await, None, "byebye once");

        settings.write().await.enable_dlna_server = true;
        let alive = next_message(&listener, long).await.expect("alive again");
        assert!(alive.contains("NTS: ssdp:alive"), "{alive}");
        stop.send(true).unwrap();
        let bye = next_message(&listener, long)
            .await
            .expect("byebye at shutdown");
        assert!(bye.contains("NTS: ssdp:byebye"), "{bye}");
        tokio::time::timeout(long, task).await.unwrap().unwrap();
    }

    /// Bound to every interface (`0.0.0.0`), the search answer and the alive announcement name the
    /// address of the interface toward the client, or the published URL when one is set; never
    /// `0.0.0.0`.
    #[tokio::test]
    async fn r3security_ssdp_location_is_a_reachable_address() {
        for (published, expected) in [
            (
                None,
                "LOCATION: http://127.0.0.1:8096/dlna/description.xml\r\n",
            ),
            (
                Some("https://media.example/jf/"),
                "LOCATION: https://media.example/jf/dlna/description.xml\r\n",
            ),
        ] {
            let config = DlnaConfig {
                bind_ip: Ipv4Addr::UNSPECIFIED,
                published_url: published.map(str::to_string),
                ..r3parental_config()
            };
            let settings = r3parental_settings(true);
            let sock = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let server = sock.local_addr().unwrap();
            let (stop, mut shutdown) = watch::channel(false);
            let (serving, searched) = (settings.clone(), config.clone());
            let responder =
                tokio::spawn(
                    async move { serve_ssdp(&sock, &searched, &serving, &mut shutdown).await },
                );
            let client = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let search = "M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nST: ssdp:all\r\n\r\n";
            client.send_to(search.as_bytes(), server).await.unwrap();
            let answer = next_message(&client, Duration::from_secs(5))
                .await
                .expect("a search answer");
            assert!(answer.contains(expected), "{answer}");

            let target = client.local_addr().unwrap();
            let advertiser = tokio::spawn(run_ssdp_advertiser_to(
                Arc::new(config),
                settings,
                target,
                Duration::from_millis(20),
                stop.subscribe(),
            ));
            let alive = next_message(&client, Duration::from_secs(5))
                .await
                .expect("an alive announcement");
            assert!(alive.contains("NTS: ssdp:alive"), "{alive}");
            assert!(alive.contains(expected), "{alive}");
            stop.send(true).unwrap();
            let long = Duration::from_secs(5);
            tokio::time::timeout(long, advertiser)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(long, responder)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn r3parental_ssdp_answers_searches_only_while_dlna_is_on() {
        let settings = r3parental_settings(true);
        let sock = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let server = sock.local_addr().unwrap();
        let (_stop, mut shutdown) = watch::channel(false);
        let serving = settings.clone();
        let task = tokio::spawn(async move {
            serve_ssdp(&sock, &r3parental_config(), &serving, &mut shutdown).await
        });
        let client = TokioUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let search = "M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nST: ssdp:all\r\n\r\n";

        client.send_to(search.as_bytes(), server).await.unwrap();
        let answer = next_message(&client, Duration::from_secs(5))
            .await
            .expect("an answer while on");
        assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
        assert!(
            answer.contains("LOCATION: http://127.0.0.1:8096/dlna/description.xml"),
            "{answer}"
        );

        // Switched off without a restart: no answer, and the responder lets the port go.
        settings.write().await.enable_dlna_server = false;
        client.send_to(search.as_bytes(), server).await.unwrap();
        assert_eq!(
            next_message(&client, Duration::from_millis(300)).await,
            None
        );
        let shut_down = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(!shut_down, "it stopped because DLNA is off");
    }

    // B130: DLNA names Rustyfin, so it carries Rustyfin's own version.
    #[test]
    fn b130_dlna_reports_the_crate_version() {
        let config = DlnaConfig {
            server_name: "Test".to_string(),
            server_id: "test-id".to_string(),
            http_port: 8096,
            bind_ip: Ipv4Addr::LOCALHOST,
            published_url: None,
            published_subnets: Default::default(),
        };
        let xml = device_description_xml(&config);
        let model_number = format!("<modelNumber>{}</modelNumber>", env!("CARGO_PKG_VERSION"));
        assert!(xml.contains(&model_number), "{xml}");
        assert_eq!(
            SSDP_SERVER,
            format!(
                "Rustyfin/{} DLNADOC/1.50 UPnP/1.0",
                env!("CARGO_PKG_VERSION")
            )
        );
    }
}
