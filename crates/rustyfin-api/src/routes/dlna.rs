use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use rustyfin_core::dlna;
use rustyfin_model::media::MediaType;

use crate::error::ApiError;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dlna/description.xml", get(description))
        .route("/dlna/ContentDirectory.xml", get(content_directory_scpd))
        .route("/dlna/ConnectionManager.xml", get(connection_manager_scpd))
        .route("/dlna/control/ContentDirectory", post(content_directory_control))
        .route("/dlna/control/ConnectionManager", post(connection_manager_control))
}

async fn description(State(state): State<AppState>) -> Response {
    let config = dlna::DlnaConfig {
        server_name: "Rustyfin".to_string(),
        server_id: state.server_id.to_string(),
        http_port: parse_port(&state.bind_addr),
        bind_ip: std::net::Ipv4Addr::UNSPECIFIED,
    };
    let xml = dlna::device_description_xml(&config);
    xml_response(xml)
}

async fn content_directory_scpd() -> Response {
    xml_response(dlna::content_directory_scpd().to_string())
}

async fn connection_manager_scpd() -> Response {
    xml_response(dlna::connection_manager_scpd().to_string())
}

async fn content_directory_control(
    State(state): State<AppState>,
    body: String,
) -> Result<Response, ApiError> {
    // Parse SOAP action from body
    if body.contains("Browse") {
        let object_id = extract_xml_value(&body, "ObjectID").unwrap_or("0".to_string());
        let browse_flag = extract_xml_value(&body, "BrowseFlag").unwrap_or_default();
        let start_index: i64 = extract_xml_value(&body, "StartingIndex")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let requested_count: i64 = extract_xml_value(&body, "RequestedCount")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);

        let didl = if object_id == "0" {
            // Root: return libraries as containers
            let libraries = rustyfin_db::repo_library::get_all_libraries(&state.db).await?;
            let mut items_xml = String::new();
            for lib in &libraries {
                items_xml.push_str(&format!(
                    r#"<container id="{id}" parentID="0" restricted="1" childCount="0">
  <dc:title>{name}</dc:title>
  <upnp:class>object.container.storageFolder</upnp:class>
</container>"#,
                    id = lib.id,
                    name = xml_escape(&lib.name),
                ));
            }
            let total = libraries.len();
            build_browse_response(&items_xml, total, total)
        } else if browse_flag == "BrowseMetadata" {
            // Return metadata for a single object
            if let Ok(id) = object_id.parse::<uuid::Uuid>() {
                if let Ok(Some(item)) = rustyfin_db::repo_media::get_item_by_id(&state.db, id).await {
                    let item_xml = media_item_to_didl(&item, &state.bind_addr);
                    build_browse_response(&item_xml, 1, 1)
                } else {
                    build_browse_response("", 0, 0)
                }
            } else {
                build_browse_response("", 0, 0)
            }
        } else {
            // BrowseDirectChildren: list children of a container
            if let Ok(parent_id) = object_id.parse::<uuid::Uuid>() {
                let items = rustyfin_db::repo_media::get_children_paged(
                    &state.db, parent_id, start_index, requested_count,
                )
                .await?;
                let total = rustyfin_db::repo_media::count_children(&state.db, parent_id).await?;
                let mut items_xml = String::new();
                for item in &items {
                    items_xml.push_str(&media_item_to_didl(item, &state.bind_addr));
                }
                build_browse_response(&items_xml, items.len(), total as usize)
            } else {
                build_browse_response("", 0, 0)
            }
        };

        Ok(soap_response("Browse", &didl))
    } else if body.contains("GetSystemUpdateID") {
        let inner = "<Id>1</Id>";
        Ok(soap_response("GetSystemUpdateID", inner))
    } else if body.contains("GetSearchCapabilities") {
        let inner = "<SearchCaps></SearchCaps>";
        Ok(soap_response("GetSearchCapabilities", inner))
    } else if body.contains("GetSortCapabilities") {
        let inner = "<SortCaps></SortCaps>";
        Ok(soap_response("GetSortCapabilities", inner))
    } else {
        Ok((StatusCode::BAD_REQUEST, "Unknown SOAP action").into_response())
    }
}

async fn connection_manager_control(body: String) -> Response {
    if body.contains("GetProtocolInfo") {
        let inner = r#"<Source>http-get:*:video/mp4:*,http-get:*:video/x-matroska:*,http-get:*:video/avi:*,http-get:*:audio/mpeg:*,http-get:*:audio/flac:*,http-get:*:image/jpeg:*</Source><Sink></Sink>"#;
        soap_response("GetProtocolInfo", inner)
    } else {
        (StatusCode::BAD_REQUEST, "Unknown SOAP action").into_response()
    }
}

fn media_item_to_didl(item: &rustyfin_model::media::MediaItem, bind_addr: &str) -> String {
    let is_container = matches!(
        item.media_type,
        MediaType::Series | MediaType::Season | MediaType::MusicAlbum
    );

    if is_container {
        format!(
            r#"<container id="{id}" parentID="{pid}" restricted="1" childCount="0">
  <dc:title>{title}</dc:title>
  <upnp:class>{class}</upnp:class>
</container>"#,
            id = item.id,
            pid = item.parent_id.map(|p| p.to_string()).unwrap_or("0".into()),
            title = xml_escape(&item.name),
            class = upnp_class(&item.media_type, true),
        )
    } else {
        let url = format!("http://{}/Items/{}/Download", bind_addr, item.id);
        let mime = match item.media_type {
            MediaType::Movie | MediaType::Episode => "video/mp4",
            MediaType::Audio => "audio/mpeg",
            MediaType::Photo => "image/jpeg",
            _ => "application/octet-stream",
        };
        let duration = item
            .runtime_ticks
            .map(|t| {
                let secs = t / 10_000_000;
                let h = secs / 3600;
                let m = (secs % 3600) / 60;
                let s = secs % 60;
                format!(r#" duration="{h}:{m:02}:{s:02}.000""#)
            })
            .unwrap_or_default();

        format!(
            r#"<item id="{id}" parentID="{pid}" restricted="1">
  <dc:title>{title}</dc:title>
  <upnp:class>{class}</upnp:class>
  <res protocolInfo="http-get:*:{mime}:*"{duration}>{url}</res>
</item>"#,
            id = item.id,
            pid = item.parent_id.map(|p| p.to_string()).unwrap_or("0".into()),
            title = xml_escape(&item.name),
            class = upnp_class(&item.media_type, false),
        )
    }
}

fn upnp_class(media_type: &MediaType, is_container: bool) -> &'static str {
    if is_container {
        return "object.container.storageFolder";
    }
    match media_type {
        MediaType::Movie | MediaType::Episode => "object.item.videoItem.movie",
        MediaType::Audio => "object.item.audioItem.musicTrack",
        MediaType::Photo => "object.item.imageItem.photo",
        _ => "object.item",
    }
}

fn build_browse_response(didl_items: &str, returned: usize, total: usize) -> String {
    let result_xml = format!(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">{didl_items}</DIDL-Lite>"#
    );
    format!(
        "<Result>{}</Result><NumberReturned>{returned}</NumberReturned><TotalMatches>{total}</TotalMatches><UpdateID>1</UpdateID>",
        xml_escape(&result_xml),
    )
}

fn soap_response(action: &str, inner: &str) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
  <s:Body>
    <u:{action}Response xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">
      {inner}
    </u:{action}Response>
  </s:Body>
</s:Envelope>"#
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")],
        body,
    )
        .into_response()
}

fn xml_response(body: String) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")],
        body,
    )
        .into_response()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

fn parse_port(bind_addr: &str) -> u16 {
    bind_addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8096)
}
