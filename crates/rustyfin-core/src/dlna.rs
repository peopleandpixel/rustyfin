use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::watch;
use tracing;

/// SSDP multicast address and port
const SSDP_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;

/// DLNA server configuration
#[derive(Debug, Clone)]
pub struct DlnaConfig {
    pub server_name: String,
    pub server_id: String,
    pub http_port: u16,
    pub bind_ip: Ipv4Addr,
}

/// Start the SSDP discovery responder as a background task.
/// Returns when the shutdown signal is received.
pub async fn run_ssdp_responder(config: Arc<DlnaConfig>, mut shutdown: watch::Receiver<bool>) {
    let bind_addr: SocketAddr = (Ipv4Addr::UNSPECIFIED, SSDP_PORT).into();

    // Use std socket to set reuse options, then convert
    let std_sock = match UdpSocket::bind(bind_addr) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("SSDP: failed to bind UDP port {SSDP_PORT}: {e}");
            return;
        }
    };
    std_sock.set_nonblocking(true).ok();

    if let Err(e) = std_sock.join_multicast_v4(&SSDP_ADDR, &config.bind_ip) {
        tracing::warn!("SSDP: failed to join multicast group: {e}");
        return;
    }

    let sock = match TokioUdpSocket::from_std(std_sock) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("SSDP: failed to create async socket: {e}");
            return;
        }
    };

    tracing::info!("SSDP responder started on {bind_addr}");

    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_ok() && *shutdown.borrow() {
                    tracing::info!("SSDP responder shutting down");
                    break;
                }
            }
            result = sock.recv_from(&mut buf) => {
                match result {
                    Ok((len, src)) => {
                        let msg = String::from_utf8_lossy(&buf[..len]);
                        if msg.contains("M-SEARCH") && msg.contains("ssdp:discover") {
                            handle_msearch(&sock, src, &config).await;
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
    let location = format!(
        "http://{}:{}/dlna/description.xml",
        config.bind_ip, config.http_port
    );
    let usn = format!("uuid:{}", config.server_id);

    let response = format!(
        "HTTP/1.1 200 OK\r\n\
         CACHE-CONTROL: max-age=1800\r\n\
         LOCATION: {location}\r\n\
         SERVER: Rustyfin/0.1 DLNADOC/1.50 UPnP/1.0\r\n\
         ST: upnp:rootdevice\r\n\
         USN: {usn}::upnp:rootdevice\r\n\
         EXT:\r\n\
         \r\n"
    );

    if let Err(e) = sock.send_to(response.as_bytes(), src).await {
        tracing::debug!("SSDP: failed to send response to {src}: {e}");
    }
}

/// Send SSDP NOTIFY alive advertisements periodically.
pub async fn run_ssdp_advertiser(config: Arc<DlnaConfig>, mut shutdown: watch::Receiver<bool>) {
    let sock = match TokioUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0u16)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("SSDP advertiser: failed to bind: {e}");
            return;
        }
    };

    let target: SocketAddr = (SSDP_ADDR, SSDP_PORT).into();
    let location = format!(
        "http://{}:{}/dlna/description.xml",
        config.bind_ip, config.http_port
    );
    let usn = format!("uuid:{}", config.server_id);

    let notify = format!(
        "NOTIFY * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         CACHE-CONTROL: max-age=1800\r\n\
         LOCATION: {location}\r\n\
         NT: upnp:rootdevice\r\n\
         NTS: ssdp:alive\r\n\
         SERVER: Rustyfin/0.1 DLNADOC/1.50 UPnP/1.0\r\n\
         USN: {usn}::upnp:rootdevice\r\n\
         \r\n"
    );

    let mut interval = tokio::time::interval(Duration::from_secs(600));

    loop {
        tokio::select! {
            result = shutdown.changed() => {
                if result.is_ok() && *shutdown.borrow() {
                    let byebye = format!(
                        "NOTIFY * HTTP/1.1\r\n\
                         HOST: 239.255.255.250:1900\r\n\
                         NT: upnp:rootdevice\r\n\
                         NTS: ssdp:byebye\r\n\
                         USN: {usn}::upnp:rootdevice\r\n\
                         \r\n"
                    );
                    sock.send_to(byebye.as_bytes(), target).await.ok();
                    tracing::info!("SSDP advertiser shutting down");
                    break;
                }
            }
            _ = interval.tick() => {
                if let Err(e) = sock.send_to(notify.as_bytes(), target).await {
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
    <modelNumber>0.1</modelNumber>
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
        name = config.server_name,
        id = config.server_id,
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
